use crate::mocks_reader::Mock;

use std::error::Error;
use std:: sync::Mutex;
use std::{convert::Infallible,sync::Arc};
use futures_util::StreamExt;
use hyper::{
    Body, Request, Response,
};
use k8s_openapi::api::core::v1::Pod;
use kube::{
    api::{Api,ListParams},
    Client, ResourceExt,
};

use tokio::io::{AsyncRead, AsyncWrite};
use tower::Service;

//kube deps
use hyper::client::conn::SendRequest;

use std::{
    fmt,
    future::Future,
    pin::Pin,
    task::{Context, Poll},
};


#[derive(Debug, Clone)]
struct RuntimeError {
    cause: String
}

impl Error for RuntimeError {}

impl RuntimeError {
    fn from(msg: &str) -> RuntimeError {
        let cause = String::from(msg);
        RuntimeError {cause}
    }
}

impl fmt::Display for RuntimeError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "Port-forwarder failed in runtime: cause {}", self.cause)
    }
}

pub struct PortForwardConnectionService {
    k8s_client: Client,
    mocks: Arc<Vec<Mock>>,
    connection: Arc<Mutex<Option<SendRequest<Body>>>>
}

impl PortForwardConnectionService {
    /// Creates a new [`PortForwardConnectionService`].
    pub fn new(k8s_client: Client, mocks: Arc<Vec<Mock>>) -> PortForwardConnectionService {
        PortForwardConnectionService {k8s_client, mocks, connection: Arc::new(Mutex::new(None))}
    }
}

impl Service<Request<Body>> for PortForwardConnectionService {
    type Response = Response<Body>;
    type Error = Infallible;
    type Future = Pin<Box<dyn Future<Output = Result<Response<Body>, Infallible>> + Send>>;

    fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Ok(()).into()
    }

    fn call(&mut self, req: Request<Body>) -> Self::Future {
        let client = self.k8s_client.clone();
        let state = self.connection.clone();

        let mut unlocked_state = state.lock().unwrap();
        let sender = unlocked_state.take();

        let send_request = self.connection.clone();
        let state_for_move_block = state.clone();
        let mocks = Arc::clone(&self.mocks);

        let future = async move {
            let headers = req.headers().clone();
            let host = String::from(headers.get("host").unwrap().to_str().unwrap().clone());

            let host_and_namespace: Vec<&str> = host.split('.').collect();

            //expect host of the pattern is: application.namespace, for example echo.default
            if host_and_namespace.len() != 2 {
                log::error!("[{}] received host has uprasable format: {}", host, host);
                return Ok(Response::builder().status(500).body("Incorrect format of the received host\n".into()).unwrap());
            }

            let application_name = host_and_namespace[0];
            let namespace = host_and_namespace[1];
            log::info!("[{}] application_name {} namespace {}", host, application_name, namespace);

            //TODO extrct
            for mock in mocks.iter() {
                if mock.request_match(&req) {
                    log::info!("[{}] serving response from the mock - mock {:?}", host, mock);
                    return Ok(mock.construct_response_from_mock());
                }
            }

            match sender {
                Some(mut sender) => {
                    log::info!("[{}] Sender already on place {:?} {:?}", host, sender, req);

                    let request = modify_request(&host, req);
                        
                    match sender.send_request(request).await {
                        Ok(response) => {                            
                            let mut send_request  = state_for_move_block.lock().unwrap();
                            log::info!("[{}] Received response from sender {:?}", host, sender);
                            send_request.replace(sender);
                            log::info!("[{}] received response {:?}", host, response);
                            Ok(modify_response(&host, response))
                        },
                        Err(_) => Ok(Response::builder().status(500).body("Unable to port-forward\n".into()).unwrap()),
                    }
                },
                None => {
                    log::info!("[{}] Setting new connection", host);
        
                    let port = match get_stream(client.clone(), application_name, &host, namespace).await {
                        Ok(port) => port,
                        Err(err) => {
                            log::error!("[{}] Unable to establish port-forward: {}",host, err);
                            return Ok(Response::builder().status(500).body("Unable to establish port-forward connection\n".into()).unwrap())
                        },
                    };

                    // let hyper drive the HTTP state in our DuplexStream via a task
                    let (mut sender, connection) = hyper::client::conn::handshake(port).await.unwrap();

                    let moved_host = host.clone();
                    tokio::spawn(async move {
                        if let Err(e) = connection.await {
                            log::error!("[{}] Error in connection: {}", moved_host,  e);
                        }
                        log::info!("[{}] connection will be closed.", moved_host)
                    });
                    
                    let request = modify_request(&host, req);

                    match sender.send_request(request).await {
                        Ok(response) => {
                            let mut send_request = send_request.lock().unwrap();
                            send_request.replace(sender);
                            log::info!("[{}] received response {:?}", host, response);
                            Ok(modify_response(&host, response))
                        },
                        Err(_) => Ok(Response::builder().status(500).body("Unable to port-forward\n".into()).unwrap()),
                    }
                },
            }
        };
        return Box::pin(future);
    }
}

async fn get_stream(client: Client, application_name: &str, host: &str, namespace: &str) 
                                -> Result<impl AsyncRead + AsyncWrite + Unpin, Box<dyn Error>> {
    let pods: Api<Pod> = Api::namespaced(client, namespace);
    let selector = format!("app={}", application_name);
    log::info!("[{}] selector= {:?}", host, selector);
    let lp = ListParams::default().labels(&selector); 
    let found_pods = match pods.list(&lp).await {
        Ok(found_pods) => found_pods,
        Err(_) => return Err(Box::new(RuntimeError::from("Unable to list pods"))),
    };

    if found_pods.items.len() == 0 {
        let err_msg = format!("No pods found for host {host} - extract: application_name: {application_name} and namespace {namespace}");
        log::error!("[{}] {}", host, err_msg);
        return Err(Box::new(RuntimeError::from(&err_msg)))
    }
                                    
    let target_pod = &found_pods.items[0];
    log::info!("[{}] forwarding to pod {:?}", host, &target_pod.name());
    
    let mut pf = match pods.portforward(&target_pod.name(), &[8080]).await {
        Ok(pf) => pf,
        Err(_) => return Err(Box::new(RuntimeError::from("Unable to obtain port-forwarder"))),
    };

    match pf.take_stream(8080) {
        Some(stream) => Ok(stream),
        None => Err(Box::new(RuntimeError::from("Unable to obtain stream")))
    }
}


fn modify_request(host: &str, req: Request<Body>) -> Request<Body> {
    // We need to make incoming request "debugable", so force Body to print each of the chunks
    // to the standard outpt. 
    let headers = req.headers().clone();
    let method = req.method().clone();
    let uri: hyper::Uri = req.uri().clone();
    
    let owned_host = String::from(host);
    let debugable_body = req.into_body()
        .map(move |chunk| {
            if let Ok(data) = &chunk {
                let string = std::str::from_utf8(data).unwrap();
                log::info!("[{owned_host}] request body chunk {}", string);
            }
            chunk
        });

    let debugable_body = Body::wrap_stream(debugable_body);

    let mut request = Request::builder()
        .uri(uri)
        .method(method)
        .body(debugable_body)
        .unwrap();

    request.headers_mut().extend(headers);
    
    request
}

fn modify_response(host: &str, res: Response<Body>) -> Response<Body> {
    let (parts, body) = res.into_parts();

    let owned_host = String::from(host);
    let debugable_body = body
        .map(move |chunk| {
            if let Ok(data) = &chunk {
                let string = std::str::from_utf8(data).unwrap();
                log::info!("[{owned_host}] resposne body chunk {}", string);
            }
            chunk
        });

    let debugable_body = Body::wrap_stream(debugable_body);
    Response::from_parts(parts, debugable_body)
}
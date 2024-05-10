use std::error::Error;
use std::fmt;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use std::convert::Infallible;
use std::task::{Context, Poll};
use futures::future::BoxFuture;
use hyper::client::conn::SendRequest;
use hyper::{body, Body};
use hyper::{Request, Response};
use k8s_openapi::api::core::v1::Pod;
use kube::api::ListParams;
use kube::{Api, Client, ResourceExt};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::time::sleep;
use tower::Service;
use futures::StreamExt;
use std::fmt::Debug;
use tower::Layer;

const MAX_RETRIES: usize = 4;

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

#[derive(Debug, Clone)]
pub struct LogLayer;

impl<S> Layer<S> for LogLayer {
    type Service = LogService<S>;

    fn layer(&self, inner: S) -> Self::Service {
        Self::Service { inner }
    }
}

#[derive(Debug, Clone)]
pub struct LogService<S> {
    inner: S,
}

impl<S> Service<Request<Body>> for LogService<S>
where
    S: Service<Request<Body>, Response = Response<Body>>,
    S::Error: Debug,
    S::Future: Send + 'static,
{
    type Response = Response<Body>;
    type Error = S::Error;
    type Future = BoxFuture<'static, Result<Response<Body>, Self::Error>>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, req: Request<Body>) -> Self::Future {
        println!("Service called with {req:?}");
        let headers = req.headers().clone();
        let method = req.method().clone();
        let uri: hyper::Uri = req.uri().clone();
        
        let host = String::from(headers.get("host").unwrap().to_str().unwrap());
        let debugable_body = req.into_body()
            .map(move |chunk| {
                if let Ok(data) = &chunk {
                    let string = std::str::from_utf8(data).unwrap();
                    log::info!("[{host}] request body chunk {}", string);
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
    
        let res = self
            .inner
            .call(request);

        Box::pin(async {
            let response = res.await.unwrap();

            let (parts, body) = response.into_parts();
            
            let debugable_body = body
                .map(move |chunk| {
                    if let Ok(data) = &chunk {
                        let maybe_printable = std::str::from_utf8(data);
                        match maybe_printable {
                            Ok(printable) => {log::info!("resposne body chunk {}", printable)}
                            Err(_) => {}
                        }
                    }
                    chunk
                });
        
            let debugable_body = Body::wrap_stream(debugable_body);
            let response = Response::from_parts(parts, debugable_body);

            Ok(response)
        })

    }
}

#[derive(Clone)]
pub struct RequestHandlingService {
    client: Client,
    upstream_connection: Arc<Mutex<Option<SendRequest<Body>>>>
}

impl RequestHandlingService {
    pub fn new(client: Client) -> RequestHandlingService {
        let empty = Arc::new(Mutex::new(None));
        RequestHandlingService{ client: client, upstream_connection: empty }
    }
}

impl Service<Request<Body>> for RequestHandlingService {
    type Response = Response<Body>;
    type Error = Infallible;
    type Future = BoxFuture<'static, Result<Self::Response, Self::Error>>;

    fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Ok(()).into()
    }

    fn call(&mut self, req: Request<Body>) -> Self::Future {
        let client = self.client.clone();
        let upstream_connection = self.upstream_connection.clone();

        let future = async move { 

            let headers = req.headers().clone();
            let method = req.method().clone();
            let uri: hyper::Uri = req.uri().to_string().parse().unwrap();
            let host = String::from(headers.get("host").unwrap().to_str().unwrap());

            let body = req.into_body();
            let full_body = body::to_bytes(body).await.unwrap();

            let mut retries: usize = 0;
            let upstream_connection = upstream_connection;

            while retries < MAX_RETRIES { 
                retries += 1;

                let client = client.clone();
                let body = full_body.clone();
                let upstream_connection = upstream_connection.clone();

                let mut request = Request::builder()
                    .uri(uri.clone())
                    .method(method.to_string().as_str())
                    .body(Body::from(body))
                    .unwrap();
                
                request.headers_mut().extend(headers.clone());

                match perform_forward(client, request, upstream_connection).await {
                    Ok(response) => {
                        return Ok(response)
                    }, 
                    Err(err) => {
                        log::error!("unable to port-forward to {}: {}",host, err);
                    }
                }

                //after connection refused or orhter issue with port-forwarding, lets sleep with backoff
                let sleep_time_ms = 100 * retries as u64;
                log::info!("waiting {}ms before retrying {} {}", sleep_time_ms, method, uri);
                sleep(Duration::from_millis(sleep_time_ms)).await;
            }
            
            Ok(Response::builder().status(500).body("Unable to port-forward\n".into()).unwrap())
        };
        Box::pin(future)
    }
}

fn take(upstream_connection: Arc<Mutex<Option<SendRequest<Body>>>>) -> Option<SendRequest<Body>> {
    upstream_connection.lock().unwrap().take()
}

fn give_it_back(sender: SendRequest<Body>, upstream_connection: Arc<Mutex<Option<SendRequest<Body>>>>) {
    upstream_connection.lock().unwrap().replace(sender);
}

async fn perform_forward(client: Client, req: Request<Body>, upstream_connection: Arc<Mutex<Option<SendRequest<Body>>>>) -> Result<Response<Body>, Box<dyn Error + Send + Sync>> {

    let headers = req.headers().clone();
    let host = String::from(headers.get("host").unwrap().to_str().unwrap());

    let host_and_namespace: Vec<&str> = host.split('.').collect();

    let maybe_already_opened = take(upstream_connection.clone());
    if let Some(mut already_opened) = maybe_already_opened {
        log::info!("[{}] using already opened conenction for {}", host, host);
        let rsp = Ok(already_opened.send_request(req).await?);   
        give_it_back(already_opened, upstream_connection);
        return rsp;
    }
    
    log::info!("[{}] no opened connection for {}", host, host);

    if host_and_namespace.len() != 2 {
        log::error!("[{}] received host has uprasable format: {}", host, host);
        return Ok(Response::builder().status(500).body("Incorrect format of the received host\n".into()).unwrap());
    }

    let application_name = host_and_namespace[0];
    let namespace = host_and_namespace[1];
    log::info!("[{}] application_name {} namespace {}", host, application_name, namespace);

    let port = get_stream(client.clone(), application_name, &host, namespace).await?;    
    let (mut sender, connection) = hyper::client::conn::handshake(port).await?;

    let moved_host = host.clone();
    tokio::spawn(async move {
        if let Err(e) = connection.await {
            log::error!("[{}] Error in connection: {}", moved_host,  e);
        }
        log::info!("[{}] connection will be closed.", moved_host)
    });

    let resp = Ok(sender.send_request(req).await?);

    {
        //here I guess we succedded, so, sender is valid
        let mut connection_state = upstream_connection.lock().unwrap();
        connection_state.replace(sender);
    }

    return resp;

}

async fn get_stream(client: Client, application_name: &str, host: &str, namespace: &str) 
                                -> Result<impl AsyncRead + AsyncWrite + Unpin, Box<dyn Error + Send + Sync>> {
                                     
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
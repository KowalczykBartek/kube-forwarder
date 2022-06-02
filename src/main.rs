mod connection_dispatcher;
mod kubernetes_utils;
mod kubernetes_api_service;

use kubernetes_api_service::ApiService;
use connection_dispatcher::ConnectionsDispatcher;
use kubernetes_utils::KubernetesRepository;
use std::error::Error;
use std::collections::HashMap;
use std::{convert::Infallible, net::SocketAddr, sync::Arc};
use hyper::{
    service::{make_service_fn, service_fn},
    Body, Request, Response, Server,
};
use tokio::sync::Mutex;
use tower::ServiceExt;
use clap::{Parser, Subcommand};
use http::{StatusCode};
use hyper::body;
use hyper::HeaderMap;
use hyper::Uri;
use bytes::Bytes;
use hyper::Method;

#[derive(Parser, Debug)]
#[clap(author, version, about, long_about = None)]
struct Args {
    #[clap(short, long)]
    kube_config: String,

    #[clap(subcommand)]
    operation: Operation,
}

#[derive(clap::ArgEnum, Clone, Debug, Subcommand)]
enum Operation {
    GenerateEtcHostsEntries{
        namespaces: Vec<String>
    },
    ForwardTraffic
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {   
    std::env::set_var("RUST_LOG", "info,kube=trace");
    env_logger::init();
     
    //define program parameter's api
    let args = Args::parse();
    let kube_config_location = &args.kube_config;
    log::info!("Received kube-config={}", kube_config_location);

    let kube_repository = KubernetesRepository::new(kube_config_location).await;
    
    match args.operation {
        Operation::GenerateEtcHostsEntries{namespaces} => {
            log::info!("generating /etc/hosts entries for {:?} namespaces", namespaces);  
            let client = kube_repository.client.clone();
            let api_service = ApiService::new(client);
            for namespace in namespaces {
                api_service.print_hosts_entries_for_namespace(&namespace).await;
            } 
        }
        Operation::ForwardTraffic => {
            let upstream_connections = HashMap::new();
            let client = kube_repository.client.clone();
            let state = ConnectionsDispatcher {
                client,
                upstream_connections
            };
        
            let context = Arc::new(Mutex::new(state));
            let make_service = make_service_fn(move |_conn| {
                let context = context.clone();
                let service = service_fn(move |req| handle(req, context.clone()));
                async move { Ok::<_, Infallible>(service) }
            });
        
            let addr = SocketAddr::from(([127, 0, 0, 1], 80));
            let server = Server::bind(&addr)
                 .serve(make_service);
         
            if let Err(e) = server.await {
                println!("server error: {}", e);
            }          
        }
    } 

    Ok(())
}

async fn handle(req: Request<Body>, 
    context: Arc<Mutex<ConnectionsDispatcher>>,
) -> Result<Response<Body>, Infallible> {
    let headers = req.headers();
    //from time to time I think rust could be smarter xD
    let host = String::from(headers.get("host").unwrap().to_str().unwrap().clone());
    log::info!("received request for host={} {:?}", host, req);

    let mut context = context.lock().await;

    let headers = req.headers().clone();
    let method = req.method().clone();
    let uri = req.uri().clone();
    let body = body::to_bytes(req.into_body()).await.unwrap();

    log::info!("forwarding reuqest");  

    log::info!("    uri {}", uri);  
    log::info!("    method {}", uri);  
    log::info!("    headers {:?}", headers);  
    log::info!("    body {:?}", body);  

    let mut mut_context = &mut *context;

    match forward_reuqest_with_retry(&mut mut_context, &host, headers, uri, method, body).await {
        Ok(response) => {
            log::info!("responding with {:?}", response);
            Ok(response)
        }, 
        Err(err) => {
            log::error!("Error occured {:?}", err);
            Ok(contruct_internal_server_error())
        }
    }
}

async fn forward_reuqest_with_retry(context: &mut ConnectionsDispatcher, target: &str, headers: HeaderMap, uri: Uri, method: Method, body: Bytes) -> Result<Response<Body>, Box<dyn Error + Send + Sync>> {

    match forward_reuqest(context, target, headers.clone(), uri.clone(), method.clone(), body.clone()).await {
        Ok(response) => {
            Ok(response)
        },
        Err(err) => {
            log::error!("Error occured {} - retrying !", err);
            context.report_broken_upstream_connection(target);
            Ok(forward_reuqest(context, target, headers.clone(), uri.clone(), method.clone(), body.clone()).await?)
        }
    }
}

async fn forward_reuqest(context: &mut ConnectionsDispatcher, target: &str, headers: HeaderMap, uri: Uri, method: Method, body: Bytes) -> Result<Response<Body>, Box<dyn Error + Send + Sync>> {

    let sender = context.get_upstream_client(target).await.unwrap();

    let mut request_builder = Request::builder()
        .method(method)
        .uri(uri);

    let new_headers = request_builder.headers_mut().unwrap();

    //copy all headers
    for header in headers {
        if let Some(header_name) = header.0 {
            new_headers.insert(header_name, header.1);
        }
    }

    let body = Body::from(body);
    let req = request_builder.body(body).unwrap();
    let resp = sender.ready().await?.send_request(req).await?;

    Ok(resp)
}

fn contruct_internal_server_error() -> Response<Body> {
    let response = Response::builder()
        .status(StatusCode::INTERNAL_SERVER_ERROR)
        .body(Body::empty())
        .unwrap();
    response
}

use std::future::ready;
use std::{convert::Infallible, net::SocketAddr};
use clap::Parser;
use hyper::{service::make_service_fn, Server};
use kube::client::ConfigExt;
use kube::config::{KubeConfigOptions, Kubeconfig};
use kube::{Client, Config};
use print_ascii::print_rocket_std_output;
use tower::ServiceBuilder;
use std::fmt::Debug;
use crate::forwarding_service::{LogLayer, RequestHandlingService};

mod print_ascii;
mod forwarding_service;

#[derive(Parser, Debug)]
#[clap(author, version, about, long_about = None)]
struct Args {
    #[clap(short, long)]
    kube_config: String,
}

#[tokio::main]
async fn main() {
    std::env::set_var("RUST_LOG", "info,kube=trace");
    env_logger::init();

    log::info!("setting up a forwarding proxy.");

    //define program parameter's api
    let args = Args::parse();
    log::info!("received clap's arguments {:?}", args);

    let kube_config_location = &args.kube_config;
    
    //k8s api
    let kubeconf = Kubeconfig::read_from(kube_config_location).unwrap();
    let opts = KubeConfigOptions::default();

    let config = Config::from_custom_kubeconfig(kubeconf, &opts).await.unwrap();
    let https = config.rustls_https_connector().unwrap();
    let service = ServiceBuilder::new()
        .layer(config.base_uri_layer())
        .option_layer(config.auth_layer().unwrap())
        .service(hyper::Client::builder().build(https));
            
    let client = Client::new(service, "there-is-no-default-namespace");

    let addr = SocketAddr::from(([127, 0, 0, 1], 80));
    print_rocket_std_output();

    let make_svc = make_service_fn(move |_conn: &hyper::server::conn::AddrStream| {
        
        let client = client.clone();
        let service = RequestHandlingService::new(client);

        let svc = ServiceBuilder::new()
        .layer(LogLayer)
        .service(service);

        ready(Ok::<_, Infallible>(svc))
    });

    let server = Server::bind(&addr).serve(make_svc);

    // Run this server for... forever!
    if let Err(e) = server.await {
        eprintln!("server error: {}", e);
    }
}

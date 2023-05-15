mod print_ascii;
mod forwarding_service;
mod mocks_reader;

use crate::mocks_reader::parse_mocks;
use forwarding_service::PortForwardConnectionService;
use print_ascii::print_rocket_std_output;
use std::net::SocketAddr;
use std::sync::Arc;
use hyper::server::conn::Http;
use tokio::net::TcpListener;
use kube::Client;
use tower::ServiceBuilder;

//kube deps
use clap::{Parser};
use kube::{client::ConfigExt, Config};
use kube::config::{Kubeconfig, KubeConfigOptions};

#[derive(Parser, Debug)]
#[clap(author, version, about, long_about = None)]
struct Args {
    #[clap(short, long)]
    kube_config: String,

    #[clap(short, long)]
    mock_location: Option<String>,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    std::env::set_var("RUST_LOG", "info,kube=trace");
    env_logger::init();

    //define program parameter's api
    let args = Args::parse();
    let kube_config_location = &args.kube_config;
    log::info!("continuing with configuration {:?}", args.mock_location);

    let mocks = match args.mock_location {
        Some(mocks_location) => parse_mocks(&mocks_location).await.unwrap_or(vec![]),
        None => vec![],
    };

    let mocks = Arc::new(mocks);

    log::info!("received mocks for kube {:?}", mocks);

    print_rocket_std_output();

    //construct kube interraction api
    let kubeconf = Kubeconfig::read_from(kube_config_location).unwrap();
    let opts = KubeConfigOptions::default();

    let config = Config::from_custom_kubeconfig(kubeconf, &opts).await.unwrap();
    let https = config.rustls_https_connector()?;
    let service = ServiceBuilder::new()
        .layer(config.base_uri_layer())
        .option_layer(config.auth_layer()?)
        .service(hyper::Client::builder().build(https));
            
    let client = Client::new(service, "there-is-no-default-namespace");

    let addr: SocketAddr = ([127, 0, 0, 1], 80).into();
    let tcp_listener = TcpListener::bind(addr).await?;
    loop {
        let (tcp_stream, _) = tcp_listener.accept().await?;

        let k8s_client = client.clone();
        let forwarding_connection = PortForwardConnectionService::new(k8s_client, Arc::clone(&mocks));

        tokio::task::spawn(async move {
            if let Err(http_err) = Http::new()
                    .http1_only(true)
                    .http1_keep_alive(true)
                    .serve_connection(tcp_stream, forwarding_connection)
                    .await {
                log::error!("Error while serving HTTP connection: {}", http_err);
            }
        });
     }

}

use kube::{client::ConfigExt, Client, Config};
use kube::config::{Kubeconfig, KubeConfigOptions};

pub struct KubernetesRepository {
    pub client: Client
}

impl KubernetesRepository {
    pub async fn new(kube_config_location: &str) -> KubernetesRepository {
        let client = KubernetesRepository::construct_kube_client(kube_config_location).await;
        KubernetesRepository {
            client
        }
    }

    async fn construct_kube_client(kube_config_location: &str) -> Client {
        let kubeconf = Kubeconfig::read_from(kube_config_location).unwrap();
        let opts = KubeConfigOptions::default();
        let config = Config::from_custom_kubeconfig(kubeconf, &opts).await.unwrap();
        let https = config.native_tls_https_connector().unwrap();
        let service = tower::ServiceBuilder::new()
            .layer(config.base_uri_layer())
            .option_layer(config.auth_layer().unwrap())
            .service(hyper::Client::builder().build(https));
        let client = Client::new(service, "there-is-no-default-namespace");
    
        client
    }
}
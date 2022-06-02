use kube::{Api, Client};
use k8s_openapi::api::core::v1::Service;
use kube::{api::{ListParams}};
use kube::{
     ResourceExt,
};
pub struct ApiService {
    client: Client,
}

impl ApiService {
    pub fn new(client: Client) -> ApiService {
        ApiService {
            client
        }
    }

    pub async fn print_hosts_entries_for_namespace(&self, namespace: &str) {
        let client = self.client.clone();
        let services: Api<Service> = Api::namespaced(client, namespace);
        let lp = ListParams::default();
        let services = services.list(&lp).await.unwrap();
        for service in services {
            println!("127.0.0.1 {}.{}", service.name(), namespace);
        }
    }
}

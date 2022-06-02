use std::error::Error;
use kube::{Api, Client};
use std::collections::HashMap;
use k8s_openapi::api::core::v1::Pod;
use kube::{api::{ListParams}};
use kube::{
     ResourceExt,
};

pub type HyperSender = hyper::client::conn::SendRequest<hyper::Body>;

pub struct ConnectionsDispatcher {
    pub client: Client,
    pub upstream_connections: HashMap<String, HyperSender>
}

impl ConnectionsDispatcher {
    pub fn report_broken_upstream_connection(&mut self, target: &str) {
        self.upstream_connections.remove(target);   
    }
    pub async fn get_upstream_client(&mut self, target: &str) -> Result<&mut HyperSender, Box<dyn Error>> {
        let saved_connection = self.upstream_connections.get(target);
        match saved_connection {
            Some(_) => {
                println!("Found existing entry");
                Ok(self.upstream_connections.get_mut(target).unwrap())
            }, 
            None => {
                let client = self.client.clone();
                let host_and_namespace: Vec<&str> = target.split('.').collect();
                let application_name = host_and_namespace[0];
                let namespace = host_and_namespace[1];
                log::info!("application_name {} namespace {}", application_name, namespace);
        
                let pods: Api<Pod> = Api::namespaced(client, &namespace);
                
                let selector = format!("app={}", application_name);
                log::info!("selector= {:?}", selector);
        
                let lp = ListParams::default().labels(&selector); 
                let found_pods = pods.list(&lp).await.unwrap();
                
                let target_pod = found_pods.iter().next().unwrap();
                log::info!("forwarding to pod {:?}", &target_pod.name());
        
                let mut forwarder = pods.portforward(&target_pod.name(), &[8080]).await.unwrap();
                let port = forwarder.ports()[0].stream().unwrap();
                let (sender, connection) = hyper::client::conn::handshake(port).await.unwrap();
        
                tokio::spawn(async move {
                    if let Err(e) = connection.await {
                        log::error!("error in connection: {}", e);
                    }
                });

                let key = String::from(target);
                self.upstream_connections.insert(key, sender);
                Ok(self.upstream_connections.get_mut(target).unwrap())
            }
        }
    }
}
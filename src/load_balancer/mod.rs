use std::net::{Ipv4Addr, SocketAddrV4, TcpListener};
use std::sync::Arc;

use color_eyre::eyre::{Report, Result};
use hyper::client::HttpConnector;
use hyper::service::{make_service_fn, service_fn};
use hyper::{Client, Server};
use rand::prelude::{SeedableRng, SmallRng};
use tokio::sync::{Mutex, RwLock};

use crate::reconciler::Reconciler;
use crate::service_registry::ServiceRegistry;

mod proxy;

#[derive(Debug)]
pub struct LoadBalancer {
    service_registry: Arc<RwLock<ServiceRegistry>>,
    client: Client<HttpConnector>,
    rng: Arc<Mutex<SmallRng>>,
    reconciler_path: Arc<str>,
    reconciler: Arc<Reconciler>,
}

impl LoadBalancer {
    pub fn new(
        service_registry: Arc<RwLock<ServiceRegistry>>,
        reconciler_path: &str,
        reconciler: Reconciler,
    ) -> Self {
        let client = Client::new();
        let rng = Arc::new(Mutex::new(SmallRng::from_entropy()));
        let reconciler_path = Arc::from(reconciler_path);
        let reconciler = Arc::new(reconciler);

        Self {
            service_registry,
            client,
            rng,
            reconciler_path,
            reconciler,
        }
    }

    pub async fn start_on(&mut self, addr: Ipv4Addr, port: u16) -> Result<()> {
        let addr = SocketAddrV4::new(addr, port);
        let listener = TcpListener::bind(addr)?;

        self.start(listener).await
    }

    pub async fn start(&mut self, listener: TcpListener) -> Result<()> {
        let service_registry = Arc::clone(&self.service_registry);
        let rng = Arc::clone(&self.rng);
        let client = self.client.clone();
        let reconciliation_path = Arc::clone(&self.reconciler_path);
        let reconciler = Arc::clone(&self.reconciler);

        let service = make_service_fn(move |_| {
            let service_registry = Arc::clone(&service_registry);
            let rng = Arc::clone(&rng);
            let client = client.clone();
            let reconciliation_path = Arc::clone(&reconciliation_path);
            let reconciler = Arc::clone(&reconciler);

            async move {
                Ok::<_, Report>(service_fn(move |req| {
                    proxy::handle_request(
                        Arc::clone(&service_registry),
                        Arc::clone(&rng),
                        client.clone(),
                        Arc::clone(&reconciliation_path),
                        Arc::clone(&reconciler),
                        req,
                    )
                }))
            }
        });

        let server = Server::from_tcp(listener)?.serve(service);
        server.await?;

        Ok(())
    }
}

#[cfg(test)]
mod tests;

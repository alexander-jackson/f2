use std::collections::HashMap;
use std::net::{Ipv4Addr, SocketAddrV4, TcpListener};
use std::sync::Arc;

use color_eyre::eyre::{Report, Result};
use hyper::client::HttpConnector;
use hyper::service::{make_service_fn, service_fn};
use hyper::{Client, Server};
use rand::prelude::{SeedableRng, SmallRng};
use tokio::sync::Mutex;

use crate::config::Service;

type ServiceMap = HashMap<Service, Vec<SocketAddrV4>>;

mod proxy;

#[derive(Clone, Debug)]
pub struct LoadBalancer {
    service_map: Arc<ServiceMap>,
    client: Client<HttpConnector>,
    rng: Arc<Mutex<SmallRng>>,
}

impl LoadBalancer {
    pub fn new(service_map: ServiceMap) -> Self {
        let service_map = Arc::new(service_map);
        let client = Client::new();
        let rng = Arc::new(Mutex::new(SmallRng::from_entropy()));

        Self {
            service_map,
            client,
            rng,
        }
    }

    pub async fn start_on(&mut self, addr: Ipv4Addr, port: u16) -> Result<()> {
        let addr = SocketAddrV4::new(addr, port);
        let listener = TcpListener::bind(addr)?;

        self.start(listener).await
    }

    pub async fn start(&mut self, listener: TcpListener) -> Result<()> {
        let service_map = Arc::clone(&self.service_map);
        let rng = Arc::clone(&self.rng);
        let client = self.client.clone();

        let service = make_service_fn(move |_| {
            let service_map = Arc::clone(&service_map);
            let rng = Arc::clone(&rng);
            let client = client.clone();

            async move {
                Ok::<_, Report>(service_fn(move |req| {
                    proxy::handle_request(
                        Arc::clone(&service_map),
                        Arc::clone(&rng),
                        client.clone(),
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

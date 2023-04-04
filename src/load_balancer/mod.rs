use std::collections::HashMap;
use std::net::{Ipv4Addr, SocketAddrV4, TcpListener};
use std::sync::Arc;

use anyhow::{Error, Result};
use hyper::client::HttpConnector;
use hyper::service::{make_service_fn, service_fn};
use hyper::{Client, Server};
use rand::prelude::{SeedableRng, SmallRng};
use tokio::sync::Mutex;

use crate::common::{Container, Registry};
use crate::config::Service;
use crate::docker;

type ServiceMap = HashMap<Service, Vec<u16>>;

mod proxy;

#[derive(Clone, Debug)]
pub struct LoadBalancer {
    registry: Arc<Registry>,
    service_map: Arc<ServiceMap>,
    client: Client<HttpConnector>,
    rng: Arc<Mutex<SmallRng>>,
}

impl LoadBalancer {
    pub fn new(registry: Registry, service_map: ServiceMap) -> Self {
        let registry = Arc::new(registry);
        let service_map = Arc::new(service_map);
        let client = Client::new();
        let rng = Arc::new(Mutex::new(SmallRng::from_entropy()));

        Self {
            registry,
            service_map,
            client,
            rng,
        }
    }

    pub async fn start_on_port(&mut self, port: u16) -> Result<()> {
        let addr = SocketAddrV4::new(Ipv4Addr::LOCALHOST, port);
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
                Ok::<_, Error>(service_fn(move |req| {
                    proxy::handle_request(
                        Arc::clone(&service_map),
                        Arc::clone(&rng),
                        client.clone(),
                        req,
                    )
                }))
            }
        });

        // Spin up the auto-reloading functionality
        for service in self.service_map.keys() {
            let registry = Arc::clone(&self.registry);

            let container = Container {
                image: service.image.clone(),
                target_port: service.port,
            };

            let current_tag = service.tag.clone();

            tokio::spawn(async move {
                loop {
                    match check_for_newer_images(&container, &registry, &current_tag).await {
                        Ok(()) => unreachable!("Should never break out of the above function"),
                        Err(e) => {
                            tracing::warn!(error = ?e, "Encountered an error while checking for newer images");
                        }
                    }
                }
            });
        }

        let server = Server::from_tcp(listener)?.serve(service);
        server.await?;

        Ok(())
    }
}

#[tracing::instrument]
async fn check_for_newer_images(
    container: &Container,
    registry: &Registry,
    current_tag: &str,
) -> Result<()> {
    loop {
        tokio::time::sleep(tokio::time::Duration::from_secs(5)).await;

        if let Some(tag) =
            docker::registry::check_for_newer_tag(container, registry, current_tag).await?
        {
            tracing::info!(%tag, "Found a new tag in the Docker registry");

            // Boot the new container
            let binding =
                docker::api::create_and_start_on_random_port(container, registry, &tag).await?;

            tracing::info!(%binding, "Started a new container with the new tag");
        }
    }
}

#[cfg(test)]
mod tests;

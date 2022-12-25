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
        let listener = TcpListener::bind(&addr)?;

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
                image: service.app.clone(),
                target_port: service.port,
            };

            let current_tag = service.tag.clone();

            tokio::spawn(async move {
                loop {
                    match check_for_newer_images(&container, &registry, &current_tag).await {
                        Ok(()) => unreachable!("Should never break out of the above function"),
                        Err(e) => {
                            tracing::error!(error = ?e, "Encountered an error while checking for newer images");
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
mod tests {
    use std::collections::HashMap;
    use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4, TcpListener};

    use anyhow::{Error, Result};
    use hyper::header::HOST;
    use hyper::service::{make_service_fn, service_fn};
    use hyper::{Body, Client, Request, Response, Server};

    use crate::common::Registry;
    use crate::config::Service;
    use crate::load_balancer::LoadBalancer;

    fn create_service(host: &'static str) -> Service {
        Service {
            app: String::from("application"),
            tag: String::from("20220813-1803"),
            port: 6500,
            replicas: 1,
            host: String::from(host),
        }
    }

    async fn handler(response: &'static str) -> Result<Response<Body>> {
        Ok(Response::new(Body::from(response)))
    }

    async fn spawn_fixed_response_server(response: &'static str) -> Result<SocketAddr> {
        let addr = SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0);
        let listener = TcpListener::bind(&addr)?;

        let service = make_service_fn(move |_| async move {
            Ok::<_, Error>(service_fn(move |_| handler(response)))
        });

        let resolved_addr = listener.local_addr()?;

        tokio::spawn(async move {
            let server = Server::from_tcp(listener)
                .expect("Failed to create server")
                .serve(service);

            server.await.expect("Failed to run server");
        });

        Ok(resolved_addr)
    }

    async fn spawn_load_balancer(
        registry: Registry,
        service_map: super::ServiceMap,
    ) -> Result<SocketAddr> {
        let addr = SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0);
        let listener = TcpListener::bind(&addr)?;

        let resolved_addr = listener.local_addr()?;

        tokio::spawn(async move {
            let mut load_balancer = LoadBalancer::new(registry, service_map);

            load_balancer
                .start(listener)
                .await
                .expect("Failed to run load balancer");
        });

        Ok(resolved_addr)
    }

    #[tokio::test]
    async fn can_proxy_requests_based_on_host_header() -> Result<()> {
        let registry = Registry {
            base: None,
            repository: String::from("blah"),
            username: None,
            password: None,
        };

        let opentracker_addr = spawn_fixed_response_server("Hello from OpenTracker").await?;
        let blackboards_addr = spawn_fixed_response_server("Hello from Blackboards").await?;

        let mut service_map = HashMap::new();

        service_map.insert(
            create_service("opentracker.app"),
            vec![opentracker_addr.port()],
        );

        service_map.insert(
            create_service("blackboards.pl"),
            vec![blackboards_addr.port()],
        );

        let load_balancer_addr = spawn_load_balancer(registry, service_map).await?;

        let client = Client::new();

        let request = Request::builder()
            .uri(format!("http://{}", load_balancer_addr))
            .header(HOST, "blackboards.pl")
            .body(Body::empty())?;

        let mut response = client.request(request).await?;
        let bytes = hyper::body::to_bytes(response.body_mut()).await?;
        let body = std::str::from_utf8(&bytes)?;

        assert_eq!(body, "Hello from Blackboards");

        Ok(())
    }
}

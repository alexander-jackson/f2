use std::collections::HashMap;
use std::net::{Ipv4Addr, SocketAddrV4};
use std::ops::DerefMut;
use std::sync::Arc;

use anyhow::Result;
use hyper::client::HttpConnector;
use hyper::http::uri::PathAndQuery;
use hyper::service::{make_service_fn, service_fn};
use hyper::{Body, Client, Error, Request, Response, Server};
use rand::prelude::{SeedableRng, SliceRandom, SmallRng};
use tokio::sync::Mutex;

use crate::common::{Container, Registry};
use crate::config::Service;
use crate::docker;

type ServiceMap = HashMap<Service, Vec<u16>>;

#[derive(Clone, Debug)]
pub struct LoadBalancer {
    port: u16,
    registry: Arc<Registry>,
    service_map: Arc<ServiceMap>,
    client: Client<HttpConnector>,
    rng: Arc<Mutex<SmallRng>>,
}

impl LoadBalancer {
    pub fn new(port: u16, registry: Registry, service_map: ServiceMap) -> Self {
        let registry = Arc::new(registry);
        let service_map = Arc::new(service_map);
        let client = Client::new();
        let rng = Arc::new(Mutex::new(SmallRng::from_entropy()));

        Self {
            port,
            registry,
            service_map,
            client,
            rng,
        }
    }

    pub async fn start(&mut self) -> Result<()> {
        // Create the server itself on the given port
        let addr = SocketAddrV4::new(Ipv4Addr::LOCALHOST, self.port).into();

        let service_map = Arc::clone(&self.service_map);
        let client = self.client.clone();
        let rng = Arc::clone(&self.rng);

        let service = make_service_fn(move |_| {
            let client = client.clone();
            let rng = Arc::clone(&rng);
            let service_map = Arc::clone(&service_map);

            async move {
                Ok::<_, Error>(service_fn(move |req| {
                    proxy_request_downstream(
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

            let current_tag = service
                .tag
                .clone()
                .expect("Failed to get tag for enriched service");

            tokio::spawn(async move {
                loop {
                    match check_for_newer_images(&container, &registry, &current_tag).await {
                        Ok(()) => unreachable!("Should never break out of the above function"),
                        Err(e) => {
                            tracing::error!(error = ?e, "Encountered an error while checking for newer images")
                        }
                    }
                }
            });
        }

        let server = Server::bind(&addr).serve(service);
        server.await?;

        Ok(())
    }
}

async fn proxy_request_downstream(
    service_map: Arc<ServiceMap>,
    rng: Arc<Mutex<SmallRng>>,
    client: Client<HttpConnector>,
    mut req: Request<Body>,
) -> Result<Response<Body>, Error> {
    let host = req
        .headers()
        .get(hyper::header::HOST)
        .expect("Failed to get `host` header")
        .to_str()
        .expect("Invalid header supplied");

    let downstreams = service_map
        .iter()
        .find_map(|(service, downstreams)| (service.host == host).then_some(downstreams))
        .expect("Failed to find downstream hosts");

    let mut rng = rng.lock().await;

    let downstream = *downstreams
        .choose(rng.deref_mut())
        .expect("No available downstreams");

    let downstream_addr = SocketAddrV4::new(Ipv4Addr::LOCALHOST, downstream);

    let path = req
        .uri()
        .path_and_query()
        .map(PathAndQuery::as_str)
        .unwrap_or("/");

    tracing::info!(%downstream_addr, %path, "Proxing request to a downstream server");

    *req.uri_mut() = format!("http://{}{}", downstream_addr, path)
        .parse()
        .unwrap();

    client.request(req).await
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
            docker::registry::check_for_newer_tag(&container, &registry, &current_tag).await?
        {
            tracing::info!(%tag, "Found a new tag in the Docker registry");

            // Boot the new container
            let binding =
                docker::api::create_and_start_on_random_port(&container, &registry, &tag).await?;

            tracing::info!(%binding, "Started a new container with the new tag");
        }
    }
}

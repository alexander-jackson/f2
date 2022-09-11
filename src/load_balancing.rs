use std::net::{Ipv4Addr, SocketAddrV4};
use std::sync::{Arc, RwLock};

use anyhow::Result;
use hyper::client::HttpConnector;
use hyper::http::uri::PathAndQuery;
use hyper::service::{make_service_fn, service_fn};
use hyper::{Body, Client, Error, Request, Response, Server};
use rand::prelude::{SeedableRng, SliceRandom, SmallRng};

use crate::common::{Container, Registry};
use crate::docker;

#[derive(Clone, Debug)]
pub struct LoadBalancer {
    port: u16,
    container: Container,
    registry: Registry,
    downstreams: Arc<RwLock<Vec<u16>>>,
    client: Client<HttpConnector>,
    rng: SmallRng,
    current_tag: String,
}

impl LoadBalancer {
    pub fn new(
        port: u16,
        container: Container,
        registry: Registry,
        downstreams: Vec<u16>,
        current_tag: String,
    ) -> Self {
        let client = Client::new();
        let rng = SmallRng::from_entropy();

        Self {
            port,
            container,
            registry,
            downstreams: Arc::new(RwLock::new(downstreams)),
            client,
            rng,
            current_tag,
        }
    }

    pub async fn start(&mut self) -> Result<()> {
        // Create the server itself on the given port
        let addr = SocketAddrV4::new(Ipv4Addr::LOCALHOST, self.port).into();

        let container = self.container.clone();
        let registry = self.registry.clone();
        let current_tag = self.current_tag.clone();

        let make_service = make_service_fn(move |_| {
            let client = self.client.clone();
            let downstreams = self.downstreams.clone();

            let reader = downstreams.read().unwrap();
            let downstream = *reader
                .choose(&mut self.rng)
                .expect("No available downstreams");

            drop(reader);

            let downstream_addr = SocketAddrV4::new(Ipv4Addr::LOCALHOST, downstream);

            async move {
                Ok::<_, Error>(service_fn(move |req| {
                    handle_request(downstream_addr, client.clone(), req)
                }))
            }
        });

        // Spin up the auto-reloading functionality
        tokio::spawn(async move {
            loop {
                let result = check_for_newer_images(&container, &registry, &current_tag).await;

                match result {
                    Ok(()) => unreachable!("Should never break out of the above function"),
                    Err(e) => {
                        tracing::error!(error = ?e, "Encountered an error while checking for newer images")
                    }
                }
            }
        });

        let server = Server::bind(&addr).serve(make_service);
        server.await?;

        Ok(())
    }
}

async fn handle_request(
    downstream_addr: SocketAddrV4,
    client: Client<HttpConnector>,
    mut req: Request<Body>,
) -> Result<Response<Body>, Error> {
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

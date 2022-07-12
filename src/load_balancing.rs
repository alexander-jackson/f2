use std::net::{Ipv4Addr, SocketAddrV4};
use std::sync::{Arc, RwLock};

use anyhow::Result;
use hyper::client::HttpConnector;
use hyper::http::uri::PathAndQuery;
use hyper::service::{make_service_fn, service_fn};
use hyper::{Body, Client, Error, Request, Response, Server, StatusCode};
use rand::prelude::{SeedableRng, SliceRandom, SmallRng};

#[derive(Clone, Debug)]
pub struct Container {
    image: String,
    tag: String,
    port: u16,
}

impl Container {
    pub fn new(image: String, tag: String, port: u16) -> Self {
        Self { image, tag, port }
    }
}

#[derive(Clone, Debug)]
pub struct LoadBalancer {
    port: u16,
    container: Container,
    downstreams: Arc<RwLock<Vec<u16>>>,
    client: Client<HttpConnector>,
    rng: SmallRng,
}

impl LoadBalancer {
    pub fn new(port: u16, container: Container, downstreams: Vec<u16>) -> Self {
        let client = Client::new();
        let rng = SmallRng::from_entropy();

        Self {
            port,
            container,
            downstreams: Arc::new(RwLock::new(downstreams)),
            client,
            rng,
        }
    }

    pub async fn start(&mut self) -> Result<()> {
        // Create the server itself on the given port
        let addr = SocketAddrV4::new(Ipv4Addr::LOCALHOST, self.port).into();

        let make_service = make_service_fn(move |_| {
            let client = self.client.clone();
            let downstreams = self.downstreams.clone();

            let reader = downstreams.read().unwrap();
            let downstream = *reader
                .choose(&mut self.rng)
                .expect("No available downstreams");

            drop(reader);

            let downstream_addr = SocketAddrV4::new(Ipv4Addr::LOCALHOST, downstream);
            let container = self.container.clone();

            async move {
                Ok::<_, Error>(service_fn(move |req| {
                    handle_request(
                        downstreams.clone(),
                        downstream_addr,
                        container.clone(),
                        client.clone(),
                        req,
                    )
                }))
            }
        });

        let server = Server::bind(&addr).serve(make_service);
        server.await?;

        Ok(())
    }
}

async fn handle_request(
    downstreams: Arc<RwLock<Vec<u16>>>,
    downstream_addr: SocketAddrV4,
    container: Container,
    client: Client<HttpConnector>,
    mut req: Request<Body>,
) -> Result<Response<Body>, Error> {
    let path = req
        .uri()
        .path_and_query()
        .map(PathAndQuery::as_str)
        .unwrap_or("/");

    if path == "/spawn" {
        tracing::info!(%path, "Spawning new container to deal with downstream traffic");

        // Spawn the container
        let port = crate::docker::create_and_start_on_random_port(
            &container.image,
            &container.tag,
            container.port as u32,
        )
        .await
        .expect("Failed to create a new container");

        let mut writer = downstreams.write().unwrap();
        writer.push(port);

        let response = Response::builder()
            .status(StatusCode::CREATED)
            .body(Body::empty())
            .expect("Failed to create response");

        return Ok(response);
    }

    tracing::info!(%downstream_addr, %path, "Proxing request to a downstream server");

    *req.uri_mut() = format!("http://{}{}", downstream_addr, path)
        .parse()
        .unwrap();

    client.request(req).await
}

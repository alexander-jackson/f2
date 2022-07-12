use std::net::{Ipv4Addr, SocketAddrV4};

use anyhow::Result;
use hyper::client::HttpConnector;
use hyper::service::{make_service_fn, service_fn};
use hyper::{Body, Client, Error, Request, Response, Server};
use rand::prelude::SliceRandom;

#[derive(Clone, Debug)]
pub struct LoadBalancer {
    port: u16,
    downstreams: Vec<u16>,
    client: Client<HttpConnector>,
}

impl LoadBalancer {
    pub fn new(port: u16, downstreams: Vec<u16>) -> Self {
        let client = Client::new();

        Self {
            port,
            downstreams,
            client,
        }
    }

    pub async fn start(&self) -> Result<()> {
        // Create the server itself on the given port
        let addr = SocketAddrV4::new(Ipv4Addr::LOCALHOST, self.port).into();
        let mut rng = rand::thread_rng();

        let make_service = make_service_fn(move |_| {
            let client = self.client.clone();
            let downstream = self
                .downstreams
                .choose(&mut rng)
                .expect("No available downstreams");

            let downstream_addr = SocketAddrV4::new(Ipv4Addr::LOCALHOST, *downstream);
            tracing::info!(%downstream_addr, "Proxing a connection for the client");

            async move {
                Ok::<_, Error>(service_fn(move |req| {
                    handle_request(downstream_addr, client.clone(), req)
                }))
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
    let uri_string = format!(
        "http://{}{}",
        downstream_addr,
        req.uri()
            .path_and_query()
            .map(|x| x.as_str())
            .unwrap_or("/")
    );

    let uri = uri_string.parse().unwrap();
    *req.uri_mut() = uri;
    client.request(req).await
}

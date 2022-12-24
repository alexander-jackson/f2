use std::net::{Ipv4Addr, SocketAddrV4};
use std::sync::Arc;

use anyhow::{Context, Error, Result};
use hyper::client::HttpConnector;
use hyper::http::uri::PathAndQuery;
use hyper::{Body, Client, Request, Response};
use rand::prelude::{SliceRandom, SmallRng};
use tokio::sync::Mutex;

use crate::load_balancer::ServiceMap;

pub async fn handle_request(
    service_map: Arc<ServiceMap>,
    rng: Arc<Mutex<SmallRng>>,
    client: Client<HttpConnector>,
    mut req: Request<Body>,
) -> Result<Response<Body>, Error> {
    let host = req
        .headers()
        .get(hyper::header::HOST)
        .context("Failed to get `host` header")?
        .to_str()?;

    let downstreams = service_map
        .iter()
        .find_map(|(service, downstreams)| (service.host == host).then_some(downstreams))
        .context("Failed to find downstream hosts")?;

    let downstream = {
        let mut rng = rng.lock().await;

        *downstreams
            .choose(&mut *rng)
            .context("Failed to select downstream host")?
    };

    let downstream_addr = SocketAddrV4::new(Ipv4Addr::LOCALHOST, downstream);
    let path = req.uri().path_and_query().map_or("/", PathAndQuery::as_str);

    tracing::info!(%downstream_addr, %path, "Proxing request to a downstream server");

    *req.uri_mut() = format!("http://{downstream_addr}{path}").parse()?;

    Ok(client.request(req).await?)
}

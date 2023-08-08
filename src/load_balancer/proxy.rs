use std::net::SocketAddrV4;
use std::sync::Arc;

use color_eyre::eyre::{ContextCompat, Result};
use hyper::client::HttpConnector;
use hyper::http::uri::PathAndQuery;
use hyper::{Body, Client, Request, Response};
use rand::prelude::{SliceRandom, SmallRng};
use tokio::sync::Mutex;

use crate::load_balancer::ServiceMap;

fn compute_path_prefix_match(path: &str, prefix: Option<&str>) -> usize {
    let Some(prefix) = prefix else { return path.len() };

    path.strip_prefix(prefix).map_or(usize::MAX, str::len)
}

pub async fn handle_request(
    service_map: Arc<ServiceMap>,
    rng: Arc<Mutex<SmallRng>>,
    client: Client<HttpConnector>,
    mut req: Request<Body>,
) -> Result<Response<Body>> {
    let host = req
        .headers()
        .get(hyper::header::HOST)
        .context("Failed to get `host` header")?
        .to_str()?;

    let uri = req.uri();

    // Filter based on the host, then do path matching for longest length
    let downstreams = service_map
        .iter()
        .filter(|entry| entry.0.host == host)
        .min_by_key(|entry| compute_path_prefix_match(uri.path(), entry.0.path_prefix.as_deref()))
        .map(|entry| entry.1);

    let Some(downstreams) = downstreams else {
        let response = Response::builder().status(404).body(Body::empty())?;
        return Ok(response);
    };

    let downstream = {
        let mut rng = rng.lock().await;

        *downstreams
            .choose(&mut *rng)
            .context("Failed to select downstream host")?
    };

    let downstream_addr = SocketAddrV4::new(*downstream.ip(), downstream.port());
    let path_and_query = uri.path_and_query().map_or("/", PathAndQuery::as_str);

    tracing::info!(%downstream_addr, %path_and_query, "Proxing request to a downstream server");

    *req.uri_mut() = format!("http://{downstream_addr}{path_and_query}").parse()?;

    Ok(client.request(req).await?)
}

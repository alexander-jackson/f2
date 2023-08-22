use std::net::SocketAddrV4;
use std::sync::Arc;

use color_eyre::eyre::{ContextCompat, Result};
use hyper::client::HttpConnector;
use hyper::http::uri::PathAndQuery;
use hyper::{Body, Client, Request, Response};
use rand::prelude::{SliceRandom, SmallRng};
use tokio::sync::{Mutex, RwLock};

use crate::load_balancer::ServiceMap;
use crate::reconciler::Reconciler;

fn compute_path_prefix_match(path: &str, prefix: Option<&str>) -> usize {
    let Some(prefix) = prefix else { return path.len() };

    path.strip_prefix(prefix).map_or(usize::MAX, str::len)
}

pub async fn handle_request(
    service_map: Arc<RwLock<ServiceMap>>,
    rng: Arc<Mutex<SmallRng>>,
    client: Client<HttpConnector>,
    reconciler: Arc<Reconciler>,
    mut req: Request<Body>,
) -> Result<Response<Body>> {
    let uri = req.uri();

    if let Some(path_and_query) = uri.path_and_query() {
        if path_and_query.path() == reconciler.get_path() {
            reconciler.reconcile().await?;
            return Ok(Response::builder().status(200).body(Body::empty())?);
        }
    }

    let host = req
        .headers()
        .get(hyper::header::HOST)
        .context("Failed to get `host` header")?
        .to_str()?;

    // Filter based on the host, then do path matching for longest length
    let read_lock = service_map.read().await;
    let downstreams = read_lock
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

    drop(read_lock);

    let downstream_addr = SocketAddrV4::new(*downstream.ip(), downstream.port());
    let path_and_query = uri.path_and_query().map_or("/", PathAndQuery::as_str);

    tracing::info!(%downstream_addr, %path_and_query, "Proxing request to a downstream server");

    *req.uri_mut() = format!("http://{downstream_addr}{path_and_query}").parse()?;

    Ok(client.request(req).await?)
}

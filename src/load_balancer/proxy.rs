use std::net::SocketAddrV4;
use std::sync::Arc;

use color_eyre::eyre::{ContextCompat, Result};
use hyper::client::HttpConnector;
use hyper::http::uri::PathAndQuery;
use hyper::{Body, Client, Request, Response};
use rand::prelude::SmallRng;
use rand::RngCore;
use tokio::sync::{Mutex, RwLock};

use crate::docker::client::DockerClient;
use crate::reconciler::Reconciler;
use crate::service_registry::ServiceRegistry;

pub async fn handle_request<C: DockerClient>(
    service_registry: Arc<RwLock<ServiceRegistry>>,
    rng: Arc<Mutex<SmallRng>>,
    client: Client<HttpConnector>,
    reconciliation_path: Arc<str>,
    reconciler: Arc<Reconciler<C>>,
    mut req: Request<Body>,
) -> Result<Response<Body>> {
    let uri = req.uri();

    if let Some(path_and_query) = uri.path_and_query() {
        if path_and_query.path() == &*reconciliation_path {
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
    let read_lock = service_registry.read().await;

    let Some((downstreams, port)) = read_lock.find_downstreams(host, uri.path()) else {
        let response = Response::builder().status(404).body(Body::empty())?;
        return Ok(response);
    };

    let downstream = {
        let mut rng = rng.lock().await;
        let next = rng.next_u32() as usize;
        let normalised = next % downstreams.len();

        downstreams
            .get_index(normalised)
            .context("Failed to select downstream host")?
            .addr
    };

    drop(read_lock);

    let path_and_query = uri.path_and_query().map_or("/", PathAndQuery::as_str);

    tracing::info!(%downstream, %path_and_query, "Proxing request to a downstream server");

    let addr = SocketAddrV4::new(downstream, port);
    *req.uri_mut() = format!("http://{addr}{path_and_query}").parse()?;

    Ok(client.request(req).await?)
}

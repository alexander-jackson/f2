use std::net::SocketAddrV4;
use std::sync::Arc;

use color_eyre::eyre::{ContextCompat, Result};
use hyper::client::HttpConnector;
use hyper::http::uri::PathAndQuery;
use hyper::{Body, Client, Request, Response};
use rand::prelude::{SliceRandom, SmallRng};
use tokio::sync::{Mutex, RwLock};

use crate::reconciler::Reconciler;
use crate::service_registry::ServiceRegistry;

pub async fn handle_request(
    service_registry: Arc<RwLock<ServiceRegistry>>,
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
    let read_lock = service_registry.read().await;
    let downstreams = read_lock.find_downstreams(host, uri.path());

    let Some(downstreams) = downstreams else {
        let response = Response::builder().status(404).body(Body::empty())?;
        return Ok(response);
    };

    let (downstreams, port) = downstreams;

    let downstream = {
        let mut rng = rng.lock().await;

        // TODO: remove this
        let downstreams: Vec<_> = downstreams.iter().collect();

        downstreams
            .choose(&mut *rng)
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

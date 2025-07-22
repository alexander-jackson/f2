use std::net::SocketAddrV4;
use std::sync::Arc;

use color_eyre::eyre::{eyre, Result};
use http::header::AUTHORIZATION;
use http::Method;
use http_body_util::combinators::BoxBody;
use http_body_util::{BodyExt, Empty};
use hyper::body::{Body, Bytes};
use hyper::http::uri::PathAndQuery;
use hyper::{Request, Response};
use hyper_util::client::legacy::connect::HttpConnector;
use hyper_util::client::legacy::Client;
use rand::prelude::SmallRng;
use rand::RngCore;
use tokio::sync::{Mutex, RwLock};

use crate::ipc::MessageBus;
use crate::service_registry::ServiceRegistry;

pub async fn handle_request<B>(
    service_registry: Arc<RwLock<ServiceRegistry>>,
    rng: Arc<Mutex<SmallRng>>,
    client: Client<HttpConnector, B>,
    reconciliation_path: Arc<str>,
    message_bus: Arc<MessageBus>,
    mut req: Request<B>,
) -> Result<Response<BoxBody<Bytes, hyper::Error>>>
where
    B: Body + Send + Unpin + 'static,
    <B as Body>::Data: Send,
    <B as Body>::Error: std::error::Error + Send + Sync + 'static,
{
    let uri = req.uri();

    if let Some(response) =
        check_for_admin_commands(&uri, &req, &message_bus, &reconciliation_path, "").await?
    {
        return Ok(response);
    }

    let host = req
        .headers()
        .get(hyper::header::HOST)
        .ok_or_else(|| eyre!("failed to get `host` header for request to {uri}"))?
        .to_str()?;

    // Filter based on the host, then do path matching for longest length
    let read_lock = service_registry.read().await;

    let Some((downstreams, port)) = read_lock.find_downstreams(host, uri.path()) else {
        tracing::debug!(%host, %uri, "no downstreams found for request");

        return Ok(Response::builder().status(404).body(empty())?);
    };

    let downstream = {
        let mut rng = rng.lock().await;
        let next = rng.next_u32() as usize;
        let normalised = next % downstreams.len();

        downstreams
            .get_index(normalised)
            .ok_or_else(|| eyre!("no downstreams found for request to {uri} with host {host}"))?
            .addr
    };

    drop(read_lock);

    let path_and_query = uri.path_and_query().map_or("/", PathAndQuery::as_str);

    tracing::debug!(%downstream, %path_and_query, "proxing request to a downstream server");

    let addr = SocketAddrV4::new(downstream, port);
    *req.uri_mut() = format!("http://{addr}{path_and_query}").parse()?;

    Ok(client.request(req).await?.map(BoxBody::new))
}

async fn check_for_admin_commands<B>(
    uri: &http::uri::Uri,
    request: &Request<B>,
    message_bus: &Arc<MessageBus>,
    reconciliation_path: &Arc<str>,
    passphrase: &str,
) -> Result<Option<Response<BoxBody<Bytes, hyper::Error>>>> {
    if request.method() != Method::PUT {
        return Ok(None);
    }

    let Some(suffix) = uri.path_and_query().map(PathAndQuery::path) else {
        return Ok(None);
    };

    if *suffix == **reconciliation_path {
        if !is_admin_request_authorised(request, passphrase) {
            tracing::warn!(
                %reconciliation_path,
                "unauthorised PUT request to reconciliation path",
            );

            return Ok(Some(Response::builder().status(403).body(empty())?));
        }

        tracing::info!(
            %reconciliation_path,
            "informing the reconciler that a PUT request was received",
        );

        message_bus.send_reconciliation_request()?;
        return Ok(Some(Response::builder().status(200).body(empty())?));
    }

    if suffix == "/certificates" {
        if !is_admin_request_authorised(request, passphrase) {
            tracing::warn!("unauthorised PUT request to certificate resolver path");

            return Ok(Some(Response::builder().status(403).body(empty())?));
        }

        tracing::info!("informing the certificate resolver that a PUT request was received");

        message_bus.send_certificate_update_request()?;
        return Ok(Some(Response::builder().status(200).body(empty())?));
    }

    Ok(None)
}

fn is_admin_request_authorised<B>(request: &Request<B>, passphase: &str) -> bool {
    let Some(header) = request.headers().get(AUTHORIZATION) else {
        return false;
    };

    let Ok(content) = header.to_str() else {
        return false;
    };

    let Some(received_passphase) = content.strip_prefix("Bearer ") else {
        return false;
    };

    return received_passphase == passphase;
}

fn empty() -> BoxBody<Bytes, hyper::Error> {
    Empty::<Bytes>::new()
        .map_err(|never| match never {})
        .boxed()
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::time::Duration;

    use color_eyre::eyre::Result;
    use http::{Method, Request};
    use http_body_util::Empty;
    use hyper::body::Bytes;
    use hyper_util::client::legacy::connect::HttpConnector;
    use hyper_util::client::legacy::Client;
    use hyper_util::rt::TokioExecutor;
    use rand::rngs::SmallRng;
    use rand::SeedableRng;
    use tokio::sync::{Mutex, RwLock};

    use crate::ipc::MessageBus;
    use crate::load_balancer::proxy::{handle_request, is_admin_request_authorised};
    use crate::service_registry::ServiceRegistry;

    /// Gets all the dependencies required for calling `handle_request`.
    fn get_dependencies() -> (
        Arc<RwLock<ServiceRegistry>>,
        Arc<Mutex<SmallRng>>,
        Client<HttpConnector, Empty<Bytes>>,
        Arc<str>,
        Arc<MessageBus>,
    ) {
        let service_registry = Arc::new(RwLock::new(ServiceRegistry::default()));
        let rng = Arc::new(Mutex::new(SmallRng::from_entropy()));
        let client = Client::builder(TokioExecutor::new()).build_http();
        let reconciliation_path = Arc::from("/reconciliation");
        let message_bus = MessageBus::new();

        (
            service_registry,
            rng,
            client,
            reconciliation_path,
            Arc::clone(&message_bus),
        )
    }

    #[tokio::test]
    async fn can_trigger_reconciliation() -> Result<()> {
        let (service_registry, rng, client, reconciliation_path, message_bus) = get_dependencies();

        let req = Request::builder()
            .method(Method::PUT)
            .uri(format!("http://example.com{}", reconciliation_path))
            .body(Empty::<Bytes>::new())
            .unwrap();

        let response = handle_request(
            service_registry,
            rng,
            client,
            reconciliation_path,
            Arc::clone(&message_bus),
            req,
        )
        .await?;

        assert_eq!(response.status(), 200, "expected a 200 OK response");

        let message = tokio::time::timeout(
            Duration::from_millis(1),
            message_bus.receive_reconciliation_request(),
        )
        .await?;

        assert!(message.is_ok(), "expected a message from the channel");

        Ok(())
    }

    #[tokio::test]
    async fn can_trigger_certificate_updates() -> Result<()> {
        let (service_registry, rng, client, reconciliation_path, message_bus) = get_dependencies();

        let req = Request::builder()
            .method(Method::PUT)
            .uri("http://example.com/certificates")
            .body(Empty::<Bytes>::new())
            .unwrap();

        let response = handle_request(
            service_registry,
            rng,
            client,
            reconciliation_path,
            Arc::clone(&message_bus),
            req,
        )
        .await?;

        assert_eq!(response.status(), 200, "expected a 200 OK response");

        let message = tokio::time::timeout(
            Duration::from_millis(1),
            message_bus.receive_certificate_update_request(),
        )
        .await?;

        assert!(message.is_ok(), "expected a message from the channel");

        Ok(())
    }

    #[test]
    fn admins_can_be_authorised_for_commands() -> Result<()> {
        let passphase = "secret-passphrase";

        let request = Request::builder()
            .method(Method::PUT)
            .header("Authorization", format!("Bearer {}", passphase))
            .uri("http://example.com/reconciliation")
            .body(Empty::<Bytes>::new())?;

        assert!(is_admin_request_authorised(&request, passphase));

        Ok(())
    }

    #[test]
    fn requests_with_invalid_passphrase_are_rejected() -> Result<()> {
        let passphase = "secret-passphrase";

        let request = Request::builder()
            .method(Method::PUT)
            .header("Authorization", "Bearer wrong-passphrase")
            .uri("http://example.com/reconciliation")
            .body(Empty::<Bytes>::new())?;

        assert!(!is_admin_request_authorised(&request, passphase));

        Ok(())
    }

    #[test]
    fn requests_with_no_passphrase_are_rejected() -> Result<()> {
        let passphase = "secret-passphrase";

        let request = Request::builder()
            .method(Method::PUT)
            .uri("http://example.com/reconciliation")
            .body(Empty::<Bytes>::new())?;

        assert!(!is_admin_request_authorised(&request, passphase));

        Ok(())
    }
}

use std::net::SocketAddrV4;
use std::sync::Arc;

use color_eyre::eyre::{eyre, Result};
use http::header::HOST;
use http::{Method, Version};
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
    req: Request<B>,
) -> Result<Response<BoxBody<Bytes, hyper::Error>>>
where
    B: Body + Send + Unpin + 'static,
    <B as Body>::Data: Send,
    <B as Body>::Error: std::error::Error + Send + Sync + 'static,
{
    let uri = req.uri();

    if req.method() == Method::PUT {
        match uri.path_and_query() {
            Some(suffix) if suffix.path() == &*reconciliation_path => {
                tracing::info!(
                    %reconciliation_path,
                    "informing the reconciler that a PUT request was received",
                );

                message_bus.send_reconciliation_request()?;
                return Ok(Response::builder().status(200).body(empty())?);
            }
            Some(suffix) if suffix.path() == "/certificates" => {
                tracing::info!(
                    "informing the certificate resolver that a PUT request was received"
                );

                message_bus.send_certificate_update_request()?;
                return Ok(Response::builder().status(200).body(empty())?);
            }
            _ => {}
        }
    }

    let host = extract_host(&req)?;

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

    let addr = SocketAddrV4::new(downstream, port);
    let path_and_query = uri.path_and_query().map_or("/", PathAndQuery::as_str);

    let target_uri = format!("http://{addr}{path_and_query}").parse()?;

    let mut mapped = map_request(req)?;
    *mapped.uri_mut() = target_uri;

    Ok(client.request(mapped).await?.map(BoxBody::new))
}

fn extract_host<B>(req: &Request<B>) -> Result<&str> {
    let uri = req.uri();

    let host = match req.version() {
        Version::HTTP_11 => req.headers().get(HOST).and_then(|h| h.to_str().ok()),
        Version::HTTP_2 => req.uri().authority().map(|a| a.as_str()),
        _ => None,
    }
    .ok_or_else(|| eyre!("failed to get host information for request to {uri}"))?;

    Ok(host)
}

fn map_request<B>(original: Request<B>) -> Result<Request<B>> {
    let uri = original.uri();

    let mut request = Request::builder()
        .method(original.method())
        .uri(uri)
        .version(Version::HTTP_11);

    for (name, value) in original.headers() {
        if !name.as_str().starts_with(':') && name != "connection" {
            request = request.header(name, value);
        }
    }

    let request = request.body(original.into_body())?;

    Ok(request)
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
    use http::header::ACCEPT;
    use http::{HeaderValue, Method, Request, Uri, Version};
    use http_body_util::Empty;
    use hyper::body::Bytes;
    use hyper_util::client::legacy::connect::HttpConnector;
    use hyper_util::client::legacy::Client;
    use hyper_util::rt::TokioExecutor;
    use rand::rngs::SmallRng;
    use rand::SeedableRng;
    use tokio::sync::{Mutex, RwLock};

    use crate::ipc::MessageBus;
    use crate::load_balancer::proxy::{extract_host, handle_request, map_request};
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
    async fn can_cause_reconciliation() -> Result<()> {
        let (service_registry, rng, client, reconciliation_path, message_bus) = get_dependencies();

        let req = Request::builder()
            .method("PUT")
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
    async fn can_cause_certificate_updates() -> Result<()> {
        let (service_registry, rng, client, reconciliation_path, message_bus) = get_dependencies();

        let req = Request::builder()
            .method("PUT")
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
    fn can_extract_hosts_for_http_11() -> Result<()> {
        let req = Request::builder()
            .method(Method::GET)
            .uri("http://example.com/path?query=1")
            .version(Version::HTTP_11)
            .header("Host", "example.com")
            .body(Empty::<Bytes>::new())?;

        let host = extract_host(&req)?;

        assert_eq!(host, "example.com");

        Ok(())
    }

    #[test]
    fn can_extract_hosts_for_http_2() -> Result<()> {
        let req = Request::builder()
            .method(Method::GET)
            .uri("http://example.com/path?query=1")
            .version(Version::HTTP_2)
            .body(Empty::<Bytes>::new())?;

        let host = extract_host(&req)?;

        assert_eq!(host, "example.com");

        Ok(())
    }

    #[test]
    fn can_map_requests_from_http2_to_http11() -> Result<()> {
        let method = Method::GET;
        let uri: Uri = "http://example.com/path?query=1".parse()?;
        let version = Version::HTTP_2;

        let header_name = ACCEPT;
        let header_value = HeaderValue::from_static("application/json");

        let req = Request::builder()
            .method(&method)
            .uri(&uri)
            .version(version)
            .header(&header_name, &header_value)
            .body(Empty::<Bytes>::new())?;

        let mapped = map_request(req)?;

        assert_eq!(mapped.method(), method);
        assert_eq!(mapped.uri(), &uri);
        assert_eq!(mapped.version(), Version::HTTP_11);
        assert_eq!(mapped.headers().get(header_name), Some(&header_value));

        Ok(())
    }
}

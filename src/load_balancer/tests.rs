use std::collections::HashMap;
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};
use std::path::PathBuf;
use std::sync::Arc;

use color_eyre::eyre::Result;
use http_body_util::{BodyExt, Full};
use hyper::body::{Bytes, Incoming};
use hyper::header::HOST;
use hyper::service::service_fn;
use hyper::{Request, Response, StatusCode};
use hyper_util::client::legacy::connect::HttpConnector;
use hyper_util::client::legacy::Client;
use hyper_util::rt::{TokioExecutor, TokioIo};
use hyper_util::server::conn::auto::Builder;
use tokio::net::TcpListener;
use tokio::sync::RwLock;

use crate::args::ConfigurationLocation;
use crate::config::{AlbConfig, Config, Service};
use crate::docker::api::StartedContainerDetails;
use crate::docker::models::ContainerId;
use crate::load_balancer::LoadBalancer;
use crate::reconciler::tests::FakeDockerClient;
use crate::reconciler::Reconciler;
use crate::service_registry::ServiceRegistry;

fn create_service<T: Into<Option<&'static str>>>(
    host: &'static str,
    port: u16,
    path_prefix: T,
) -> Service {
    Service {
        port,
        host: String::from(host),
        path_prefix: path_prefix.into().map(ToOwned::to_owned),
        ..Default::default()
    }
}

fn add_container(service_registry: &mut ServiceRegistry, name: &str) {
    let details = StartedContainerDetails {
        id: ContainerId(String::from("6cd915f16ab3")),
        addr: Ipv4Addr::LOCALHOST,
    };

    service_registry.add_container(name, details);
}

async fn handler(response: &'static str) -> Result<Response<Full<Bytes>>> {
    Ok(Response::new(Full::from(response)))
}

async fn handle_health_checks(req: Request<Incoming>) -> Result<Response<Full<Bytes>>> {
    let response = match req.uri().path() {
        "/health" => Response::new(Full::default()),
        _ => Response::builder().status(404).body(Full::default())?,
    };

    Ok(response)
}

async fn spawn_fixed_response_server(response: &'static str) -> Result<SocketAddr> {
    let addr = SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0);
    let listener = TcpListener::bind(&addr).await?;

    let resolved_addr = listener.local_addr()?;

    tokio::spawn(async move {
        loop {
            let (stream, _) = listener.accept().await.unwrap();
            let io = TokioIo::new(stream);

            Builder::new(TokioExecutor::new())
                .serve_connection(io, service_fn(move |_| handler(response)))
                .await
                .unwrap();
        }
    });

    Ok(resolved_addr)
}

async fn spawn_load_balancer(service_registry: ServiceRegistry) -> Result<SocketAddr> {
    let addr = SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0);
    let listener = TcpListener::bind(&addr).await?;

    let resolved_addr = listener.local_addr()?;
    let service_registry = Arc::new(RwLock::new(service_registry));

    tokio::spawn(async move {
        let load_balancer = LoadBalancer::new(
            Arc::clone(&service_registry),
            "/reconciliation",
            Reconciler::new(
                Arc::clone(&service_registry),
                ConfigurationLocation::Filesystem(PathBuf::new()),
                Config {
                    alb: AlbConfig {
                        addr: Ipv4Addr::LOCALHOST,
                        port: 5000,
                        reconciliation: String::from("/reconciliation"),
                        tls: None,
                        mtls: None,
                    },
                    secrets: None,
                    services: HashMap::new(),
                },
                FakeDockerClient::default(),
            ),
        );

        load_balancer
            .start(listener, None, None)
            .await
            .expect("Failed to run load balancer");
    });

    Ok(resolved_addr)
}

#[tokio::test]
async fn can_proxy_requests_based_on_host_header() -> Result<()> {
    let opentracker_addr = spawn_fixed_response_server("Hello from OpenTracker").await?;
    let blackboards_addr = spawn_fixed_response_server("Hello from Blackboards").await?;

    let mut service_registry = ServiceRegistry::new();

    service_registry.define(
        "opentracker",
        create_service("opentracker.app", opentracker_addr.port(), None),
    );

    service_registry.define(
        "blackboards",
        create_service("blackboards.pl", blackboards_addr.port(), None),
    );

    add_container(&mut service_registry, "opentracker");
    add_container(&mut service_registry, "blackboards");

    let load_balancer_addr = spawn_load_balancer(service_registry).await?;

    let client = Client::builder(TokioExecutor::new()).build_http();

    let request = Request::builder()
        .uri(format!("http://{}", load_balancer_addr))
        .header(HOST, "blackboards.pl")
        .body(Full::default())?;

    let body = get_response_body(&client, request).await?;

    assert_eq!(body, "Hello from Blackboards");

    Ok(())
}

#[tokio::test]
async fn request_paths_are_proxied_downstream() -> Result<()> {
    let host = "opentracker.app";

    let addr = SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0);
    let listener = TcpListener::bind(&addr).await?;

    let resolved_addr = listener.local_addr()?;

    tokio::spawn(async move {
        loop {
            let (stream, _) = listener.accept().await.unwrap();
            let io = TokioIo::new(stream);

            Builder::new(TokioExecutor::new())
                .serve_connection(io, service_fn(move |req| handle_health_checks(req)))
                .await
                .unwrap();
        }
    });

    let mut service_registry = ServiceRegistry::new();

    service_registry.define("service", create_service(host, resolved_addr.port(), None));
    add_container(&mut service_registry, "service");

    let load_balancer_addr = spawn_load_balancer(service_registry).await?;

    let client = Client::builder(TokioExecutor::new()).build_http();

    let request = Request::builder()
        .uri(format!("http://{}/health", load_balancer_addr))
        .header(HOST, host)
        .body(Full::<Bytes>::default())?;

    let response = client.request(request).await?;

    assert_eq!(response.status(), StatusCode::OK);

    let request = Request::builder()
        .uri(format!("http://{}/something-else", load_balancer_addr))
        .header(HOST, host)
        .body(Full::default())?;

    let response = client.request(request).await?;

    assert_eq!(response.status(), StatusCode::NOT_FOUND);

    Ok(())
}

async fn get_response_body(
    client: &Client<HttpConnector, Full<Bytes>>,
    request: Request<Full<Bytes>>,
) -> Result<String> {
    let mut response = client.request(request).await?;
    let body = response.body_mut();

    let bytes = body.collect().await?;
    let text = String::from_utf8(bytes.to_bytes().to_vec())?;

    Ok(text)
}

#[tokio::test]
async fn can_proxy_downstream_based_on_path_prefixes() -> Result<()> {
    let frontend_reply = "Hello from the frontend";
    let backend_reply = "Hello from the backend";

    let frontend_addr = spawn_fixed_response_server(frontend_reply).await?;
    let backend_addr = spawn_fixed_response_server(backend_reply).await?;

    // 2 services on the same host, different paths
    let host = "opentracker.app";
    let mut service_registry = ServiceRegistry::new();

    service_registry.define("frontend", create_service(host, frontend_addr.port(), None));
    service_registry.define("backend", create_service(host, backend_addr.port(), "/api"));

    add_container(&mut service_registry, "frontend");
    add_container(&mut service_registry, "backend");

    let load_balancer_addr = spawn_load_balancer(service_registry).await?;

    let client = Client::builder(TokioExecutor::new()).build_http();

    let request = Request::builder()
        .uri(format!("http://{load_balancer_addr}/health"))
        .header(HOST, "opentracker.app")
        .body(Full::default())?;

    assert_eq!(get_response_body(&client, request).await?, frontend_reply);

    let request = Request::builder()
        .uri(format!("http://{load_balancer_addr}/api/health"))
        .header(HOST, "opentracker.app")
        .body(Full::default())?;

    assert_eq!(get_response_body(&client, request).await?, backend_reply);

    Ok(())
}

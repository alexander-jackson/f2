use std::collections::{HashMap, HashSet};
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};
use std::sync::Arc;

use arc_swap::ArcSwap;
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

use crate::config::{AlbConfig, Config, Route, Scheme, Service};
use crate::docker::api::StartedContainerDetails;
use crate::docker::models::ContainerId;
use crate::ipc::MessageBus;
use crate::load_balancer::LoadBalancer;
use crate::service_registry::ServiceRegistry;

fn create_service<T: Into<Option<&'static str>>>(
    host: &'static str,
    port: u16,
    path_prefix: T,
) -> Service {
    Service {
        routes: HashSet::from([Route {
            host: String::from(host),
            prefix: path_prefix.into().map(ToOwned::to_owned),
            port,
        }]),
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

    let config = Config {
        alb: AlbConfig {
            addr: Ipv4Addr::LOCALHOST,
            ports: HashMap::from([(Scheme::Http, resolved_addr.port())]),
            reconciliation: String::from("/reconciliation"),
            tls: None,
            mtls: None,
        },
        secrets: None,
        services: HashMap::new(),
    };

    let config = Arc::new(ArcSwap::from_pointee(config));
    let message_bus = MessageBus::new();

    tokio::spawn(async move {
        let message_bus = Arc::clone(&message_bus);
        let load_balancer = LoadBalancer::new(service_registry, config, message_bus);

        let listeners = HashMap::from([(Scheme::Http, listener)]);

        load_balancer
            .run(listeners, None, None)
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

    let addr = spawn_load_balancer(service_registry).await?;
    let client = Client::builder(TokioExecutor::new()).build_http();

    let request = Request::builder()
        .uri(format!("http://{}", addr))
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

    let addr = spawn_load_balancer(service_registry).await?;
    let client = Client::builder(TokioExecutor::new()).build_http();

    let request = Request::builder()
        .uri(format!("http://{}/health", addr))
        .header(HOST, host)
        .body(Full::<Bytes>::default())?;

    let response = client.request(request).await?;

    assert_eq!(response.status(), StatusCode::OK);

    let request = Request::builder()
        .uri(format!("http://{}/something-else", addr))
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

    let addr = spawn_load_balancer(service_registry).await?;
    let client = Client::builder(TokioExecutor::new()).build_http();

    let request = Request::builder()
        .uri(format!("http://{}/health", addr))
        .header(HOST, "opentracker.app")
        .body(Full::default())?;

    assert_eq!(get_response_body(&client, request).await?, frontend_reply);

    let request = Request::builder()
        .uri(format!("http://{}/api/health", addr))
        .header(HOST, "opentracker.app")
        .body(Full::default())?;

    assert_eq!(get_response_body(&client, request).await?, backend_reply);

    Ok(())
}

#[tokio::test]
async fn can_proxy_to_different_ports_based_on_route_configuration() -> Result<()> {
    let internal_reply = "Hello from the internal service";
    let external_reply = "Hello from the external service";

    let addr = SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0);
    let internal_listener = TcpListener::bind(&addr).await?;
    let internal_addr = internal_listener.local_addr()?;

    let external_listener = TcpListener::bind(&addr).await?;
    let external_addr = external_listener.local_addr()?;

    tokio::spawn(async move {
        loop {
            // handle a connection on the internal service, then on the external service
            let (stream, _) = internal_listener.accept().await.unwrap();
            let io = TokioIo::new(stream);

            tokio::spawn(async move {
                Builder::new(TokioExecutor::new())
                    .serve_connection(io, service_fn(move |_| handler(internal_reply)))
                    .await
                    .unwrap();
            });

            let (stream, _) = external_listener.accept().await.unwrap();
            let io = TokioIo::new(stream);

            tokio::spawn(async move {
                Builder::new(TokioExecutor::new())
                    .serve_connection(io, service_fn(move |_| handler(external_reply)))
                    .await
                    .unwrap();
            });
        }
    });

    // 2 services on the same host, different paths
    let name = "opentracker";
    let internal_host = "internal.opentracker.app";
    let external_host = "external.opentracker.app";
    let mut service_registry = ServiceRegistry::new();

    let service = Service {
        routes: HashSet::from([
            Route {
                host: String::from(internal_host),
                prefix: None,
                port: internal_addr.port(),
            },
            Route {
                host: String::from(external_host),
                prefix: None,
                port: external_addr.port(),
            },
        ]),
        ..Default::default()
    };

    service_registry.define(name, service);

    add_container(&mut service_registry, name);

    let addr = spawn_load_balancer(service_registry).await?;
    let client = Client::builder(TokioExecutor::new()).build_http();

    let request = Request::builder()
        .uri(format!("http://{}/", addr))
        .header(HOST, internal_host)
        .body(Full::default())?;

    assert_eq!(get_response_body(&client, request).await?, internal_reply);

    let request = Request::builder()
        .uri(format!("http://{}/", addr))
        .header(HOST, external_host)
        .body(Full::default())?;

    assert_eq!(get_response_body(&client, request).await?, external_reply);

    Ok(())
}

use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4, TcpListener};

use anyhow::{Error, Result};
use hyper::client::HttpConnector;
use hyper::header::HOST;
use hyper::service::{make_service_fn, service_fn};
use hyper::{Body, Client, Request, Response, Server, StatusCode};

use crate::common::Registry;
use crate::config::Service;
use crate::load_balancer::LoadBalancer;

fn some_registry() -> Registry {
    Registry {
        base: None,
        repository: String::from("blah"),
        username: None,
        password: None,
    }
}

fn create_service<T: Into<Option<&'static str>>>(host: &'static str, path_prefix: T) -> Service {
    Service {
        app: String::from("application"),
        tag: String::from("20220813-1803"),
        port: 6500,
        replicas: 1,
        host: String::from(host),
        path_prefix: path_prefix.into().map(ToOwned::to_owned),
    }
}

async fn handler(response: &'static str) -> Result<Response<Body>> {
    Ok(Response::new(Body::from(response)))
}

async fn handle_health_checks(req: Request<Body>) -> Result<Response<Body>> {
    let response = match req.uri().path() {
        "/health" => Response::new(Body::empty()),
        _ => Response::builder().status(404).body(Body::empty())?,
    };

    Ok(response)
}

async fn spawn_fixed_response_server(response: &'static str) -> Result<SocketAddr> {
    let addr = SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0);
    let listener = TcpListener::bind(&addr)?;

    let service =
        make_service_fn(
            move |_| async move { Ok::<_, Error>(service_fn(move |_| handler(response))) },
        );

    let resolved_addr = listener.local_addr()?;

    tokio::spawn(async move {
        let server = Server::from_tcp(listener)
            .expect("Failed to create server")
            .serve(service);

        server.await.expect("Failed to run server");
    });

    Ok(resolved_addr)
}

async fn spawn_load_balancer(service_map: super::ServiceMap) -> Result<SocketAddr> {
    let addr = SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0);
    let listener = TcpListener::bind(&addr)?;

    let resolved_addr = listener.local_addr()?;

    tokio::spawn(async move {
        let registry = some_registry();
        let mut load_balancer = LoadBalancer::new(registry, service_map);

        load_balancer
            .start(listener)
            .await
            .expect("Failed to run load balancer");
    });

    Ok(resolved_addr)
}

#[tokio::test]
async fn can_proxy_requests_based_on_host_header() -> Result<()> {
    let opentracker_addr = spawn_fixed_response_server("Hello from OpenTracker").await?;
    let blackboards_addr = spawn_fixed_response_server("Hello from Blackboards").await?;

    let service_map = [
        (
            create_service("opentracker.app", None),
            vec![opentracker_addr.port()],
        ),
        (
            create_service("blackboards.pl", None),
            vec![blackboards_addr.port()],
        ),
    ]
    .into_iter()
    .collect();

    let load_balancer_addr = spawn_load_balancer(service_map).await?;

    let client = Client::new();

    let request = Request::builder()
        .uri(format!("http://{}", load_balancer_addr))
        .header(HOST, "blackboards.pl")
        .body(Body::empty())?;

    let mut response = client.request(request).await?;
    let bytes = hyper::body::to_bytes(response.body_mut()).await?;
    let body = std::str::from_utf8(&bytes)?;

    assert_eq!(body, "Hello from Blackboards");

    Ok(())
}

#[tokio::test]
async fn request_paths_are_proxied_downstream() -> Result<()> {
    let host = "opentracker.app";

    let addr = SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0);
    let listener = TcpListener::bind(&addr)?;

    let service = make_service_fn(move |_| async move {
        Ok::<_, Error>(service_fn(move |req| handle_health_checks(req)))
    });

    let resolved_addr = listener.local_addr()?;

    tokio::spawn(async move {
        let server = Server::from_tcp(listener)
            .expect("Failed to create server")
            .serve(service);

        server.await.expect("Failed to run server");
    });

    let service_map = [(create_service(host, None), vec![resolved_addr.port()])]
        .into_iter()
        .collect();

    let load_balancer_addr = spawn_load_balancer(service_map).await?;

    let client = Client::new();

    let request = Request::builder()
        .uri(format!("http://{}/health", load_balancer_addr))
        .header(HOST, host)
        .body(Body::empty())?;

    let response = client.request(request).await?;

    assert_eq!(response.status(), StatusCode::OK);

    let request = Request::builder()
        .uri(format!("http://{}/something-else", load_balancer_addr))
        .header(HOST, host)
        .body(Body::empty())?;

    let response = client.request(request).await?;

    assert_eq!(response.status(), StatusCode::NOT_FOUND);

    Ok(())
}

async fn get_response_body(
    client: &Client<HttpConnector>,
    request: Request<Body>,
) -> Result<String> {
    let mut response = client.request(request).await?;
    let body = response.body_mut();
    let bytes = hyper::body::to_bytes(body).await?;
    let text = String::from_utf8(bytes.to_vec())?;

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
    let service_map = [
        (create_service(host, None), vec![frontend_addr.port()]),
        (create_service(host, "/api"), vec![backend_addr.port()]),
    ]
    .into_iter()
    .collect();

    let load_balancer_addr = spawn_load_balancer(service_map).await?;

    let client = Client::new();

    let request = Request::builder()
        .uri(format!("http://{load_balancer_addr}/health"))
        .header(HOST, "opentracker.app")
        .body(Body::empty())?;

    assert_eq!(get_response_body(&client, request).await?, frontend_reply);

    let request = Request::builder()
        .uri(format!("http://{load_balancer_addr}/api/health"))
        .header(HOST, "opentracker.app")
        .body(Body::empty())?;

    assert_eq!(get_response_body(&client, request).await?, backend_reply);

    Ok(())
}

use std::collections::HashMap;
use std::net::SocketAddrV4;
use std::sync::Arc;

use arc_swap::ArcSwap;
use color_eyre::eyre::Result;
use docker::client::{Client, DockerClient};
use rsa::RsaPrivateKey;
use service_registry::ServiceRegistry;
use tokio::net::TcpListener;
use tokio::sync::RwLock;
use tracing::level_filters::LevelFilter;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::EnvFilter;

use crate::args::Args;
use crate::common::Container;
use crate::config::{Config, Service};
use crate::docker::api::create_and_start_container;
use crate::load_balancer::LoadBalancer;
use crate::reconciler::Reconciler;

mod args;
mod common;
mod config;
mod crypto;
mod docker;
mod health;
mod load_balancer;
mod reconciler;
mod service_registry;

fn setup() -> Result<()> {
    color_eyre::install()?;

    let fmt_layer = tracing_subscriber::fmt::layer();
    let env_filter_layer = EnvFilter::builder()
        .with_default_directive(LevelFilter::INFO.into())
        .from_env()?;

    tracing_subscriber::registry()
        .with(fmt_layer)
        .with(env_filter_layer)
        .init();

    Ok(())
}

#[tokio::main]
async fn main() -> Result<()> {
    setup()?;

    let args = Args::parse()?;
    let config = Arc::new(ArcSwap::from_pointee(
        Config::from_location(&args.config_location).await?,
    ));

    let alb_config = &config.load().alb;

    let addr = alb_config.addr;
    let port = alb_config.port;
    let tls = alb_config.tls.clone();
    let mtls = alb_config.mtls.clone();

    let mut service_registry = ServiceRegistry::new();
    let private_key = config.load().get_private_key().await?;

    let docker_client = Client::new("/var/run/docker.sock");
    let services = &config.load().services;

    start_services(
        &docker_client,
        services,
        &mut service_registry,
        private_key.as_ref(),
    )
    .await?;

    let service_registry = Arc::new(RwLock::new(service_registry));
    let reconciliation_path = alb_config.reconciliation.clone();

    let reconciler = Reconciler::new(
        Arc::clone(&service_registry),
        args.config_location.clone(),
        Arc::clone(&config),
        docker_client,
    );

    let listener = TcpListener::bind(SocketAddrV4::new(addr, port)).await?;
    let load_balancer = LoadBalancer::new(
        service_registry,
        &reconciliation_path,
        reconciler,
        Arc::clone(&config),
    );

    load_balancer.start(listener, tls, mtls).await?;

    Ok(())
}

async fn start_services<C: DockerClient>(
    client: &C,
    services: &HashMap<String, Service>,
    service_registry: &mut ServiceRegistry,
    private_key: Option<&RsaPrivateKey>,
) -> Result<()> {
    for (name, service) in services {
        service_registry.define(name, service.clone());

        let tag = &service.tag;
        let container = Container::from(service);

        tracing::info!("Starting {name} with tag {tag}");

        for _ in 0..service.replicas.get() {
            let details = create_and_start_container(client, &container, tag, private_key).await?;
            service_registry.add_container(name, details);
        }
    }

    Ok(())
}

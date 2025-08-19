use std::collections::HashMap;
use std::net::SocketAddrV4;
use std::sync::Arc;

use arc_swap::ArcSwap;
use color_eyre::eyre::{eyre, Result};
use docker::client::{Client, DockerClient};
use rsa::RsaPrivateKey;
use service_registry::ServiceRegistry;
use tokio::net::TcpListener;
use tokio::signal::unix::SignalKind;
use tokio::sync::RwLock;
use tracing::level_filters::LevelFilter;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::EnvFilter;

use crate::args::Args;
use crate::common::Container;
use crate::config::{Config, Service};
use crate::docker::api::create_and_start_container;
use crate::ipc::MessageBus;
use crate::load_balancer::LoadBalancer;
use crate::reconciler::Reconciler;

mod args;
mod common;
mod config;
mod crypto;
mod docker;
mod health;
mod ipc;
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
    let tls = alb_config.tls.clone();
    let mtls = alb_config.mtls.clone();

    let mut service_registry = ServiceRegistry::new();
    let private_key = config.load().get_private_key().await?;

    let docker_client = Client::default();
    let services = &config.load().services;

    start_services(
        &docker_client,
        services,
        &mut service_registry,
        private_key.as_ref(),
    )
    .await?;

    let service_registry = Arc::new(RwLock::new(service_registry));
    let message_bus = MessageBus::new();

    let reconciler = Reconciler::new(
        Arc::clone(&service_registry),
        args.config_location.clone(),
        Arc::clone(&config),
        docker_client,
        Arc::clone(&message_bus),
    );

    let mut listeners = HashMap::new();

    for (protocol, port) in alb_config.ports.iter() {
        let listener = TcpListener::bind(SocketAddrV4::new(addr, *port)).await?;
        listeners.insert(protocol.clone(), listener);
    }

    let load_balancer = LoadBalancer::new(service_registry, config, message_bus);
    let shutdown_signal = handle_shutdown_signal();

    tokio::try_join!(
        load_balancer.run(listeners, tls, mtls),
        reconciler.run(),
        shutdown_signal
    )?;

    tracing::info!("shutting down gracefully, all components have completed their tasks");

    Ok(())
}

async fn handle_shutdown_signal() -> Result<()> {
    let ctrl_c = async {
        tokio::signal::ctrl_c()
            .await
            .expect("failed to install Ctrl+C handler");
    };

    let terminate = async {
        tokio::signal::unix::signal(SignalKind::terminate())
            .expect("failed to install signal handler")
            .recv()
            .await;
    };

    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }

    Err(eyre!("shutdown signal received, exiting..."))
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

        tracing::info!(%name, %tag, "starting service");

        for _ in 0..service.replicas.get() {
            let details = create_and_start_container(client, &container, tag, private_key).await?;
            service_registry.add_container(name, details);
        }
    }

    Ok(())
}

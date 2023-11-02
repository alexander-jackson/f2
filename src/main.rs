use std::collections::HashMap;
use std::net::Ipv4Addr;
use std::str::FromStr;
use std::sync::Arc;

use color_eyre::eyre::Result;
use rsa::RsaPrivateKey;
use service_registry::ServiceRegistry;
use tokio::sync::RwLock;

use crate::args::Args;
use crate::common::Container;
use crate::config::{AuxillaryService, Config, Service};
use crate::docker::api::create_and_start_container;
use crate::load_balancer::LoadBalancer;
use crate::reconciler::Reconciler;

mod args;
mod common;
mod config;
mod crypto;
mod docker;
mod load_balancer;
mod reconciler;
mod service_registry;

fn setup() -> Result<()> {
    color_eyre::install()?;

    // Set `RUST_LOG` if not set
    if std::env::var("RUST_LOG").is_err() {
        std::env::set_var("RUST_LOG", "info");
    }

    tracing_subscriber::fmt::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    Ok(())
}

#[tokio::main]
async fn main() -> Result<()> {
    setup()?;

    let args = Args::parse()?;
    let config = Config::from_location(&args.config_location).await?;

    let addr = Ipv4Addr::from_str(&config.alb.addr)?;
    let port = config.alb.port;
    let tls = config.alb.tls.clone();

    let mut service_registry = ServiceRegistry::new();
    let private_key = config.get_private_key().await?;

    start_services(
        &config.services,
        &mut service_registry,
        private_key.as_ref(),
    )
    .await?;

    // Start the auxillary services
    if let Some(services) = &config.auxillary_services {
        start_auxillary_services(services, private_key.as_ref()).await?;
    }

    let service_registry = Arc::new(RwLock::new(service_registry));
    let reconciliation_path = config.alb.reconciliation.clone();

    let reconciler = Reconciler::new(
        Arc::clone(&service_registry),
        args.config_location.clone(),
        config,
    );

    let mut load_balancer = LoadBalancer::new(service_registry, &reconciliation_path, reconciler);

    load_balancer.start_on(addr, port, tls).await?;

    Ok(())
}

async fn start_services(
    services: &HashMap<String, Service>,
    service_registry: &mut ServiceRegistry,
    private_key: Option<&RsaPrivateKey>,
) -> Result<()> {
    for (name, service) in services {
        service_registry.define(name, service.clone());

        let tag = &service.tag;
        let container = Container::from(service);

        tracing::info!("Starting {name} with tag {tag}");

        for _ in 0..service.replicas {
            let details = create_and_start_container(&container, tag, private_key).await?;
            service_registry.add_container(name, details);
        }
    }

    Ok(())
}

async fn start_auxillary_services(
    services: &[AuxillaryService],
    private_key: Option<&RsaPrivateKey>,
) -> Result<()> {
    for service in services {
        let tag = &service.tag;
        let container = Container::from(&service.clone());
        let details = create_and_start_container(&container, tag, private_key).await?;

        tracing::info!("Started {} on port {}", service.image, details.addr);
    }

    Ok(())
}

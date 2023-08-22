use std::collections::HashMap;
use std::net::{Ipv4Addr, SocketAddrV4};
use std::str::FromStr;

use color_eyre::eyre::Result;

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
    let mut config = Config::from_location(&args.config_location).await?;

    let private_key = crypto::get_private_key("ENV_PRIVATE_KEY")?;
    config.resolve_secrets(&private_key)?;

    let addr = Ipv4Addr::from_str(&config.alb.addr)?;
    let port = config.alb.port;

    let service_map = start_services(&config.services).await?;

    // Start the auxillary services
    if let Some(services) = &config.auxillary_services {
        start_auxillary_services(services).await?;
    }

    let (sender, receiver) = tokio::sync::mpsc::channel(100);

    let reconciler = Reconciler::new(
        &config.alb.reconciliation.clone(),
        args.config_location.clone(),
        config,
        sender,
    );

    let mut load_balancer = LoadBalancer::new(service_map, reconciler, receiver);
    load_balancer.start_on(addr, port).await?;

    Ok(())
}

async fn start_services(
    services: &HashMap<String, Service>,
) -> Result<HashMap<Service, Vec<SocketAddrV4>>> {
    let mut service_map: HashMap<Service, Vec<SocketAddrV4>> = HashMap::new();

    for (name, service) in services {
        let tag = &service.tag;
        let container = Container::from(service);
        let mut ports = Vec::new();

        tracing::info!("Starting {name} with tag {tag}");

        for _ in 0..service.replicas {
            let addr = create_and_start_container(&container, tag).await?;
            ports.push(SocketAddrV4::new(addr, container.target_port));
        }

        service_map.insert(service.clone(), ports);
    }

    Ok(service_map)
}

async fn start_auxillary_services(services: &[AuxillaryService]) -> Result<()> {
    for service in services {
        let tag = &service.tag;
        let container = Container::from(&service.clone());
        let port = create_and_start_container(&container, tag).await?;

        tracing::info!("Started {} on port {port}", service.image);
    }

    Ok(())
}

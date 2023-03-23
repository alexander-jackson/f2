use std::collections::HashMap;

use anyhow::Result;

use crate::args::Args;
use crate::common::{Container, Registry};
use crate::config::{Config, Service};
use crate::docker::api::create_and_start_on_random_port;
use crate::load_balancer::LoadBalancer;

mod args;
mod common;
mod config;
mod docker;
mod load_balancer;

fn setup() {
    // Set `RUST_LOG` if not set
    if std::env::var("RUST_LOG").is_err() {
        std::env::set_var("RUST_LOG", "info");
    }

    tracing_subscriber::fmt::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();
}

#[tokio::main]
async fn main() -> Result<()> {
    setup();

    let args = Args::parse()?;
    let config = Config::from_file(args.get_config_path())?;
    let registry = Registry::from(config.registry);
    let service_map = start_services(config.services, &registry).await?;

    let mut load_balancer = LoadBalancer::new(registry, service_map);
    load_balancer.start_on_port(5000).await?;

    Ok(())
}

async fn start_services(
    services: Vec<Service>,
    registry: &Registry,
) -> Result<HashMap<Service, Vec<u16>>> {
    let mut service_map: HashMap<Service, Vec<u16>> = HashMap::new();

    for service in services {
        let tag = &service.tag;
        let container = Container::from(&service);
        let mut ports = Vec::new();

        for _ in 0..service.replicas {
            let port = create_and_start_on_random_port(&container, registry, tag).await?;
            ports.push(port);
        }

        service_map.insert(service, ports);
    }

    Ok(service_map)
}

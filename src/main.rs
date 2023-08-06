use std::collections::HashMap;
use std::net::Ipv4Addr;
use std::str::FromStr;

use color_eyre::eyre::Result;

use crate::args::Args;
use crate::common::Container;
use crate::config::{AuxillaryService, Config, Service};
use crate::docker::api::create_and_start_on_random_port;
use crate::load_balancer::LoadBalancer;

mod args;
mod common;
mod config;
mod docker;
mod load_balancer;

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

    let service_map = start_services(config.services).await?;

    // Start the auxillary services
    if let Some(services) = config.auxillary_services {
        start_auxillary_services(services).await?;
    }

    let mut load_balancer = LoadBalancer::new(service_map);
    load_balancer.start_on(addr, port).await?;

    Ok(())
}

async fn start_services(services: Vec<Service>) -> Result<HashMap<Service, Vec<u16>>> {
    let mut service_map: HashMap<Service, Vec<u16>> = HashMap::new();

    for service in services {
        let tag = &service.tag;
        let container = Container::from(&service);
        let mut ports = Vec::new();

        for _ in 0..service.replicas {
            let port = create_and_start_on_random_port(&container, tag).await?;
            ports.push(port);
        }

        service_map.insert(service, ports);
    }

    Ok(service_map)
}

async fn start_auxillary_services(services: Vec<AuxillaryService>) -> Result<()> {
    for service in services {
        let tag = &service.tag;
        let container = Container::from(&service);
        let port = create_and_start_on_random_port(&container, tag).await?;

        tracing::info!("Started {} on port {port}", service.image);
    }

    Ok(())
}

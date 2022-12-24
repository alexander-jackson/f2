use std::collections::HashMap;
use std::fs;

use anyhow::Result;

use crate::args::Args;
use crate::common::{Container, Registry};
use crate::config::{Config, Service};
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
    let raw_config = fs::read_to_string(args.get_config_path())?;
    let config: Config = serde_yaml::from_str(&raw_config)?;

    let registry = Registry {
        base: config.registry.endpoint,
        repository: config.registry.repository_account,
        username: config.registry.username,
        password: config.registry.password,
    };

    let mut service_map: HashMap<Service, Vec<u16>> = HashMap::new();

    for service in &config.services {
        let container = Container {
            image: service.app.clone(),
            target_port: service.port,
        };

        // Fetch the latest tag for the client, if it hasn't been pinned in the config
        let tag = match service.tag.clone() {
            Some(t) => t,
            None => docker::registry::fetch_latest_tag(&container, &registry)
                .await?
                .expect("No tags found"),
        };

        let mut ports = Vec::new();

        for _ in 0..service.replicas {
            let port =
                docker::api::create_and_start_on_random_port(&container, &registry, &tag).await?;

            ports.push(port);
        }

        let enriched_service = Service {
            app: service.app.clone(),
            tag: Some(tag),
            port: service.port,
            replicas: service.replicas,
            host: service.host.clone(),
        };

        service_map.insert(enriched_service, ports);
    }

    let mut load_balancer = LoadBalancer::new(4999, registry, service_map);

    load_balancer.start().await?;

    Ok(())
}

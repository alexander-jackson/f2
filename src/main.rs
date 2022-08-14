use std::fs;

use anyhow::Result;

use crate::common::{Container, Registry};
use crate::config::Config;
use crate::load_balancing::LoadBalancer;

mod common;
mod config;
mod docker;
mod docker_registry;
mod load_balancing;

struct Args {
    config: Option<String>,
}

impl Args {
    fn parse() -> Result<Self> {
        let mut args = pico_args::Arguments::from_env();

        Ok(Self {
            config: args.opt_value_from_str("--config")?,
        })
    }

    fn get_config_path(&self) -> String {
        self.config
            .clone()
            .unwrap_or_else(|| String::from("f2.toml"))
    }
}

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
    let config: Config = toml::from_str(&raw_config)?;

    let container = Container {
        image: config.app.clone(),
        target_port: config.port,
    };

    let registry = Registry {
        base: config.registry.endpoint,
        repository: config.registry.repository_account,
        username: config.registry.username,
        password: config.registry.password,
    };

    // Fetch the latest tag for the client, if it hasn't been pinned in the config
    let tag = match config.tag {
        Some(t) => t,
        None => docker_registry::fetch_latest_tag(&container, &registry)
            .await?
            .expect("No tags found"),
    };

    // Define some ports
    let container_count = config.replicas;
    let mut ports = Vec::new();

    // Start all the containers
    for _ in 0..container_count {
        let port = docker::create_and_start_on_random_port(&container, &registry, &tag).await?;

        ports.push(port);
    }

    let mut load_balancer = LoadBalancer::new(4999, container, registry, ports, tag);

    load_balancer.start().await?;

    Ok(())
}

use std::fs;

use anyhow::Result;

use crate::config::Config;
use crate::load_balancing::{Container, LoadBalancer};

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

    // Define some ports
    let container_count = config.replicas;
    let mut ports = Vec::new();

    // Start all the containers
    for _ in 0..container_count {
        let port =
            docker::create_and_start_on_random_port(&config.app, &config.tag, config.port as u32)
                .await?;

        ports.push(port);
    }

    let container = Container::new(config.app.clone(), config.tag.clone(), config.port);

    let mut load_balancer = LoadBalancer::new(4999, container, ports, config.registry);

    load_balancer.start().await?;

    Ok(())
}

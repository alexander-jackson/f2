use anyhow::Result;

use crate::load_balancing::{Container, LoadBalancer};

mod docker;
mod load_balancing;

struct Args {
    image: String,
    tag: String,
    container_port: u16,
}

fn parse_args() -> Result<Args> {
    let mut args = pico_args::Arguments::from_env();

    Ok(Args {
        image: args.value_from_str("--image")?,
        tag: args.value_from_str("--tag")?,
        container_port: args.value_from_str("--port")?,
    })
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

    let args = parse_args()?;

    // Define some ports
    let container_count = 2;
    let mut ports = Vec::new();

    // Start all the containers
    for _ in 0..container_count {
        let port = docker::create_and_start_on_random_port(
            &args.image,
            &args.tag,
            args.container_port as u32,
        )
        .await?;

        ports.push(port);
    }

    let container = Container::new(args.image.clone(), args.tag.clone(), args.container_port);

    let mut load_balancer = LoadBalancer::new(4999, container, ports);
    load_balancer.start().await?;

    Ok(())
}

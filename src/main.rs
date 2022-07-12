use crate::load_balancing::LoadBalancer;

mod docker;
mod load_balancing;

fn setup() {
    tracing_subscriber::fmt::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    setup();

    // Define some ports
    let ports = vec![5000, 5001];

    // Start all the containers
    for port in &ports {
        docker::create_and_start("alexanderjackson/echo-server", "2046", *port as u32).await?;
    }

    let mut load_balancer = LoadBalancer::new(4999, ports);
    load_balancer.start().await?;

    Ok(())
}

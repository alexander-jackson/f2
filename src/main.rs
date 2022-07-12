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
    let container_count = 2;
    let mut ports = Vec::new();

    // Start all the containers
    for _ in 0..container_count {
        let port =
            docker::create_and_start_on_random_port("alexanderjackson/echo-server", "2046", 5000)
                .await?;

        ports.push(port);
    }

    let mut load_balancer = LoadBalancer::new(4999, ports);
    load_balancer.start().await?;

    Ok(())
}

use anyhow::Result;
use docker_api::container::{ContainerCreateOpts, PublishPort};
use docker_api::{Container, Docker};

pub async fn create_and_start(repository: &str, tag: &str, port: u32) -> Result<()> {
    let client = Docker::unix("/var/run/docker.sock");

    // Create the container
    let name = format!("{}:{}", repository, tag);

    let container_options = ContainerCreateOpts::builder(&name)
        .expose(PublishPort::tcp(5000), port)
        .build();

    let res = client.containers().create(&container_options).await?;
    let container_id = res.id();

    let container = Container::new(client, container_id);
    container.start().await?;

    tracing::info!(%container_id, %name, %port, "Created and started a container");

    Ok(())
}

use anyhow::Result;
use docker_api::container::{ContainerCreateOpts, PublishPort};
use docker_api::network::PortMap;
use docker_api::Docker;

use crate::common::{Container, Registry};

#[tracing::instrument]
pub async fn create_and_start_on_random_port(
    container: &Container,
    registry: &Registry,
    tag: &str,
) -> Result<u16> {
    let client = Docker::unix("/var/run/docker.sock");

    // Create the container
    let name = format!("{}:{}", container.image, tag);

    // Ensure the container exists locally
    pull_image_if_needed().await?;

    let container_options = ContainerCreateOpts::builder(&name)
        .expose(PublishPort::tcp(container.target_port as u32), 0)
        .build();

    let res = client.containers().create(&container_options).await?;
    let container_id = res.id();

    let docker_container = docker_api::Container::new(client.clone(), container_id);
    docker_container.start().await?;

    tracing::info!(%container_id, %name, %container.target_port, "Created and started a container");

    // Get the container itself and the port details
    let details = client.containers().get(container_id).inspect().await?;
    let ports = details.network_settings.ports;

    let binding =
        find_binding(container.target_port, ports).expect("Failed to find a port mapping");

    tracing::info!(%container.image, %tag, %binding, "Found the binding for a container");

    Ok(binding)
}

async fn pull_image_if_needed() -> Result<()> {
    Ok(())
}

fn find_binding(port: u16, mapping: Option<PortMap>) -> Option<u16> {
    mapping?
        .iter()
        .filter(|(k, _)| k.split_once('/').unwrap().0.parse() == Ok(port))
        .map(|(_, v)| {
            v.as_ref()
                .unwrap()
                .first()
                .unwrap()
                .host_port
                .parse()
                .unwrap()
        })
        .next()
}

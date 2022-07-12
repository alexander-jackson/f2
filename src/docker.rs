use anyhow::Result;
use docker_api::container::{ContainerCreateOpts, PublishPort};
use docker_api::network::PortMap;
use docker_api::{Container, Docker};

pub async fn create_and_start_on_random_port(
    repository: &str,
    tag: &str,
    port: u32,
) -> Result<u16> {
    let client = Docker::unix("/var/run/docker.sock");

    // Create the container
    let name = format!("{}:{}", repository, tag);

    let container_options = ContainerCreateOpts::builder(&name)
        .expose(PublishPort::tcp(port), 0)
        .build();

    let res = client.containers().create(&container_options).await?;
    let container_id = res.id();

    let container = Container::new(client.clone(), container_id);
    container.start().await?;

    tracing::info!(%container_id, %name, %port, "Created and started a container");

    // Get the container itself and the port details
    let details = client.containers().get(container_id).inspect().await?;
    let ports = details.network_settings.ports;

    let binding = find_binding(port, ports).expect("Failed to find a port mapping");

    tracing::info!(%repository, %tag, %binding, "Found the binding for a container");

    Ok(binding)
}

fn find_binding(port: u32, mapping: Option<PortMap>) -> Option<u16> {
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

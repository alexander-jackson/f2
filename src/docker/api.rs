use std::net::Ipv4Addr;

use color_eyre::eyre::Result;

use crate::common::Container;
use crate::docker::client::Client;

#[derive(Debug, Eq, PartialEq, Hash)]
pub struct StartedContainerDetails {
    pub id: String,
    pub addr: Ipv4Addr,
}

#[tracing::instrument]
pub async fn create_and_start_container(
    container: &Container,
    tag: &str,
) -> Result<StartedContainerDetails> {
    let client = Client::new("/var/run/docker.sock");

    // Ensure the image exists locally
    pull_image_if_needed(&client, container, tag).await?;

    // Create the container
    let name = format!("{}:{tag}", container.image);
    let target_port = container.target_port;

    let id = client
        .create_container(&name, &container.environment)
        .await?;

    client.start_container(&id).await?;

    tracing::info!(%id, %name, %target_port, "Created and started a container");

    // Get the container itself and the port details
    let addr = client.get_container_ip(&id).await?;

    tracing::info!(%container.image, %tag, %id, %addr, "Started a container and got the IP address");

    Ok(StartedContainerDetails { id, addr })
}

async fn pull_image_if_needed(client: &Client, container: &Container, tag: &str) -> Result<()> {
    // Check whether we have the image locally
    let expected_tag = format!("{}:{tag}", container.image);

    let local_images = client.fetch_images().await?;

    // Find all the ones with tags
    let exists = local_images
        .iter()
        .any(|image| image.repo_tags.contains(&expected_tag));

    if exists {
        tracing::info!(?container, %tag, "Image already exists locally");
        return Ok(());
    }

    tracing::info!(?container, %tag, "Image does not exist locally, pulling from repository");

    // Pull the image from the remote
    client.pull_image(&container.image, tag).await?;

    tracing::info!(?container, %tag, "Successfully pulled the image from the repository");

    Ok(())
}

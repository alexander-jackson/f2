use std::collections::HashMap;
use std::net::Ipv4Addr;
use std::path::{Path, PathBuf};

use color_eyre::eyre::{eyre, Context, Result};
use rsa::RsaPrivateKey;

use crate::common::Container;
use crate::config::VolumeDefinition;
use crate::docker::client::{DockerClient, DOCKER_NETWORK_NAME};
use crate::docker::models::ContainerId;

use super::models::NetworkId;

#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub struct StartedContainerDetails {
    pub id: ContainerId,
    pub addr: Ipv4Addr,
}

#[tracing::instrument(skip(client, private_key))]
pub async fn create_and_start_container<C: DockerClient>(
    client: &C,
    container: &Container,
    tag: &str,
    private_key: Option<&RsaPrivateKey>,
) -> Result<StartedContainerDetails> {
    let Container {
        image,
        environment,
        volumes,
    } = &container;

    // Ensure the image exists locally
    pull_image_if_needed(client, container, tag).await?;

    // Fetch the identifier for the Docker network
    let network_id = fetch_network_id(client).await?;

    // Create the container
    let name = format!("{image}:{tag}");

    let hostname = generate_hostname(image);
    let environment = environment.decrypt(private_key)?;
    let volumes = format_volumes(image, tag, volumes, private_key).await?;

    tracing::debug!(%name, ?volumes, "creating container with the following details");

    let id = client
        .create_container(
            &name,
            &Some(environment),
            &volumes,
            Some((&network_id, &hostname)),
        )
        .await?;

    client.start_container(&id).await?;

    tracing::info!(%id, %name, %hostname, "created and started a container");

    // Get the container itself and the port details
    let addr = client.get_container_ip(&id).await?;

    tracing::info!(
        %image,
        %tag,
        %id,
        %addr,
        %hostname,
        network = %DOCKER_NETWORK_NAME,
        "started container"
    );

    Ok(StartedContainerDetails { id, addr })
}

/// Fetches the Docker network ID by its name, returning an error if it does not exist.
async fn fetch_network_id<C: DockerClient>(client: &C) -> Result<NetworkId> {
    client
        .get_network_by_name(DOCKER_NETWORK_NAME)
        .await?
        .ok_or_else(|| {
            eyre!(
                "Docker network '{}' not found. Please create it before starting containers.",
                DOCKER_NETWORK_NAME
            )
        })
}

/// Generates a container name and hostname based on the image and tag.
fn generate_hostname(image: &str) -> String {
    image
        .split('/')
        .next_back()
        .unwrap_or(image)
        .split(':')
        .next()
        .unwrap_or(image)
        .to_string()
}

/// Formats the volumes for a container, resolving their content and writing it to a temporary file.
async fn format_volumes(
    image: &str,
    tag: &str,
    volumes: &HashMap<String, VolumeDefinition>,
    private_key: Option<&RsaPrivateKey>,
) -> Result<HashMap<String, String>> {
    let mut resolved_volumes = HashMap::new();

    for (name, definition) in volumes {
        let span = tracing::info_span!("processing a volume definition", %name, ?definition);
        let _guard = span.enter();

        let raw_content = definition.source.resolve().await?;
        let content = decrypt_content(&raw_content, private_key)
            .wrap_err_with(|| format!("failed to decrypt content for volume '{name}'"))?;

        tracing::info!(bytes = %content.len(), "decrypted content for volume");

        // Ensure we're handling paths correctly regardless of trailing slashes
        let clean_target = definition.target.trim_end_matches('/');
        let target_filename = Path::new(clean_target)
            .file_name()
            .ok_or_else(|| eyre!("invalid target path: {}", definition.target))?;

        // write the content to a temporary file
        let directory: PathBuf = format!("/tmp/f2/{image}/{tag}/{name}").into();
        let path = directory.join(target_filename);

        // Ensure the directory exists and write the content
        std::fs::create_dir_all(&directory)?;
        std::fs::write(&path, content)?;

        tracing::info!(?directory, ?path, "wrote volume content to temporary file");

        resolved_volumes.insert(
            path.to_string_lossy().into_owned(),
            definition.target.clone(),
        );
    }

    Ok(resolved_volumes)
}

#[tracing::instrument(skip(client))]
async fn pull_image_if_needed<C: DockerClient>(
    client: &C,
    container: &Container,
    tag: &str,
) -> Result<()> {
    // Check whether we have the image locally
    let expected_tag = format!("{}:{tag}", container.image);

    let local_images = client.fetch_images().await?;

    // Find all the ones with matching tags
    let exists = local_images
        .iter()
        .any(|image| image.repo_tags.contains(&expected_tag));

    if exists {
        tracing::info!("image already exists locally");
        return Ok(());
    }

    tracing::info!("image does not exist locally, pulling from repository");

    // Pull the image from the remote
    client.pull_image(&container.image, tag).await?;

    tracing::info!("successfully pulled the image from the repository");

    Ok(())
}

/// Finds occurrances of content wrapped in `{{ <secret> }}` and decrypts them using the provided
/// private key, replacing the original content with the decrypted one.
fn decrypt_content(content: &[u8], private_key: Option<&RsaPrivateKey>) -> Result<Vec<u8>> {
    let Some(private_key) = private_key else {
        return Ok(content.to_vec());
    };

    let Ok(content) = std::str::from_utf8(content) else {
        return Err(eyre!("content is not valid UTF-8"));
    };

    let segments = find_replaceable_segments(content);
    let mut decrypted_content = String::new();

    for segment in segments {
        let content = match segment {
            Segment::Text(text) => text,
            Segment::Secret { encrypted } => crate::crypto::decrypt(&encrypted, private_key)?,
        };

        decrypted_content.push_str(&content);
    }

    Ok(decrypted_content.into_bytes())
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum Segment {
    Secret { encrypted: String },
    Text(String),
}

fn find_replaceable_segments(content: &str) -> Vec<Segment> {
    let mut segments = Vec::new();
    let mut characters = content.chars().peekable();
    let mut current_segment = String::new();

    while let Some(c) = characters.next() {
        if c == '{' && characters.peek() == Some(&'{') {
            // Start of a segment
            if !current_segment.is_empty() {
                segments.push(Segment::Text(current_segment.clone()));
                current_segment.clear();
            }

            current_segment.push(c);
            current_segment.push(characters.next().unwrap());
        } else if c == '}' && characters.peek() == Some(&'}') {
            current_segment.push(c);
            current_segment.push(characters.next().unwrap());
            segments.push(Segment::Secret {
                encrypted: current_segment
                    .trim_start_matches("{{ ")
                    .trim_end_matches(" }}")
                    .to_string(),
            });
            current_segment.clear();
        } else {
            // Regular character
            current_segment.push(c);
        }
    }

    // If there's any remaining content, add it as a segment
    if !current_segment.is_empty() {
        segments.push(Segment::Text(current_segment));
    }

    segments
}

#[cfg(test)]
mod tests {
    use crate::docker::api::{find_replaceable_segments, generate_hostname, Segment};

    #[test]
    fn can_find_replaceable_content_correctly() {
        let content =
            "This is a test with {{ secret1 }} and {{ secret2 }} and some {{ secret3 }} at the end.";

        let segments = find_replaceable_segments(content);

        let expected_segments = vec![
            Segment::Text("This is a test with ".to_string()),
            Segment::Secret {
                encrypted: "secret1".to_string(),
            },
            Segment::Text(" and ".to_string()),
            Segment::Secret {
                encrypted: "secret2".to_string(),
            },
            Segment::Text(" and some ".to_string()),
            Segment::Secret {
                encrypted: "secret3".to_string(),
            },
            Segment::Text(" at the end.".to_string()),
        ];

        assert_eq!(segments, expected_segments);
    }

    #[test]
    fn can_generate_container_names() {
        assert_eq!(generate_hostname("nginx"), "nginx");
    }

    #[test]
    fn can_generate_container_names_with_slash() {
        assert_eq!(generate_hostname("company/nginx"), "nginx");
    }

    #[test]
    fn can_generate_container_names_with_slash_and_colon() {
        assert_eq!(generate_hostname("company/nginx:tag"), "nginx");
    }
}

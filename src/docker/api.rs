use std::collections::HashMap;
use std::net::Ipv4Addr;
use std::path::{Path, PathBuf};

use color_eyre::eyre::{eyre, Context, Result};
use rsa::RsaPrivateKey;

use crate::common::Container;
use crate::docker::client::DockerClient;
use crate::docker::models::ContainerId;

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
    // Ensure the image exists locally
    pull_image_if_needed(client, container, tag).await?;

    // Create the container
    let name = format!("{}:{tag}", container.image);
    let target_port = container.target_port;

    let environment = container.decrypt_environment(private_key)?;
    let mut volumes = HashMap::new();

    for (name, definition) in &container.volumes {
        let raw_content = definition.source.resolve().await?;
        let content = decrypt_content(&raw_content, private_key)
            .wrap_err_with(|| format!("failed to decrypt content for volume '{name}'"))?;

        tracing::info!(%name, ?definition, "processing volume definition");

        // Ensure we're handling paths correctly regardless of trailing slashes
        let clean_target = definition.target.trim_end_matches('/');
        let target_filename = Path::new(clean_target)
            .file_name()
            .ok_or_else(|| eyre!("invalid target path: {}", definition.target))?;

        // write the content to a temporary file
        let directory: PathBuf = format!("/tmp/f2/{}/{}/{}", container.image, tag, name).into();
        let path = directory.join(target_filename);

        // Ensure the directory exists and write the content
        std::fs::create_dir_all(&directory)?;
        std::fs::write(&path, content)?;

        tracing::info!(?directory, ?path, "wrote volume content to temporary file");

        volumes.insert(
            path.to_string_lossy().into_owned(),
            definition.target.clone(),
        );
    }

    let docker_volumes = volumes
        .values()
        .map(|target_path| (target_path.clone(), HashMap::<String, String>::new()))
        .collect::<HashMap<_, _>>();

    tracing::debug!(%name, ?volumes, "creating container with the following details");

    let id = client
        .create_container(&name, &environment, &docker_volumes, &volumes)
        .await?;

    client.start_container(&id).await?;

    tracing::info!(%id, %name, %target_port, "Created and started a container");

    // Get the container itself and the port details
    let addr = client.get_container_ip(&id).await?;

    tracing::info!(%container.image, %tag, %id, %addr, "Started a container and got the IP address");

    Ok(StartedContainerDetails { id, addr })
}

async fn pull_image_if_needed<C: DockerClient>(
    client: &C,
    container: &Container,
    tag: &str,
) -> Result<()> {
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
    use crate::docker::api::{find_replaceable_segments, Segment};

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
}

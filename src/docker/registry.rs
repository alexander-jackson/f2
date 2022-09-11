use anyhow::Result;
use chrono::NaiveDateTime;
use dkregistry::v2::Client;
use futures::TryStreamExt;

use crate::common::{Container, Registry};

const DOCKER_HUB_REGISTRY: &str = "registry-1.docker.io";
const TAG_FORMAT: &str = "%Y%m%d-%H%M";

#[tracing::instrument]
pub async fn check_for_newer_tag(
    container: &Container,
    registry: &Registry,
    current_tag: &str,
) -> Result<Option<String>> {
    // Fetch the tags
    let tags = fetch_tags(container, registry).await?;

    tracing::debug!(count = %tags.len(), "Found some tags in the Docker registry");

    find_newer_tag(current_tag, &tags)
}

#[tracing::instrument]
async fn fetch_tags(container: &Container, registry: &Registry) -> Result<Vec<String>> {
    let image_repository = format!("{}/{}", registry.repository, container.image);
    let scope = format!("repository:{}:pull", image_repository);
    let base = registry.base.as_deref().unwrap_or(DOCKER_HUB_REGISTRY);

    let client = Client::configure()
        .registry(base)
        .username(registry.username.clone())
        .password(registry.password.clone())
        .build()?
        .authenticate(&[&scope])
        .await?;

    tracing::debug!(%image_repository, "Fetching tags from the repository");

    let tags = client
        .get_tags(&image_repository, None)
        .try_collect()
        .await?;

    Ok(tags)
}

#[tracing::instrument]
fn find_newer_tag(current_tag: &str, tags: &[String]) -> Result<Option<String>> {
    // Parse the current tag
    let current_tag_time = NaiveDateTime::parse_from_str(current_tag, TAG_FORMAT)?;

    // Find all the tags newer than this one
    let tag = tags
        .iter()
        .filter_map(|tag| {
            let parsed = NaiveDateTime::parse_from_str(tag, TAG_FORMAT).ok()?;

            if parsed > current_tag_time {
                return Some((tag, parsed));
            }

            None
        })
        .max_by_key(|e| e.1)
        .map(|e| e.0.clone());

    Ok(tag)
}

#[tracing::instrument]
pub async fn fetch_latest_tag(
    container: &Container,
    registry: &Registry,
) -> Result<Option<String>> {
    let tags = fetch_tags(container, registry).await?;

    let latest = tags
        .iter()
        .filter_map(|tag| {
            NaiveDateTime::parse_from_str(tag, TAG_FORMAT)
                .ok()
                .map(|p| (tag, p))
        })
        .max_by_key(|e| e.1)
        .map(|e| e.0.clone());

    Ok(latest)
}

#[cfg(test)]
mod tests {
    use crate::docker::registry::find_newer_tag;

    #[test]
    fn empty_when_no_newer_tag_exists() {
        let tags = [
            String::from("20220611-0938"),
            String::from("20220605-1124"),
            String::from("20220517-2028"),
        ];

        // Use the newest tag
        let current = tags[0].as_str();

        // Check we get an empty result
        let found = find_newer_tag(current, &tags).unwrap();
        assert_eq!(found, None);
    }

    #[test]
    fn returns_a_newer_tag_if_it_exists() {
        let tags = [
            String::from("20220611-0938"),
            String::from("20220605-1124"),
            String::from("20220517-2028"),
        ];

        // Use the second most recent tag
        let current = tags[1].as_str();

        // Check we get the newest tag now
        let found = find_newer_tag(current, &tags).unwrap();
        assert_eq!(found, Some(tags[0].clone()));
    }

    #[test]
    fn returns_the_newest_possible_tag() {
        let tags = [
            String::from("20220611-0938"),
            String::from("20220605-1124"),
            String::from("20220517-2028"),
        ];

        // Use the oldest tag
        let current = tags[2].as_str();

        // Check we get the newest tag now
        let found = find_newer_tag(current, &tags).unwrap();
        assert_eq!(found, Some(tags[0].clone()));
    }
}

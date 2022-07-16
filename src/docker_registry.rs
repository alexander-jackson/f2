use anyhow::Result;
use chrono::NaiveDateTime;
use dkregistry::v2::Client;
use futures::StreamExt;

use crate::config::RegistryConfig;

const DOCKER_HUB_REGISTRY: &str = "registry-1.docker.io";
const TAG_FORMAT: &str = "%Y%m%d-%H%M";

#[tracing::instrument]
pub async fn check_for_newer_tag(
    app: &str,
    current_tag: &str,
    registry: &RegistryConfig,
) -> Result<Option<String>> {
    let repository = format!("{}/{}", registry.repository_account, app);

    // Fetch the tags
    let tags = fetch_tags(&repository, registry).await?;

    tracing::debug!(count = %tags.len(), "Found some tags in the Docker registry");

    find_newer_tag(current_tag, &tags)
}

#[tracing::instrument]
async fn fetch_tags(repository: &str, registry: &RegistryConfig) -> Result<Vec<String>> {
    let scope = format!("repository:{}:pull", repository);
    let endpoint = registry.endpoint.as_deref().unwrap_or(DOCKER_HUB_REGISTRY);

    let client = Client::configure()
        .registry(endpoint)
        .username(registry.username.clone())
        .password(registry.password.clone())
        .build()?
        .authenticate(&[&scope])
        .await?;

    let tags = client
        .get_tags(repository, None)
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .filter_map(Result::ok)
        .collect();

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

#[cfg(test)]
mod tests {
    use crate::docker_registry::find_newer_tag;

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

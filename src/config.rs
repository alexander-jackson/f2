use std::collections::HashMap;
use std::hash::{Hash, Hasher};

use color_eyre::eyre::{Context, Result};
use rsa::RsaPrivateKey;
use serde::Deserialize;

use crate::args::ConfigurationLocation;
use crate::crypto::decrypt;

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Diff {
    TagUpdate { name: String, value: String },
}

#[derive(Clone, Debug, Deserialize)]
pub struct Config {
    pub alb: AlbConfig,
    pub services: HashMap<String, Service>,
    pub auxillary_services: Option<Vec<AuxillaryService>>,
}

impl Config {
    pub async fn from_location(location: &ConfigurationLocation) -> Result<Self> {
        let bytes = location
            .fetch()
            .await
            .with_context(|| "Failed to fetch configuration")?;

        let config = serde_yaml::from_slice(&bytes)?;

        Ok(config)
    }

    pub fn resolve_secrets(&mut self, key: &RsaPrivateKey) -> Result<()> {
        for (_, service) in self.services.iter_mut() {
            service.resolve_secrets(key)?;
        }

        Ok(())
    }

    pub fn diff(&self, right: &Self) -> Option<Vec<Diff>> {
        let mut diff = Vec::new();

        for (name, service) in &self.services {
            // If it was defined before
            if let Some(definition) = right.services.get(name) {
                // Check for tag updates
                if service.tag != definition.tag {
                    diff.push(Diff::TagUpdate {
                        name: name.to_owned(),
                        value: definition.tag.to_owned(),
                    });
                }
            }
        }

        match diff.len() {
            0 => None,
            _ => Some(diff),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Deserialize)]
pub struct AlbConfig {
    pub addr: String,
    pub port: u16,
    pub reconciliation: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Deserialize)]
pub struct Service {
    pub image: String,
    pub tag: String,
    pub port: u16,
    pub replicas: u32,
    pub host: String,
    pub path_prefix: Option<String>,
    pub environment: Option<HashMap<String, String>>,
}

impl Service {
    pub fn resolve_secrets(&mut self, key: &RsaPrivateKey) -> Result<()> {
        let Some(ref mut environment) = self.environment else {
            return Ok(());
        };

        for (config_key, value) in environment.iter_mut() {
            tracing::info!("Resolving secret for {config_key}");

            if let Some(rhs) = value.strip_prefix("secret:") {
                *value = decrypt(rhs, key).wrap_err_with(|| {
                    format!("Failed to decrypt secret value for '{config_key}'")
                })?;
            }
        }

        Ok(())
    }
}

impl Hash for Service {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.image.hash(state);
        self.tag.hash(state);
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Deserialize)]
pub struct AuxillaryService {
    pub image: String,
    pub tag: String,
    pub port: u16,
    pub environment: Option<HashMap<String, String>>,
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use crate::config::{AlbConfig, Config, Diff, Service};

    fn some_config() -> Config {
        let mut services = HashMap::new();
        services.insert(
            String::from("backend"),
            Service {
                image: String::from("org/backend"),
                tag: String::from("1"),
                port: 5000,
                replicas: 1,
                host: String::from("example.com"),
                path_prefix: None,
                environment: None,
            },
        );

        Config {
            alb: AlbConfig {
                addr: String::new(),
                port: 5000,
                reconciliation: String::new(),
            },
            services,
            auxillary_services: None,
        }
    }

    #[test]
    fn can_diff_configurations() {
        let left = some_config();

        let mut right = left.clone();
        right.services.get_mut("backend").unwrap().tag = String::from("2");

        let diff = left.diff(&right);

        assert_eq!(
            diff,
            Some(vec![Diff::TagUpdate {
                name: String::from("backend"),
                value: String::from("2")
            }])
        );
    }

    #[test]
    fn same_configuration_produces_an_empty_diff() {
        let left = some_config();
        let diff = left.diff(&left);

        assert_eq!(diff, None);
    }

    #[test]
    fn changes_other_than_tag_are_ignored() {
        let left = some_config();
        let mut right = left.clone();

        right.services.get_mut("backend").unwrap().replicas = 2;

        let diff = left.diff(&right);

        assert_eq!(diff, None);
    }
}

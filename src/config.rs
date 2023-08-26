use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::path::PathBuf;

use color_eyre::eyre::{eyre, Context, Result};
use rsa::RsaPrivateKey;
use serde::Deserialize;

use crate::args::ConfigurationLocation;
use crate::crypto::decrypt;

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Diff {
    TagUpdate { name: String, value: String },
    ServiceAddition { name: String, definition: Service },
    ServiceRemoval { name: String },
}

#[derive(Clone, Debug, Deserialize)]
pub struct Config {
    pub alb: AlbConfig,
    pub secrets: Option<SecretConfig>,
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

    pub fn resolve_secrets(&mut self, key: Option<&RsaPrivateKey>) -> Result<()> {
        for (_, service) in self.services.iter_mut() {
            service.resolve_secrets(key)?;
        }

        Ok(())
    }

    pub fn diff(&self, right: &Self) -> Option<Vec<Diff>> {
        let mut diff = Vec::new();

        for (name, service) in &self.services {
            // If it is still defined
            if let Some(definition) = right.services.get(name) {
                // Check for tag updates
                if service.tag != definition.tag {
                    diff.push(Diff::TagUpdate {
                        name: name.into(),
                        value: definition.tag.clone(),
                    });
                }
            } else {
                // No longer defined, must have been removed entirely
                diff.push(Diff::ServiceRemoval { name: name.into() })
            }
        }

        // Check if any services have been added
        for (name, definition) in &right.services {
            if self.services.get(name).is_none() {
                diff.push(Diff::ServiceAddition {
                    name: name.into(),
                    definition: definition.clone(),
                })
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
    pub tls: Option<TlsConfig>,
}

#[derive(Clone, Debug, Eq, PartialEq, Deserialize)]
pub struct SecretConfig {
    pub private_key: ExternalBytes,
}

#[derive(Clone, Debug, Eq, PartialEq, Deserialize)]
pub struct TlsConfig {
    cert_file: ExternalBytes,
    key_file: ExternalBytes,
}

impl TlsConfig {
    pub async fn resolve_files(&self) -> Result<(Vec<u8>, Vec<u8>)> {
        let cert = self.cert_file.resolve().await?;
        let key = self.key_file.resolve().await?;

        Ok((cert, key))
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Deserialize)]
#[serde(tag = "location", rename_all = "lowercase")]
pub enum ExternalBytes {
    Filesystem { path: PathBuf },
    S3 { bucket: String, key: String },
}

impl ExternalBytes {
    pub async fn resolve(&self) -> Result<Vec<u8>> {
        let bytes = match self {
            Self::Filesystem { path } => tokio::fs::read(path).await?,
            Self::S3 { bucket, key } => {
                let config = aws_config::load_from_env().await;
                let client = aws_sdk_s3::Client::new(&config);

                let response = client.get_object().bucket(bucket).key(key).send().await?;

                response.body.collect().await?.to_vec()
            }
        };

        tracing::debug!("Resolving a file at {self:?}, got {} bytes", bytes.len());

        Ok(bytes)
    }
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
    pub fn resolve_secrets(&mut self, key: Option<&RsaPrivateKey>) -> Result<()> {
        let Some(ref mut environment) = self.environment else {
            return Ok(());
        };

        for (config_key, value) in environment.iter_mut() {
            tracing::info!("Resolving secret for {config_key}");

            if let Some(rhs) = value.strip_prefix("secret:") {
                let key = key.ok_or_else(|| eyre!("Tried to decrypt secret without a key"))?;

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
                tls: None,
            },
            secrets: None,
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

    #[test]
    fn can_notice_additional_services() {
        let left = some_config();
        let mut right = left.clone();

        let service = Service {
            image: String::from("org/frontend"),
            tag: String::from("latest"),
            port: 80,
            replicas: 1,
            host: String::from("example.com"),
            path_prefix: None,
            environment: None,
        };

        right.services.insert("frontend".into(), service.clone());

        let diff = left.diff(&right);

        assert_eq!(
            diff,
            Some(vec![Diff::ServiceAddition {
                name: "frontend".into(),
                definition: service,
            }])
        )
    }

    #[test]
    fn can_notice_removal_of_services() {
        let left = some_config();
        let mut right = left.clone();

        right.services.remove("backend");

        let diff = left.diff(&right);

        assert_eq!(
            diff,
            Some(vec![Diff::ServiceRemoval {
                name: "backend".into()
            }])
        )
    }
}

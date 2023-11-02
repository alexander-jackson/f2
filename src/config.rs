use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::path::PathBuf;

use color_eyre::eyre::{eyre, Context, Result};
use rsa::RsaPrivateKey;
use serde::Deserialize;

use crate::args::ConfigurationLocation;
use crate::crypto::{decrypt, parse_private_key};

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Diff {
    Alteration {
        name: String,
        new_definition: Service,
    },
    Addition {
        name: String,
        definition: Service,
    },
    Removal {
        name: String,
    },
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

    pub async fn get_private_key(&self) -> Result<Option<RsaPrivateKey>> {
        let private_key = match self.secrets.as_ref() {
            Some(secrets) => {
                let bytes = secrets.private_key.resolve().await?;
                let key = parse_private_key(&bytes)?;

                Some(key)
            }
            None => None,
        };

        Ok(private_key)
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
                // Check for service alterations
                if service != definition {
                    diff.push(Diff::Alteration {
                        name: name.into(),
                        new_definition: definition.clone(),
                    });
                }
            } else {
                // No longer defined, must have been removed entirely
                diff.push(Diff::Removal { name: name.into() })
            }
        }

        // Check if any services have been added
        for (name, definition) in &right.services {
            if self.services.get(name).is_none() {
                diff.push(Diff::Addition {
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
    pub domains: HashMap<String, TlsSecrets>,
}

#[derive(Clone, Debug, Eq, PartialEq, Deserialize)]
pub struct TlsSecrets {
    cert_file: ExternalBytes,
    key_file: ExternalBytes,
}

impl TlsSecrets {
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
            Some(vec![Diff::Alteration {
                name: String::from("backend"),
                new_definition: right.services.get("backend").unwrap().clone()
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
    fn can_notice_non_tag_changes() {
        let left = some_config();

        let mut right = left.clone();

        // Add an environment with some variables
        let mut environment = HashMap::new();
        environment.insert("NEW_PROPERTY".into(), "some-value".into());

        right.services.get_mut("backend").unwrap().environment = Some(environment);

        let diff = left.diff(&right);

        assert_eq!(
            diff,
            Some(vec![Diff::Alteration {
                name: String::from("backend"),
                new_definition: right.services.get("backend").unwrap().clone()
            }])
        );
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
            Some(vec![Diff::Addition {
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
            Some(vec![Diff::Removal {
                name: "backend".into()
            }])
        )
    }
}

use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::net::Ipv4Addr;
use std::num::NonZeroU8;
use std::ops::Deref;
use std::path::PathBuf;

use aws_config::BehaviorVersion;
use color_eyre::eyre::{eyre, Context, Result};
use rsa::RsaPrivateKey;
use serde::Deserialize;

use crate::args::ConfigurationLocation;
use crate::crypto::parse_private_key;

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Diff {
    Alteration {
        name: String,
        old_definition: Service,
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

    pub fn diff(&self, right: &Self) -> Option<Vec<Diff>> {
        let mut diff = Vec::new();

        for (name, service) in &self.services {
            // If it is still defined
            if let Some(definition) = right.services.get(name) {
                // Check for service alterations
                if service != definition {
                    diff.push(Diff::Alteration {
                        name: name.into(),
                        old_definition: service.clone(),
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
            if !self.services.contains_key(name) {
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
    pub addr: Ipv4Addr,
    pub port: u16,
    pub reconciliation: String,
    pub tls: Option<TlsConfig>,
    pub mtls: Option<MtlsConfig>,
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
pub struct MtlsConfig {
    /// The certificate to use as the trust anchor when validating incoming requests.
    pub anchor: ExternalBytes,
    /// The domains to apply mTLS to.
    pub domains: HashSet<String>,
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
                let config = aws_config::load_defaults(BehaviorVersion::latest()).await;
                let client = aws_sdk_s3::Client::new(&config);

                let response = client.get_object().bucket(bucket).key(key).send().await?;

                response.body.collect().await?.to_vec()
            }
        };

        tracing::debug!("Resolving a file at {self:?}, got {} bytes", bytes.len());

        Ok(bytes)
    }
}

#[derive(Copy, Clone, Debug, Eq, PartialEq, Deserialize)]
pub struct ReplicaCount(NonZeroU8);

impl TryFrom<u8> for ReplicaCount {
    type Error = color_eyre::Report;

    fn try_from(value: u8) -> Result<Self> {
        NonZeroU8::new(value)
            .map(Self)
            .ok_or_else(|| eyre!("invalid value provided for replica count"))
    }
}

impl Default for ReplicaCount {
    fn default() -> Self {
        Self(NonZeroU8::MIN)
    }
}

impl Deref for ReplicaCount {
    type Target = NonZeroU8;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

#[derive(Copy, Clone, Debug, Eq, PartialEq, Deserialize)]
pub enum ShutdownMode {
    Graceful,
    Forceful,
}

impl Default for ShutdownMode {
    fn default() -> Self {
        Self::Forceful
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Deserialize)]
pub struct Service {
    pub image: String,
    pub tag: String,
    pub port: u16,
    pub replicas: ReplicaCount,
    pub host: String,
    pub path_prefix: Option<String>,
    #[serde(default)]
    pub environment: HashMap<String, String>,
    #[serde(default)]
    pub volumes: HashMap<String, String>,
    #[serde(default)]
    pub shutdown_mode: ShutdownMode,
}

impl Hash for Service {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.image.hash(state);
        self.tag.hash(state);
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::net::Ipv4Addr;

    use crate::config::{AlbConfig, Config, Diff, Service};

    fn some_config() -> Config {
        let mut services = HashMap::new();
        services.insert(String::from("backend"), Service::default());

        Config {
            alb: AlbConfig {
                addr: Ipv4Addr::LOCALHOST,
                port: 5000,
                reconciliation: String::new(),
                tls: None,
                mtls: None,
            },
            secrets: None,
            services,
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
                old_definition: left.services.get("backend").unwrap().clone(),
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

        right.services.get_mut("backend").unwrap().environment = environment;

        let diff = left.diff(&right);

        assert_eq!(
            diff,
            Some(vec![Diff::Alteration {
                name: String::from("backend"),
                old_definition: left.services.get("backend").unwrap().clone(),
                new_definition: right.services.get("backend").unwrap().clone()
            }])
        );
    }

    #[test]
    fn can_notice_additional_services() {
        let left = some_config();
        let mut right = left.clone();

        let service = Service::default();
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

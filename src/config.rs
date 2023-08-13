use std::collections::HashMap;
use std::hash::{Hash, Hasher};

use color_eyre::eyre::{Context, Result};
use rsa::RsaPrivateKey;
use serde::Deserialize;

use crate::args::ConfigurationLocation;
use crate::crypto::decrypt;

#[derive(Clone, Debug, Deserialize)]
pub struct Config {
    pub alb: AlbConfig,
    pub services: Vec<Service>,
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
        for service in self.services.iter_mut() {
            service.resolve_secrets(key)?;
        }

        Ok(())
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Deserialize)]
pub struct AlbConfig {
    pub addr: String,
    pub port: u16,
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
        let Some(ref mut environment) = self.environment else { return Ok(()) };

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

use std::collections::HashMap;
use std::hash::{Hash, Hasher};

use color_eyre::eyre::{Context, Result};
use serde::Deserialize;

use crate::args::ConfigurationLocation;

#[derive(Clone, Debug, Deserialize)]
pub struct Config {
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

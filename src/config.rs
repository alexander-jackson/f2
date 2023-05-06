use std::collections::HashMap;
use std::fs::File;
use std::hash::{Hash, Hasher};
use std::path::Path;

use anyhow::Result;
use serde::Deserialize;

#[derive(Clone, Debug, Deserialize)]
pub struct Config {
    pub services: Vec<Service>,
    pub auxillary_services: Vec<AuxillaryService>,
}

impl Config {
    pub fn from_file<P: AsRef<Path>>(path: P) -> Result<Self> {
        let file = File::open(path)?;
        let config = serde_yaml::from_reader(file)?;

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

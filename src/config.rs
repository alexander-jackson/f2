use std::fs::File;
use std::path::Path;

use anyhow::Result;
use serde::Deserialize;

#[derive(Clone, Debug, Deserialize)]
pub struct Config {
    pub registry: RegistryDetails,
    pub services: Vec<Service>,
}

impl Config {
    pub fn from_file<P: AsRef<Path>>(path: P) -> Result<Self> {
        let file = File::open(path)?;
        let config = serde_yaml::from_reader(file)?;

        Ok(config)
    }
}

#[derive(Clone, Debug, Eq, Hash, PartialEq, Deserialize)]
pub struct Service {
    pub app: String,
    pub tag: String,
    pub port: u16,
    pub replicas: u32,
    pub host: String,
}

#[derive(Clone, Debug, Deserialize)]
pub struct RegistryDetails {
    pub endpoint: Option<String>,
    pub repository_account: String,
    pub username: Option<String>,
    pub password: Option<String>,
}

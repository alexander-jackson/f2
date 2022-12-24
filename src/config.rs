use serde::Deserialize;

#[derive(Clone, Debug, Deserialize)]
pub struct Config {
    pub registry: RegistryConfig,
    pub services: Vec<Service>,
}

#[derive(Clone, Debug, Eq, Hash, PartialEq, Deserialize)]
pub struct Service {
    pub app: String,
    pub tag: Option<String>,
    pub port: u16,
    pub replicas: u32,
    pub host: String,
}

#[derive(Clone, Debug, Deserialize)]
pub struct RegistryConfig {
    pub endpoint: Option<String>,
    pub repository_account: String,
    pub username: Option<String>,
    pub password: Option<String>,
}

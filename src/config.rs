use serde::Deserialize;

#[derive(Clone, Debug, Deserialize)]
pub struct Config {
    pub app: String,
    pub tag: Option<String>,
    pub port: u16,
    pub replicas: u32,
    pub registry: RegistryConfig,
}

#[derive(Clone, Debug, Deserialize)]
pub struct RegistryConfig {
    pub endpoint: Option<String>,
    pub repository_account: String,
    pub username: Option<String>,
    pub password: Option<String>,
}

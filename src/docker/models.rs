use std::collections::HashMap;

use serde::{Deserialize, Serialize};

type EmptyMap = HashMap<(), ()>;

#[derive(Debug, Deserialize)]
pub struct ImageSummary {
    #[serde(rename = "RepoTags")]
    pub repo_tags: Vec<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct CreateContainerOptions {
    pub image: String,
    pub exposed_ports: HashMap<String, EmptyMap>,
    pub host_config: HostConfig,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct HostConfig {
    pub port_bindings: HashMap<String, Vec<PortBinding>>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct PortBinding {
    pub host_ip: Option<String>,
    pub host_port: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct CreateContainerResponse {
    pub id: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct InspectContainerResponse {
    pub network_settings: NetworkSettings,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct NetworkSettings {
    pub ports: HashMap<String, Option<Vec<PortBinding>>>,
}

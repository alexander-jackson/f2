use std::collections::HashMap;
use std::fmt;
use std::net::Ipv4Addr;

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Eq, PartialEq, Hash, Deserialize)]
pub struct ContainerId(pub String);

impl fmt::Display for ContainerId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Hash, Deserialize)]
pub struct NetworkId(pub String);

impl fmt::Display for NetworkId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[cfg(test)]
impl ContainerId {
    pub fn random() -> Self {
        use rand::RngCore;

        let mut rng = rand::rngs::ThreadRng::default();
        let mut buf: [u8; 6] = [0; 6];
        rng.fill_bytes(buf.as_mut_slice());

        Self(hex::encode(buf))
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct ImageSummary {
    pub repo_tags: Vec<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct CreateContainerOptions<'a> {
    pub image: String,
    pub env: Vec<String>,
    pub volumes: &'a HashMap<String, HashMap<String, String>>,
    pub host_config: HostConfig,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub networking_config: Option<NetworkingConfig>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hostname: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct HostConfig {
    pub binds: Vec<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct NetworkingConfig {
    pub endpoints_config: HashMap<String, EndpointConfig>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct EndpointConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub aliases: Option<Vec<String>>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct CreateContainerResponse {
    pub id: ContainerId,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct InspectContainerResponse {
    pub network_settings: NetworkSettings,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct NetworkSettings {
    pub networks: HashMap<String, NetworkInfo>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct NetworkInfo {
    #[serde(rename = "IPAddress")]
    pub ip_address: Ipv4Addr,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct Network {
    pub id: String,
    pub name: String,
}

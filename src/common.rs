use crate::config::{RegistryDetails, Service};

#[derive(Clone, Debug)]
pub struct Container {
    pub image: String,
    pub target_port: u16,
}

impl From<&Service> for Container {
    fn from(service: &Service) -> Self {
        Self {
            image: service.image.clone(),
            target_port: service.port,
        }
    }
}

#[derive(Clone, Debug)]
pub struct Registry {
    pub base: Option<String>,
    pub username: Option<String>,
    pub password: Option<String>,
}

impl From<RegistryDetails> for Registry {
    fn from(registry: RegistryDetails) -> Self {
        Self {
            base: registry.endpoint,
            username: registry.username,
            password: registry.password,
        }
    }
}

use std::collections::HashMap;

use crate::config::{AuxillaryService, Service};

#[derive(Clone, Debug)]
pub struct Container {
    pub image: String,
    pub target_port: u16,
    pub environment: Option<HashMap<String, String>>,
}

impl From<&Service> for Container {
    fn from(service: &Service) -> Self {
        Self {
            image: service.image.clone(),
            target_port: service.port,
            environment: service.environment.clone(),
        }
    }
}

impl From<&AuxillaryService> for Container {
    fn from(service: &AuxillaryService) -> Self {
        Self {
            image: service.image.clone(),
            target_port: service.port,
            environment: service.environment.clone(),
        }
    }
}

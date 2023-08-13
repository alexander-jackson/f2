use std::{collections::HashMap, fmt};

use crate::config::{AuxillaryService, Service};

#[derive(Clone)]
pub struct Container {
    pub image: String,
    pub target_port: u16,
    pub environment: Option<HashMap<String, String>>,
}

impl fmt::Debug for Container {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Container")
            .field("image", &self.image)
            .field("target_port", &self.target_port)
            .finish()
    }
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

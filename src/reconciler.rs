use std::collections::HashSet;
use std::sync::Arc;

use color_eyre::Result;
use tokio::sync::RwLock;

use crate::args::ConfigurationLocation;
use crate::common::Container;
use crate::config::{Config, Diff};
use crate::docker::api::create_and_start_container;
use crate::docker::client::Client;
use crate::service_registry::ServiceRegistry;

#[derive(Debug, Clone)]
pub struct Reconciler {
    registry: Arc<RwLock<ServiceRegistry>>,
    config_location: Arc<ConfigurationLocation>,
    config: Arc<RwLock<Config>>,
}

impl Reconciler {
    pub fn new(
        registry: Arc<RwLock<ServiceRegistry>>,
        config_location: ConfigurationLocation,
        config: Config,
    ) -> Self {
        Self {
            registry,
            config_location: Arc::new(config_location),
            config: Arc::new(RwLock::new(config)),
        }
    }

    pub async fn reconcile(&self) -> Result<()> {
        let new_config = Config::from_location(&self.config_location).await?;
        let read_lock = self.config.read().await;
        let old_config = &read_lock;

        if let Some(diff) = old_config.diff(&new_config) {
            // Drop the read lock, acquire a write one
            drop(read_lock);

            let mut write_lock = self.config.write().await;
            *write_lock = new_config;

            // Drop the write lock and begin sending events
            drop(write_lock);

            for event in diff {
                self.handle_diff(event).await?;
            }
        }

        Ok(())
    }

    async fn handle_diff(&self, diff: Diff) -> Result<()> {
        match diff {
            Diff::TagUpdate { name, value } => {
                let read_lock = self.registry.read().await;
                let definition = read_lock.get_definition(&name).unwrap();
                let running_containers: HashSet<_> =
                    read_lock.get_running_containers(&name).unwrap().clone();

                let container = Container::from(definition);
                drop(read_lock);

                let details = create_and_start_container(&container, &value)
                    .await
                    .unwrap();

                let mut write_lock = self.registry.write().await;

                // Add the new container, remove the older ones
                write_lock.add_container(&name, details);

                for details in &running_containers {
                    write_lock.remove_container_by_id(&name, &details.id);
                }

                drop(write_lock);

                // Delete the previously running containers
                for details in &running_containers {
                    let client = Client::new("/var/run/docker.sock");
                    client.remove_container(&details.id).await?;
                }
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {}

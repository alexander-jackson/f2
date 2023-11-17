use std::sync::Arc;

use color_eyre::Result;
use indexmap::IndexSet;
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
        tracing::info!("Handling a diff: {diff:?}");

        match diff {
            Diff::Alteration {
                name,
                new_definition,
            } => {
                let read_lock = self.registry.read().await;
                let definition = read_lock.get_definition(&name).unwrap();
                let replicas = definition.replicas;
                let running_containers: IndexSet<_> =
                    read_lock.get_running_containers(&name).unwrap().clone();

                let container = Container::from(&new_definition);
                drop(read_lock);

                // Keep the locks short, create everything then add to the LB
                let mut started_containers = Vec::new();

                let private_key = self.config.read().await.get_private_key().await?;

                for _ in 0..replicas {
                    let details = create_and_start_container(
                        &container,
                        &new_definition.tag,
                        private_key.as_ref(),
                    )
                    .await?;

                    started_containers.push(details);
                }

                let mut write_lock = self.registry.write().await;
                write_lock.define(&name, new_definition);

                started_containers
                    .into_iter()
                    .for_each(|details| write_lock.add_container(&name, details));

                for details in &running_containers {
                    write_lock.remove_container_by_id(&name, &details.id);
                }

                drop(write_lock);

                // Delete the previously running containers
                let client = Client::new("/var/run/docker.sock");

                for details in &running_containers {
                    client.remove_container(&details.id).await?;
                }
            }
            Diff::Addition { name, definition } => {
                // Start some containers, then add to the LB
                let replicas = definition.replicas;
                let container = Container::from(&definition);
                let mut started_containers = Vec::new();

                let private_key = self.config.read().await.get_private_key().await?;

                for _ in 0..replicas {
                    let details = create_and_start_container(
                        &container,
                        &definition.tag,
                        private_key.as_ref(),
                    )
                    .await?;

                    started_containers.push(details);
                }

                let mut write_lock = self.registry.write().await;

                write_lock.define(&name, definition);

                started_containers
                    .into_iter()
                    .for_each(|details| write_lock.add_container(&name, details));
            }
            Diff::Removal { name } => {
                let read_lock = self.registry.read().await;

                // Get the running containers
                let running_containers = read_lock.get_running_containers(&name).cloned();
                drop(read_lock);

                // Remove them from the LB
                if let Some(containers) = running_containers {
                    let mut write_lock = self.registry.write().await;

                    write_lock.undefine(&name);
                    write_lock.remove_all_containers(&name);

                    drop(write_lock);

                    // Remove the running containers
                    let client = Client::new("/var/run/docker.sock");

                    for details in &containers {
                        client.remove_container(&details.id).await?;
                    }
                }
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {}

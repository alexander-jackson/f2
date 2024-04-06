use std::sync::Arc;

use color_eyre::Result;
use indexmap::IndexSet;
use tokio::sync::RwLock;

use crate::args::ConfigurationLocation;
use crate::common::Container;
use crate::config::{Config, Diff};
use crate::docker::api::create_and_start_container;
use crate::docker::client::DockerClient;
use crate::service_registry::ServiceRegistry;

#[derive(Debug, Clone)]
pub struct Reconciler<C: DockerClient> {
    registry: Arc<RwLock<ServiceRegistry>>,
    config_location: Arc<ConfigurationLocation>,
    config: Arc<RwLock<Config>>,
    docker_client: C,
}

impl<C: DockerClient> Reconciler<C> {
    pub fn new(
        registry: Arc<RwLock<ServiceRegistry>>,
        config_location: ConfigurationLocation,
        config: Config,
        docker_client: C,
    ) -> Self {
        Self {
            registry,
            config_location: Arc::new(config_location),
            config: Arc::new(RwLock::new(config)),
            docker_client,
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
                        &self.docker_client,
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
                for details in &running_containers {
                    self.docker_client.remove_container(&details.id).await?;
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
                        &self.docker_client,
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
                    for details in &containers {
                        self.docker_client.remove_container(&details.id).await?;
                    }
                }
            }
        }

        Ok(())
    }
}

#[cfg(test)]
pub mod tests {
    use std::collections::HashMap;
    use std::net::Ipv4Addr;
    use std::path::PathBuf;
    use std::sync::Arc;

    use color_eyre::eyre::Result;
    use tokio::sync::RwLock;

    use crate::args::ConfigurationLocation;
    use crate::common::Environment;
    use crate::config::{AlbConfig, Config, Diff, Service};
    use crate::docker::api::StartedContainerDetails;
    use crate::docker::client::DockerClient;
    use crate::docker::models::{ContainerId, ImageSummary};
    use crate::reconciler::Reconciler;
    use crate::service_registry::ServiceRegistry;

    #[derive(Clone, Debug, Default)]
    struct DockerState {
        images: Vec<ImageSummary>,
        containers: Vec<(ContainerId, String)>,
    }

    #[derive(Clone, Default)]
    pub struct FakeDockerClient {
        state: Arc<RwLock<DockerState>>,
    }

    #[async_trait::async_trait]
    impl DockerClient for FakeDockerClient {
        async fn fetch_images(&self) -> Result<Vec<ImageSummary>> {
            let lock = self.state.read().await;

            Ok(lock.images.clone())
        }

        async fn pull_image(&self, _image: &str, _tag: &str) -> Result<()> {
            Ok(())
        }

        async fn create_container(
            &self,
            image: &str,
            environment: &Option<Environment>,
        ) -> Result<ContainerId> {
            let container_id = ContainerId::random();

            let mut lock = self.state.write().await;
            lock.containers
                .push((container_id.clone(), image.to_owned()));

            Ok(container_id)
        }

        async fn start_container(&self, id: &ContainerId) -> Result<()> {
            Ok(())
        }

        async fn get_container_ip(&self, id: &ContainerId) -> Result<Ipv4Addr> {
            Ok(Ipv4Addr::LOCALHOST)
        }

        async fn remove_container(&self, id: &ContainerId) -> Result<()> {
            let mut lock = self.state.write().await;
            lock.containers.retain(|c| c.0 != *id);

            Ok(())
        }
    }

    fn create_reconciler<C: DockerClient>(
        registry: ServiceRegistry,
        docker_client: C,
    ) -> Reconciler<C> {
        let config = Config {
            alb: AlbConfig {
                addr: Ipv4Addr::LOCALHOST,
                port: 5000,
                reconciliation: String::new(),
                tls: None,
            },
            secrets: None,
            services: HashMap::new(),
            auxillary_services: None,
        };

        Reconciler::new(
            Arc::new(RwLock::new(registry)),
            ConfigurationLocation::Filesystem(PathBuf::new()),
            config,
            docker_client,
        )
    }

    #[tokio::test]
    async fn can_handle_addition_of_services() -> Result<()> {
        let registry = ServiceRegistry::new();

        let image = "alexanderjackson/f2";
        let tag = "latest";

        let service = Service {
            image: image.to_owned(),
            tag: tag.to_owned(),
            port: 5000,
            replicas: 1,
            host: "localhost".to_owned(),
            path_prefix: None,
            environment: None,
        };

        let docker_client = FakeDockerClient::default();
        let reconciler = create_reconciler(registry, docker_client.clone());

        reconciler
            .handle_diff(Diff::Addition {
                name: "foobar".to_owned(),
                definition: service,
            })
            .await?;

        // Check we now have some containers in the Docker state
        let lock = docker_client.state.read().await;
        let containers = &lock.containers;

        let container_id = containers.iter().find_map(|(id, image_and_tag)| {
            image_and_tag.eq(&format!("{image}:{tag}")).then_some(id)
        });

        assert!(container_id.is_some());

        Ok(())
    }

    #[tokio::test]
    async fn can_handle_removal_of_service() -> Result<()> {
        let mut registry = ServiceRegistry::new();

        let service = "foobar";
        let image = "alexanderjackson/f2";
        let tag = "latest";

        let docker_client = FakeDockerClient::default();

        let id = docker_client
            .create_container(&format!("{image}:{tag}"), &None)
            .await?;

        registry.add_container(
            service,
            StartedContainerDetails {
                id,
                addr: Ipv4Addr::LOCALHOST,
            },
        );

        let reconciler = create_reconciler(registry, docker_client.clone());

        let diff = Diff::Removal {
            name: service.to_owned(),
        };

        reconciler.handle_diff(diff).await?;

        // Check the state is now empty
        let lock = docker_client.state.read().await;
        let containers = &lock.containers;

        assert!(containers.is_empty());

        Ok(())
    }
}

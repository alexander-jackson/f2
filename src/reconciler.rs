use std::sync::Arc;

use color_eyre::eyre::{eyre, Result};
use indexmap::IndexSet;
use tokio::sync::RwLock;

use crate::args::ConfigurationLocation;
use crate::common::Container;
use crate::config::{Config, Diff, Service};
use crate::docker::api::{create_and_start_container, StartedContainerDetails};
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

    async fn get_running_containers(
        &self,
        name: &str,
    ) -> Option<IndexSet<StartedContainerDetails>> {
        let read_lock = self.registry.read().await;

        read_lock.get_running_containers(name).cloned()
    }

    async fn start_multiple_containers(
        &self,
        name: &str,
        new_definition: Service,
        replicas: u32,
    ) -> Result<()> {
        // Keep the locks short, create everything then add to the LB
        let mut started_containers = Vec::new();

        let private_key = self.config.read().await.get_private_key().await?;
        let container = Container::from(&new_definition);

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
        write_lock.define(name, new_definition);

        started_containers
            .into_iter()
            .for_each(|details| write_lock.add_container(name, details));

        Ok(())
    }

    async fn handle_alteration(&self, name: String, new_definition: Service) -> Result<()> {
        let running_containers = self
            .get_running_containers(&name)
            .await
            .ok_or_else(|| eyre!("Failed to get running containers for {name}"))?;

        let replicas = new_definition.replicas;

        self.start_multiple_containers(&name, new_definition, replicas)
            .await?;

        let mut write_lock = self.registry.write().await;

        for details in &running_containers {
            write_lock.remove_container_by_id(&name, &details.id);
        }

        drop(write_lock);

        for details in &running_containers {
            self.docker_client.remove_container(&details.id).await?;
        }

        Ok(())
    }

    async fn handle_addition(&self, name: String, definition: Service) -> Result<()> {
        let replicas = definition.replicas;

        self.start_multiple_containers(&name, definition, replicas)
            .await?;

        Ok(())
    }

    async fn handle_removal(&self, name: String) -> Result<()> {
        let running_containers = self.get_running_containers(&name).await;

        // Remove them from the LB
        if let Some(containers) = running_containers {
            let mut write_lock = self.registry.write().await;

            write_lock.undefine(&name);
            write_lock.remove_all_containers(&name);

            drop(write_lock);

            for details in &containers {
                self.docker_client.remove_container(&details.id).await?;
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
            } => self.handle_alteration(name, new_definition).await?,
            Diff::Addition { name, definition } => self.handle_addition(name, definition).await?,
            Diff::Removal { name } => self.handle_removal(name).await?,
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
            _environment: &Option<Environment>,
            _volumes: &HashMap<String, String>,
        ) -> Result<ContainerId> {
            let container_id = ContainerId::random();

            let mut lock = self.state.write().await;
            lock.containers
                .push((container_id.clone(), image.to_owned()));

            Ok(container_id)
        }

        async fn start_container(&self, _id: &ContainerId) -> Result<()> {
            Ok(())
        }

        async fn get_container_ip(&self, _id: &ContainerId) -> Result<Ipv4Addr> {
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
            replicas: 1,
            ..Default::default()
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
            .create_container(&format!("{image}:{tag}"), &None, &HashMap::new())
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

    #[tokio::test]
    async fn can_handle_scale_up_of_service() -> Result<()> {
        let mut registry = ServiceRegistry::new();

        let service = "foobar";
        let image = "alexanderjackson/f2";
        let tag = "latest";

        let docker_client = FakeDockerClient::default();

        let service_definition = Service {
            image: image.to_owned(),
            tag: tag.to_owned(),
            replicas: 1,
            ..Default::default()
        };

        let mut altered_definition = service_definition.clone();
        altered_definition.replicas += 1;

        let image_and_tag = format!("{image}:{tag}");
        let id = docker_client
            .create_container(&image_and_tag, &None, &HashMap::new())
            .await?;

        registry.define(service, service_definition);
        registry.add_container(
            service,
            StartedContainerDetails {
                id: id.clone(),
                addr: Ipv4Addr::LOCALHOST,
            },
        );

        let reconciler = create_reconciler(registry, docker_client.clone());

        let diff = Diff::Alteration {
            name: service.to_owned(),
            new_definition: altered_definition,
        };

        reconciler.handle_diff(diff).await?;

        // Check we now have 2 containers for this image and tag
        let lock = docker_client.state.read().await;
        let containers = &lock.containers;
        let matching_containers = containers.iter().filter(|c| c.1 == image_and_tag).count();

        assert_eq!(matching_containers, 2);

        // Neither of these containers are our original one
        let original_container = containers.iter().find(|c| c.0 == id);
        assert!(original_container.is_none());

        Ok(())
    }
}

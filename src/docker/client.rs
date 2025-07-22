use std::collections::HashMap;
use std::net::Ipv4Addr;

use color_eyre::eyre::{self, eyre, Context, Result};
use color_eyre::Section;
use http::Response;
use http_body_util::{BodyExt, Full};
use hyper::body::{Bytes, Incoming};
use hyper::{Method, Request, Uri};
use hyper_util::client::legacy::Client as HyperClient;
use hyperlocal::{UnixClientExt, UnixConnector};
use serde::de::DeserializeOwned;

use crate::common::Environment;
use crate::docker::models::{
    CreateContainerOptions, CreateContainerResponse, EndpointConfig, HostConfig, ImageSummary,
    InspectContainerResponse, Network, NetworkId, NetworkingConfig,
};

use super::models::ContainerId;

pub const DOCKER_NETWORK_NAME: &str = "internal";

#[async_trait::async_trait]
pub trait DockerClient {
    async fn fetch_images(&self) -> Result<Vec<ImageSummary>>;
    async fn pull_image(&self, image: &str, tag: &str) -> Result<()>;

    async fn get_network_by_name(&self, name: &str) -> Result<Option<NetworkId>>;

    async fn create_container(
        &self,
        image: &str,
        environment: &Option<Environment>,
        volumes: &HashMap<String, HashMap<String, String>>,
        docker_volumes: &HashMap<String, String>,
        hostname: Option<&str>,
        network: Option<(&NetworkId, &str)>,
    ) -> Result<ContainerId>;

    async fn start_container(&self, id: &ContainerId) -> Result<()>;

    async fn get_container_ip(&self, id: &ContainerId) -> Result<Ipv4Addr>;

    async fn stop_container(&self, id: &ContainerId) -> Result<()>;

    async fn remove_container(&self, id: &ContainerId) -> Result<()>;
}

pub struct Client {
    client: HyperClient<UnixConnector, Full<Bytes>>,
    base: String,
}

impl Default for Client {
    fn default() -> Self {
        let base = String::from("/var/run/docker.sock");

        tracing::debug!(%base, "created a new Docker client");

        Self {
            client: HyperClient::unix(),
            base,
        }
    }
}

impl Client {
    fn build_uri(&self, endpoint: &str) -> Uri {
        hyperlocal::Uri::new(&self.base, endpoint).into()
    }
}

#[async_trait::async_trait]
impl DockerClient for Client {
    async fn fetch_images(&self) -> Result<Vec<ImageSummary>> {
        let uri = self.build_uri("/images/json");

        tracing::info!(%uri, "Fetching images from the Docker server");

        let response = self.client.get(uri).await?;

        Ok(deserialize_body(response).await?)
    }

    async fn pull_image(&self, image: &str, tag: &str) -> Result<()> {
        let path_and_query = format!("/images/create?fromImage={image}:{tag}");
        let uri = self.build_uri(&path_and_query);

        tracing::info!(%image, %tag, "Pulling an image from the Docker registry");

        let request = Request::builder()
            .uri(uri)
            .method(Method::POST)
            .body(Full::default())?;

        let response = self.client.request(request).await?;

        // Check the image actually exists on the remote
        eyre::ensure!(
            response.status().is_success(),
            "Failed to pull image {image}:{tag} from the remote, it may not exist",
        );

        // Make sure we read the whole body
        read_body(response).await?;

        Ok(())
    }

    #[tracing::instrument(skip(self))]
    async fn get_network_by_name(&self, name: &str) -> Result<Option<NetworkId>> {
        let uri = self.build_uri("/networks");

        tracing::info!(%name, "Searching for network by name");

        let response = self.client.get(uri).await?;
        let networks: Vec<Network> = deserialize_body(response).await?;

        let network = networks.iter().find(|n| n.name == name);

        Ok(network.map(|n| NetworkId(n.id.clone())))
    }

    #[tracing::instrument(skip(self, environment))]
    async fn create_container(
        &self,
        image: &str,
        environment: &Option<Environment>,
        volumes: &HashMap<String, HashMap<String, String>>,
        docker_volumes: &HashMap<String, String>,
        hostname: Option<&str>,
        network: Option<(&NetworkId, &str)>,
    ) -> Result<ContainerId> {
        let uri = self.build_uri("/containers/create");

        let env = format_environment_variables(environment);

        let host_config = HostConfig {
            binds: docker_volumes
                .iter()
                .map(|(host_path, container_path)| format!("{host_path}:{container_path}"))
                .collect(),
        };

        tracing::info!(?volumes, ?host_config, "creating a container");

        // Setup networking configuration if a network is provided
        let networking_config = network.map(|(network_id, container_alias)| {
            let mut endpoints_config = HashMap::new();
            let aliases = vec![container_alias.to_string()];

            endpoints_config.insert(
                network_id.0.clone(),
                EndpointConfig {
                    aliases: Some(aliases),
                },
            );

            NetworkingConfig { endpoints_config }
        });

        let options = CreateContainerOptions {
            image: String::from(image),
            env,
            volumes: &HashMap::new(),
            host_config,
            networking_config,
            hostname: hostname.map(|s| s.to_owned()),
        };

        let body = serde_json::to_vec(&options)?;

        let request = Request::builder()
            .uri(uri)
            .method(Method::POST)
            .header(hyper::http::header::CONTENT_TYPE, "application/json")
            .body(Full::new(Bytes::from(body)))?;

        let response = self.client.request(request).await?;
        let body: CreateContainerResponse = deserialize_body(response)
            .await
            .wrap_err_with(|| format!("failed to create container with image {image}"))?;

        tracing::info!(?body, "container created successfully");

        Ok(body.id)
    }

    async fn start_container(&self, id: &ContainerId) -> Result<()> {
        let path = format!("/containers/{id}/start");
        let uri = self.build_uri(&path);

        tracing::info!(?id, "starting a container");

        let request = Request::builder()
            .uri(uri)
            .method(Method::POST)
            .body(Full::default())?;

        self.client.request(request).await?;

        Ok(())
    }

    async fn get_container_ip(&self, id: &ContainerId) -> Result<Ipv4Addr> {
        let path = format!("/containers/{id}/json");
        let uri = self.build_uri(&path);

        tracing::info!(?id, "fetching exposed ports for a container");

        let request = Request::builder()
            .uri(uri)
            .method(Method::GET)
            .body(Full::default())?;

        let response = self.client.request(request).await?;
        let payload: InspectContainerResponse = deserialize_body(response)
            .await
            .wrap_err_with(|| format!("failed to inspect container {id}"))
            .suggestion("Does the container exist?")?;

        let ip_address = payload
            .network_settings
            .networks
            .get(DOCKER_NETWORK_NAME)
            .map(|network| network.ip_address)
            .ok_or_else(|| {
                eyre!("Container {id} is not connected to the {DOCKER_NETWORK_NAME} network")
            })?;

        Ok(ip_address)
    }

    async fn stop_container(&self, id: &ContainerId) -> Result<()> {
        let path = format!("/containers/{id}/stop?signal=SIGTERM&t=15");
        let uri = self.build_uri(&path);

        tracing::info!(%id, "stopping a container");

        let request = Request::builder()
            .uri(uri)
            .method(Method::POST)
            .body(Full::default())?;

        self.client.request(request).await?;

        Ok(())
    }

    async fn remove_container(&self, id: &ContainerId) -> Result<()> {
        let path = format!("/containers/{id}?force=true");
        let uri = self.build_uri(&path);

        tracing::info!(%id, "removing a container forcefully");

        let request = Request::builder()
            .uri(uri)
            .method(Method::DELETE)
            .body(Full::default())?;

        self.client.request(request).await?;

        Ok(())
    }
}

fn format_environment_variables(environment: &Option<Environment>) -> Vec<String> {
    let Some(environment) = environment else {
        return Vec::new();
    };

    environment
        .variables
        .iter()
        .map(|(k, v)| format!("{k}={v}"))
        .collect()
}

async fn read_body(response: Response<Incoming>) -> Result<Bytes> {
    let collected = response
        .into_body()
        .collect()
        .await
        .wrap_err("failed to read response body")?;

    let bytes = collected.to_bytes();

    Ok(bytes)
}

async fn deserialize_body<T>(response: Response<Incoming>) -> Result<T>
where
    T: DeserializeOwned,
{
    let bytes = read_body(response).await?;
    let decoded = std::str::from_utf8(&bytes)?;
    let json = serde_json::from_str(decoded)?;

    Ok(json)
}

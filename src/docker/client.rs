use std::collections::HashMap;
use std::net::Ipv4Addr;

use color_eyre::eyre::{self, Context, Result};
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
    CreateContainerOptions, CreateContainerResponse, HostConfig, ImageSummary,
    InspectContainerResponse, NetworkSettings,
};

use super::models::ContainerId;

#[async_trait::async_trait]
pub trait DockerClient {
    async fn fetch_images(&self) -> Result<Vec<ImageSummary>>;
    async fn pull_image(&self, image: &str, tag: &str) -> Result<()>;

    async fn create_container(
        &self,
        image: &str,
        environment: &Option<Environment>,
        volumes: &HashMap<String, HashMap<String, String>>,
        docker_volumes: &HashMap<String, String>,
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

    async fn create_container(
        &self,
        image: &str,
        environment: &Option<Environment>,
        volumes: &HashMap<String, HashMap<String, String>>,
        docker_volumes: &HashMap<String, String>,
    ) -> Result<ContainerId> {
        let uri = self.build_uri("/containers/create");

        let env = format_environment_variables(environment);

        let host_config = HostConfig {
            binds: docker_volumes
                .iter()
                .map(|(host_path, container_path)| format!("{host_path}:{container_path}"))
                .collect(),
        };

        let options = CreateContainerOptions {
            image: String::from(image),
            env,
            volumes,
            host_config,
        };

        tracing::info!(%image, "Creating a container");

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

        tracing::info!(?body, "Container created successfully");

        Ok(body.id)
    }

    async fn start_container(&self, id: &ContainerId) -> Result<()> {
        let path = format!("/containers/{id}/start");
        let uri = self.build_uri(&path);

        tracing::info!(?id, "Starting a container");

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

        tracing::info!(?id, "Fetching exposed ports for a container");

        let request = Request::builder()
            .uri(uri)
            .method(Method::GET)
            .body(Full::default())?;

        let response = self.client.request(request).await?;
        let payload: InspectContainerResponse = deserialize_body(response)
            .await
            .wrap_err_with(|| format!("failed to inspect container {id}"))
            .suggestion("Does the container exist?")?;

        let NetworkSettings { ip_address } = payload.network_settings;

        Ok(ip_address)
    }

    async fn stop_container(&self, id: &ContainerId) -> Result<()> {
        let path = format!("/containers/{id}/stop?signal=SIGTERM&t=15");
        let uri = self.build_uri(&path);

        tracing::info!(%id, "Stopping a container");

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

        tracing::info!(%id, "Removing a container forcefully");

        let request = Request::builder()
            .uri(uri)
            .method(Method::DELETE)
            .body(Full::default())?;

        self.client.request(request).await?;

        Ok(())
    }
}

impl Client {
    pub fn new(base: &str) -> Self {
        tracing::debug!(%base, "Created a new Docker client");

        Self {
            client: HyperClient::unix(),
            base: String::from(base),
        }
    }

    fn build_uri(&self, endpoint: &str) -> Uri {
        hyperlocal::Uri::new(&self.base, endpoint).into()
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

async fn read_body(mut response: Response<Incoming>) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();

    while let Some(frame_result) = response.frame().await {
        let frame = frame_result?;

        if let Some(segment) = frame.data_ref() {
            bytes.extend_from_slice(segment);
        }
    }

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

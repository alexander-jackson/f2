use std::net::Ipv4Addr;

use color_eyre::eyre::{self, Result};
use hyper::{Body, Method, Request, Uri};
use hyperlocal::{UnixClientExt, UnixConnector};

use crate::common::Environment;
use crate::docker::models::{
    CreateContainerOptions, CreateContainerResponse, ImageSummary, InspectContainerResponse,
    NetworkSettings,
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
    ) -> Result<ContainerId>;

    async fn start_container(&self, id: &ContainerId) -> Result<()>;

    async fn get_container_ip(&self, id: &ContainerId) -> Result<Ipv4Addr>;

    async fn remove_container(&self, id: &ContainerId) -> Result<()>;
}

pub struct Client {
    client: hyper::Client<UnixConnector, Body>,
    base: String,
}

#[async_trait::async_trait]
impl DockerClient for Client {
    async fn fetch_images(&self) -> Result<Vec<ImageSummary>> {
        let uri = self.build_uri("/images/json");

        tracing::info!(%uri, "Fetching images from the Docker server");

        let mut response = self.client.get(uri).await?;
        let bytes = hyper::body::to_bytes(response.body_mut()).await?;

        let summaries = serde_json::from_slice(&bytes)?;

        Ok(summaries)
    }

    async fn pull_image(&self, image: &str, tag: &str) -> Result<()> {
        let path_and_query = format!("/images/create?fromImage={image}:{tag}");
        let uri = self.build_uri(&path_and_query);

        tracing::info!(%image, %tag, "Pulling an image from the Docker registry");

        let request = Request::builder()
            .uri(uri)
            .method(Method::POST)
            .body(Body::empty())?;

        let mut response = self.client.request(request).await?;

        // Check the image actually exists on the remote
        eyre::ensure!(
            response.status().is_success(),
            "Failed to pull image {image}:{tag} from the remote, it may not exist",
        );

        // Make sure we read the whole body
        hyper::body::to_bytes(response.body_mut()).await?;

        Ok(())
    }

    async fn create_container(
        &self,
        image: &str,
        environment: &Option<Environment>,
    ) -> Result<ContainerId> {
        let uri = self.build_uri("/containers/create");

        let env = format_environment_variables(environment);

        let options = CreateContainerOptions {
            image: String::from(image),
            env,
        };

        tracing::info!(%image, "Creating a container");

        let body = serde_json::to_vec(&options)?;

        let request = Request::builder()
            .uri(uri)
            .method(Method::POST)
            .header(hyper::http::header::CONTENT_TYPE, "application/json")
            .body(Body::from(body))?;

        let mut response = self.client.request(request).await?;
        let bytes = hyper::body::to_bytes(response.body_mut()).await?;
        let body: CreateContainerResponse = serde_json::from_slice(&bytes)?;

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
            .body(Body::empty())?;

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
            .body(Body::empty())?;

        let mut response = self.client.request(request).await?;
        let bytes = hyper::body::to_bytes(response.body_mut()).await?;
        let payload: InspectContainerResponse = serde_json::from_slice(&bytes)?;

        let NetworkSettings { ip_address } = payload.network_settings;

        Ok(ip_address)
    }

    async fn remove_container(&self, id: &ContainerId) -> Result<()> {
        let path = format!("/containers/{id}?force=true");
        let uri = self.build_uri(&path);

        tracing::info!(%id, "Removing a container forcefully");

        let request = Request::builder()
            .uri(uri)
            .method(Method::DELETE)
            .body(Body::empty())?;

        self.client.request(request).await?;

        Ok(())
    }
}

impl Client {
    pub fn new(base: &str) -> Self {
        tracing::debug!(%base, "Created a new Docker client");

        Self {
            client: hyper::Client::unix(),
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

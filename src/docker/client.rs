use std::collections::HashMap;

use anyhow::Result;
use hyper::{Body, Method, Request, Uri};
use hyperlocal::{UnixClientExt, UnixConnector};

use crate::docker::models::{
    CreateContainerOptions, CreateContainerResponse, HostConfig, ImageSummary,
    InspectContainerResponse, PortBinding,
};

pub struct Client {
    client: hyper::Client<UnixConnector, Body>,
    base: String,
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

    pub async fn fetch_images(&self) -> Result<Vec<ImageSummary>> {
        let uri = self.build_uri("/images/json");

        tracing::info!(%uri, "Fetching images from the Docker server");

        let mut response = self.client.get(uri).await?;
        let bytes = hyper::body::to_bytes(response.body_mut()).await?;

        let summaries = serde_json::from_slice(&bytes)?;

        Ok(summaries)
    }

    pub async fn pull_image(&self, repo: &str, image: &str, tag: &str) -> Result<()> {
        let path_and_query = format!("/images/create?fromImage={repo}/{image}:{tag}");
        let uri = self.build_uri(&path_and_query);

        tracing::info!(%repo, %image, %tag, "Pulling an image from the Docker registry");

        let request = Request::builder()
            .uri(uri)
            .method(Method::POST)
            .body(Body::empty())?;

        let mut response = self.client.request(request).await?;

        // Check the image actually exists on the remote
        anyhow::ensure!(
            response.status().is_success(),
            "Failed to pull image {repo}/{image}:{tag} from the remote, it may not exist",
        );

        // Make sure we read the whole body
        hyper::body::to_bytes(response.body_mut()).await?;

        Ok(())
    }

    pub async fn create_container(
        &self,
        image: &str,
        port_mapping: &HashMap<u16, u16>,
    ) -> Result<String> {
        let uri = self.build_uri("/containers/create");

        let mut exposed_ports = HashMap::new();
        let mut port_bindings = HashMap::new();

        for (key, value) in port_mapping.iter() {
            let port_and_protocol = format!("{key}/tcp");

            let binding = PortBinding {
                host_ip: None,
                host_port: value.to_string(),
            };

            exposed_ports.insert(port_and_protocol.clone(), HashMap::new());
            port_bindings.insert(port_and_protocol, vec![binding]);
        }

        let options = CreateContainerOptions {
            image: String::from(image),
            exposed_ports,
            host_config: HostConfig { port_bindings },
        };

        tracing::info!(?options, "Creating a container");

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

    pub async fn start_container(&self, id: &str) -> Result<()> {
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

    pub async fn get_exposed_ports(&self, id: &str) -> Result<HashMap<u16, u16>> {
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

        // Find all the TCP exposed ports
        let tcp_exposed_ports = payload
            .network_settings
            .ports
            .iter()
            .filter_map(|(k, v)| {
                k.strip_suffix("/tcp").and_then(|container_port| {
                    let bound = v.as_ref().and_then(|ports| {
                        ports
                            .first()
                            .map(|binding| binding.host_port.parse().unwrap())
                    })?;

                    Some((container_port.parse().unwrap(), bound))
                })
            })
            .collect();

        Ok(tcp_exposed_ports)
    }
}

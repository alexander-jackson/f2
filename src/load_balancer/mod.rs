use std::collections::HashMap;
use std::net::{Ipv4Addr, SocketAddrV4, TcpListener};
use std::sync::Arc;

use color_eyre::eyre::{Report, Result};
use hyper::client::HttpConnector;
use hyper::service::{make_service_fn, service_fn};
use hyper::{Client, Server};
use rand::prelude::{SeedableRng, SmallRng};
use tokio::sync::mpsc::Receiver;
use tokio::sync::{Mutex, RwLock};

use crate::common::Container;
use crate::config::{Diff, Service};
use crate::docker::api::create_and_start_container;
use crate::reconciler::Reconciler;

type ServiceMap = HashMap<Service, Vec<SocketAddrV4>>;

mod proxy;

#[derive(Debug)]
pub struct LoadBalancer {
    service_map: Arc<RwLock<ServiceMap>>,
    client: Client<HttpConnector>,
    rng: Arc<Mutex<SmallRng>>,
    reconciler: Arc<Reconciler>,
    receiver: Arc<Mutex<Receiver<Diff>>>,
}

impl LoadBalancer {
    pub fn new(service_map: ServiceMap, reconciler: Reconciler, receiver: Receiver<Diff>) -> Self {
        let service_map = Arc::new(RwLock::new(service_map));
        let client = Client::new();
        let rng = Arc::new(Mutex::new(SmallRng::from_entropy()));
        let reconciler = Arc::new(reconciler);
        let receiver = Arc::new(Mutex::new(receiver));

        Self {
            service_map,
            client,
            rng,
            reconciler,
            receiver,
        }
    }

    pub async fn start_on(&mut self, addr: Ipv4Addr, port: u16) -> Result<()> {
        let addr = SocketAddrV4::new(addr, port);
        let listener = TcpListener::bind(addr)?;

        self.start(listener).await
    }

    pub async fn start(&mut self, listener: TcpListener) -> Result<()> {
        let service_map = Arc::clone(&self.service_map);
        let rng = Arc::clone(&self.rng);
        let client = self.client.clone();
        let reconciler = Arc::clone(&self.reconciler);

        let service = make_service_fn(move |_| {
            let service_map = Arc::clone(&service_map);
            let rng = Arc::clone(&rng);
            let client = client.clone();
            let reconciler = Arc::clone(&reconciler);

            async move {
                Ok::<_, Report>(service_fn(move |req| {
                    proxy::handle_request(
                        Arc::clone(&service_map),
                        Arc::clone(&rng),
                        client.clone(),
                        Arc::clone(&reconciler),
                        req,
                    )
                }))
            }
        });

        let receiver = Arc::clone(&self.receiver);
        let service_map = Arc::clone(&self.service_map);

        tokio::spawn(async move {
            loop {
                let mut lock = receiver.lock().await;

                if let Some(diff) = lock.recv().await {
                    tracing::info!("Got a diff from the reconciler: {diff:?}");

                    // Apply the change
                    match diff {
                        Diff::TagUpdate { image, value } => {
                            let read_lock = service_map.read().await;
                            let service = read_lock
                                .iter()
                                .find(|(service, _)| service.image == image)
                                .unwrap()
                                .0
                                .clone();

                            let container = Container::from(&service);
                            drop(read_lock);

                            let addr = create_and_start_container(&container, &value)
                                .await
                                .unwrap();

                            let mut write_lock = service_map.write().await;
                            let downstreams = write_lock.get_mut(&service).unwrap();

                            downstreams.push(SocketAddrV4::new(addr, service.port));
                        }
                    }
                }
            }
        });

        let server = Server::from_tcp(listener)?.serve(service);
        server.await?;

        Ok(())
    }
}

#[cfg(test)]
mod tests;

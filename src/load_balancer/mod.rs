use std::sync::Arc;

use color_eyre::eyre::Result;
use hyper::body::Incoming;
use hyper::service::service_fn;
use hyper_util::client::legacy::connect::HttpConnector;
use hyper_util::client::legacy::Client;
use hyper_util::rt::{TokioExecutor, TokioIo};
use hyper_util::server::conn::auto::Builder;
use mutual_tls::{ConnectionContext, MutualTlsServer, Protocol};
use rand::prelude::{SeedableRng, SmallRng};
use rustls::pki_types::CertificateDer;
use rustls::server::danger::ClientCertVerifier;
use rustls::server::{NoClientAuth, WebPkiClientVerifier};
use rustls::RootCertStore;
use tokio::net::TcpListener;
use tokio::sync::{Mutex, RwLock};

use crate::config::{MtlsConfig, TlsConfig};
use crate::docker::client::DockerClient;
use crate::reconciler::Reconciler;
use crate::service_registry::ServiceRegistry;

use self::tls::CertificateResolver;

mod proxy;
mod tls;

#[derive(Debug)]
pub struct LoadBalancer<C: DockerClient> {
    service_registry: Arc<RwLock<ServiceRegistry>>,
    client: Client<HttpConnector, Incoming>,
    rng: Arc<Mutex<SmallRng>>,
    reconciler_path: Arc<str>,
    reconciler: Arc<Reconciler<C>>,
}

impl<C: DockerClient + Sync + Send + 'static> LoadBalancer<C> {
    pub fn new(
        service_registry: Arc<RwLock<ServiceRegistry>>,
        reconciler_path: &str,
        reconciler: Reconciler<C>,
    ) -> Self {
        let client = Client::builder(TokioExecutor::new()).build_http();
        let rng = Arc::new(Mutex::new(SmallRng::from_entropy()));
        let reconciler_path = Arc::from(reconciler_path);
        let reconciler = Arc::new(reconciler);

        Self {
            service_registry,
            client,
            rng,
            reconciler_path,
            reconciler,
        }
    }

    pub async fn start(
        &mut self,
        listener: TcpListener,
        tls: Option<TlsConfig>,
        mtls: Option<MtlsConfig>,
    ) -> Result<()> {
        let service_factory = |_| {
            let service_registry = Arc::clone(&self.service_registry);
            let rng = Arc::clone(&self.rng);
            let client = self.client.clone();
            let reconciler_path = Arc::clone(&self.reconciler_path);
            let reconciler = Arc::clone(&self.reconciler);

            service_fn(move |req| {
                let service_registry = Arc::clone(&service_registry);
                let rng = Arc::clone(&rng);
                let client = client.clone();
                let reconciler_path = Arc::clone(&reconciler_path);
                let reconciler = Arc::clone(&reconciler);

                async move {
                    proxy::handle_request(
                        service_registry,
                        rng,
                        client,
                        reconciler_path,
                        reconciler,
                        req,
                    )
                    .await
                }
            })
        };

        if let Some(tls) = tls {
            let verifier: Arc<dyn ClientCertVerifier> = match &mtls {
                Some(config) => {
                    let bytes = config.anchor.resolve().await?;
                    let certificate = CertificateDer::from_slice(&bytes);

                    let mut store = RootCertStore::empty();
                    store.add(certificate)?;

                    WebPkiClientVerifier::builder(Arc::new(store)).build()?
                }
                None => Arc::new(NoClientAuth),
            };

            let resolver = Arc::new(CertificateResolver::new(&tls.domains).await?);

            let protocols = tls
                .domains
                .keys()
                .map(|domain| {
                    let protocol = mtls
                        .as_ref()
                        .map(|config| &config.domains)
                        .and_then(|domains| domains.contains(domain).then_some(Protocol::Mutual))
                        .unwrap_or(Protocol::Public);

                    (domain.to_owned(), protocol)
                })
                .collect();

            let server = MutualTlsServer::new(protocols, verifier, resolver, service_factory);

            server.run(listener).await;
        } else {
            loop {
                let (stream, _) = listener.accept().await?;
                let io = TokioIo::new(stream);

                let service = service_factory(ConnectionContext { unit: None });

                tokio::spawn(async move {
                    if let Err(e) = Builder::new(TokioExecutor::new())
                        .serve_connection(io, service)
                        .await
                    {
                        tracing::warn!(%e, "error handling connection");
                    }
                });
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests;

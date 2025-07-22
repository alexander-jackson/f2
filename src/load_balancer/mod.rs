use std::io::Cursor;
use std::sync::Arc;

use arc_swap::ArcSwap;
use color_eyre::eyre::Result;
use hyper::body::Incoming;
use hyper::service::service_fn;
use hyper_util::client::legacy::connect::HttpConnector;
use hyper_util::client::legacy::Client;
use hyper_util::rt::{TokioExecutor, TokioIo};
use hyper_util::server::conn::auto::Builder;
use mutual_tls::{ConnectionContext, ServerConfiguration};
use rand::prelude::{SeedableRng, SmallRng};
use rustls::server::danger::ClientCertVerifier;
use rustls::server::{NoClientAuth, WebPkiClientVerifier};
use rustls::RootCertStore;
use tls::DynamicAuthenticationLevelResolver;
use tokio::net::TcpListener;
use tokio::sync::{Mutex, RwLock};

use crate::config::{Config, MtlsConfig, TlsConfig};
use crate::ipc::MessageBus;
use crate::load_balancer::tls::CertificateResolver;
use crate::service_registry::ServiceRegistry;

mod proxy;
mod tls;

#[derive(Debug)]
pub struct LoadBalancer {
    service_registry: Arc<RwLock<ServiceRegistry>>,
    client: Client<HttpConnector, Incoming>,
    rng: Arc<Mutex<SmallRng>>,
    config: Arc<ArcSwap<Config>>,
    message_bus: Arc<MessageBus>,
}

impl LoadBalancer {
    pub fn new(
        service_registry: Arc<RwLock<ServiceRegistry>>,
        config: Arc<ArcSwap<Config>>,
        message_bus: Arc<MessageBus>,
    ) -> Self {
        let client = Client::builder(TokioExecutor::new()).build_http();
        let rng = Arc::new(Mutex::new(SmallRng::from_entropy()));

        Self {
            service_registry,
            client,
            rng,
            config,
            message_bus,
        }
    }

    pub async fn run(
        self,
        listener: TcpListener,
        tls: Option<TlsConfig>,
        mtls: Option<MtlsConfig>,
    ) -> Result<()> {
        let reconciliation_path = Arc::from(self.config.load().alb.reconciliation.as_str());
        let message_bus = Arc::clone(&self.message_bus);

        let service_factory = move |ctx| {
            let service_registry = Arc::clone(&self.service_registry);
            let rng = Arc::clone(&self.rng);
            let client = self.client.clone();
            let reconciliation_path = Arc::clone(&reconciliation_path);
            let message_bus = Arc::clone(&message_bus);

            service_fn(move |req| {
                let service_registry = Arc::clone(&service_registry);
                let rng = Arc::clone(&rng);
                let client = client.clone();
                let reconciliation_path = Arc::clone(&reconciliation_path);
                let message_bus = Arc::clone(&message_bus);

                async move {
                    proxy::handle_request(
                        service_registry,
                        rng,
                        client,
                        reconciliation_path,
                        message_bus,
                        req,
                    )
                    .await
                }
            })
        };

        if let Some(tls) = tls {
            let client_cert_verifier: Arc<dyn ClientCertVerifier> = match &mtls {
                Some(config) => {
                    let bytes = config.anchor.resolve().await?;
                    let mut cursor = Cursor::new(bytes);

                    let mut store = RootCertStore::empty();
                    let certs = rustls_pemfile::certs(&mut cursor).filter_map(Result::ok);
                    let (added, ignored) = store.add_parsable_certificates(certs);

                    tracing::info!(%added, %ignored, "set up the trust store");

                    WebPkiClientVerifier::builder(Arc::new(store)).build()?
                }
                None => Arc::new(NoClientAuth),
            };

            let config = Arc::new(tls.domains);
            let message_bus = Arc::clone(&self.message_bus);

            let certificate_resolver =
                Arc::new(CertificateResolver::new(config, message_bus).await?);
            let authentication_level_resolver =
                DynamicAuthenticationLevelResolver::new(Arc::clone(&self.config));

            let server_configuration = ServerConfiguration::default();
            let server = mutual_tls::Server::new(
                authentication_level_resolver,
                client_cert_verifier,
                certificate_resolver,
                service_factory,
                server_configuration,
            );

            server.run(listener).await;
        } else {
            loop {
                let (stream, _) = listener.accept().await?;
                let io = TokioIo::new(stream);

                let service = service_factory(ConnectionContext { common_name: None });

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

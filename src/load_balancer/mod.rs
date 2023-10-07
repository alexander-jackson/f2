use std::io::Cursor;
use std::net::{Ipv4Addr, SocketAddrV4, TcpListener};
use std::sync::Arc;

use color_eyre::eyre::{Report, Result};
use hyper::client::HttpConnector;
use hyper::server::conn::AddrIncoming;
use hyper::service::{make_service_fn, service_fn};
use hyper::{Client, Server};
use hyper_rustls::TlsAcceptor;
use rand::prelude::{SeedableRng, SmallRng};
use rustls::server::ResolvesServerCertUsingSni;
use rustls::sign::{any_supported_type, CertifiedKey};
use tokio::sync::{Mutex, RwLock};

use crate::config::TlsConfig;
use crate::reconciler::Reconciler;
use crate::service_registry::ServiceRegistry;

mod proxy;

#[derive(Debug)]
pub struct LoadBalancer {
    service_registry: Arc<RwLock<ServiceRegistry>>,
    client: Client<HttpConnector>,
    rng: Arc<Mutex<SmallRng>>,
    reconciler_path: Arc<str>,
    reconciler: Arc<Reconciler>,
}

impl LoadBalancer {
    pub fn new(
        service_registry: Arc<RwLock<ServiceRegistry>>,
        reconciler_path: &str,
        reconciler: Reconciler,
    ) -> Self {
        let client = Client::new();
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

    pub async fn start_on(
        &mut self,
        addr: Ipv4Addr,
        port: u16,
        tls: Option<TlsConfig>,
    ) -> Result<()> {
        let addr = SocketAddrV4::new(addr, port);
        let listener = TcpListener::bind(addr)?;

        self.start(listener, tls).await
    }

    pub async fn start(&mut self, listener: TcpListener, tls: Option<TlsConfig>) -> Result<()> {
        let service_registry = Arc::clone(&self.service_registry);
        let rng = Arc::clone(&self.rng);
        let client = self.client.clone();
        let reconciliation_path = Arc::clone(&self.reconciler_path);
        let reconciler = Arc::clone(&self.reconciler);

        if let Some(tls) = tls {
            let mut certificate_resolver = ResolvesServerCertUsingSni::new();

            for (domain, secrets) in tls.domains {
                let (cert, key) = secrets.resolve_files().await?;
                let certified_key = parse_certified_key(&cert, &key)?;

                certificate_resolver.add(&domain, certified_key)?;
            }

            let config = rustls::ServerConfig::builder()
                .with_safe_defaults()
                .with_no_client_auth()
                .with_cert_resolver(Arc::new(certificate_resolver));

            let listener = tokio::net::TcpListener::from_std(listener)?;
            let incoming = AddrIncoming::from_listener(listener)?;
            let acceptor = TlsAcceptor::builder()
                .with_tls_config(config)
                .with_http11_alpn()
                .with_incoming(incoming);

            let service = make_service_fn(move |_| {
                let service_registry = Arc::clone(&service_registry);
                let rng = Arc::clone(&rng);
                let client = client.clone();
                let reconciliation_path = Arc::clone(&reconciliation_path);
                let reconciler = Arc::clone(&reconciler);

                async move {
                    Ok::<_, Report>(service_fn(move |req| {
                        proxy::handle_request(
                            Arc::clone(&service_registry),
                            Arc::clone(&rng),
                            client.clone(),
                            Arc::clone(&reconciliation_path),
                            Arc::clone(&reconciler),
                            req,
                        )
                    }))
                }
            });

            let server = Server::builder(acceptor);

            server.serve(service).await?;
        } else {
            let server = Server::from_tcp(listener)?;

            let service = make_service_fn(move |_| {
                let service_registry = Arc::clone(&service_registry);
                let rng = Arc::clone(&rng);
                let client = client.clone();
                let reconciliation_path = Arc::clone(&reconciliation_path);
                let reconciler = Arc::clone(&reconciler);

                async move {
                    Ok::<_, Report>(service_fn(move |req| {
                        proxy::handle_request(
                            Arc::clone(&service_registry),
                            Arc::clone(&rng),
                            client.clone(),
                            Arc::clone(&reconciliation_path),
                            Arc::clone(&reconciler),
                            req,
                        )
                    }))
                }
            });

            server.serve(service).await?;
        };

        Ok(())
    }
}

fn parse_certified_key(cert: &[u8], key: &[u8]) -> Result<CertifiedKey> {
    let mut cert = Cursor::new(cert);
    let mut key = Cursor::new(key);

    let cert: Vec<_> = rustls_pemfile::certs(&mut cert)?
        .into_iter()
        .map(rustls::Certificate)
        .collect();

    let keys = rustls_pemfile::pkcs8_private_keys(&mut key)?;
    let key = rustls::PrivateKey(keys[0].clone());
    let key = any_supported_type(&key)?;

    let certified_key = CertifiedKey {
        cert,
        key,
        ocsp: None,
        sct_list: None,
    };

    Ok(certified_key)
}

#[cfg(test)]
mod tests;

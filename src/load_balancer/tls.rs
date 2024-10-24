use std::collections::HashMap;
use std::io::Cursor;
use std::sync::Arc;

use arc_swap::ArcSwap;
use color_eyre::eyre::{eyre, Result};
use itertools::Itertools;
use rustls::crypto::ring::sign::any_supported_type;
use rustls::pki_types::PrivateKeyDer;
use rustls::server::{ClientHello, ResolvesServerCert};
use rustls::sign::CertifiedKey;

use crate::config::TlsSecrets;

type Configuration = HashMap<String, TlsSecrets>;
type Domains = HashMap<String, Arc<CertifiedKey>>;

#[derive(Debug, Default)]
pub struct CertificateResolver {
    domains: ArcSwap<Domains>,
}

impl CertificateResolver {
    pub async fn new(config: &Configuration) -> Result<Self> {
        let mut resolver = Self::default();

        resolver.discover(config).await?;

        Ok(resolver)
    }

    pub async fn discover(&mut self, config: &Configuration) -> Result<()> {
        let mut domains = HashMap::new();

        for (domain, secrets) in config {
            let (cert, key) = secrets.resolve_files().await?;
            let certified_key = parse_certified_key(&cert, &key)?;

            domains.insert(domain.to_owned(), Arc::new(certified_key));
        }

        self.domains.store(Arc::new(domains));

        Ok(())
    }
}

fn parse_certified_key(cert: &[u8], key: &[u8]) -> Result<CertifiedKey> {
    let mut cert = Cursor::new(cert);
    let mut key = Cursor::new(key);

    let cert: Vec<_> = rustls_pemfile::certs(&mut cert).try_collect()?;

    let keys = rustls_pemfile::pkcs8_private_keys(&mut key)
        .next()
        .ok_or_else(|| eyre!("failed to get private key"))??;

    let key = PrivateKeyDer::Pkcs8(keys);
    let key = any_supported_type(&key)?;

    let certified_key = CertifiedKey {
        cert,
        key,
        ocsp: None,
    };

    Ok(certified_key)
}

impl ResolvesServerCert for CertificateResolver {
    fn resolve(&self, client_hello: ClientHello) -> Option<Arc<CertifiedKey>> {
        let server_name = client_hello.server_name()?;
        let domains = self.domains.load();

        domains.get(server_name).cloned()
    }
}

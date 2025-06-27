use std::collections::HashMap;
use std::io::Cursor;
use std::sync::Arc;

use arc_swap::ArcSwap;
use color_eyre::eyre::{eyre, Result};
use itertools::Itertools;
use mutual_tls::{AuthenticationLevel, AuthenticationLevelResolver};
use rustls::crypto::ring::sign::any_supported_type;
use rustls::pki_types::PrivateKeyDer;
use rustls::server::{ClientHello, ResolvesServerCert};
use rustls::sign::CertifiedKey;

use crate::config::{Config, TlsSecrets};
use crate::ipc::MessageBus;

type Configuration = HashMap<String, TlsSecrets>;
type Domains = HashMap<String, Arc<CertifiedKey>>;

#[derive(Debug, Default)]
pub struct CertificateResolver {
    domains: Arc<ArcSwap<Domains>>,
}

async fn resolve_and_parse_certificates(config: &Configuration) -> Result<Domains> {
    let mut domains = HashMap::new();

    for (domain, secrets) in config {
        let (cert, key) = secrets.resolve_files().await?;
        let certified_key = parse_certified_key(&cert, &key)?;

        domains.insert(domain.to_owned(), Arc::new(certified_key));
    }

    Ok(domains)
}

async fn poll_for_certificate_updates(
    message_bus: Arc<MessageBus>,
    config: &Configuration,
    domains: Arc<ArcSwap<Domains>>,
) -> Result<()> {
    while message_bus
        .receive_certificate_update_request()
        .await
        .is_ok()
    {
        let span = tracing::info_span!("certificate_update");
        let _enter = span.enter();

        tracing::info!("processing certificate update request");

        match resolve_and_parse_certificates(config).await {
            Ok(new_domains) => {
                domains.store(Arc::new(new_domains));

                tracing::info!("successfully updated the certificate");
            }
            Err(error) => {
                tracing::error!(%error, "failed to update certificate");
            }
        }
    }

    tracing::info!("certificate update request receiver closed, stopping resolver");

    Ok(())
}

impl CertificateResolver {
    pub async fn new(config: Arc<Configuration>, message_bus: Arc<MessageBus>) -> Result<Self> {
        let domains = resolve_and_parse_certificates(&config).await?;
        let domains = Arc::new(ArcSwap::from_pointee(domains));

        let resolver = Self {
            domains: Arc::clone(&domains),
        };

        tokio::spawn({
            async move {
                poll_for_certificate_updates(message_bus, &config, domains)
                    .await
                    .unwrap_or_else(|error| {
                        tracing::error!(%error, "failed to poll for certificate updates");
                    });
            }
        });

        Ok(resolver)
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

#[derive(Debug)]
pub struct DynamicAuthenticationLevelResolver {
    config: Arc<ArcSwap<Config>>,
}

impl DynamicAuthenticationLevelResolver {
    pub fn new(config: Arc<ArcSwap<Config>>) -> Arc<Self> {
        Arc::new(Self { config })
    }
}

impl AuthenticationLevelResolver for DynamicAuthenticationLevelResolver {
    fn resolve(&self, client_hello: &str) -> Option<AuthenticationLevel> {
        let config = self.config.load();

        let Some(mtls) = config.alb.mtls.as_ref() else {
            return Some(AuthenticationLevel::Standard);
        };

        if mtls.domains.contains(client_hello) {
            Some(AuthenticationLevel::Mutual)
        } else {
            Some(AuthenticationLevel::Standard)
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::{HashMap, HashSet};
    use std::net::Ipv4Addr;
    use std::path::{Path, PathBuf};
    use std::sync::Arc;
    use std::time::Duration;

    use arc_swap::ArcSwap;
    use color_eyre::eyre::{eyre, Result};
    use mutual_tls::{AuthenticationLevel, AuthenticationLevelResolver};
    use rustls::pki_types::pem::PemObject;
    use rustls::pki_types::CertificateDer;

    use crate::config::{AlbConfig, Config, ExternalBytes, MtlsConfig, TlsSecrets};
    use crate::ipc::MessageBus;
    use crate::load_balancer::tls::{CertificateResolver, DynamicAuthenticationLevelResolver};

    const PRIMARY_DOMAIN: &str = "primary.example.com";
    const SECONDARY_DOMAIN: &str = "secondary.example.com";

    #[test]
    fn configuration_updates_propagate_immediately() {
        let domain1 = "example.com";
        let domain2 = "example.org";

        let alb = AlbConfig {
            addr: Ipv4Addr::LOCALHOST,
            port: 5000,
            reconciliation: String::new(),
            tls: None,
            mtls: Some(MtlsConfig {
                anchor: ExternalBytes::Filesystem {
                    path: PathBuf::new(),
                },
                domains: HashSet::from([domain1.to_string()]),
            }),
        };

        let mut original_config = Config {
            alb,
            secrets: None,
            services: HashMap::new(),
        };

        let config = Arc::new(ArcSwap::from_pointee(original_config.clone()));
        let resolver = DynamicAuthenticationLevelResolver::new(Arc::clone(&config));

        assert!(matches!(
            resolver.resolve(&domain1),
            Some(AuthenticationLevel::Mutual)
        ));
        assert!(matches!(
            resolver.resolve(&domain2),
            Some(AuthenticationLevel::Standard)
        ));

        original_config
            .alb
            .mtls
            .as_mut()
            .unwrap()
            .domains
            .insert(domain2.to_string());

        config.store(Arc::new(original_config));

        assert!(matches!(
            resolver.resolve(&domain1),
            Some(AuthenticationLevel::Mutual)
        ));
        assert!(matches!(
            resolver.resolve(&domain2),
            Some(AuthenticationLevel::Mutual)
        ));
    }

    /// Builds a `TlsSecrets` instance from the given certificate and key paths.
    fn build_tls_secrets(cert_path: &Path, key_path: &Path) -> TlsSecrets {
        let cert_file = ExternalBytes::Filesystem {
            path: cert_path.to_owned(),
        };

        let key_file = ExternalBytes::Filesystem {
            path: key_path.to_owned(),
        };

        TlsSecrets::new(cert_file, key_file)
    }

    /// Stages a resource from the `resources` directory into the specified directory.
    async fn stage_resource<D: AsRef<Path>, S: AsRef<Path>>(dir: D, source: S) -> Result<PathBuf> {
        let filename = source
            .as_ref()
            .file_name()
            .ok_or_else(|| eyre!("source path does not have a file name"))?;

        let destination = dir.as_ref().join(filename);
        let source = Path::new("resources").join(source);

        tokio::fs::copy(source, &destination).await?;

        Ok(destination)
    }

    /// Builds a configuration for the `CertificateResolver` from a list of domain, certificate, and key pairs.
    fn build_resolver_config(secrets: &[(&str, &Path, &Path)]) -> HashMap<String, TlsSecrets> {
        secrets
            .iter()
            .map(|(domain, cert_path, key_path)| {
                let secrets = build_tls_secrets(cert_path, key_path);
                (domain.to_string(), secrets)
            })
            .collect()
    }

    /// Verifies that the certificate for the specified domain matches the expected certificate for
    /// the current state of the resolver.
    fn verify_certificate_matches(
        resolver: &CertificateResolver,
        domain: &str,
        expected_cert_path: &str,
    ) -> Result<()> {
        let domains = resolver.domains.load();
        let certified_key = domains
            .get(domain)
            .ok_or_else(|| eyre!("no certificate found for domain `{}`", domain))?;

        let expected_cert_path = Path::new("resources").join(expected_cert_path);

        let cert = &certified_key.cert[0];
        let expected = CertificateDer::from_pem_file(expected_cert_path)?;

        assert_eq!(*cert, expected, "certificate does not match expected");

        Ok(())
    }

    /// Builds a `CertificateResolver` instance with the given configuration, returning a sender
    /// for certificate update requests and the resolver itself.
    async fn build_resolver(
        config: HashMap<String, TlsSecrets>,
    ) -> Result<(Arc<MessageBus>, CertificateResolver)> {
        let message_bus = MessageBus::new();
        let resolver = CertificateResolver::new(Arc::new(config), Arc::clone(&message_bus)).await?;

        Ok((message_bus, resolver))
    }

    #[tokio::test]
    async fn can_load_certificates_initially() -> Result<()> {
        let temp_dir = tempfile::tempdir()?;

        let certificate_path = stage_resource(temp_dir.path(), "certificates/old.crt").await?;
        let key_path = stage_resource(temp_dir.path(), "certificates/old.key").await?;

        let config = build_resolver_config(&[(PRIMARY_DOMAIN, &certificate_path, &key_path)]);
        let (_, resolver) = build_resolver(config).await?;

        verify_certificate_matches(&resolver, PRIMARY_DOMAIN, "certificates/old.crt")?;

        Ok(())
    }

    #[tokio::test]
    async fn certificates_can_be_rolled_automatically() -> Result<()> {
        let temp_dir = tempfile::tempdir()?;

        let certificate_path = stage_resource(temp_dir.path(), "certificates/old.crt").await?;
        let key_path = stage_resource(temp_dir.path(), "certificates/old.key").await?;

        let config = build_resolver_config(&[(PRIMARY_DOMAIN, &certificate_path, &key_path)]);
        let (message_bus, resolver) = build_resolver(config).await?;

        // copy the new certificates over
        tokio::fs::copy("resources/certificates/new.crt", &certificate_path).await?;
        tokio::fs::copy("resources/certificates/new.key", &key_path).await?;

        // inform the resolver about a certificate update
        message_bus.send_certificate_update_request()?;

        // check that the resolver returns the new certificate, after a little bit of time
        tokio::time::sleep(Duration::from_millis(5)).await;

        verify_certificate_matches(&resolver, PRIMARY_DOMAIN, "certificates/new.crt")?;

        Ok(())
    }

    #[tokio::test]
    async fn all_domains_are_updated_on_notifications() -> Result<()> {
        let temp_dir = tempfile::tempdir()?;

        let first_certificate_path = temp_dir.path().join("first-cert.pem");
        let first_key_path = temp_dir.path().join("first-key.pem");

        let second_certificate_path = temp_dir.path().join("second-cert.pem");
        let second_key_path = temp_dir.path().join("second-key.pem");

        tokio::fs::copy("resources/certificates/old.crt", &first_certificate_path).await?;
        tokio::fs::copy("resources/certificates/old.key", &first_key_path).await?;

        tokio::fs::copy("resources/certificates/old.crt", &second_certificate_path).await?;
        tokio::fs::copy("resources/certificates/old.key", &second_key_path).await?;

        let config = build_resolver_config(&[
            (PRIMARY_DOMAIN, &first_certificate_path, &first_key_path),
            (SECONDARY_DOMAIN, &second_certificate_path, &second_key_path),
        ]);

        let (message_bus, resolver) = build_resolver(config).await?;

        // copy the new certificates over for both domains
        tokio::fs::copy("resources/certificates/new.crt", &first_certificate_path).await?;
        tokio::fs::copy("resources/certificates/new.key", &first_key_path).await?;

        tokio::fs::copy("resources/certificates/new.crt", &second_certificate_path).await?;
        tokio::fs::copy("resources/certificates/new.key", &second_key_path).await?;

        // inform the resolver about a certificate update
        message_bus.send_certificate_update_request()?;

        // check that the resolver returns the new certificates
        tokio::time::sleep(Duration::from_millis(5)).await;

        verify_certificate_matches(&resolver, PRIMARY_DOMAIN, "certificates/new.crt")?;
        verify_certificate_matches(&resolver, SECONDARY_DOMAIN, "certificates/new.crt")?;

        Ok(())
    }
}

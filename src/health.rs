#![allow(dead_code)]

use std::time::Duration;

use color_eyre::eyre::Result;
use hyper::{Client, Uri};

#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum HealthCheckResult {
    Success,
    Failure,
}

pub struct HealthCheckConfiguration {
    period: Duration,
    success_threshold: u32,
    failure_threshold: u32,
}

impl HealthCheckConfiguration {
    pub fn new(period: Duration, success_threshold: u32, failure_threshold: u32) -> Self {
        Self {
            period,
            success_threshold,
            failure_threshold,
        }
    }
}

pub struct HealthCheck {
    target: Uri,
    configuration: HealthCheckConfiguration,
}

impl HealthCheck {
    pub fn new(target: Uri, configuration: HealthCheckConfiguration) -> Self {
        Self {
            target,
            configuration,
        }
    }

    pub async fn run(&self) -> Result<HealthCheckResult> {
        let client = Client::new();

        let mut successes = 0;
        let mut failures = 0;

        loop {
            tokio::time::sleep(self.configuration.period).await;

            match client.get(self.target.clone()).await {
                Ok(res) if res.status().is_success() => successes += 1,
                _ => failures += 1,
            }

            dbg!(successes, failures);

            if successes >= self.configuration.success_threshold {
                return Ok(HealthCheckResult::Success);
            }

            if failures >= self.configuration.failure_threshold {
                return Ok(HealthCheckResult::Failure);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::net::{Ipv4Addr, SocketAddrV4, TcpListener};
    use std::str::FromStr;
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::sync::Arc;
    use std::time::Duration;

    use color_eyre::eyre::{Report, Result};
    use hyper::service::{make_service_fn, service_fn};
    use hyper::{Body, Response, Server, Uri};

    use crate::health::{HealthCheck, HealthCheckConfiguration, HealthCheckResult};

    #[tokio::test]
    async fn can_perform_health_checks() -> Result<()> {
        let addr = SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0);
        let listener = TcpListener::bind(&addr)?;

        let service = make_service_fn(move |_| async move {
            Ok::<_, Report>(service_fn(move |_| async move {
                Ok::<_, Report>(Response::new(Body::from("OK")))
            }))
        });

        let resolved_addr = listener.local_addr()?;

        tokio::spawn(async move {
            let server = Server::from_tcp(listener)
                .expect("Failed to create server")
                .serve(service);

            server.await.expect("Failed to run server");
        });

        let target = format!("http://{resolved_addr}");
        let target = Uri::from_str(&target)?;

        let configuration = HealthCheckConfiguration::new(Duration::from_millis(2), 1, 1);
        let health_check = HealthCheck::new(target, configuration);

        let result = health_check.run().await?;

        assert_eq!(result, HealthCheckResult::Success);

        Ok(())
    }

    #[tokio::test]
    async fn health_checks_can_be_failed() -> Result<()> {
        let addr = SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0);
        let listener = TcpListener::bind(&addr)?;

        // service always responds with a 500
        let service = make_service_fn(move |_| async move {
            Ok::<_, Report>(service_fn(move |_| async move {
                Ok::<_, Report>(
                    Response::builder()
                        .status(500)
                        .body(Body::default())
                        .unwrap(),
                )
            }))
        });

        let resolved_addr = listener.local_addr()?;

        tokio::spawn(async move {
            let server = Server::from_tcp(listener)
                .expect("Failed to create server")
                .serve(service);

            server.await.expect("Failed to run server");
        });

        let target = format!("http://{resolved_addr}");
        let target = Uri::from_str(&target)?;

        let configuration = HealthCheckConfiguration::new(Duration::from_millis(2), 1, 1);
        let health_check = HealthCheck::new(target, configuration);

        let result = health_check.run().await?;

        assert_eq!(result, HealthCheckResult::Failure);

        Ok(())
    }

    #[tokio::test]
    async fn health_checks_are_retried() -> Result<()> {
        let addr = SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0);
        let listener = TcpListener::bind(&addr)?;

        let requests = Arc::new(AtomicU32::new(1));

        // service responds with 2 500s and then a 200
        let service = make_service_fn(move |_| {
            let requests = Arc::clone(&requests);

            async move {
                Ok::<_, Report>(service_fn(move |_| {
                    let requests = Arc::clone(&requests);

                    async move {
                        let status = match requests.fetch_add(1, Ordering::SeqCst) {
                            3 => 200,
                            _ => 500,
                        };

                        Ok::<_, Report>(
                            Response::builder()
                                .status(status)
                                .body(Body::default())
                                .unwrap(),
                        )
                    }
                }))
            }
        });

        let resolved_addr = listener.local_addr()?;

        tokio::spawn(async move {
            let server = Server::from_tcp(listener)
                .expect("Failed to create server")
                .serve(service);

            server.await.expect("Failed to run server");
        });

        let target = format!("http://{resolved_addr}");
        let target = Uri::from_str(&target)?;

        // 3 failures and we fail overall, but a single successful response is enough
        let configuration = HealthCheckConfiguration::new(Duration::from_millis(2), 1, 3);
        let health_check = HealthCheck::new(target, configuration);

        let result = health_check.run().await?;

        assert_eq!(result, HealthCheckResult::Success);

        Ok(())
    }
}

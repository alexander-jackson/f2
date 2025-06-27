use std::sync::Arc;

use color_eyre::eyre::{eyre, Result};
use flume::{Receiver, Sender};
use uuid::Uuid;

pub struct Message<T> {
    identifier: Uuid,
    #[allow(dead_code)]
    content: T,
}

#[derive(Debug)]
pub struct CertificateUpdateRequest;
#[derive(Debug)]
pub struct ReconciliationRequest;

#[derive(Debug)]
pub struct ChannelPair<T> {
    sender: Sender<Message<T>>,
    receiver: Receiver<Message<T>>,
}

impl<T> ChannelPair<T> {
    pub fn new() -> Self {
        let (sender, receiver) = flume::unbounded();

        ChannelPair { sender, receiver }
    }
}

#[derive(Debug)]
pub struct MessageBus {
    reconciliation: ChannelPair<ReconciliationRequest>,
    resolver: ChannelPair<CertificateUpdateRequest>,
}

impl MessageBus {
    pub fn new() -> Arc<Self> {
        let reconciliation_pair = ChannelPair::<ReconciliationRequest>::new();
        let resolver_pair = ChannelPair::<CertificateUpdateRequest>::new();

        let message_bus = MessageBus {
            reconciliation: reconciliation_pair,
            resolver: resolver_pair,
        };

        Arc::new(message_bus)
    }

    pub fn send_reconciliation_request(&self) -> Result<Uuid> {
        let identifier = Uuid::new_v4();
        let message = Message {
            identifier,
            content: ReconciliationRequest,
        };

        tracing::debug!(%identifier, "sending reconciliation request");

        self.reconciliation
            .sender
            .send(message)
            .map_err(|_| eyre!("Failed to send reconciliation request"))?;

        Ok(identifier)
    }

    pub fn send_certificate_update_request(&self) -> Result<Uuid> {
        let identifier = Uuid::new_v4();
        let message = Message {
            identifier,
            content: CertificateUpdateRequest,
        };

        tracing::debug!(%identifier, "sending certificate update request");

        self.resolver
            .sender
            .send(message)
            .map_err(|_| eyre!("Failed to send certificate update request"))?;

        Ok(identifier)
    }

    pub async fn receive_reconciliation_request(
        &self,
    ) -> Result<Message<ReconciliationRequest>, flume::RecvError> {
        let received = self.reconciliation.receiver.recv_async().await?;

        tracing::debug!(%received.identifier, "received reconciliation request");

        Ok(received)
    }

    pub async fn receive_certificate_update_request(
        &self,
    ) -> Result<Message<CertificateUpdateRequest>, flume::RecvError> {
        let received = self.resolver.receiver.recv_async().await?;

        tracing::debug!(%received.identifier, "received certificate update request");

        Ok(received)
    }
}

#[cfg(test)]
mod tests {
    use color_eyre::eyre::Result;

    use crate::ipc::MessageBus;

    #[tokio::test]
    async fn can_send_and_receive_reconciliation_requests() -> Result<()> {
        let message_bus = MessageBus::new();

        let sent = message_bus.send_reconciliation_request()?;
        let received = message_bus.receive_reconciliation_request().await?;

        assert_eq!(sent, received.identifier);

        Ok(())
    }

    #[tokio::test]
    async fn can_send_and_receive_certificate_update_requests() -> Result<()> {
        let message_bus = MessageBus::new();

        let sent = message_bus.send_certificate_update_request()?;
        let received = message_bus.receive_certificate_update_request().await?;

        assert_eq!(sent, received.identifier);

        Ok(())
    }
}

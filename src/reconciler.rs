use std::sync::Arc;

use color_eyre::Result;
use tokio::sync::mpsc::Sender;
use tokio::sync::RwLock;

use crate::args::ConfigurationLocation;
use crate::config::{Config, Diff};

#[derive(Debug, Clone)]
pub struct Reconciler {
    alb_path: Arc<str>,
    config_location: Arc<ConfigurationLocation>,
    config: Arc<RwLock<Config>>,
    sender: Sender<Diff>,
}

impl Reconciler {
    pub fn new(
        alb_path: &str,
        config_location: ConfigurationLocation,
        config: Config,
        sender: Sender<Diff>,
    ) -> Self {
        Self {
            alb_path: Arc::from(alb_path),
            config_location: Arc::new(config_location),
            config: Arc::new(RwLock::new(config)),
            sender,
        }
    }

    pub fn get_path(&self) -> &str {
        &*self.alb_path
    }

    pub async fn reconcile(&self) -> Result<()> {
        let new_config = Config::from_location(&self.config_location).await?;
        let read_lock = self.config.read().await;
        let old_config = &read_lock;

        if let Some(diff) = old_config.diff(&new_config) {
            // Drop the read lock, acquire a write one
            drop(read_lock);

            let mut write_lock = self.config.write().await;
            *write_lock = new_config;

            // Drop the write lock and begin sending events
            drop(write_lock);

            for event in diff {
                self.sender.send(event).await?;
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {}

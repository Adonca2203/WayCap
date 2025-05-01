use tokio::sync::mpsc;
use zbus::interface;

use crate::application_config::{AppConfig, AppConfigDbus};

pub trait GameClip {
    async fn save_clip(&self);
    async fn update_config(&self, new_config: AppConfigDbus) -> zbus::fdo::Result<()>;
}

pub struct ClipService {
    save_tx: mpsc::Sender<()>,
    config_tx: mpsc::Sender<AppConfig>,
}

impl ClipService {
    pub fn new(save_tx: mpsc::Sender<()>, config_tx: mpsc::Sender<AppConfig>) -> Self {
        Self { save_tx, config_tx }
    }
}

#[interface(name = "com.rust.WayCap")]
impl GameClip for ClipService {
    async fn save_clip(&self) {
        log::debug!("Save clip received!");
        let _ = self.save_tx.send(()).await;
    }

    async fn update_config(&self, new_config: AppConfigDbus) -> zbus::fdo::Result<()> {
        let config = AppConfig::try_from(new_config).map_err(zbus::fdo::Error::Failed)?;
        let _ = self.config_tx.send(config).await;
        Ok(())
    }
}

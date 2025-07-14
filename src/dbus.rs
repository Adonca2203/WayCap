use tokio::sync::mpsc;
use zbus::interface;

use crate::application_config::{AppConfig, AppConfigDbus, AppModeDbus};

pub trait GameClip {
    async fn save_clip(&self);
    async fn update_config(&self, new_config: AppConfigDbus) -> zbus::fdo::Result<()>;
    async fn change_mode(&self, new_mode: AppModeDbus) -> zbus::fdo::Result<()>;
}

pub struct ClipService {
    save_tx: mpsc::Sender<()>,
    config_tx: mpsc::Sender<AppConfig>,
    change_mode_tx: mpsc::Sender<AppModeDbus>,
}

impl ClipService {
    pub fn new(
        save_tx: mpsc::Sender<()>,
        config_tx: mpsc::Sender<AppConfig>,
        change_mode_tx: mpsc::Sender<AppModeDbus>,
    ) -> Self {
        Self {
            save_tx,
            config_tx,
            change_mode_tx,
        }
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

    async fn change_mode(&self, new_mode: AppModeDbus) -> zbus::fdo::Result<()> {
        let _ = self.change_mode_tx.send(new_mode).await;
        Ok(())
    }
}

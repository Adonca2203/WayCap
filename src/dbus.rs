use log::debug;
use tokio::sync::mpsc;
use zbus::interface;

use crate::application_config::AppConfig;

pub trait GameClip {
    async fn save_clip(&self);
    async fn update_config(&self, new_config: AppConfig);
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
        let _ = self.save_tx.send(()).await;
        debug!("Save clip received!");
    }

    async fn update_config(&self, new_config: AppConfig) {
        let _ = self.config_tx.send(new_config).await;
    }
}

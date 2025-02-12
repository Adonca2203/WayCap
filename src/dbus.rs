use log::debug;
use tokio::sync::mpsc;
use zbus::interface;

pub trait GameClip {
    async fn save_clip(&self);
}

pub struct ClipService {
    save_tx: mpsc::Sender<()>,
}

impl ClipService {
    pub fn new(save_tx: mpsc::Sender<()>) -> Self {
        Self { save_tx }
    }
}

#[interface(name = "com.rust.GameClip")]
impl GameClip for ClipService {
    async fn save_clip(&self) {
        let _ = self.save_tx.send(()).await;
        debug!("Save clip received!");
    }
}

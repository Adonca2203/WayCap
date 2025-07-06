use crate::{
    app_context::AppContext,
    application_config::{update_config, AppConfig},
    dbus,
    modes::AppMode,
};
use anyhow::Result;
use std::sync::{atomic::AtomicBool, Arc};
use tokio::sync::mpsc;
use waycap_rs::pipeline::builder::CaptureBuilder;
use zbus::{connection, Connection};

pub struct WayCap<M: AppMode> {
    context: AppContext,
    dbus_conn: Option<Connection>,
    dbus_save_rx: mpsc::Receiver<()>,
    dbus_config_rx: mpsc::Receiver<AppConfig>,
    mode: M,
}

impl<M: AppMode> WayCap<M> {
    pub async fn new(mut mode: M, _config: AppConfig) -> Result<Self> {
        simple_logging::log_to_file("logs.txt", log::LevelFilter::Info)?;
        let saving = Arc::new(AtomicBool::new(false));
        let stop = Arc::new(AtomicBool::new(false));
        let join_handles: Vec<std::thread::JoinHandle<()>> = Vec::new();

        let (dbus_save_tx, dbus_save_rx) = mpsc::channel(1);
        let (dbus_config_tx, dbus_config_rx): (mpsc::Sender<AppConfig>, mpsc::Receiver<AppConfig>) =
            mpsc::channel(1);
        let clip_service = dbus::ClipService::new(dbus_save_tx, dbus_config_tx);

        log::debug!("Creating dbus connection");
        let connection = connection::Builder::session()?
            .name("com.rust.WayCap")?
            .serve_at("/com/rust/WayCap", clip_service)?
            .build()
            .await?;

        let mut capture = CaptureBuilder::new()
            .with_audio()
            .with_quality_preset(waycap_rs::types::config::QualityPreset::Medium)
            .with_cursor_shown()
            .with_audio_encoder(waycap_rs::types::config::AudioEncoder::Opus)
            .build()?;

        capture.start()?;
        let mut ctx = AppContext {
            saving,
            stop,
            join_handles,
            capture,
        };

        mode.init(&mut ctx).await?;

        Ok(Self {
            context: ctx,
            dbus_save_rx,
            dbus_config_rx,
            mode,
            dbus_conn: Some(connection),
        })
    }

    pub async fn run(&mut self) -> Result<()> {
        loop {
            tokio::select! {
                _ = self.dbus_save_rx.recv() => {
                    log::debug!("Saving...");
                    self.mode.on_save(&mut self.context).await?;
                },
                Some(cfg) = self.dbus_config_rx.recv() => {
                    update_config(cfg);
                },
                _ = tokio::signal::ctrl_c() => {
                    log::debug!("Shutting down");
                    self.mode.on_shutdown(&mut self.context).await?;
                    break;
                }
            }
        }

        if let Some(conn) = self.dbus_conn.take() {
            if let Err(e) = conn.close().await {
                log::error!("Error closing dbus connection: {e:?}");
            }
        }

        for handle in self.context.join_handles.drain(..) {
            if let Err(e) = handle.join() {
                log::error!("Error shutting down a worker handle: {e:?}");
            }
        }

        Ok(())
    }
}

use crate::{
    app_context::AppContext,
    application_config::{update_config, AppConfig},
    dbus,
    modes::AppMode,
    pw_capture::{audio_stream::AudioCapture, video_stream::VideoCapture},
    RawAudioFrame, RawVideoFrame, Terminate,
};
use anyhow::Result;
use pipewire::{self as pw};
use portal_screencast::{CursorMode, ScreenCast, SourceType};
use ringbuf::{traits::Split, HeapRb};
use std::{
    sync::{atomic::AtomicBool, Arc},
    time::{Duration, Instant, SystemTime},
};
use tokio::sync::mpsc;
use zbus::{connection, Connection};

pub struct WayCap<M: AppMode> {
    context: AppContext,
    dbus_conn: Option<Connection>,
    dbus_save_rx: mpsc::Receiver<()>,
    dbus_config_rx: mpsc::Receiver<AppConfig>,
    pw_video_terminate_tx: pw::channel::Sender<Terminate>,
    pw_audio_terminate_tx: pw::channel::Sender<Terminate>,
    mode: M,
}

impl<M: AppMode> WayCap<M> {
    pub async fn new(mut mode: M, config: AppConfig) -> Result<Self> {
        simple_logging::log_to_file("logs.txt", log::LevelFilter::Trace)?;
        let current_time = SystemTime::now();
        let saving = Arc::new(AtomicBool::new(false));
        let stop = Arc::new(AtomicBool::new(false));
        let mut join_handles: Vec<std::thread::JoinHandle<()>> = Vec::new();

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

        let audio_ready = Arc::new(AtomicBool::new(false));
        let video_ready = Arc::new(AtomicBool::new(false));

        let (mut width, mut height) = (0, 0);

        let video_ring_buffer = HeapRb::<RawVideoFrame>::new(250);
        let (video_ring_sender, video_ring_receiver) = video_ring_buffer.split();

        let (pw_video_sender, pw_video_receiver) = pw::channel::channel();
        let (resolution_sender, mut resolution_receiver) = mpsc::channel::<(u32, u32)>(2);
        let video_ready_pw = Arc::clone(&video_ready);
        let audio_ready_pw = Arc::clone(&audio_ready);
        let saving_video = Arc::clone(&saving);

        let mut screen_cast = ScreenCast::new()?;
        screen_cast.set_source_types(SourceType::all());
        screen_cast.set_cursor_mode(CursorMode::EMBEDDED);
        let active_cast = screen_cast.start(None)?;

        let fd = active_cast.pipewire_fd();
        let stream = active_cast.streams().next().unwrap();
        let stream_node = stream.pipewire_node();

        let pw_video_capture = std::thread::spawn(move || {
            let video_cap = VideoCapture::new(video_ready_pw, audio_ready_pw);
            video_cap
                .run(
                    fd,
                    stream_node,
                    video_ring_sender,
                    pw_video_receiver,
                    saving_video,
                    current_time,
                    resolution_sender,
                )
                .unwrap();

            let _ = active_cast.close();
        });

        // Window mode return (0, 0) for dimensions so we have to get it from pipewire
        if (width, height) == (0, 0) {
            // Wait to get back a negotiated resolution from pipewire
            let timeout = Duration::from_secs(5);
            let start = Instant::now();
            loop {
                if let Ok((recv_width, recv_height)) = resolution_receiver.try_recv() {
                    (width, height) = (recv_width, recv_height);
                    break;
                }

                if start.elapsed() > timeout {
                    log::error!("Timeout waiting for PipeWire negotiated resolution.");
                    std::process::exit(1);
                }

                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        }
        join_handles.push(pw_video_capture);

        let audio_ring_buffer = HeapRb::<RawAudioFrame>::new(10);
        let (audio_ring_sender, audio_ring_receiver) = audio_ring_buffer.split();
        let (pw_audio_sender, pw_audio_recv) = pw::channel::channel();
        let saving_audio = Arc::clone(&saving);
        let pw_audio_worker = std::thread::spawn(move || {
            log::debug!("Starting audio stream");
            let audio_cap = AudioCapture::new(video_ready, audio_ready);
            audio_cap
                .run(audio_ring_sender, current_time, pw_audio_recv, saving_audio)
                .unwrap();
        });

        join_handles.push(pw_audio_worker);

        let mut ctx = AppContext {
            saving,
            stop,
            join_handles,
            width,
            height,
            video_ring_receiver: Some(video_ring_receiver),
            audio_ring_receiver: Some(audio_ring_receiver),
            config,
        };

        mode.init(&mut ctx).await?;

        Ok(Self {
            context: ctx,
            dbus_save_rx,
            dbus_config_rx,
            pw_video_terminate_tx: pw_video_sender,
            pw_audio_terminate_tx: pw_audio_sender,
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

        // Shutdown capture threads
        if self.pw_video_terminate_tx.send(Terminate).is_err() {
            log::error!("Error sending terminate signal to pipewire video capture.");
        }
        if self.pw_audio_terminate_tx.send(Terminate).is_err() {
            log::error!("Error sending terminate signal to pipewire audio capture.");
        }

        if let Some(conn) = self.dbus_conn.take() {
            if let Err(e) = conn.close().await {
                log::error!("Error closing dbus connection: {:?}", e);
            }
        }

        for handle in self.context.join_handles.drain(..) {
            if let Err(e) = handle.join() {
                log::error!("Error shutting down a worker handle: {:?}", e);
            }
        }

        Ok(())
    }
}

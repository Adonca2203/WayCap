mod dbus;
mod ffmpeg_encoder;
mod pipewire_capture;

use std::sync::Arc;

use anyhow::{Error, Result};
use pipewire_capture::PipewireCapture;
use log::{debug, LevelFilter};
use portal_screencast::{ScreenCast, SourceType};
use tokio::sync::{mpsc, Mutex};
use zbus::connection;

#[tokio::main]
async fn main() -> Result<(), Error> {
    let _ = simple_logging::log_to_file("logs.txt", LevelFilter::Debug);

    // TODO: Grab these dynamically based on screen picked
    // with screencast portal?
    let width = 2560;
    let height = 1440;
    let fps = 240;
    let max_seconds = 300;

    let (save_tx, mut save_rx) = mpsc::channel(1);
    let clip_service = dbus::ClipService::new(save_tx);
    let encoder = Arc::new(Mutex::new(ffmpeg_encoder::FfmpegEncoder::new(
        width,
        height,
        fps,
        max_seconds,
    )?));

    let encoder_thread = Arc::clone(&encoder);

    debug!("Creating dbus connection");
    let _connection = connection::Builder::session()?
        .name("com.rust.GameClip")?
        .serve_at("/com/rust/GameClip", clip_service)?
        .build()
        .await?;

    debug!("Selecting screen to record");
    let mut screen_cast = ScreenCast::new()?;
    screen_cast.set_source_types(SourceType::MONITOR);

    let screen_cast = screen_cast.start(None)?;

    let fd = screen_cast.pipewire_fd();
    debug!("Stream nodes: {}", screen_cast.streams().count());
    let stream_node = screen_cast.streams().next().unwrap().pipewire_node();
    debug!("Pipewire fd: {}", fd);

    std::thread::spawn(move || {
        let video_encoder_clone = Arc::clone(&encoder_thread);
        let audio_encoder_clone = Arc::clone(&encoder_thread);
        debug!("Creating pipewire stream");
        let _capture = PipewireCapture::new(
            fd,
            stream_node,
            move |frame, time| {
                video_encoder_clone
                    .blocking_lock()
                    .process_frame(&frame, time)
                    .unwrap();
            },
            move |audio, timestamp| {
                audio_encoder_clone
                    .blocking_lock()
                    .process_audio(&audio, timestamp)
                    .unwrap();
            },
        )
        .unwrap();
    });

    // Main event loop
    loop {
        tokio::select! {
            _ = save_rx.recv() => {
                let filename = format!("clip_{}.mp4", chrono::Local::now().timestamp());
                if encoder.lock().await.video_buffer.is_empty() {
                    debug!("No encoded packets to save!")
                }
                else {
                    encoder.lock().await.save_buffer(&filename)?;
                    debug!("Saved file {}", filename);
                }
            }
        }
    }
}

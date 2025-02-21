mod dbus;
mod ffmpeg_encoder;
mod local_pipewire;

use std::sync::Arc;

use anyhow::{Error, Result};
use local_pipewire::PipewireCapture;
use log::{debug, LevelFilter};
use portal_screencast::ScreenCast;
use tokio::sync::{mpsc, Mutex};
use zbus::connection;

#[tokio::main]
async fn main() -> Result<(), Error> {
    let _ = simple_logging::log_to_file("logs.txt", LevelFilter::Debug);

    let width = 2560;
    let height = 1440;
    let fps = 60;
    let max_seconds = 3;

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
    let screen_cast = ScreenCast::new()?.start(None)?;

    let fd = screen_cast.pipewire_fd();
    debug!("Stream nodes: {}", screen_cast.streams().count());
    let stream_node = screen_cast.streams().next().unwrap().pipewire_node();
    debug!("Pipewire fd: {}", fd);

    std::thread::spawn(move || {
        let encoder_clone = Arc::clone(&encoder_thread);
        debug!("Creating pipewire stream");
        let _capture = PipewireCapture::new(fd, stream_node, move |frame, time| {
            encoder_clone.blocking_lock().process_frame(&frame, time).unwrap();
        })
        .unwrap();
    });

    // Main event loop
    loop {
        tokio::select! {
            _ = save_rx.recv() => {
                let filename = format!("clip_{}.mp4", chrono::Local::now().timestamp());
                if encoder.lock().await.buffer.is_empty() {
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

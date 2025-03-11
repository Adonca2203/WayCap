mod dbus;
mod ffmpeg_encoder;
mod pipewire_capture;
mod encoders;

use std::sync::Arc;

use anyhow::{Error, Result};
use pipewire_capture::PipewireCapture;
use log::{debug, LevelFilter};
use portal_screencast::{CursorMode, ScreenCast, SourceType};
use tokio::sync::{mpsc, Mutex};
use zbus::connection;

#[tokio::main]
async fn main() -> Result<(), Error> {
    let _ = simple_logging::log_to_file("logs.txt", LevelFilter::Debug);

    let mut screen_cast = ScreenCast::new()?;
    screen_cast.set_source_types(SourceType::MONITOR);
    screen_cast.set_cursor_mode(CursorMode::EMBEDDED);
    let screen_cast = screen_cast.start(None)?;

    let fd = screen_cast.pipewire_fd();
    let stream = screen_cast.streams().next().unwrap();
    let stream_node = stream.pipewire_node();
    let (width, height) = stream.size();

    let target_fps = 60;
    let max_seconds = 300;

    let (save_tx, mut save_rx) = mpsc::channel(1);
    let clip_service = dbus::ClipService::new(save_tx);
    let encoder = Arc::new(Mutex::new(ffmpeg_encoder::FfmpegEncoder::new(
        width,
        height,
        target_fps,
        max_seconds,
    )?));

    let encoder_thread = Arc::clone(&encoder);

    debug!("Creating dbus connection");
    let _connection = connection::Builder::session()?
        .name("com.rust.GameClip")?
        .serve_at("/com/rust/GameClip", clip_service)?
        .build()
        .await?;


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
                    .process_video(&frame, time)
                    .unwrap();
            },
            move |mut audio, timestamp| {
                audio_encoder_clone
                    .blocking_lock()
                    .process_audio(&mut audio, timestamp)
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

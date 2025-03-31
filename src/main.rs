mod application_config;
mod dbus;
mod encoders;
mod pipewire_capture;

use std::sync::Arc;

use anyhow::{Context, Error, Result};
use application_config::load_or_create_config;
use encoders::{
    audio_encoder::AudioEncoder,
    buffer::{AudioBuffer, VideoBuffer},
    video_encoder::VideoEncoder,
};
use ffmpeg_next::{self as ffmpeg};
use log::{debug, LevelFilter};
use pipewire_capture::PipewireCapture;
use portal_screencast::{CursorMode, ScreenCast, SourceType};
use tokio::sync::{mpsc, Mutex};
use zbus::connection;

const VIDEO_STREAM: usize = 0;
const AUDIO_STREAM: usize = 1;

#[tokio::main]
async fn main() -> Result<(), Error> {
    let _ = simple_logging::log_to_file("logs.txt", LevelFilter::Debug);
    let config = load_or_create_config();

    let mut screen_cast = ScreenCast::new()?;
    screen_cast.set_source_types(SourceType::MONITOR);
    screen_cast.set_cursor_mode(CursorMode::EMBEDDED);
    let screen_cast = screen_cast.start(None)?;

    let fd = screen_cast.pipewire_fd();
    let stream = screen_cast.streams().next().unwrap();
    let stream_node = stream.pipewire_node();
    let (width, height) = stream.size();

    let (save_tx, mut save_rx) = mpsc::channel(1);
    let clip_service = dbus::ClipService::new(save_tx);

    debug!("Creating dbus connection");
    let _connection = connection::Builder::session()?
        .name("com.rust.GameClip")?
        .serve_at("/com/rust/GameClip", clip_service)?
        .build()
        .await?;

    let (video_sender, mut video_receiver) = mpsc::channel::<(Vec<u8>, i64)>(10);
    let (audio_sender, mut audio_receiver) = mpsc::channel::<Vec<f32>>(10);

    let video_encoder = Arc::new(Mutex::new(VideoEncoder::new(
        width,
        height,
        config.max_seconds,
        &config.encoder,
    )?));
    let audio_encoder = Arc::new(Mutex::new(AudioEncoder::new(config.max_seconds)?));

    std::thread::spawn(move || {
        debug!("Creating pipewire stream");
        let _capture =
            PipewireCapture::new(fd, stream_node, video_sender, audio_sender, config.use_mic)
                .unwrap();
    });

    // Main event loop
    loop {
        tokio::select! {
            _ = save_rx.recv() => {
                // Stop capturing video and audio while we save by taking out the locks
                let (mut video_lock, mut audio_lock) = tokio::join!(
                    video_encoder.lock(),
                    audio_encoder.lock()
                );

                // Drain both encoders of any remaining frames being processed
                video_lock.drain()?;
                audio_lock.drain()?;

                let filename = format!("clip_{}.mp4", chrono::Local::now().timestamp());
                let video_buffer = video_lock.get_buffer();
                let video_encoder = video_lock.get_encoder();

                let audio_buffer = audio_lock.get_buffer();
                let audio_encoder = audio_lock.get_encoder();

                save_buffer(&filename, video_buffer, video_encoder, audio_buffer, audio_encoder)?;

                // Reset video and audio buffers
                audio_lock.reset();
                video_lock.reset();

                debug!("Done saving!");
            },
            Some((frame, time)) = video_receiver.recv() => {
                video_encoder.lock().await.process(&frame, time)?;
            },
            Some(samples) = audio_receiver.recv() => {
                audio_encoder.lock().await.process(&samples)?;
            }
        }
    }
}

fn save_buffer(
    filename: &str,
    video_buffer: VideoBuffer,
    video_encoder: &ffmpeg::codec::encoder::Video,
    audio_buffer: AudioBuffer,
    audio_encoder: &ffmpeg::codec::encoder::Audio,
) -> Result<()> {
    let mut output = ffmpeg::format::output(&filename)?;

    let video_codec = video_encoder
        .codec()
        .context("Could not find expected video codec")?;

    let mut video_stream = output.add_stream(video_codec)?;
    video_stream.set_time_base(video_encoder.time_base());
    video_stream.set_parameters(&video_encoder);

    let audio_codec = audio_encoder
        .codec()
        .context("Could not find expected audio codec")?;

    let mut audio_stream = output.add_stream(audio_codec)?;
    audio_stream.set_time_base(audio_encoder.time_base());
    audio_stream.set_parameters(&audio_encoder);

    output.write_header()?;

    let video_buffer_gops = video_buffer.get_full_gops()?;
    let newest_video_pts = video_buffer_gops
        .values()
        .map(|frame| frame.get_pts())
        .max()
        .context("Count not get newest pts in full GOPs")?
        .clone();

    // Write video
    let first_pts_offset = video_buffer
        .oldest_pts()
        .context("Could not get oldest pts when muxing.")?;
    for (dts, frame_data) in video_buffer_gops {
        let pts_offset = frame_data.get_pts() - first_pts_offset;
        let mut dts_offset = dts - first_pts_offset;

        if dts_offset < 0 {
            dts_offset = 0;
        }

        let mut packet = ffmpeg::codec::packet::Packet::copy(&frame_data.get_raw_bytes());
        packet.set_pts(Some(pts_offset));
        packet.set_dts(Some(dts_offset));

        packet.set_stream(VIDEO_STREAM);

        packet
            .write_interleaved(&mut output)
            .expect("Could not write video interleaved");
    }
    // Write audio
    let oldest_frame_offset = audio_buffer
        .oldest_chunk()
        .context("Could not get oldest chunk")?;

    for (pts_in_micros, frame) in audio_buffer.get_frames() {
        // Don't write any more audio if we would exceed video (clip to max video)
        if pts_in_micros > newest_video_pts {
            break;
        }

        let offset = frame.get_pts() - oldest_frame_offset;

        let mut packet = ffmpeg::codec::packet::Packet::copy(&frame.get_data());
        packet.set_pts(Some(offset));
        packet.set_dts(Some(offset));

        packet.set_stream(AUDIO_STREAM);

        packet
            .write_interleaved(&mut output)
            .expect("Could not write audio interleaved");
    }

    output.write_trailer()?;

    Ok(())
}

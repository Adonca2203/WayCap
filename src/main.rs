mod dbus;
mod encoders;
mod pipewire_capture;

use std::{collections::VecDeque, sync::Arc};

use anyhow::{Error, Result};
use encoders::{
    audio_encoder::{AudioEncoder, AudioFrameData},
    video_encoder::{FrameBuffer, VideoEncoder},
};
use ffmpeg_next::{self as ffmpeg};
use log::{debug, LevelFilter};
use pipewire_capture::PipewireCapture;
use portal_screencast::{CursorMode, ScreenCast, SourceType};
use tokio::sync::{mpsc, Mutex};
use zbus::connection;

// Most if not all of these should be customizeable via a conf file
const NVENC: &str = "h264_nvenc";
const VIDEO_STREAM: usize = 0;
const AUDIO_STREAM: usize = 1;
const MAX_SECONDS: usize = 300;
const USE_MIC: bool = false;

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

    let (save_tx, mut save_rx) = mpsc::channel(1);
    let clip_service = dbus::ClipService::new(save_tx);

    debug!("Creating dbus connection");
    let _connection = connection::Builder::session()?
        .name("com.rust.GameClip")?
        .serve_at("/com/rust/GameClip", clip_service)?
        .build()
        .await?;

    let (video_sender, mut video_receiver) = mpsc::channel::<(Vec<u8>, i64)>(10);
    let (audio_sender, mut audio_receiver) = mpsc::channel::<(Vec<f32>, i64)>(10);

    let video_encoder = Arc::new(Mutex::new(VideoEncoder::new(
        width,
        height,
        MAX_SECONDS as u32,
        NVENC,
    )?));
    let audio_encoder = Arc::new(Mutex::new(AudioEncoder::new(MAX_SECONDS as u32)?));

    std::thread::spawn(move || {
        debug!("Creating pipewire stream");
        let _capture =
            PipewireCapture::new(fd, stream_node, video_sender, audio_sender, USE_MIC).unwrap();
    });

    // Main event loop
    loop {
        tokio::select! {
            _ = save_rx.recv() => {
                // Stop capturing video and audio by taking out the lock
                let (mut video_lock, audio_lock) = tokio::join!(
                    video_encoder.lock(),
                    audio_encoder.lock()
                );

                video_lock.drain()?;
                let filename = format!("clip_{}.mp4", chrono::Local::now().timestamp());
                let video_buffer = video_lock.get_buffer();
                let video_encoder = video_lock.get_encoder();

                let audio_buffer = audio_lock.get_buffer();
                let audio_encoder = audio_lock.get_encoder();

                save_buffer(&filename, video_buffer, video_encoder, audio_buffer, audio_encoder)?;

                debug!("Done saving!");
                drop(video_lock);
                drop(audio_lock);
            },
            Some((frame, time)) = video_receiver.recv() => {
                video_encoder.lock().await.process(&frame, time)?;
            },
            Some((samples, time)) = audio_receiver.recv() => {
                audio_encoder.lock().await.process(&samples, time)?;
            }
        }
    }
}

fn save_buffer(
    filename: &str,
    video_buffer: FrameBuffer,
    video_encoder: &ffmpeg::codec::encoder::Video,
    audio_buffer: VecDeque<AudioFrameData>,
    audio_encoder: &ffmpeg::codec::encoder::Audio,
) -> Result<(), ffmpeg::Error> {
    let mut output = ffmpeg::format::output(&filename)?;

    let video_codec = video_encoder.codec().unwrap();
    let mut video_stream = output.add_stream(video_codec)?;
    video_stream.set_time_base(video_encoder.time_base());
    video_stream.set_parameters(&video_encoder);

    let audio_codec = audio_encoder.codec().unwrap();
    let mut audio_stream = output.add_stream(audio_codec)?;
    audio_stream.set_time_base(audio_encoder.time_base());
    audio_stream.set_parameters(&audio_encoder);

    if let Err(err) = output.write_header() {
        debug!(
            "Ran into the following error while writing header: {:?}",
            err
        );
        return Err(err);
    }

    // Write video
    let first_pts_offset = video_buffer.oldest_pts().unwrap_or(0);
    let last_i_frame = video_buffer.get_last_gop_start();
    let mut dts_num = 0;
    for (_, frame_data) in video_buffer.frames.range(..last_i_frame) {
        let pts_offset = frame_data.pts - first_pts_offset;

        let mut packet = ffmpeg::codec::packet::Packet::copy(&frame_data.frame_bytes);
        packet.set_pts(Some(pts_offset));
        packet.set_dts(Some(dts_num));

        packet.set_stream(VIDEO_STREAM);

        packet
            .write_interleaved(&mut output)
            .expect("Could not write video interleaved");

        dts_num += 1;
    }

    // Write audio
    let first_frame_offset = audio_buffer.front().unwrap().chunk_time;
    for frame in audio_buffer {
        let offset = frame.chunk_time - first_frame_offset;

        let mut packet = ffmpeg::codec::packet::Packet::copy(&frame.frame_bytes);
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

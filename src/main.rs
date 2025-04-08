mod application_config;
mod dbus;
mod encoders;
mod pw_capture;

use std::{
    sync::{atomic::AtomicBool, Arc},
    time::SystemTime,
};

use anyhow::{Context, Error, Result};
use application_config::load_or_create_config;
use encoders::{
    audio_encoder::AudioEncoder,
    buffer::{AudioBuffer, VideoBuffer},
    video_encoder::VideoEncoder,
};
use ffmpeg_next::{self as ffmpeg};
use log::{debug, info, warn, LevelFilter};
use pipewire::{self as pw};
use portal_screencast::{CursorMode, ScreenCast, SourceType};
use pw_capture::{audio_stream::AudioCapture, video_stream::VideoCapture};
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

    // Need to adjust this buffer size so we don't block the capture threads too long but also keep
    // it within reason
    let (video_sender, mut video_receiver) = mpsc::channel::<(Vec<u8>, i64)>(1024);
    let (audio_sender, mut audio_receiver) = mpsc::channel::<(Vec<f32>, i64)>(1024);

    let video_encoder = Arc::new(Mutex::new(VideoEncoder::new(
        width,
        height,
        config.max_seconds,
        &config.encoder,
    )?));
    let audio_encoder = Arc::new(Mutex::new(AudioEncoder::new(config.max_seconds)?));

    let video_ready = Arc::new(AtomicBool::new(false));
    let audio_ready = Arc::new(AtomicBool::new(false));

    let vr_clone = Arc::clone(&video_ready);
    let ar_clone = Arc::clone(&audio_ready);
    pw::init();
    ffmpeg::log::set_level(ffmpeg_next::log::Level::Info);
    ffmpeg::init()?;

    let current_time = SystemTime::now();

    std::thread::spawn(move || {
        debug!("Starting video stream");
        let _video = VideoCapture::run(
            fd,
            stream_node,
            video_sender,
            video_ready,
            audio_ready,
            current_time,
        )
        .unwrap();
    });

    std::thread::spawn(move || {
        debug!("Starting audio stream");
        let _audio = AudioCapture::run(
            stream_node,
            audio_sender,
            vr_clone,
            ar_clone,
            config.use_mic,
            current_time,
        )
        .unwrap();
    });

    let saving = AtomicBool::new(false);
    // Main event loop
    loop {
        tokio::select! {
            _ = save_rx.recv() => {
                // Stop capturing video and audio while we save by taking out the locks
                saving.store(true, std::sync::atomic::Ordering::Release);
                let (mut video_lock, mut audio_lock) = tokio::join!(
                    video_encoder.lock(),
                    audio_encoder.lock()
                );

                // Drain both encoders of any remaining frames being processed
                video_lock.drain()?;
                audio_lock.drain()?;

                let filename = format!("clip_{}.mp4", chrono::Local::now().timestamp());
                let video_buffer = video_lock.get_buffer();
                let video_encoder = video_lock
                    .get_encoder()
                    .as_ref()
                    .context("Could not get video encoder")?;

                let audio_buffer = audio_lock.get_buffer();
                let audio_encoder = audio_lock
                    .get_encoder()
                    .as_ref()
                    .context("Could not get audio encoder")?;

                save_buffer(&filename, video_buffer, video_encoder, audio_buffer, audio_encoder)?;

                video_lock.reset_encoder()?;
                audio_lock.reset_encoder()?;
                saving.store(false, std::sync::atomic::Ordering::Release);

                debug!("Done saving!");
            },
            Some((frame, time)) = video_receiver.recv() => {
                let now = SystemTime::now();
                // If we are saving then just drop the frame we don't want it
                if saving.load(std::sync::atomic::Ordering::Acquire) {
                    continue;
                }

                if video_receiver.capacity() <= 10 {
                    warn!("Video receiver almost a full capacity. Increase the default buffer size");
                    warn!("Current max capacity: {:?}", video_receiver.max_capacity());
                    continue;
                }

                video_encoder.lock().await.process(&frame, time)?;

                // 1024 @ 48khz
                if now.elapsed().unwrap().as_micros() > 21330 {
                    warn!("We likely missed a frame in video. Check pipewire logs");
                    warn!("Took {:?} to execute process.", now.elapsed());
                }
            },
            Some((samples, time)) = audio_receiver.recv() => {
                // If we are saving then just drop the frame we don't want it
                if saving.load(std::sync::atomic::Ordering::Acquire) {
                    continue;
                }

                // TODO: Move the RMS logic into it's own thread as sometimes it takes too long and
                // blocks this thread for too long which causes us to reach capacity and block the
                // pipewire thread too long causing us to miss frames
                //
                // TODO: Figure out how to detect dropped frames and pad audio encoder as it is
                // causing audio desync
                if audio_receiver.capacity() <= 10 {
                    warn!("Audio receiver almost a full capacity. Increase the default buffer size");
                    warn!("Current max capacity: {:?}", audio_receiver.max_capacity());
                    continue;
                }

                audio_encoder.lock().await.process(&samples, time)?;
            },
            _ = tokio::signal::ctrl_c() => {
                info!("Shutting down");
                break;
            }
        }
    }
    Ok(())
}

fn save_buffer(
    filename: &str,
    video_buffer: &VideoBuffer,
    video_encoder: &ffmpeg::codec::encoder::Video,
    audio_buffer: &AudioBuffer,
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

    let last_keyframe = video_buffer
        .get_last_gop_start()
        .context("Could not get last keyframe dts")?;

    let newest_video_pts = video_buffer
        .get_frames()
        .get(last_keyframe)
        .context("Could not get last keyframe")?
        .get_pts();

    // Write video
    let first_pts_offset = video_buffer
        .oldest_pts()
        .context("Could not get oldest pts when muxing.")?;
    debug!("VIDEO SAVE START");
    for (dts, frame_data) in video_buffer.get_frames().range(..=last_keyframe) {
        let pts_offset = frame_data.get_pts() - first_pts_offset;
        let mut dts_offset = dts - first_pts_offset;

        debug!("PTS offset: {:?}", pts_offset);
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
    debug!("VIDEO SAVE END");

    // Write audio
    let oldest_frame_offset = audio_buffer
        .oldest_pts()
        .context("Could not get oldest chunk")?;

    let oldest_capture_time = audio_buffer.get_capture_times();

    debug!("AUDIO SAVE START");
    let mut iter = 0;
    for (pts, frame) in audio_buffer.get_frames() {
        // Don't write any more audio if we would exceed video (clip to max video)
        if &oldest_capture_time[iter] > newest_video_pts {
            break;
        }

        let offset = pts - oldest_frame_offset;

        debug!(
            "PTS IN MICROS: {:?}, PTS IN TIME SCALE: {:?}",
            oldest_capture_time[iter], offset
        );

        let mut packet = ffmpeg::codec::packet::Packet::copy(&frame);
        packet.set_pts(Some(offset));
        packet.set_dts(Some(offset));

        packet.set_stream(AUDIO_STREAM);

        packet
            .write_interleaved(&mut output)
            .expect("Could not write audio interleaved");

        iter += 1;
    }
    debug!("AUDIO SAVE END");

    output.write_trailer()?;

    Ok(())
}

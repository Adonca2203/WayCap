mod application_config;
mod dbus;
mod encoders;
mod pw_capture;

use std::{
    sync::{atomic::AtomicBool, Arc},
    time::{Duration, SystemTime},
};

use anyhow::{Context, Error, Result};
use application_config::load_or_create_config;
use encoders::{
    audio_encoder::AudioEncoder,
    buffer::{AudioBuffer, VideoBuffer},
    video_encoder::VideoEncoder,
};
use ffmpeg_next::{self as ffmpeg};
use log::{debug, error, info, trace, warn, LevelFilter};
use pipewire::{self as pw};
use portal_screencast::{CursorMode, ScreenCast, SourceType};
use pw_capture::{audio_stream::AudioCapture, video_stream::VideoCapture};
use ringbuf::{
    traits::{Consumer, Producer, Split},
    HeapRb,
};
use tokio::sync::{mpsc, Mutex};
use zbus::connection;

const VIDEO_STREAM: usize = 0;
const AUDIO_STREAM: usize = 1;

pub struct RawAudioFrame {
    samples: Vec<f32>,
    timestamp: i64,
}

impl RawAudioFrame {
    pub fn get_samples_mut(&mut self) -> &mut Vec<f32> {
        &mut self.samples
    }

    pub fn get_samples(&mut self) -> &Vec<f32> {
        &self.samples
    }
}

pub struct RawVideoFrame {
    bytes: Vec<u8>,
    timestamp: i64,
}

impl RawVideoFrame {
    pub fn get_bytes(&self) -> &Vec<u8> {
        &self.bytes
    }
}

pub struct Terminate;

#[tokio::main]
async fn main() -> Result<(), Error> {
    let _ = simple_logging::log_to_file("logs.txt", LevelFilter::Debug);

    let config = load_or_create_config();
    debug!("CONFIG: {:?}", config);

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

    // Video
    let video_encoder = Arc::new(Mutex::new(VideoEncoder::new(
        width,
        height,
        config.max_seconds,
        &config.encoder,
    )?));
    let video_encoder_clone = Arc::clone(&video_encoder);
    let video_ready = Arc::new(AtomicBool::new(false));
    let vr_clone = Arc::clone(&video_ready);
    let (video_sender, mut video_receiver) = mpsc::channel::<RawVideoFrame>(500);
    let video_ring_buffer = HeapRb::<RawVideoFrame>::new(500);
    let (mut video_ring_sender, mut video_ring_receiver) = video_ring_buffer.split();

    // Audio
    let audio_encoder = Arc::new(Mutex::new(AudioEncoder::new(config.max_seconds)?));
    let audio_encoder_clone = Arc::clone(&audio_encoder);
    let audio_ready = Arc::new(AtomicBool::new(false));
    let (audio_sender, mut audio_receiver) = mpsc::channel::<RawAudioFrame>(10);
    let audio_ring_buffer = HeapRb::<RawAudioFrame>::new(10);
    let (mut audio_ring_sender, mut audio_ring_receiver) = audio_ring_buffer.split();
    let ar_clone = Arc::clone(&audio_ready);

    pw::init();
    ffmpeg::log::set_level(ffmpeg_next::log::Level::Info);
    ffmpeg::init()?;

    let current_time = SystemTime::now();

    // Create audio worker thread
    let stop = Arc::new(AtomicBool::new(false));
    let stop_audio_clone = Arc::clone(&stop);
    let audio_worker = std::thread::spawn(move || loop {
        if stop_audio_clone.load(std::sync::atomic::Ordering::Acquire) {
            break;
        }

        while let Some(mut raw_frame) = audio_ring_receiver.try_pop() {
            let now = SystemTime::now();
            if let Err(e) = audio_encoder_clone.blocking_lock().process(&mut raw_frame) {
                error!(
                    "Error processing audio frame at {:?}: {:?}",
                    raw_frame.timestamp, e
                );
            }
            trace!(
                "Took {:?} to process this audio frame at {:?}",
                now.elapsed(),
                raw_frame.timestamp
            );
        }
        std::thread::sleep(Duration::from_nanos(100));
    });

    // Create video worker
    let stop_video_clone = Arc::clone(&stop);
    let video_worder = std::thread::spawn(move || loop {
        if stop_video_clone.load(std::sync::atomic::Ordering::Acquire) {
            break;
        }

        while let Some(raw_frame) = video_ring_receiver.try_pop() {
            let now = SystemTime::now();
            if let Err(e) = video_encoder_clone.blocking_lock().process(&raw_frame) {
                error!(
                    "Error processing video frame at {:?}: {:?}",
                    raw_frame.timestamp, e
                );
            }

            trace!(
                "Took {:?} to process this video frame at {:?}",
                now.elapsed(),
                raw_frame.timestamp
            );
        }
        std::thread::sleep(Duration::from_nanos(100));
    });

    let saving = Arc::new(AtomicBool::new(false));

    let (pw_video_sender, pw_video_recv) = pw::channel::channel::<Terminate>();
    let saving_video_clone = Arc::clone(&saving);
    let pw_video_worker = std::thread::spawn(move || {
        debug!("Starting video stream");
        let _video = VideoCapture::run(
            fd,
            stream_node,
            video_sender,
            video_ready,
            audio_ready,
            current_time,
            pw_video_recv,
            saving_video_clone,
        )
        .unwrap();
    });

    let (pw_audio_sender, pw_audio_recv) = pw::channel::channel::<Terminate>();
    let saving_audio_clone = Arc::clone(&saving);
    let pw_audio_worker = std::thread::spawn(move || {
        debug!("Starting audio stream");
        let _audio = AudioCapture::run(
            stream_node,
            audio_sender,
            vr_clone,
            ar_clone,
            config.use_mic,
            current_time,
            pw_audio_recv,
            saving_audio_clone,
        )
        .unwrap();
    });

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

                drop(video_lock);
                drop(audio_lock);
                saving.store(false, std::sync::atomic::Ordering::Release);

                debug!("Done saving!");
            },
            Some(raw_frame) = video_receiver.recv() => {
                // Send the data to the worker thread and exit as to not block this one
                if let Err(_) = video_ring_sender.try_push(raw_frame) {
                    warn!("Trying to push but the video ring buff is full. Consider increasing the max");
                }
            },
            Some(raw_frame) = audio_receiver.recv() => {
                // Send the data to the worker thread and exit as to not block this one
                if let Err(_) = audio_ring_sender.try_push(raw_frame) {
                    warn!("Trying to push but the audio ring buff is full. Consider increasing the max");
                }
            },
            _ = tokio::signal::ctrl_c() => {
                info!("Shutting down");
                stop.store(true, std::sync::atomic::Ordering::Release);
                let _ = pw_video_sender.send(Terminate);
                let _ = pw_audio_sender.send(Terminate);
                let (mut video_lock, mut audio_lock) = tokio::join!(
                    video_encoder.lock(),
                    audio_encoder.lock()
                );
                video_lock.reset_encoder()?;
                audio_lock.reset_encoder()?;

                break;
            }
        }
    }

    let _ = audio_worker.join();
    let _ = video_worder.join();
    let _ = pw_video_worker.join();
    let _ = pw_audio_worker.join();
    debug!("Done shutting down!");
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

    let oldest_capture_time = audio_buffer.get_capture_times();

    // Write video
    let mut first_pts_offset = video_buffer
        .oldest_pts()
        .context("Could not get oldest pts when muxing.")?;
    let mut first_offset = false;
    debug!("VIDEO SAVE START");
    for (dts, frame_data) in video_buffer.get_frames().range(..=last_keyframe) {
        // If video starts before audio try and catch up as much as possible
        // (At worst a 20ms gap)
        if &oldest_capture_time[0] > frame_data.get_pts() {
            continue;
        }
        if first_offset == false {
            first_pts_offset = *frame_data.get_pts();
            first_offset = true;
        }
        let pts_offset = frame_data.get_pts() - first_pts_offset;
        let mut dts_offset = dts - first_pts_offset;

        debug!(
            "Capture Timestamp: {:?}, PTS offset: {:?}",
            frame_data.get_pts(),
            pts_offset
        );
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

    debug!("AUDIO SAVE START");
    let mut iter = 0;
    for (pts, frame) in audio_buffer.get_frames() {
        // Don't write any more audio if we would exceed video (clip to max video)
        if &oldest_capture_time[iter] > newest_video_pts {
            debug!(
                "Oldest capture time {:?}, in time scale: {:?}",
                oldest_capture_time[iter], pts
            );
            break;
        }
        // If audio starts before video try and catch up as much as possible
        // (At worst a 20ms gap)
        if &oldest_capture_time[iter] < &first_pts_offset {
            debug!("Audio timestamp is before video, skipping writing this one");
            debug!(
                "Capture time: {:?}, Video time: {:?}",
                oldest_capture_time[iter], first_pts_offset
            );
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

use std::{
    sync::{atomic::AtomicBool, Arc},
    time::{Duration, SystemTime},
};

use anyhow::Context;
use ringbuf::{consumer::Consumer, HeapCons};
use tokio::sync::Mutex;

use crate::{
    app_context::AppContext,
    application_config,
    encoders::{
        audio_encoder::{AudioEncoder, FfmpegAudioEncoder},
        nvenc_encoder::NvencEncoder,
        vaapi_encoder::VaapiEncoder,
        video_encoder::{VideoEncoder, ONE_MICROS},
    },
    save_buffer, RawAudioFrame, RawVideoFrame, FRAME_INTERVAL,
};

use super::AppMode;

pub struct ShadowCapMode {
    max_seconds: u32,
    audio_encoder: Option<Arc<Mutex<AudioEncoder<FfmpegAudioEncoder>>>>,
    video_encoder: Option<Arc<Mutex<dyn VideoEncoder + Send>>>,
}

impl AppMode for ShadowCapMode {
    async fn init(&mut self, ctx: &mut AppContext) -> anyhow::Result<()> {
        log::debug!("Initializing context for Shadow Capture Mode");
        // Video
        let video_encoder: Arc<Mutex<dyn VideoEncoder + Send>> =
            match ctx.config.encoder {
                application_config::EncoderToUse::H264Nvenc => Arc::new(Mutex::new(
                    NvencEncoder::new(ctx.width, ctx.height, self.max_seconds, ctx.config.quality)?,
                )),
                application_config::EncoderToUse::H264Vaapi => Arc::new(Mutex::new(
                    VaapiEncoder::new(ctx.width, ctx.height, self.max_seconds, ctx.config.quality)?,
                )),
            };

        let video_owned_recv = ctx
            .video_ring_receiver
            .take()
            .context("Could not take ownership of the video ring buffer")?;

        let video_worker = Self::create_video_worker(
            Arc::clone(&ctx.stop),
            video_owned_recv,
            Arc::clone(&video_encoder),
        );
        ctx.join_handles.push(video_worker);
        self.video_encoder = Some(video_encoder);

        // Audio
        let audio_encoder = Arc::new(Mutex::new(AudioEncoder::new_with_encoder(
            FfmpegAudioEncoder::new_opus,
            ctx.config.max_seconds,
        )?));

        let audio_owned_recv = ctx
            .audio_ring_receiver
            .take()
            .context("Could not take ownership of the audio ring buffer")?;

        let audio_worker = Self::create_audio_worker(
            Arc::clone(&ctx.stop),
            audio_owned_recv,
            Arc::clone(&audio_encoder),
        );
        ctx.join_handles.push(audio_worker);
        self.audio_encoder = Some(audio_encoder);

        log::debug!("Successfully initialized Shadow Capture Mode");
        Ok(())
    }

    async fn on_save(&mut self, ctx: &mut AppContext) -> anyhow::Result<()> {
        ctx.saving.store(true, std::sync::atomic::Ordering::Release);
        if let Some(video_encoder) = &self.video_encoder {
            if let Some(audio_encoder) = &self.audio_encoder {
                let (mut video_lock, mut audio_lock) =
                    tokio::join!(video_encoder.lock(), audio_encoder.lock());

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

                save_buffer(
                    &filename,
                    video_buffer,
                    video_encoder,
                    audio_buffer,
                    audio_encoder,
                )?;

                video_lock.reset()?;
                audio_lock.reset_encoder(FfmpegAudioEncoder::new_opus)?;
            }
        }

        ctx.saving
            .store(false, std::sync::atomic::Ordering::Release);
        log::debug!("Done saving!");
        Ok(())
    }

    async fn on_shutdown(&mut self, ctx: &mut AppContext) -> anyhow::Result<()> {
        log::info!("Shutting down");
        // Stop processing new frames and exit worker threads
        ctx.saving.store(true, std::sync::atomic::Ordering::Release);
        ctx.stop.store(true, std::sync::atomic::Ordering::Release);

        // Drop encoders -- drop impl should clean up any remaining frames
        self.audio_encoder.take();
        self.video_encoder.take();
        Ok(())
    }
}

impl ShadowCapMode {
    pub async fn new(max_seconds: u32) -> anyhow::Result<Self> {
        anyhow::ensure!(
            max_seconds <= 86400,
            "Max seconds is above 24 hours. This is too much time for shadow capture"
        );
        Ok(Self {
            max_seconds: max_seconds * ONE_MICROS as u32,
            audio_encoder: None,
            video_encoder: None,
        })
    }

    // These look to be generic between modes and the only real thing that changes is what we do
    // once we get an encoded frame back for modes like recording or screen sharing over network so
    // these can probably go in a more common place one i start implementing those

    /// Creates a worker thread that polls a `HeapCons<RawAudioFrame>>` and sends anything on it to
    /// the its encoder for processing
    /// # Arguments
    /// * `stop_audio` - Atomic bool for telling the thread to exit
    /// * `mut audio_receiver` - The ring buf to poll
    /// * `audio_encoder` - The audio encoder which will process the frames
    fn create_audio_worker(
        stop_audio: Arc<AtomicBool>,
        mut audio_receiver: HeapCons<RawAudioFrame>,
        audio_encoder: Arc<Mutex<AudioEncoder<FfmpegAudioEncoder>>>,
    ) -> std::thread::JoinHandle<()> {
        std::thread::spawn(move || loop {
            if stop_audio.load(std::sync::atomic::Ordering::Acquire) {
                break;
            }

            while let Some(mut raw_frame) = audio_receiver.try_pop() {
                let now = SystemTime::now();
                if let Err(e) = audio_encoder.blocking_lock().process(&mut raw_frame) {
                    log::error!(
                        "Error processing audio frame at {:?}: {:?}",
                        raw_frame.timestamp,
                        e
                    );
                }
                log::trace!(
                    "Took {:?} to process this audio frame at {:?}",
                    now.elapsed(),
                    raw_frame.timestamp
                );
            }
            std::thread::sleep(Duration::from_nanos(100));
        })
    }

    /// Creates a worker thread that polls a `HeapCons<RawVideoFrame>>` and sends anything on it to
    /// the its encoder for processing
    /// # Arguments
    /// * `stop_video` - Atomic bool for telling the thread to exit
    /// * `mut video_receiver` - The ring buf to poll
    /// * `video_encoder` - The video encoder which will process the frames
    fn create_video_worker(
        stop_video: Arc<AtomicBool>,
        mut video_receiver: HeapCons<RawVideoFrame>,
        video_encoder: Arc<Mutex<dyn VideoEncoder + Send>>,
    ) -> std::thread::JoinHandle<()> {
        std::thread::spawn(move || {
            let mut last_timestamp: u64 = 0;
            loop {
                if stop_video.load(std::sync::atomic::Ordering::Acquire) {
                    break;
                }

                while let Some(raw_frame) = video_receiver.try_pop() {
                    let now = SystemTime::now();
                    let current_time = *raw_frame.get_timestamp() as u64;

                    // Throttle FPS
                    if current_time < last_timestamp + FRAME_INTERVAL {
                        continue;
                    }

                    last_timestamp = current_time;
                    if let Err(e) = video_encoder.blocking_lock().process(&raw_frame) {
                        log::error!(
                            "Error processing video frame at {:?}: {:?}",
                            raw_frame.timestamp,
                            e
                        );
                    }

                    log::trace!(
                        "Took {:?} to process this video frame at {:?}",
                        now.elapsed(),
                        raw_frame.timestamp
                    );
                }
                std::thread::sleep(Duration::from_nanos(100));
            }
        })
    }
}

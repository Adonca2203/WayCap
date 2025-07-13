use std::{
    sync::{atomic::AtomicBool, Arc},
    time::Duration,
};

use crossbeam::channel::Receiver;
use tokio::sync::Mutex;
use waycap_rs::types::{audio_frame::EncodedAudioFrame, video_frame::EncodedVideoFrame};

use crate::{
    app_context::AppContext,
    encoders::buffer::{ShadowCaptureAudioBuffer, ShadowCaptureVideoBuffer},
    save_buffer,
};

use super::AppMode;

pub struct ShadowCapMode {
    video_buffer: Arc<Mutex<ShadowCaptureVideoBuffer>>,
    audio_buffer: Arc<Mutex<ShadowCaptureAudioBuffer>>,
}

impl AppMode for ShadowCapMode {
    async fn init(&mut self, ctx: &mut AppContext) -> anyhow::Result<()> {
        log::debug!("Initializing context for Shadow Capture Mode");

        let video_owned_recv = ctx.capture.take_video_receiver();

        let shadow_worker = Self::create_shadow_video_worker(
            video_owned_recv,
            Arc::clone(&self.video_buffer),
            Arc::clone(&ctx.stop),
        );
        ctx.join_handles.push(shadow_worker);

        let audio_owned_recv = ctx.capture.take_audio_receiver()?;

        let audio_shadow_worker = Self::create_shadow_audio_worker(
            audio_owned_recv,
            Arc::clone(&self.audio_buffer),
            Arc::clone(&ctx.stop),
        );
        ctx.join_handles.push(audio_shadow_worker);

        log::debug!("Successfully initialized Shadow Capture Mode");
        Ok(())
    }

    async fn on_save(&mut self, ctx: &mut AppContext) -> anyhow::Result<()> {
        ctx.saving.store(true, std::sync::atomic::Ordering::Release);
        ctx.capture.finish()?;
        log::info!("Saving clip...");

        let (mut video_buffer, mut audio_buffer) =
            tokio::join!(self.video_buffer.lock(), self.audio_buffer.lock());
        let filename = format!("clip_{}.mp4", chrono::Local::now().timestamp());

        save_buffer(&filename, &video_buffer, &audio_buffer, &ctx.capture)?;

        video_buffer.reset();
        audio_buffer.reset();
        ctx.capture.reset()?;
        ctx.saving
            .store(false, std::sync::atomic::Ordering::Release);
        ctx.capture.start()?;

        log::info!("Done saving!");
        Ok(())
    }

    async fn on_shutdown(&mut self, ctx: &mut AppContext) -> anyhow::Result<()> {
        log::info!("Shutting down");
        // Stop processing new frames and exit worker threads
        ctx.saving.store(true, std::sync::atomic::Ordering::Release);
        ctx.stop.store(true, std::sync::atomic::Ordering::Release);
        Ok(())
    }
}

impl ShadowCapMode {
    pub async fn new(max_seconds: u32) -> anyhow::Result<Self> {
        anyhow::ensure!(
            max_seconds <= 86400,
            "Max seconds is above 24 hours. This is too much time for shadow capture"
        );

        let actual_max = max_seconds * 1_000_000_u32;
        Ok(Self {
            video_buffer: Arc::new(Mutex::new(ShadowCaptureVideoBuffer::new(
                actual_max as usize,
            ))),
            audio_buffer: Arc::new(Mutex::new(ShadowCaptureAudioBuffer::new(
                actual_max as usize,
            ))),
        })
    }

    fn create_shadow_video_worker(
        recv: Receiver<EncodedVideoFrame>,
        buffer: Arc<Mutex<ShadowCaptureVideoBuffer>>,
        stop: Arc<AtomicBool>,
    ) -> std::thread::JoinHandle<()> {
        std::thread::spawn(move || loop {
            if stop.load(std::sync::atomic::Ordering::Acquire) {
                while recv.try_recv().is_ok() {} // Drain any remaining frames to avoid error
                                                 // logging
                break;
            }

            while let Ok(encoded_frame) = recv.try_recv() {
                // Still receive but discard any frames received if we cannot acquire the lock
                if let Ok(mut buf) = buffer.try_lock() {
                    buf.insert(encoded_frame.dts, encoded_frame);
                }
            }

            std::thread::sleep(Duration::from_millis(100));
        })
    }

    fn create_shadow_audio_worker(
        recv: Receiver<EncodedAudioFrame>,
        audio_buffer: Arc<Mutex<ShadowCaptureAudioBuffer>>,
        stop: Arc<AtomicBool>,
    ) -> std::thread::JoinHandle<()> {
        std::thread::spawn(move || loop {
            if stop.load(std::sync::atomic::Ordering::Acquire) {
                while recv.try_recv().is_ok() {} // Drain any remaining frames to avoid error
                                                 // logging
                break;
            }

            while let Ok(encoded_frame) = recv.try_recv() {
                // Still receive but discard any frames received if we cannot acquire the lock
                if let Ok(mut buf) = audio_buffer.try_lock() {
                    buf.insert_capture_time(encoded_frame.timestamp);
                    buf.insert(encoded_frame.pts, encoded_frame.data);
                }
            }

            std::thread::sleep(Duration::from_millis(100));
        })
    }
}

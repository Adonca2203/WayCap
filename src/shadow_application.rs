#![allow(unused)]
use pipewire::{self as pw};
use std::{
    sync::{
        atomic::{AtomicBool, AtomicU8},
        Arc,
    },
    time::{Duration, Instant, SystemTime},
};

use anyhow::{anyhow, Context, Result};
use log::error;
use portal_screencast::{CursorMode, ScreenCast, SourceType};
use ringbuf::{traits::Split, HeapRb};
use tokio::sync::mpsc;

use crate::{pw_capture::video_stream::VideoCapture, RawVideoFrame, Terminate};

trait Application {
    fn new() -> Result<Self>
    where
        Self: Sized;
    fn init(&mut self);
    fn run(&mut self);
}

enum AppState {
    Initializing = 0,
    Recording = 1,
    Saving = 2,
}

struct AtomicAppState {
    inner: AtomicU8,
}

impl AtomicAppState {
    fn new(state: AppState) -> Self {
        Self {
            inner: AtomicU8::new(state as u8),
        }
    }

    fn load(&self) -> Result<AppState> {
        match self.inner.load(std::sync::atomic::Ordering::SeqCst) {
            0 => Ok(AppState::Initializing),
            1 => Ok(AppState::Recording),
            2 => Ok(AppState::Saving),
            _ => Err(anyhow!("Unknown Application State")),
        }
    }

    fn store(&self, state: AppState) {
        self.inner
            .store(state as u8, std::sync::atomic::Ordering::SeqCst);
    }
}

pub struct ShadowApp {
    current_state: AtomicAppState,
    width: u32,
    height: u32,
    start_time: SystemTime,
}

impl Application for ShadowApp {
    fn new() -> Result<Self> {
        let current_time = SystemTime::now();
        let saving = Arc::new(AtomicBool::new(false));
        // XDG Portal get a screen or window
        let mut screen_cast = ScreenCast::new()?;
        screen_cast.set_source_types(SourceType::all());
        screen_cast.set_cursor_mode(CursorMode::EMBEDDED);
        let screen_cast = screen_cast.start(None)?;

        let fd = screen_cast.pipewire_fd();
        let stream = screen_cast
            .streams()
            .next()
            .context("Could not unwrap stream")?;
        let stream_node = stream.pipewire_node();
        let (mut width, mut height) = stream.size();

        let video_ready = Arc::new(AtomicBool::new(false));
        let vr_clone = Arc::clone(&video_ready);
        let audio_ready = Arc::new(AtomicBool::new(false));
        let ar_clone = Arc::clone(&audio_ready);
        let video_ring_buffer = HeapRb::<RawVideoFrame>::new(250);
        let (video_ring_sender, video_ring_receiver) = video_ring_buffer.split();

        let (pw_video_sender, pw_video_recv) = pw::channel::channel::<Terminate>();
        let saving_video_clone = Arc::clone(&saving);
        let (resolution_sender, mut resolution_receiver) = mpsc::channel::<(u32, u32)>(2);
        let pw_video_capture_worker = std::thread::spawn(move || {
            let video_cap = VideoCapture::new(video_ready, audio_ready);
            video_cap
                .run(
                    fd,
                    stream_node,
                    video_ring_sender,
                    pw_video_recv,
                    saving,
                    current_time,
                    resolution_sender,
                )
                .unwrap();
        });

        // Window mode return (0, 0) for dimensions to we have to get it from pipewire
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
                    error!("Timeout waiting for PipeWire negotiated resolution.");
                    std::process::exit(1);
                }

                std::thread::sleep(Duration::from_millis(10));
            }
        }

        Ok(Self {
            current_state: AtomicAppState::new(AppState::Initializing),
            width,
            height,
            start_time: current_time,
        })
    }

    fn init(&mut self) {
        todo!()
    }

    fn run(&mut self) {
        todo!()
    }
}

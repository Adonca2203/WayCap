use crate::{application_config::AppConfig, RawAudioFrame, RawVideoFrame};
use ringbuf::HeapCons;
use std::sync::{atomic::AtomicBool, Arc};

pub struct AppContext {
    pub saving: Arc<AtomicBool>,
    pub stop: Arc<AtomicBool>,
    pub join_handles: Vec<tokio::task::JoinHandle<()>>,
    pub config: AppConfig,
    pub width: u32,
    pub height: u32,
    pub video_ring_receiver: Option<HeapCons<RawVideoFrame>>,
    pub audio_ring_receiver: Option<HeapCons<RawAudioFrame>>,
}

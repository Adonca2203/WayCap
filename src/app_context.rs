use std::sync::{atomic::AtomicBool, Arc};
use waycap_rs::Capture;

pub struct AppContext {
    pub saving: Arc<AtomicBool>,
    pub stop: Arc<AtomicBool>,
    pub join_handles: Vec<std::thread::JoinHandle<()>>,
    pub capture: Capture,
}

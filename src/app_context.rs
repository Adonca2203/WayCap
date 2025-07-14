use std::sync::{atomic::AtomicBool, Arc};
use waycap_rs::Capture;

use crate::application_config::AppConfig;

pub struct AppContext {
    pub saving: Arc<AtomicBool>,
    pub stop: Arc<AtomicBool>,
    pub join_handles: Vec<std::thread::JoinHandle<()>>,
    pub capture: Capture,
    pub config: AppConfig,
}

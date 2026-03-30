use std::sync::{atomic::AtomicBool, Arc};
use waycap_rs::{Capture, DynamicEncoder};

use crate::application_config::AppConfig;

pub struct AppContext {
    pub saving: Arc<AtomicBool>,
    pub stop: Arc<AtomicBool>,
    pub join_handles: Vec<std::thread::JoinHandle<()>>,
    pub capture: Capture<DynamicEncoder>,
    pub config: AppConfig,
    pub hint: String,
}

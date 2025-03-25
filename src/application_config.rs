use std::{fs, path::Path};

use anyhow::Result;
use config::{Config, File};
use directories::ProjectDirs;
use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all="UPPERCASE")]
pub enum QualityPreset {
    LOW,
    MEDIUM,
    HIGH
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(default)]
pub struct AppConfig {
    pub encoder: String,
    pub max_seconds: u32,
    pub use_mic: bool,
    pub quality: QualityPreset,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            encoder: "h264_nvenc".to_string(),
            max_seconds: 300,
            use_mic: false,
            quality: QualityPreset::MEDIUM
        }
    }
}

pub fn load_or_create_config() -> AppConfig {
    let mut settings = Config::builder();

    // Check for an user level config
    if let Some(proj_dirs) = ProjectDirs::from("com", "rust", "auto-screen-recorder") {
        let config_path = proj_dirs.config_dir().join("config.toml");

        if !config_path.exists() {
            let default_config = AppConfig::default();
            write_default_config(&config_path, &default_config)
                .expect("Failed to write default config file");
        }

        settings = settings.add_source(File::from(config_path).required(false));
    }

    let config = settings.build();

    match config {
        Ok(c) => c.try_deserialize().unwrap_or_default(),
        Err(_) => AppConfig::default(),
    }
}

fn write_default_config(path: &Path, config: &AppConfig) -> Result<()> {
    let toml_str = toml::to_string_pretty(config)?;

    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }

    fs::write(path, toml_str)?;

    Ok(())
}

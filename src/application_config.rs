use std::{fs, path::Path};

use anyhow::Result;
use config::{Config, File};
use directories::ProjectDirs;
use serde::{Deserialize, Serialize};
use zbus::zvariant::Type;

#[derive(Debug, Deserialize, Serialize, Clone, Copy)]
#[serde(rename_all = "UPPERCASE")]
pub enum QualityPreset {
    Low,
    Medium,
    High,
    Ultra,
}

#[derive(Debug, Deserialize, Serialize, Clone, Copy)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum EncoderToUse {
    H264Nvenc,
    H264Vaapi,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(default)]
pub struct AppConfig {
    pub encoder: EncoderToUse,
    pub max_seconds: u32,
    pub use_mic: bool,
    pub quality: QualityPreset,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            encoder: EncoderToUse::H264Vaapi,
            max_seconds: 300,
            use_mic: false,
            quality: QualityPreset::Medium,
        }
    }
}

#[derive(Type, Serialize, Deserialize)]
pub struct AppConfigDbus {
    pub encoder: String,
    pub max_seconds: u32,
    pub use_mic: bool,
    pub quality: String,
}

impl TryFrom<AppConfigDbus> for AppConfig {
    type Error = String;

    fn try_from(value: AppConfigDbus) -> Result<Self, Self::Error> {
        let encoder = match value.encoder.to_lowercase().as_str() {
            "h264_nvenc" => Ok(EncoderToUse::H264Nvenc),
            "h264_vaapi" => Ok(EncoderToUse::H264Vaapi),
            other => Err(format!(
                "Unknown encoder: {:?}, Valid values: {:?}",
                other,
                vec!(EncoderToUse::H264Nvenc, EncoderToUse::H264Vaapi)
            )),
        }?;

        let quality = match value.quality.to_lowercase().as_str() {
            "low" => Ok(QualityPreset::Low),
            "medium" => Ok(QualityPreset::Medium),
            "high" => Ok(QualityPreset::High),
            "ultra" => Ok(QualityPreset::Ultra),
            other => Err(format!(
                "Unknown quality value: {:?}, Valid values: {:?}",
                other,
                vec!(
                    QualityPreset::Low,
                    QualityPreset::Medium,
                    QualityPreset::High,
                    QualityPreset::Ultra
                )
            )),
        }?;

        Ok(AppConfig {
            encoder,
            max_seconds: value.max_seconds,
            use_mic: value.use_mic,
            quality,
        })
    }
}

pub fn load_or_create_config() -> AppConfig {
    let mut settings = Config::builder();

    // Check for an user level config
    if let Some(proj_dirs) = ProjectDirs::from("com", "rust", "waycap") {
        let config_path = proj_dirs.config_dir().join("config.toml");

        if !config_path.exists() {
            let default_config = AppConfig::default();
            write_config(&config_path, &default_config)
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

pub fn update_config(config: AppConfig) -> AppConfig {
    let mut settings = Config::builder();

    if let Some(proj_dirs) = ProjectDirs::from("com", "rust", "waycap") {
        let config_path = proj_dirs.config_dir().join("config.toml");

        write_config(&config_path, &config).expect("Failed to update config file");

        settings = settings.add_source(File::from(config_path).required(false));
    }

    let config = settings.build();

    match config {
        Ok(c) => c.try_deserialize().unwrap_or_default(),
        Err(_) => AppConfig::default(),
    }
}

fn write_config(path: &Path, config: &AppConfig) -> Result<()> {
    let toml_str = toml::to_string_pretty(config)?;

    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }

    fs::write(path, toml_str)?;

    Ok(())
}

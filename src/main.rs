#![deny(
    clippy::all,
    clippy::correctness,
    clippy::style,
    clippy::complexity,
    clippy::perf
)]

mod app_context;
mod application_config;
mod dbus;
mod encoders;
mod modes;
mod pw_capture;
mod waycap;

use anyhow::{Context, Error, Result};
use application_config::load_or_create_config;
use encoders::{
    audio_encoder::{AudioEncoderImpl, FfmpegAudioEncoder},
    buffer::{AudioBuffer, VideoBuffer},
    video_encoder::ONE_MICROS,
};
use ffmpeg_next::{self as ffmpeg};
use modes::shadow_cap::ShadowCapMode;
use pipewire::{self as pw};
use waycap::WayCap;

const VIDEO_STREAM: usize = 0;
const AUDIO_STREAM: usize = 1;
const TARGET_FPS: usize = 60;
const FRAME_INTERVAL: u64 = (ONE_MICROS / TARGET_FPS) as u64;

#[derive(Debug)]
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

#[derive(Debug)]
pub struct RawVideoFrame {
    bytes: Vec<u8>,
    timestamp: i64,
}

impl RawVideoFrame {
    pub fn get_bytes(&self) -> &Vec<u8> {
        &self.bytes
    }

    pub fn get_timestamp(&self) -> &i64 {
        &self.timestamp
    }
}

pub struct Terminate;

#[tokio::main(flavor = "multi_thread", worker_threads = 4)]
async fn main() -> Result<(), Error> {
    pw::init();
    ffmpeg::init()?;
    let config = load_or_create_config();
    log::debug!("Config: {:?}", config);
    let mode = ShadowCapMode::new(config.max_seconds).await?;

    let mut app = WayCap::new(mode, config).await?;

    app.run().await?;
    log::debug!("Shutdown successfully");
    Ok(())
}

fn save_buffer(
    filename: &str,
    video_buffer: &VideoBuffer,
    video_encoder: &ffmpeg::codec::encoder::Video,
    audio_buffer: &AudioBuffer,
    audio_encoder: &FfmpegAudioEncoder,
) -> Result<()> {
    let mut output = ffmpeg::format::output(&filename)?;

    let video_codec = video_encoder
        .codec()
        .context("Could not find expected video codec")?;

    let mut video_stream = output.add_stream(video_codec)?;
    video_stream.set_time_base(video_encoder.time_base());
    video_stream.set_parameters(video_encoder);

    let audio_codec = audio_encoder
        .codec()
        .context("Could not find expected audio codec")?;

    let mut audio_stream = output.add_stream(audio_codec)?;
    audio_stream.set_time_base(audio_encoder.time_base());
    audio_stream.set_parameters(audio_encoder.as_ref());

    output.write_header()?;

    let last_keyframe = video_buffer
        .get_last_gop_start()
        .context("Could not get last keyframe dts")?;

    let mut newest_video_pts = 0;
    let audio_capture_timestamps = audio_buffer.get_capture_times();

    // Write video
    let mut first_pts_offset: i64 = 0;
    let mut first_offset = false;
    log::debug!("VIDEO SAVE START");
    for (dts, frame_data) in video_buffer.get_frames().range(..=last_keyframe) {
        // If video starts before audio try and catch up as much as possible
        // (At worst a 20ms gap)
        if &audio_capture_timestamps[0] > frame_data.get_pts() && !*frame_data.is_key() {
            log::debug!(
                "Skipping Video Frame Captured at: {:?}, DTS: {:?}",
                frame_data.get_pts(),
                dts,
            );
            continue;
        }

        if !first_offset {
            first_pts_offset = *frame_data.get_pts();
            first_offset = true;
        }

        let pts_offset = frame_data.get_pts() - first_pts_offset;
        let dts_offset = dts - first_pts_offset;

        let mut packet = ffmpeg::codec::packet::Packet::copy(frame_data.get_raw_bytes());
        packet.set_pts(Some(pts_offset));
        packet.set_dts(Some(dts_offset));

        packet.set_stream(VIDEO_STREAM);

        packet
            .write_interleaved(&mut output)
            .expect("Could not write video interleaved");
        newest_video_pts = *frame_data.get_pts();
    }
    log::debug!("VIDEO SAVE END");

    // Write audio
    let mut oldest_frame_offset = 0;
    let mut first_offset = false;
    log::debug!("AUDIO SAVE START");
    let mut iter = 0;
    for (pts, frame) in audio_buffer.get_frames() {
        // Don't write any more audio if we would exceed video (clip to max video)
        if audio_capture_timestamps[iter] > newest_video_pts {
            log::debug!(
                "Oldest capture time {:?}, in time scale: {:?}",
                audio_capture_timestamps[iter],
                pts
            );
            break;
        }

        // If audio starts before video try and catch up as much as possible
        // (At worst a 20ms gap)
        if audio_capture_timestamps[iter] < first_pts_offset {
            log::debug!(
                "Skipping Audio Frame due to capture time being: {:?} while first video pts is: {:?} pts: {:?}",
                &audio_capture_timestamps[iter],
                &first_pts_offset,
                pts
            );
            iter += 1;
            continue;
        }

        if !first_offset {
            oldest_frame_offset = *pts;
            first_offset = true;
        }

        let offset = pts - oldest_frame_offset;

        log::debug!(
            "PTS IN MICROS: {:?}, PTS IN TIME SCALE: {:?}",
            audio_capture_timestamps[iter],
            offset
        );

        let mut packet = ffmpeg::codec::packet::Packet::copy(frame);
        packet.set_pts(Some(offset));
        packet.set_dts(Some(offset));

        packet.set_stream(AUDIO_STREAM);

        packet
            .write_interleaved(&mut output)
            .expect("Could not write audio interleaved");

        iter += 1;
    }
    log::debug!("AUDIO SAVE END");

    output.write_trailer()?;

    Ok(())
}

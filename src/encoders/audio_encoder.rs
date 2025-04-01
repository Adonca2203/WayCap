use std::collections::VecDeque;

use anyhow::Result;
use ffmpeg_next::{self as ffmpeg, Rational};
use log::debug;

use super::{
    buffer::{AudioBuffer, AudioFrameData},
    video_encoder::ONE_MILLIS,
};

const MIN_RMS: f32 = 0.01;

pub struct AudioEncoder {
    encoder: ffmpeg::codec::encoder::Audio,
    audio_buffer: AudioBuffer,
    next_pts: i64,
    leftover_data: VecDeque<f32>,
}

impl AudioEncoder {
    pub fn new(max_seconds: u32) -> Result<Self, ffmpeg::Error> {
        let encoder = Self::create_opus_encoder()?;
        let max_time = max_seconds as usize * ONE_MILLIS;

        Ok(Self {
            encoder,
            audio_buffer: AudioBuffer::new(max_time),
            next_pts: 0,
            leftover_data: VecDeque::new(),
        })
    }

    fn create_opus_encoder() -> Result<ffmpeg::codec::encoder::Audio, ffmpeg::Error> {
        let encoder_codec = ffmpeg::codec::encoder::find(ffmpeg_next::codec::Id::OPUS)
            .ok_or(ffmpeg::Error::EncoderNotFound)?;

        let mut encoder_ctx = ffmpeg::codec::context::Context::new_with_codec(encoder_codec)
            .encoder()
            .audio()?;

        encoder_ctx.set_rate(48000);
        encoder_ctx.set_bit_rate(70_000);
        encoder_ctx.set_format(ffmpeg::format::Sample::F32(
            ffmpeg_next::format::sample::Type::Packed,
        ));
        encoder_ctx.set_time_base(Rational::new(1, 48000));
        encoder_ctx.set_frame_rate(Some(Rational::new(1, 48000)));
        encoder_ctx.set_channel_layout(ffmpeg::channel_layout::ChannelLayout::STEREO);

        let mut encoder = encoder_ctx.open()?;

        // Opus frame size is based on n channels so need to update it
        unsafe {
            (*encoder.as_mut_ptr()).frame_size =
                (encoder.frame_size() as i32 * encoder.channels() as i32) as i32;
        }

        debug!("Frame size: {}", encoder.frame_size());

        Ok(encoder)
    }

    fn boost_with_rms(&mut self, samples: &mut [f32]) -> Result<(), ffmpeg::Error> {
        let sum_sqrs = samples.iter().map(|&s| s * s).sum::<f32>();
        let rms = (sum_sqrs / samples.len() as f32).sqrt();

        let gain = if rms > 0.0 && rms < MIN_RMS {
            MIN_RMS / rms
        } else {
            1.0
        };

        let gain = gain.min(5.0);
        for sample in samples.iter_mut() {
            *sample *= gain;
        }
        Ok(())
    }

    pub fn process(&mut self, audio: &[f32], capture_time: i64) -> Result<(), ffmpeg::Error> {
        let n_channels = self.encoder.channels() as usize;
        let total_samples = audio.len();

        if total_samples % n_channels != 0 {
            return Err(ffmpeg::Error::InvalidData);
        }

        let frame_size = self.encoder.frame_size() as usize;

        // Boost the audio so that even if system audio level is low
        // it's still audible in playback
        let mut mut_audio = Vec::from(audio);
        self.boost_with_rms(&mut mut_audio)?;
        self.leftover_data.extend(mut_audio);

        while self.leftover_data.len() >= frame_size {
            let frame_samples: Vec<f32> = self.leftover_data.drain(..frame_size).collect();
            let mut frame = ffmpeg::frame::Audio::new(
                self.encoder.format(),
                frame_size,
                self.encoder.channel_layout(),
            );

            frame.plane_mut(0).copy_from_slice(&frame_samples);
            frame.set_pts(Some(self.next_pts));
            frame.set_rate(self.encoder.rate());

            self.encoder.send_frame(&frame)?;
            let mut packet = ffmpeg::codec::packet::Packet::empty();
            while self.encoder.receive_packet(&mut packet).is_ok() {
                if let Some(data) = packet.data() {
                    let pts = packet.pts().unwrap_or(0);
                    let frame_data = AudioFrameData::new(data.to_vec(), pts);
                    self.audio_buffer.insert(pts, frame_data);
                }
            }

            self.next_pts += frame_size as i64;
        }

        Ok(())
    }

    pub fn get_encoder(&self) -> &ffmpeg::codec::encoder::Audio {
        &self.encoder
    }

    pub fn get_buffer(&self) -> &AudioBuffer {
        &self.audio_buffer
    }

    // Drain remaining frames being processed in the encoder
    pub fn drain(&mut self) -> Result<(), ffmpeg::Error> {
        let mut packet = ffmpeg::codec::packet::Packet::empty();
        while self.encoder.receive_packet(&mut packet).is_ok() {
            if let Some(data) = packet.data() {
                let pts = packet.pts().unwrap_or(0);
                let frame_data = AudioFrameData::new(data.to_vec(), pts);
                self.audio_buffer.insert(pts, frame_data.clone());
            }
        }
        Ok(())
    }
}

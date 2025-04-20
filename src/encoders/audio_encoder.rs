use std::collections::VecDeque;

use anyhow::Result;
use ffmpeg_next::{self as ffmpeg, Rational};

use crate::RawAudioFrame;

use super::{buffer::AudioBuffer, video_encoder::ONE_MICROS};

const MIN_RMS: f32 = 0.01;

pub struct AudioEncoder {
    encoder: Option<ffmpeg::codec::encoder::Audio>,
    audio_buffer: AudioBuffer,
    next_pts: i64,
    leftover_data: VecDeque<f32>,
}

impl AudioEncoder {
    pub fn new(max_seconds: u32) -> Result<Self, ffmpeg::Error> {
        let encoder = Some(Self::create_opus_encoder()?);
        let max_time = max_seconds as usize * ONE_MICROS;

        Ok(Self {
            encoder,
            audio_buffer: AudioBuffer::new(max_time),
            next_pts: 0,
            leftover_data: VecDeque::new(),
        })
    }

    pub fn process(&mut self, raw_frame: &mut RawAudioFrame) -> Result<(), ffmpeg::Error> {
        if let Some(ref mut encoder) = self.encoder {
            let n_channels = encoder.channels() as usize;
            let total_samples = raw_frame.samples.len();

            if total_samples % n_channels != 0 {
                return Err(ffmpeg::Error::InvalidData);
            }

            let frame_size = encoder.frame_size() as usize;

            // Boost the audio so that even if system audio level is low
            // it's still audible in playback
            Self::boost_with_rms(raw_frame.get_samples_mut())?;
            self.leftover_data.extend(raw_frame.get_samples());

            // Send chunked frames to encoder
            while self.leftover_data.len() >= frame_size {
                let frame_samples: Vec<f32> = self.leftover_data.drain(..frame_size).collect();
                let mut frame = ffmpeg::frame::Audio::new(
                    encoder.format(),
                    frame_size,
                    encoder.channel_layout(),
                );

                // Capture time in vec
                frame.plane_mut(0).copy_from_slice(&frame_samples);
                frame.set_pts(Some(self.next_pts));
                frame.set_rate(encoder.rate());

                self.audio_buffer.insert_capture_time(raw_frame.timestamp);
                encoder.send_frame(&frame)?;

                // Try and get a frame back from encoder
                let mut packet = ffmpeg::codec::packet::Packet::empty();
                if encoder.receive_packet(&mut packet).is_ok() {
                    if let Some(data) = packet.data() {
                        let pts = packet.pts().unwrap_or(0);
                        self.audio_buffer.insert_frame(pts, data.to_vec());
                    }
                }

                self.next_pts += frame_size as i64;
            }
        }

        Ok(())
    }

    pub fn get_encoder(&self) -> &Option<ffmpeg::codec::encoder::Audio> {
        &self.encoder
    }

    pub fn get_buffer(&self) -> &AudioBuffer {
        &self.audio_buffer
    }

    // Drain remaining frames being processed in the encoder
    pub fn drain(&mut self) -> Result<(), ffmpeg::Error> {
        if let Some(ref mut encoder) = self.encoder {
            encoder.send_eof()?;
            let mut packet = ffmpeg::codec::packet::Packet::empty();
            while encoder.receive_packet(&mut packet).is_ok() {
                if let Some(data) = packet.data() {
                    let pts = packet.pts().unwrap_or(0);
                    self.audio_buffer.insert_frame(pts, data.to_vec());
                }
            }
        }
        Ok(())
    }

    pub fn drop_encoder(&mut self) {
        self.encoder.take();
        self.audio_buffer.reset();
    }

    pub fn reset_encoder(&mut self) -> Result<(), ffmpeg::Error> {
        self.drop_encoder();

        self.encoder = Some(Self::create_opus_encoder()?);
        Ok(())
    }
}

impl AudioEncoder {
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

        Ok(encoder)
    }

    fn boost_with_rms(samples: &mut [f32]) -> Result<(), ffmpeg::Error> {
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
}

use std::collections::VecDeque;

use ffmpeg_next::{self as ffmpeg, Rational};

use super::video_encoder::ONE_MILLIS;

// Need to tweak these
const MIN_AUDIO_VOLUME: f32 = 0.1;
const MAX_AUDIO_VOLUME: f32 = 1.0;

#[derive(Clone, Debug)]
pub struct AudioFrameData {
    pub frame_bytes: Vec<u8>,
    pub capture_time: i64,
    pub chunk_time: i64,
}

impl AudioFrameData {
    fn new() -> Self {
        Self {
            frame_bytes: Vec::new(),
            capture_time: 0,
            chunk_time: 0,
        }
    }

    fn set_capture_time(&mut self, time: i64) {
        self.capture_time = time;
    }

    fn set_frame_bytes(&mut self, frame_bytes: Vec<u8>) {
        self.frame_bytes = frame_bytes;
    }

    fn set_chunk_time(&mut self, time: i64) {
        self.chunk_time = time;
    }
}

pub struct AudioEncoder {
    encoder: ffmpeg::codec::encoder::Audio,
    audio_buffer: VecDeque<AudioFrameData>,
    max_time: usize,
    next_pts: i64,
    leftover_data: VecDeque<f32>,
}

impl AudioEncoder {
    pub fn new(max_time: u32) -> Result<Self, ffmpeg::Error> {
        let encoder = create_opus_encoder()?;

        Ok(Self {
            encoder,
            audio_buffer: VecDeque::new(),
            max_time: (max_time as usize * ONE_MILLIS),
            next_pts: 0,
            leftover_data: VecDeque::new(),
        })
    }

    pub fn process(&mut self, audio: &[f32], time_micro: i64) -> Result<(), ffmpeg::Error> {
        let n_channels = self.encoder.channels() as usize;
        let total_samples = audio.len();

        if total_samples % n_channels != 0 {
            return Err(ffmpeg::Error::InvalidData);
        }

        let mut frame_data = AudioFrameData::new();
        frame_data.set_capture_time(time_micro);

        let frame_size = self.encoder.frame_size() as usize;

        // Normalize the audio so that even if system audio level is low
        // it's still audible in playback
        let mut mut_audio = Vec::from(audio);
        self.normalize_audio_volume(&mut mut_audio, MIN_AUDIO_VOLUME, MAX_AUDIO_VOLUME);

        self.leftover_data.extend(mut_audio.iter().copied());

        while self.leftover_data.len() >= frame_size {
            while let Some(oldest_video) = self.audio_buffer.front() {
                if let Some(newest_audio) = self.audio_buffer.back() {
                    if newest_audio.capture_time - oldest_video.capture_time > self.max_time as i64
                    {
                        self.audio_buffer.pop_front();
                    }
                    break;
                }
            }

            let frame_samples: Vec<f32> = self.leftover_data.drain(..frame_size).collect();
            let mut frame = ffmpeg::frame::Audio::new(
                self.encoder.format(),
                frame_size,
                self.encoder.channel_layout(),
            );

            frame.plane_mut(0).copy_from_slice(&frame_samples);
            frame.set_pts(Some(self.next_pts));
            frame_data.set_chunk_time(self.next_pts);
            self.encoder.send_frame(&frame)?;

            let mut packet = ffmpeg::codec::packet::Packet::empty();
            while self.encoder.receive_packet(&mut packet).is_ok() {
                if let Some(data) = packet.data() {
                    frame_data.set_frame_bytes(data.to_vec());
                    self.audio_buffer.push_back(frame_data.clone());
                }
            }

            self.next_pts += 960;
        }

        Ok(())
    }

    fn normalize_audio_volume(&mut self, samples: &mut [f32], min_volume: f32, max_volume: f32) {
        let peak = samples
            .iter()
            .copied()
            .map(f32::abs)
            .reduce(f32::max)
            .unwrap_or(0.0);

        if peak > 0.0 {
            let target_peak = peak.clamp(min_volume, max_volume);
            let scaling_factor = target_peak / peak;

            for sample in samples.iter_mut() {
                *sample *= scaling_factor;
            }
        }
    }

    pub fn get_encoder(&self) -> &ffmpeg::codec::encoder::Audio {
        &self.encoder
    }

    pub fn get_buffer(&self) -> VecDeque<AudioFrameData> {
        self.audio_buffer.clone()
    }
}

pub fn create_opus_encoder() -> Result<ffmpeg::codec::encoder::Audio, ffmpeg::Error> {
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

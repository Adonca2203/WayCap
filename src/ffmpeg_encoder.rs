use std::{collections::VecDeque, usize};

use anyhow::Result;
use ffmpeg_next::{
    self as ffmpeg,
    software::scaling::{Context as Scaler, Flags},
    Rational,
};
use log::debug;

pub struct FfmpegEncoder {
    video_encoder: ffmpeg::codec::encoder::Video,
    audio_encoder: ffmpeg::codec::encoder::Audio,
    pub video_buffer: VecDeque<VideoFrameData>,
    pub audio_buffer: VecDeque<AudioFrameData>,
    max_time: usize,
    keyframe_indexes: Vec<usize>,
}

#[derive(Clone, Debug)]
pub struct VideoFrameData {
    frame_bytes: Vec<u8>,
    time: i64,
}

#[derive(Clone, Debug)]
pub struct AudioFrameData {
    frame_bytes: Vec<u8>,
    capture_time: i64,
    chunk_time: i64,
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

impl VideoFrameData {
    fn new() -> Self {
        Self {
            frame_bytes: Vec::new(),
            time: 0,
        }
    }

    fn set_time(&mut self, time: i64) {
        self.time = time;
    }

    fn set_frame_bytes(&mut self, frame_bytes: Vec<u8>) {
        self.frame_bytes = frame_bytes;
    }
}

impl FfmpegEncoder {
    pub fn new(
        width: u32,
        height: u32,
        fps: u32,
        buffer_seconds: u32,
    ) -> Result<Self, ffmpeg::Error> {
        let _ = ffmpeg::init();

        let video_encoder = create_nvenc_encoder(width, height, fps)?;

        let audio_encoder = create_opus_encoder()?;
        Ok(Self {
            video_encoder,
            video_buffer: VecDeque::new(),
            audio_buffer: VecDeque::new(),
            // Seconds in micro seconds
            max_time: (buffer_seconds as usize * 1_000_000),
            keyframe_indexes: Vec::new(),
            audio_encoder,
        })
    }

    pub fn process_frame(&mut self, frame: &[u8], time_micro: i64) -> Result<(), ffmpeg::Error> {
        let mut scaler = Scaler::get(
            ffmpeg_next::format::Pixel::BGRA,
            self.video_encoder.width(),
            self.video_encoder.height(),
            ffmpeg_next::format::Pixel::NV12,
            self.video_encoder.width(),
            self.video_encoder.height(),
            Flags::BILINEAR,
        )?;

        let mut frame_data = VideoFrameData::new();
        frame_data.set_time(time_micro);

        let mut src_frame = ffmpeg::util::frame::video::Video::new(
            ffmpeg_next::format::Pixel::BGRA,
            self.video_encoder.width(),
            self.video_encoder.height(),
        );

        src_frame.set_pts(Some(time_micro));
        src_frame.data_mut(0).copy_from_slice(frame);

        // Create destination frame in NV12 format
        let mut dst_frame = ffmpeg::util::frame::video::Video::new(
            ffmpeg_next::format::Pixel::NV12,
            self.video_encoder.width(),
            self.video_encoder.height(),
        );
        dst_frame.set_pts(Some(time_micro));
        scaler.run(&src_frame, &mut dst_frame)?;

        self.video_encoder.send_frame(&dst_frame)?;

        let mut packet = ffmpeg::codec::packet::Packet::empty();
        if self.video_encoder.receive_packet(&mut packet).is_ok() {
            if let Some(data) = packet.data() {
                frame_data.set_frame_bytes(data.to_vec());

                // Keep the buffer to max
                while let Some(oldest) = self.video_buffer.front() {
                    if let Some(newest) = self.video_buffer.back() {
                        if newest.time - oldest.time >= self.max_time as i64
                            && self.keyframe_indexes.len() > 0
                        {
                            debug!("{:?}", self.keyframe_indexes);
                            let drained = self
                                .video_buffer
                                .drain(0..self.keyframe_indexes[0] as usize);

                            self.keyframe_indexes
                                .iter_mut()
                                .for_each(|index| *index -= drained.len());
                            self.keyframe_indexes.retain(|&index| index != 0);

                            debug!("Drained {} frames.", drained.len());
                        } else {
                            break;
                        }
                    }
                }

                self.video_buffer.push_back(frame_data);
                if packet.is_key() && self.video_buffer.len() > 1 {
                    self.keyframe_indexes.push(self.video_buffer.len() - 1);
                }
            };
        }

        Ok(())
    }

    pub fn process_audio(&mut self, audio: &[u8], time_micro: i64) -> Result<(), ffmpeg::Error> {
        let audio_f32: &[f32] = bytemuck::cast_slice(audio);
        let n_channels = self.audio_encoder.channels() as usize;
        let total_samples = audio_f32.len();

        if total_samples % n_channels != 0 {
            return Err(ffmpeg::Error::InvalidData);
        }

        let mut frame_data = AudioFrameData::new();
        frame_data.set_capture_time(time_micro);

        let frame_size = self.audio_encoder.frame_size() as usize;

        let time_per_frame = frame_size;

        // Need to figure out how to properly export this getting too long clips
        let initial_pts =
            time_micro * self.audio_encoder.time_base().denominator() as i64 / 1_000_000;

        for (i, chunk) in audio_f32.chunks_exact(frame_size).enumerate() {
            let mut frame = ffmpeg::frame::Audio::new(
                self.audio_encoder.format(),
                frame_size,
                self.audio_encoder.channel_layout(),
            );
            frame.plane_mut(0).copy_from_slice(chunk);

            let pts = initial_pts + (i as i64 * time_per_frame as i64);
            frame_data.set_chunk_time(pts);
            frame.set_pts(Some(pts));

            debug!("PTS: {}", pts);

            self.audio_encoder.send_frame(&frame)?;

            let mut packet = ffmpeg::codec::packet::Packet::empty();
            while self.audio_encoder.receive_packet(&mut packet).is_ok() {
                if let Some(data) = packet.data() {
                    frame_data.set_frame_bytes(data.to_vec());
                    self.audio_buffer.push_back(frame_data.clone());
                }
            }
        }

        Ok(())
    }

    pub fn save_buffer(&mut self, filename: &str) -> Result<(), ffmpeg::Error> {
        let video_buffer_clone = &self.video_buffer.clone();
        let audio_buffer_clone = &self.audio_buffer.clone();
        if let Some(newest_video) = video_buffer_clone.back() {
            if let Some(newest_audio) = audio_buffer_clone.back() {
                debug!(
                    "Newest Vid TS: {}, Audio TS: {}",
                    newest_video.time, newest_audio.capture_time
                );
            }
        }

        let codec = self.video_encoder.codec().unwrap();
        let mut output = ffmpeg::format::output(&filename)?;
        let mut stream = output.add_stream(codec)?;
        stream.set_rate(self.video_encoder.frame_rate());
        stream.set_time_base(self.video_encoder.time_base());
        stream.set_parameters(&self.video_encoder);

        if let Err(err) = output.write_header() {
            debug!(
                "Ran into the following error while writing header: {:?}",
                err
            );
            return Err(err);
        }

        let first_frame_offset = video_buffer_clone.front().unwrap().time;
        for frame in video_buffer_clone {
            let offset = frame.time - first_frame_offset;

            let mut packet = ffmpeg::codec::packet::Packet::copy(&frame.frame_bytes);
            packet.set_pts(Some(offset));
            packet.set_dts(Some(offset));

            debug!("Offset PTS: {}, Frame actual PTS: {}", offset, frame.time,);

            packet.set_stream(0);

            packet
                .write_interleaved(&mut output)
                .expect("Could not write interleaved");
        }

        output.write_trailer()?;

        Ok(())
    }

    pub fn save_audio(&mut self, filename: &str) -> Result<(), ffmpeg::Error> {
        let audio_buffer_clone = &self.audio_buffer.clone();
        let codec = self.audio_encoder.codec().unwrap();
        let mut output = ffmpeg::format::output(&filename)?;
        let mut stream = output.add_stream(codec)?;
        stream.set_rate(self.audio_encoder.frame_rate());
        stream.set_time_base(self.audio_encoder.time_base());
        stream.set_parameters(&self.audio_encoder);

        output.write_header()?;

        debug!(
            "Last frame time: {}",
            audio_buffer_clone.back().unwrap().chunk_time
        );

        for data in audio_buffer_clone {
            let mut packet = ffmpeg::codec::packet::Packet::copy(&data.frame_bytes);
            packet.set_pts(Some(data.chunk_time));
            packet.set_dts(Some(data.chunk_time));

            packet.set_stream(0);

            packet.write_interleaved(&mut output)?;
        }

        output.write_trailer()?;

        Ok(())
    }
}

fn create_nvenc_encoder(
    width: u32,
    height: u32,
    target_fps: u32,
) -> Result<ffmpeg::codec::encoder::Video, ffmpeg::Error> {
    let encoder_codec =
        ffmpeg::codec::encoder::find_by_name("h264_nvenc").ok_or(ffmpeg::Error::EncoderNotFound)?;

    let mut encoder_ctx = ffmpeg::codec::context::Context::new_with_codec(encoder_codec)
        .encoder()
        .video()?;

    encoder_ctx.set_width(width);
    encoder_ctx.set_height(height);
    encoder_ctx.set_format(ffmpeg::format::Pixel::NV12);
    encoder_ctx.set_frame_rate(Some(Rational::new(target_fps as i32, 1)));
    encoder_ctx.set_bit_rate(5_000_000);
    encoder_ctx.set_time_base(Rational::new(1, 1_000_000));

    // Needed to insert I-Frames more frequently so we don't lose full seconds
    // when popping frames from the front
    encoder_ctx.set_gop(30);

    let encoder_params = ffmpeg::codec::Parameters::new();

    encoder_ctx.set_parameters(encoder_params)?;
    let encoder = encoder_ctx.open()?;

    Ok(encoder)
}

fn create_opus_encoder() -> Result<ffmpeg::codec::encoder::Audio, ffmpeg::Error> {
    let encoder_codec = ffmpeg::codec::encoder::find(ffmpeg_next::codec::Id::OPUS)
        .ok_or(ffmpeg::Error::EncoderNotFound)?;

    let mut encoder_ctx = ffmpeg::codec::context::Context::new_with_codec(encoder_codec)
        .encoder()
        .audio()?;

    encoder_ctx.set_rate(48000);
    encoder_ctx.set_format(ffmpeg::format::Sample::F32(
        ffmpeg_next::format::sample::Type::Packed,
    ));
    encoder_ctx.set_time_base(Rational::new(1, 48000));
    encoder_ctx.set_channel_layout(ffmpeg::channel_layout::ChannelLayout::STEREO);

    let mut encoder = encoder_ctx.open()?;

    // Opus frame size is based on n channels so need to update it
    unsafe {
        (*encoder.as_mut_ptr()).frame_size =
            (encoder.frame_size() as i32 * encoder.channels() as i32) as i32;
    }

    Ok(encoder)
}

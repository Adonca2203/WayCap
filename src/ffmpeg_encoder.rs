use std::{collections::VecDeque, usize};

use anyhow::Result;
use ffmpeg_next::{
    self as ffmpeg,
    codec::traits::Encoder,
    software::scaling::{Context as Scaler, Flags},
    Rational,
};
use log::{debug, warn};

const VIDEO_STREAM: usize = 0;
const AUDIO_STREAM: usize = 1;

pub struct FfmpegEncoder {
    video_encoder: ffmpeg::codec::encoder::Video,
    audio_encoder: ffmpeg::codec::encoder::Audio,
    pub video_buffer: VecDeque<VideoFrameData>,
    pub audio_buffer: VecDeque<AudioFrameData>,
    max_time: usize,
    keyframe_indexes: Vec<usize>,
    next_pts: i64,
    leftover_audio_data: VecDeque<f32>,
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

        let video_encoder = match create_video_encoder(width, height, fps, "h264_nvenc") {
            Ok(video_encoder) => video_encoder,
            Err(_) => {
                debug!("Could not find h264_nvenc encoder. Trying AMD");
                match create_video_encoder(width, height, fps, "h264_amf") {
                    Ok(video_encoder) => video_encoder,
                    Err(_) => {
                        warn!("Could not find h264_amf encoder. Falling back to CPU based encoder");
                        match create_video_encoder(width, height, fps, "h264_amf") {
                            Ok(video_encoder) => {
                                warn!("It's not recommended to use a CPU based encoder for this application but no GPU based one could be found.");
                                video_encoder
                            }
                            Err(e) => return Err(e),
                        }
                    }
                }
            }
        };

        let audio_encoder = create_opus_encoder()?;

        Ok(Self {
            video_encoder,
            video_buffer: VecDeque::new(),
            audio_buffer: VecDeque::new(),
            // Seconds in micro seconds
            max_time: (buffer_seconds as usize * 1_000_000),
            keyframe_indexes: Vec::new(),
            audio_encoder,
            next_pts: 0,
            leftover_audio_data: VecDeque::new(),
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

    pub fn process_audio(&mut self, audio: &[f32], time_micro: i64) -> Result<(), ffmpeg::Error> {
        let n_channels = self.audio_encoder.channels() as usize;
        let total_samples = audio.len();

        if total_samples % n_channels != 0 {
            return Err(ffmpeg::Error::InvalidData);
        }

        let mut frame_data = AudioFrameData::new();
        frame_data.set_capture_time(time_micro);

        let frame_size = self.audio_encoder.frame_size() as usize;

        self.leftover_audio_data.extend(audio);

        while self.leftover_audio_data.len() >= frame_size {
            let frame_samples: Vec<f32> = self.leftover_audio_data.drain(..frame_size).collect();
            let mut frame = ffmpeg::frame::Audio::new(
                self.audio_encoder.format(),
                frame_size,
                self.audio_encoder.channel_layout(),
            );

            frame.plane_mut(0).copy_from_slice(&frame_samples);
            frame.set_pts(Some(self.next_pts));
            frame_data.set_chunk_time(self.next_pts);
            self.audio_encoder.send_frame(&frame)?;

            let mut packet = ffmpeg::codec::packet::Packet::empty();
            while self.audio_encoder.receive_packet(&mut packet).is_ok() {
                if let Some(data) = packet.data() {
                    frame_data.set_frame_bytes(data.to_vec());
                    self.audio_buffer.push_back(frame_data.clone());
                }
            }

            self.next_pts += 960;
        }

        Ok(())
    }

    pub fn save_buffer(&mut self, filename: &str) -> Result<(), ffmpeg::Error> {
        let video_buffer_clone = &self.video_buffer.clone();
        let mut audio_buffer_clone = self.audio_buffer.clone();

        let mut output = ffmpeg::format::output(&filename)?;

        let video_codec = self.video_encoder.codec().unwrap();
        let mut video_stream = output.add_stream(video_codec)?;
        video_stream.set_rate(self.video_encoder.frame_rate());
        video_stream.set_time_base(self.video_encoder.time_base());
        video_stream.set_parameters(&self.video_encoder);

        let audio_codec = self.audio_encoder.codec().unwrap();
        let mut audio_stream = output.add_stream(audio_codec)?;
        audio_stream.set_rate(self.audio_encoder.frame_rate());
        audio_stream.set_time_base(self.audio_encoder.time_base());
        audio_stream.set_parameters(&self.audio_encoder);

        if let Err(err) = output.write_header() {
            debug!(
                "Ran into the following error while writing header: {:?}",
                err
            );
            return Err(err);
        }

        // Align audio buffer timestamp to video buffer
        while let Some(audio_frame) = audio_buffer_clone.front() {
            if let Some(video_frame) = video_buffer_clone.front() {
                if audio_frame.capture_time < video_frame.time {
                    audio_buffer_clone.pop_front();
                    continue;
                }
            }
            break;
        }

        if let Some(newest_video) = video_buffer_clone.front() {
            if let Some(newest_audio) = audio_buffer_clone.front() {
                debug!(
                    "Newest Vid TS: {}, Audio TS: {}",
                    newest_video.time, newest_audio.capture_time
                );
            }
        }

        // Write video
        let first_frame_offset = video_buffer_clone.front().unwrap().time;
        for frame in video_buffer_clone {
            let offset = frame.time - first_frame_offset;

            let mut packet = ffmpeg::codec::packet::Packet::copy(&frame.frame_bytes);
            packet.set_pts(Some(offset));
            packet.set_dts(Some(offset));

            packet.set_stream(VIDEO_STREAM);

            packet
                .write_interleaved(&mut output)
                .expect("Could not write video interleaved");
        }

        // Write audio
        let first_frame_offset = audio_buffer_clone.front().unwrap().chunk_time;
        for frame in audio_buffer_clone {
            let offset = frame.chunk_time - first_frame_offset;

            let mut packet = ffmpeg::codec::packet::Packet::copy(&frame.frame_bytes);
            packet.set_pts(Some(offset));
            packet.set_dts(Some(offset));

            packet.set_stream(AUDIO_STREAM);

            packet
                .write_interleaved(&mut output)
                .expect("Could not write audio interleaved");
        }

        output.write_trailer()?;

        Ok(())
    }

    // Use this one to test output of audio pipewire stream seems to have
    // weird static so will need to test more
    #[allow(dead_code)]
    pub fn save_audio(&mut self, filename: &str) -> Result<(), ffmpeg::Error> {
        let audio_buffer_clone = &self.audio_buffer.clone();
        let codec = self.audio_encoder.codec().unwrap();
        let mut output = ffmpeg::format::output(&filename)?;
        let mut stream = output.add_stream(codec)?;
        stream.set_rate(self.audio_encoder.frame_rate());
        stream.set_time_base(self.audio_encoder.time_base());
        stream.set_parameters(&self.audio_encoder);

        output.write_header()?;

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

fn create_video_encoder(
    width: u32,
    height: u32,
    target_fps: u32,
    encoder_name: &str,
) -> Result<ffmpeg::codec::encoder::Video, ffmpeg::Error> {
    let encoder_codec =
        ffmpeg::codec::encoder::find_by_name(encoder_name).ok_or(ffmpeg::Error::EncoderNotFound)?;

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
    encoder_ctx.set_bit_rate(128_000);
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

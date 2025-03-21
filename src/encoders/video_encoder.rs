use std::collections::BTreeMap;

use ffmpeg_next::{self as ffmpeg, Rational};

pub const ONE_MILLIS: usize = 1_000_000;
const GOP_SIZE: u32 = 30;

#[derive(Clone, Debug)]
pub struct VideoFrameData {
    pub frame_bytes: Vec<u8>,
    pub pts: i64,
    is_key: bool,
}

pub struct VideoEncoder {
    encoder: ffmpeg::codec::encoder::Video,
    video_buffer: FrameBuffer,
}

#[derive(Clone)]
pub struct FrameBuffer {
    /// Maps Frames by DTS -> Frame Information so it is ordered properly at muxing time
    pub frames: BTreeMap<i64, VideoFrameData>,
    max_time: usize,
}

impl FrameBuffer {
    fn new(max_time: usize) -> Self {
        Self {
            frames: BTreeMap::new(),
            max_time,
        }
    }

    fn insert(&mut self, timestamp: i64, frame: VideoFrameData) {
        self.frames.insert(timestamp, frame);

        // Keep the buffer to max
        while let Some(oldest) = self.oldest_pts() {
            if let Some(newest) = self.newest_pts() {
                if newest - oldest >= self.max_time as i64 {
                    self.trim_oldest_gop();
                } else {
                    break;
                }
            }
        }
    }

    pub fn newest_pts(&self) -> Option<i64> {
        self.frames.values().map(|frame| frame.pts).max()
    }

    pub fn oldest_pts(&self) -> Option<i64> {
        self.frames.values().map(|frame| frame.pts).min()
    }

    pub fn get_last_gop_start(&self) -> i64 {
        for (dts, frame) in self.frames.iter().rev() {
            if frame.is_key {
                return *dts;
            }
        }
        -1
    }

    fn trim_oldest_gop(&mut self) {
        let mut first_key_frame = true;
        for _ in 0..self.frames.len() {
            if let Some((&oldest, frame)) = self.frames.iter().next() {
                if frame.is_key && !first_key_frame {
                    break;
                } else {
                    first_key_frame = false;
                }

                self.frames.remove(&oldest);
            } else {
                break;
            }
        }
    }
}

impl VideoFrameData {
    fn new(frame_bytes: Vec<u8>, is_key: bool, dts: i64) -> Self {
        Self {
            frame_bytes,
            is_key,
            pts: dts,
        }
    }
}

impl VideoEncoder {
    pub fn new(
        width: u32,
        height: u32,
        max_buffer_seconds: u32,
        encoder_name: &str,
    ) -> Result<Self, ffmpeg::Error> {
        ffmpeg::init()?;

        let encoder = create_encoder(width, height, encoder_name)?;
        let max_time = max_buffer_seconds as usize * ONE_MILLIS;

        Ok(Self {
            encoder,
            video_buffer: FrameBuffer::new(max_time),
        })
    }

    pub fn process(&mut self, frame: &[u8], time_micro: i64) -> Result<(), ffmpeg::Error> {
        let mut src_frame = ffmpeg::util::frame::video::Video::new(
            ffmpeg_next::format::Pixel::BGRA,
            self.encoder.width(),
            self.encoder.height(),
        );

        src_frame.set_pts(Some(time_micro));
        src_frame.data_mut(0).copy_from_slice(frame);

        self.encoder.send_frame(&src_frame).unwrap();

        let mut packet = ffmpeg::codec::packet::Packet::empty();
        if self.encoder.receive_packet(&mut packet).is_ok() {
            if let Some(data) = packet.data() {
                let frame_data =
                    VideoFrameData::new(data.to_vec(), packet.is_key(), packet.pts().unwrap_or(0));

                self.video_buffer
                    .insert(packet.dts().unwrap_or(0), frame_data);
            };
        }

        Ok(())
    }

    /// Drain the encoder of any remaining frames it is processing
    pub fn drain(&mut self) -> Result<(), ffmpeg::Error> {
        let mut packet = ffmpeg::codec::packet::Packet::empty();
        while self.encoder.receive_packet(&mut packet).is_ok() {
            if let Some(data) = packet.data() {
                let frame_data =
                    VideoFrameData::new(data.to_vec(), packet.is_key(), packet.pts().unwrap_or(0));

                self.video_buffer
                    .insert(packet.dts().unwrap_or(0), frame_data);
            };
            packet = ffmpeg::codec::packet::Packet::empty();
        }

        Ok(())
    }

    pub fn get_encoder(&self) -> &ffmpeg::codec::encoder::Video {
        &self.encoder
    }

    pub fn get_buffer(&self) -> FrameBuffer {
        self.video_buffer.clone()
    }
}

fn create_encoder(
    width: u32,
    height: u32,
    encoder_name: &str,
) -> Result<ffmpeg::codec::encoder::Video, ffmpeg::Error> {
    let encoder_codec =
        ffmpeg::codec::encoder::find_by_name(encoder_name).ok_or(ffmpeg::Error::EncoderNotFound)?;

    let mut encoder_ctx = ffmpeg::codec::context::Context::new_with_codec(encoder_codec)
        .encoder()
        .video()?;

    encoder_ctx.set_width(width);
    encoder_ctx.set_height(height);
    encoder_ctx.set_format(ffmpeg::format::Pixel::BGRA);
    encoder_ctx.set_frame_rate(Some(Rational::new(1, 60)));

    // These should be part of a config file
    encoder_ctx.set_bit_rate(12_000_000);
    encoder_ctx.set_max_bit_rate(16_000_000);
    encoder_ctx.set_time_base(Rational::new(1, 1_000_000));

    // Needed to insert I-Frames more frequently so we don't lose full seconds
    // when popping frames from the front
    encoder_ctx.set_gop(GOP_SIZE);

    let encoder_params = ffmpeg::codec::Parameters::new();
    let mut opts = ffmpeg::Dictionary::new();
    opts.set("vsync", "vfr");

    encoder_ctx.set_parameters(encoder_params)?;
    let encoder = encoder_ctx.open_with(opts)?;

    Ok(encoder)
}

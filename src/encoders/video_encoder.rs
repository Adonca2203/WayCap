use ffmpeg_next::{self as ffmpeg, Rational};

use crate::application_config::{load_or_create_config, QualityPreset};

use super::buffer::{FrameBuffer, VideoFrameData};

pub const ONE_MILLIS: usize = 1_000_000;
const GOP_SIZE: u32 = 30;

pub struct VideoEncoder {
    encoder: ffmpeg::codec::encoder::Video,
    video_buffer: FrameBuffer,
}

impl VideoEncoder {
    pub fn new(
        width: u32,
        height: u32,
        max_buffer_seconds: u32,
        encoder_name: &str,
    ) -> Result<Self, ffmpeg::Error> {
        ffmpeg::log::set_level(ffmpeg_next::log::Level::Debug);
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
    let config = load_or_create_config();
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
    encoder_ctx.set_time_base(Rational::new(1, 1_000_000));

    // Needed to insert I-Frames more frequently so we don't lose full seconds
    // when popping frames from the front
    encoder_ctx.set_gop(GOP_SIZE);

    let encoder_params = ffmpeg::codec::Parameters::new();
    let mut opts = ffmpeg::Dictionary::new();
    match config.quality {
        QualityPreset::LOW => {
            opts.set("vsync", "vfr");
            opts.set("rc", "vbr");
            opts.set("preset", "p2");
            opts.set("tune", "hq");
            opts.set("cq", "45");
        }
        QualityPreset::MEDIUM => {
            opts.set("vsync", "vfr");
            opts.set("rc", "vbr");
            opts.set("preset", "p4");
            opts.set("tune", "hq");
            opts.set("cq", "25");
        }
        QualityPreset::HIGH => {
            opts.set("vsync", "vfr");
            opts.set("rc", "vbr");
            opts.set("preset", "p7");
            opts.set("tune", "hq");
            opts.set("cq", "1");
        }
    }

    encoder_ctx.set_parameters(encoder_params)?;
    let encoder = encoder_ctx.open_with(opts)?;

    Ok(encoder)
}

use ffmpeg_next::{self as ffmpeg, Rational};

use crate::application_config::{load_or_create_config, QualityPreset};

use super::buffer::{VideoBuffer, VideoFrameData};

pub const ONE_MILLIS: usize = 1_000_000;
const GOP_SIZE: u32 = 30;

pub struct VideoEncoder {
    encoder: Option<ffmpeg::codec::encoder::Video>,
    video_buffer: VideoBuffer,
    width: u32,
    height: u32,
    encoder_name: String,
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

        let encoder = Some(Self::create_encoder(width, height, encoder_name)?);
        let max_time = max_buffer_seconds as usize * ONE_MILLIS;

        Ok(Self {
            encoder,
            video_buffer: VideoBuffer::new(max_time),
            width,
            height,
            encoder_name: encoder_name.to_string(),
        })
    }

    pub fn process(&mut self, frame: &[u8], time_micro: i64) -> Result<(), ffmpeg::Error> {
        if let Some(ref mut encoder) = self.encoder {
            let mut src_frame = ffmpeg::util::frame::video::Video::new(
                ffmpeg_next::format::Pixel::BGRA,
                encoder.width(),
                encoder.height(),
            );

            src_frame.set_pts(Some(time_micro));
            src_frame.data_mut(0).copy_from_slice(frame);

            encoder.send_frame(&src_frame).unwrap();

            let mut packet = ffmpeg::codec::packet::Packet::empty();
            if encoder.receive_packet(&mut packet).is_ok() {
                if let Some(data) = packet.data() {
                    let frame_data = VideoFrameData::new(
                        data.to_vec(),
                        packet.is_key(),
                        packet.pts().unwrap_or(0),
                    );

                    self.video_buffer
                        .insert(packet.dts().unwrap_or(0), frame_data);
                };
            }
        }
        Ok(())
    }

    /// Drain the encoder of any remaining frames it is processing
    pub fn drain(&mut self) -> Result<(), ffmpeg::Error> {
        if let Some(ref mut encoder) = self.encoder {
            encoder.send_eof()?;
            let mut packet = ffmpeg::codec::packet::Packet::empty();
            while encoder.receive_packet(&mut packet).is_ok() {
                if let Some(data) = packet.data() {
                    let frame_data = VideoFrameData::new(
                        data.to_vec(),
                        packet.is_key(),
                        packet.pts().unwrap_or(0),
                    );

                    self.video_buffer
                        .insert(packet.dts().unwrap_or(0), frame_data);
                };
                packet = ffmpeg::codec::packet::Packet::empty();
            }
        }
        Ok(())
    }

    pub fn reset_encoder(&mut self) -> Result<(), ffmpeg::Error> {
        // Drop the encoder
        self.encoder.take();
        self.video_buffer.reset();

        // Recreate it
        self.encoder = Some(Self::create_encoder(
            self.width,
            self.height,
            &self.encoder_name,
        )?);
        Ok(())
    }

    pub fn get_encoder(&self) -> &Option<ffmpeg::codec::encoder::Video> {
        &self.encoder
    }

    pub fn get_buffer(&self) -> &VideoBuffer {
        &self.video_buffer
    }
}

impl VideoEncoder {
    fn create_encoder(
        width: u32,
        height: u32,
        encoder_name: &str,
    ) -> Result<ffmpeg::codec::encoder::Video, ffmpeg::Error> {
        let config = load_or_create_config();
        let encoder_codec = ffmpeg::codec::encoder::find_by_name(encoder_name)
            .ok_or(ffmpeg::Error::EncoderNotFound)?;

        let mut encoder_ctx = ffmpeg::codec::context::Context::new_with_codec(encoder_codec)
            .encoder()
            .video()?;

        encoder_ctx.set_width(width);
        encoder_ctx.set_height(height);
        encoder_ctx.set_format(ffmpeg::format::Pixel::BGRA);
        encoder_ctx.set_frame_rate(Some(Rational::new(1, 60)));
        encoder_ctx.set_bit_rate(16_000_000);

        // These should be part of a config file
        encoder_ctx.set_time_base(Rational::new(1, 1_000_000));

        // Needed to insert I-Frames more frequently so we don't lose full seconds
        // when popping frames from the front
        encoder_ctx.set_gop(GOP_SIZE);

        let encoder_params = ffmpeg::codec::Parameters::new();
        let mut opts = ffmpeg::Dictionary::new();

        // TODO: Fine tune these presets and show estimated file sizes for each in the
        // README
        match config.quality {
            // 1.5 GB file for a 5 minute recording
            QualityPreset::LOW => {
                opts.set("vsync", "vfr");
                opts.set("rc", "vbr");
                opts.set("preset", "p2");
                opts.set("tune", "hq");
                opts.set("cq", "25");
                opts.set("b:v", "20M");
            }
            QualityPreset::MEDIUM => {
                opts.set("vsync", "vfr");
                opts.set("rc", "vbr");
                opts.set("preset", "p4");
                opts.set("tune", "hq");
                opts.set("cq", "18");
                opts.set("b:v", "40M");
            }
            QualityPreset::HIGH => {
                opts.set("vsync", "vfr");
                opts.set("rc", "vbr");
                opts.set("preset", "p7");
                opts.set("tune", "hq");
                opts.set("cq", "10");
                opts.set("b:v", "80M");
            }
            QualityPreset::HIGHEST => {
                opts.set("vsync", "vfr");
                opts.set("rc", "vbr");
                opts.set("preset", "p7");
                opts.set("tune", "hq");
                opts.set("cq", "1");
                opts.set("b:v", "120M");
            }
        }

        encoder_ctx.set_parameters(encoder_params)?;
        let encoder = encoder_ctx.open_with(opts)?;

        Ok(encoder)
    }
}

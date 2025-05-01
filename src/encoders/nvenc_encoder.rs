use ffmpeg_next::{self as ffmpeg, Rational};

use crate::{
    application_config::{load_or_create_config, AppConfig, QualityPreset},
    RawVideoFrame,
};

use super::{
    buffer::{VideoBuffer, VideoFrameData},
    video_encoder::{VideoEncoder, GOP_SIZE, ONE_MICROS},
};

pub struct NvencEncoder {
    encoder: Option<ffmpeg::codec::encoder::Video>,
    video_buffer: VideoBuffer,
    width: u32,
    height: u32,
    encoder_name: String,
}

impl VideoEncoder for NvencEncoder {
    fn new(width: u32, height: u32, max_buffer_seconds: u32) -> anyhow::Result<Self>
    where
        Self: Sized,
    {
        let encoder_name = "h264_nvenc";
        let encoder = Some(Self::create_encoder(width, height, encoder_name)?);
        let max_time = max_buffer_seconds as usize * ONE_MICROS;

        Ok(Self {
            encoder,
            video_buffer: VideoBuffer::new(max_time),
            width,
            height,
            encoder_name: encoder_name.to_string(),
        })
    }

    fn process(&mut self, frame: &RawVideoFrame) -> Result<(), ffmpeg::Error> {
        if let Some(ref mut encoder) = self.encoder {
            let mut src_frame = ffmpeg::util::frame::video::Video::new(
                ffmpeg_next::format::Pixel::BGRA,
                encoder.width(),
                encoder.height(),
            );

            src_frame.set_pts(Some(*frame.get_timestamp()));
            src_frame.data_mut(0).copy_from_slice(frame.get_bytes());

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
    fn drain(&mut self) -> Result<(), ffmpeg::Error> {
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

    fn drop_encoder(&mut self) {
        self.video_buffer.reset();
        self.encoder.take();
    }

    fn reset(&mut self) -> anyhow::Result<()> {
        self.drop_encoder();
        self.encoder = Some(Self::create_encoder(
            self.width,
            self.height,
            &self.encoder_name,
        )?);
        Ok(())
    }

    fn get_encoder(&self) -> &Option<ffmpeg::codec::encoder::Video> {
        &self.encoder
    }

    fn get_buffer(&self) -> &VideoBuffer {
        &self.video_buffer
    }
}

impl NvencEncoder {
    fn create_encoder(
        width: u32,
        height: u32,
        encoder: &str,
    ) -> Result<ffmpeg::codec::encoder::Video, ffmpeg::Error> {
        let config = load_or_create_config();
        let encoder_codec =
            ffmpeg::codec::encoder::find_by_name(encoder).ok_or(ffmpeg::Error::EncoderNotFound)?;

        let mut encoder_ctx = ffmpeg::codec::context::Context::new_with_codec(encoder_codec)
            .encoder()
            .video()?;

        encoder_ctx.set_width(width);
        encoder_ctx.set_height(height);
        encoder_ctx.set_format(ffmpeg::format::Pixel::BGRA);
        encoder_ctx.set_bit_rate(16_000_000);

        // These should be part of a config file
        encoder_ctx.set_time_base(Rational::new(1, 1_000_000));

        // Needed to insert I-Frames more frequently so we don't lose full seconds
        // when popping frames from the front
        encoder_ctx.set_gop(GOP_SIZE);

        let encoder_params = ffmpeg::codec::Parameters::new();

        let opts = Self::get_encoder_params(&config);

        encoder_ctx.set_parameters(encoder_params)?;
        let encoder = encoder_ctx.open_with(opts)?;

        Ok(encoder)
    }

    fn get_encoder_params(config: &AppConfig) -> ffmpeg::Dictionary {
        let mut opts = ffmpeg::Dictionary::new();
        opts.set("vsync", "vfr");
        opts.set("rc", "vbr");
        opts.set("tune", "hq");
        match config.quality {
            QualityPreset::Low => {
                opts.set("preset", "p2");
                opts.set("cq", "30");
                opts.set("b:v", "20M");
            }
            QualityPreset::Medium => {
                opts.set("preset", "p4");
                opts.set("cq", "25");
                opts.set("b:v", "40M");
            }
            QualityPreset::High => {
                opts.set("preset", "p7");
                opts.set("cq", "20");
                opts.set("b:v", "80M");
            }
            QualityPreset::Ultra => {
                opts.set("preset", "p7");
                opts.set("cq", "15");
                opts.set("b:v", "120M");
            }
        }
        opts
    }
}

impl Drop for NvencEncoder {
    fn drop(&mut self) {
        let _ = self.drain();
        self.drop_encoder();
    }
}

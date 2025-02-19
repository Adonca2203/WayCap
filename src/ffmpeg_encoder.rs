use std::collections::VecDeque;

use anyhow::Result;
use ffmpeg_next::{
    self as ffmpeg,
    software::scaling::{Context as Scaler, Flags},
    Rational,
};
use log::debug;

pub struct FfmpegEncoder {
    encoder: ffmpeg::codec::encoder::Video,
    pub buffer: VecDeque<FrameData>,
    max_frames: usize,
}

#[derive(Clone, Debug)]
pub struct FrameData {
    frame_bytes: Vec<u8>,
    time: i64,
}

impl FrameData {
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

        let encoder_codec = ffmpeg::codec::encoder::find_by_name("h264_nvenc")
            .ok_or(ffmpeg::Error::EncoderNotFound)?;

        debug!("Setting codec context");
        let mut encoder_ctx = ffmpeg::codec::context::Context::new_with_codec(encoder_codec)
            .encoder()
            .video()?;

        encoder_ctx.set_width(width);
        encoder_ctx.set_height(height);
        encoder_ctx.set_format(ffmpeg::format::Pixel::NV12);
        encoder_ctx.set_frame_rate(Some(Rational::new(fps as i32, 1)));
        encoder_ctx.set_bit_rate(5_000_000);
        encoder_ctx.set_time_base(Rational::new(1, fps as i32 * 1000));

        let encoder_params = ffmpeg::codec::Parameters::new();

        encoder_ctx.set_parameters(encoder_params)?;
        debug!("Opening encoder.");
        let encoder = encoder_ctx.open()?;

        Ok(Self {
            encoder,
            buffer: VecDeque::new(),
            max_frames: (buffer_seconds * fps) as usize,
        })
    }

    pub fn process_frame(&mut self, frame: &[u8], time_micro: i64) -> Result<(), ffmpeg::Error> {
        let mut scaler = Scaler::get(
            ffmpeg_next::format::Pixel::BGRA,
            self.encoder.width(),
            self.encoder.height(),
            ffmpeg_next::format::Pixel::NV12,
            self.encoder.width(),
            self.encoder.height(),
            Flags::BILINEAR,
        )?;

        let mut frame_data = FrameData::new();
        frame_data.set_time(time_micro);

        let mut src_frame = ffmpeg::util::frame::video::Video::new(
            ffmpeg_next::format::Pixel::BGRA,
            self.encoder.width(),
            self.encoder.height(),
        );

        src_frame.set_pts(Some(time_micro));

        src_frame.data_mut(0).copy_from_slice(frame);

        // Create destination frame in NV12 format
        let mut dst_frame = ffmpeg::util::frame::video::Video::new(
            ffmpeg_next::format::Pixel::NV12,
            self.encoder.width(),
            self.encoder.height(),
        );

        dst_frame.set_pts(Some(time_micro));

        // debug!("Converting...");
        scaler.run(&src_frame, &mut dst_frame)?;

        // debug!("Sending frame to encoder");
        self.encoder.send_frame(&dst_frame)?;

        let mut packet = ffmpeg::codec::packet::Packet::empty();

        if self.encoder.receive_packet(&mut packet).is_ok() {
            if let Some(data) = packet.data() {
                frame_data.set_frame_bytes(data.to_vec());

                self.buffer.push_back(frame_data);

                // Keep the buffer to max
                while self.buffer.len() > self.max_frames {
                    self.buffer.pop_front();
                }
            };
        }

        Ok(())
    }

    pub fn save_buffer(&mut self, filename: &str) -> Result<(), ffmpeg::Error> {
        let buffer_clone = &self.buffer.clone();

        let codec = self.encoder.codec().unwrap();
        let mut output = ffmpeg::format::output(&filename)?;
        let mut stream = output.add_stream(codec)?;
        stream.set_rate(self.encoder.frame_rate());
        stream.set_time_base(self.encoder.time_base());
        stream.set_parameters(&self.encoder);

        if let Err(err) = output.write_header() {
            debug!(
                "Ran into the following error while writing header: {:?}",
                err
            );
            return Err(err);
        }

        for frame in buffer_clone {
            let tb = self.encoder.time_base();

            let pts = (frame.time as f64 * tb.denominator() as f64) / 1_000_000.0;

            let mut packet = ffmpeg::codec::packet::Packet::copy(&frame.frame_bytes);
            packet.set_pts(Some(pts.round() as i64));
            packet.set_dts(Some(pts.round() as i64));
            packet.set_stream(0);

            packet
                .write_interleaved(&mut output)
                .expect("Could not write interleaved");
        }

        output.write_trailer()?;

        Ok(())
    }
}

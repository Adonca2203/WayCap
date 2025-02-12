use std::{
    collections::VecDeque,
    time::{Duration, SystemTime},
};

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
    start_time: SystemTime,
}

#[derive(Clone, Debug)]
pub struct FrameData {
    frame_bytes: Vec<u8>,
    pts: i64, // Presentation Time Scale
    dts: i64, // Decode Time Scale (Should be the same as pts unless we want to be fancy)
}

impl FrameData {
    fn new() -> Self {
        Self {
            frame_bytes: Vec::new(),
            pts: 0,
            dts: 0,
        }
    }

    fn set_pts(&mut self, pts: i64) {
        self.pts = pts;
    }

    fn set_dts(&mut self, dts: i64) {
        self.dts = dts;
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
        encoder_ctx.set_frame_rate(Some(fps as f64));
        encoder_ctx.set_bit_rate(5_000_000);
        encoder_ctx.set_time_base(Rational::new(1, fps as i32));

        let encoder_params = ffmpeg::codec::Parameters::new();

        encoder_ctx.set_parameters(encoder_params)?;
        debug!("Opening encoder.");
        let encoder = encoder_ctx.open()?;

        Ok(Self {
            encoder,
            buffer: VecDeque::new(),
            max_frames: (buffer_seconds * fps) as usize,
            start_time: SystemTime::now(),
        })
    }

    pub fn process_frame(&mut self, frame: &[u8]) -> Result<(), ffmpeg::Error> {
        debug!(
            "Frame received: {} bytes, expected: {} bytes",
            frame.len(),
            self.encoder.width() * self.encoder.height() * 4
        );
        debug!("Processing frame");

        debug!("Trying to convert BGRx to NV12");
        let mut scaler = Scaler::get(
            ffmpeg_next::format::Pixel::BGRA,
            self.encoder.width(),
            self.encoder.height(),
            ffmpeg_next::format::Pixel::NV12,
            self.encoder.width(),
            self.encoder.height(),
            Flags::BILINEAR,
        )?;

        let elapsed_millis = self
            .start_time
            .elapsed()
            .unwrap_or(Duration::ZERO)
            .as_millis() as i64;

        let pts_step = (elapsed_millis * self.encoder.time_base().denominator() as i64) / 1_000;

        let pts = if let Some(prev_frame) = self.buffer.back() {
            if pts_step > prev_frame.pts {
                pts_step
            } else {
                prev_frame.pts + pts_step
            }
        } else {
            pts_step
        };

        let mut frame_data = FrameData::new();
        frame_data.set_pts(pts);
        frame_data.set_dts(pts);

        let mut src_frame = ffmpeg::util::frame::video::Video::new(
            ffmpeg_next::format::Pixel::BGRA,
            self.encoder.width(),
            self.encoder.height(),
        );

        src_frame.set_pts(Some(pts));

        src_frame.data_mut(0).copy_from_slice(frame);

        // Create destination frame in NV12 format
        let mut dst_frame = ffmpeg::util::frame::video::Video::new(
            ffmpeg_next::format::Pixel::NV12,
            self.encoder.width(),
            self.encoder.height(),
        );

        dst_frame.set_pts(Some(pts));

        debug!("Converting...");
        scaler.run(&src_frame, &mut dst_frame)?;

        debug!("Sending frame to encoder");
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
        debug!("Attemting to save {}.", filename);
        debug!("Using codec {:?}", codec.name());
        let mut output = ffmpeg::format::output(&filename)?;
        let mut stream = output.add_stream(codec)?;
        stream.set_rate(self.encoder.frame_rate());
        stream.set_time_base(self.encoder.time_base());
        stream.set_parameters(&self.encoder);

        debug!("Writing header");
        if let Err(err) = output.write_header() {
            debug!(
                "Ran into the following error while writing header: {:?}",
                err
            );
            return Err(err);
        }

        for (frame_number, frame) in buffer_clone.iter().enumerate() {
            debug!(
                "Writing frame {} to buffer, with pts {}",
                frame_number, frame.pts
            );
            let mut packet = ffmpeg::codec::packet::Packet::copy(&frame.frame_bytes);
            packet.set_pts(Some(frame.pts));
            packet.set_dts(Some(frame.dts));
            packet.set_stream(0);

            packet
                .write_interleaved(&mut output)
                .expect("Could not write interleaved");
        }

        output.write_trailer()?;

        Ok(())
    }
}

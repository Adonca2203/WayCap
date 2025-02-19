use std::collections::VecDeque;

use anyhow::Result;
use ffmpeg_next::{
    self as ffmpeg,
    software::scaling::{Context as Scaler, Flags},
    Rational,
};
use log::debug;

pub struct FfmpegEncoder {
    pub buffer: VecDeque<FrameData>,
    max_frames: usize,
    width: u32,
    height: u32,
    fps: u32,
}

#[derive(Clone)]
pub struct FrameData {
    time: i64,
    video_frame: Option<ffmpeg::util::frame::Video>,
}

impl FrameData {
    fn new() -> Self {
        Self {
            time: 0,
            video_frame: None,
        }
    }

    fn set_time(&mut self, time: i64) {
        self.time = time;
    }

    fn set_video_frame(&mut self, video_frame: ffmpeg::util::frame::Video) {
        self.video_frame = Some(video_frame);
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

        Ok(Self {
            buffer: VecDeque::new(),
            max_frames: (buffer_seconds * fps) as usize,
            height,
            width,
            fps,
        })
    }

    pub fn process_frame(&mut self, frame: &[u8], time_micro: i64) -> Result<(), ffmpeg::Error> {
        let mut scaler = Scaler::get(
            ffmpeg_next::format::Pixel::BGRA,
            self.width,
            self.height,
            ffmpeg_next::format::Pixel::NV12,
            self.width,
            self.height,
            Flags::BILINEAR,
        )?;

        let mut frame_data = FrameData::new();
        frame_data.set_time(time_micro);

        let mut src_frame = ffmpeg::util::frame::video::Video::new(
            ffmpeg_next::format::Pixel::BGRA,
            self.width,
            self.height,
        );

        src_frame.set_pts(Some(time_micro));

        src_frame.data_mut(0).copy_from_slice(frame);

        // Create destination frame in NV12 format
        let mut dst_frame = ffmpeg::util::frame::video::Video::new(
            ffmpeg_next::format::Pixel::NV12,
            self.width,
            self.height,
        );

        dst_frame.set_pts(Some(time_micro));

        scaler.run(&src_frame, &mut dst_frame)?;

        frame_data.set_video_frame(dst_frame);

        self.buffer.push_back(frame_data);

        while self.buffer.len() > self.max_frames {
            self.buffer.pop_front();
        }
        Ok(())
    }

    pub fn save_buffer(&mut self, filename: &str) -> Result<(), ffmpeg::Error> {
        let mut buffer_clone = self.buffer.clone();

        let mut encoder = self.create_encoder().expect("Failed to create encoder");
        let codec = encoder.codec().unwrap();
        let mut output = ffmpeg::format::output(&filename)?;
        let mut stream = output.add_stream(codec)?;
        stream.set_rate(encoder.frame_rate());
        stream.set_time_base(encoder.time_base());
        stream.set_parameters(&encoder);

        if let Err(err) = output.write_header() {
            debug!(
                "Ran into the following error while writing header: {:?}",
                err
            );
            return Err(err);
        }

        let first_frame_offset = buffer_clone.front().unwrap().time;
        for frame in buffer_clone.iter_mut() {
            let tb = encoder.time_base();
            let offset = frame.time - first_frame_offset;

            let pts = (offset as f64 * tb.denominator() as f64) / 1_000_000.0;
            let frame_video = frame.video_frame.as_mut().expect("Frame does not exist");
            frame_video.set_pts(Some(pts.round() as i64));

            encoder
                .send_frame(&frame_video)
                .expect("Error sending frame to encoder");

            let mut packet = ffmpeg::codec::packet::Packet::empty();
            while let Ok(_) = encoder.receive_packet(&mut packet) {
                packet
                    .write_interleaved(&mut output)
                    .expect("Error writing packet");

                packet = ffmpeg::codec::packet::Packet::empty();
            }
        }

        // Begin flushing
        encoder.send_eof().expect("Error sending null frame");

        let mut packet = ffmpeg::codec::packet::Packet::empty();
        while let Err(err) = encoder.receive_packet(&mut packet) {
            if err == ffmpeg::Error::Eof {
                debug!("Reached encoder EOF done processing frames.");
            } else {
                return Err(err);
            }

            packet
                .write_interleaved(&mut output)
                .expect("Error writing packet");

            packet = ffmpeg::codec::packet::Packet::empty();
        }

        output.write_trailer()?;

        Ok(())
    }

    fn create_encoder(&mut self) -> Result<ffmpeg::codec::encoder::Video, ffmpeg::Error> {
        let encoder_codec = ffmpeg::codec::encoder::find_by_name("h264_nvenc")
            .ok_or(ffmpeg::Error::EncoderNotFound)?;

        debug!("Setting codec context");
        let mut encoder_ctx = ffmpeg::codec::context::Context::new_with_codec(encoder_codec)
            .encoder()
            .video()?;

        encoder_ctx.set_width(self.width);
        encoder_ctx.set_height(self.height);
        encoder_ctx.set_format(ffmpeg::format::Pixel::NV12);
        encoder_ctx.set_frame_rate(Some(Rational::new(self.fps as i32, 1)));
        encoder_ctx.set_bit_rate(50_000_000);
        encoder_ctx.set_time_base(Rational::new(1, self.fps as i32 * 1000));

        let encoder_params = ffmpeg::codec::Parameters::new();

        encoder_ctx.set_parameters(encoder_params)?;
        let encoder = encoder_ctx.open()?;

        Ok(encoder)
    }
}

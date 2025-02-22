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
    max_time: usize,
    keyframe_indexes: Vec<usize>,
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

        let encoder = create_nvenc_encoder(width, height, fps)?;
        Ok(Self {
            encoder,
            buffer: VecDeque::new(),
            // Seconds in micro seconds
            max_time: (buffer_seconds as usize * 1_000_000),
            keyframe_indexes: Vec::new(),
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
        scaler.run(&src_frame, &mut dst_frame)?;

        self.encoder.send_frame(&dst_frame)?;

        let mut packet = ffmpeg::codec::packet::Packet::empty();
        if self.encoder.receive_packet(&mut packet).is_ok() {
            if let Some(data) = packet.data() {
                frame_data.set_frame_bytes(data.to_vec());

                // Keep the buffer to max
                while let Some(oldest) = self.buffer.front() {
                    if let Some(newest) = self.buffer.back() {
                        if newest.time - oldest.time >= self.max_time as i64
                            && self.keyframe_indexes.len() > 0
                        {
                            debug!("{:?}", self.keyframe_indexes);
                            let drained = self.buffer.drain(0..self.keyframe_indexes[0] as usize);

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

                self.buffer.push_back(frame_data);
                if packet.is_key() && self.buffer.len() > 1 {
                    self.keyframe_indexes.push(self.buffer.len() - 1);
                }
            };
        }

        Ok(())
    }

    pub fn save_buffer(&mut self, filename: &str) -> Result<(), ffmpeg::Error> {
        debug!("Keyframes: {:?}", self.keyframe_indexes);
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

        let first_frame_offset = buffer_clone.front().unwrap().time;
        for frame in buffer_clone {
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

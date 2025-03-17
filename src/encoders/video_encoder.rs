use std::collections::VecDeque;

use ffmpeg_next::{self as ffmpeg, Rational};
use log::debug;

pub const ONE_MILLIS: usize = 1_000_000;

#[derive(Clone, Debug)]
pub struct VideoFrameData {
    pub frame_bytes: Vec<u8>,
    pub capture_time: i64,
}

pub struct VideoEncoder {
    encoder: ffmpeg::codec::encoder::Video,
    video_buffer: VecDeque<VideoFrameData>,
    max_time: usize,
    keyframe_indexes: VecDeque<usize>,
    last_frame_time: Option<i64>,
    frame_interval: i64,
}

impl VideoFrameData {
    fn new() -> Self {
        Self {
            frame_bytes: Vec::new(),
            capture_time: 0,
        }
    }

    fn set_frame_bytes(&mut self, data: Vec<u8>) {
        self.frame_bytes = data;
    }

    fn set_capture_time(&mut self, time: i64) {
        self.capture_time = time;
    }
}

impl VideoEncoder {
    pub fn new(
        width: u32,
        height: u32,
        target_fps: u32,
        max_buffer_seconds: u32,
        encoder_name: &str,
    ) -> Result<Self, ffmpeg::Error> {
        ffmpeg::log::set_level(ffmpeg_next::log::Level::Debug);
        ffmpeg::init()?;

        let encoder = create_encoder(width, height, target_fps, encoder_name)?;

        Ok(Self {
            encoder,
            video_buffer: VecDeque::new(),
            max_time: (max_buffer_seconds as usize * ONE_MILLIS),
            keyframe_indexes: VecDeque::new(),
            last_frame_time: None,
            frame_interval: ONE_MILLIS as i64 / target_fps as i64,
        })
    }

    pub fn process(&mut self, frame: &[u8], time_micro: i64) -> Result<(), ffmpeg::Error> {
        // Throttle to target input framerate
        // if let Some(last_time) = self.last_frame_time {
        //     let elapsed = time_micro - last_time;
        //     if elapsed < self.frame_interval {
        //         debug!(
        //             "Discarding this frame. \nElapsed: {}\nFrame Interval: {}",
        //             elapsed, self.frame_interval
        //         );
        //         return Ok(());
        //     }
        // }

        self.last_frame_time = Some(time_micro);

        let mut frame_data = VideoFrameData::new();
        frame_data.set_capture_time(time_micro);

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
                frame_data.set_frame_bytes(data.to_vec());

                // Keep the buffer to max
                while let Some(oldest) = self.video_buffer.front() {
                    if let Some(newest) = self.video_buffer.back() {
                        if newest.capture_time - oldest.capture_time >= self.max_time as i64
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
                    self.keyframe_indexes.push_back(self.video_buffer.len() - 1);
                }
            };
        }

        Ok(())
    }

    pub async fn get_encoder(&self) -> &ffmpeg::codec::encoder::Video {
        &self.encoder
    }

    pub async fn get_buffer(&self) -> VecDeque<VideoFrameData> {
        self.video_buffer.clone()
    }
}

fn create_encoder(
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

    // The quality still doesn't look amazing not sure if encoding issue
    // or something I need to do on pipewire end?
    encoder_ctx.set_width(width);
    encoder_ctx.set_height(height);
    encoder_ctx.set_format(ffmpeg::format::Pixel::BGRA);
    encoder_ctx.set_bit_rate(100_000_000);
    // These should be part of a config file
    encoder_ctx.set_time_base(Rational::new(1, 1_000_000));

    // Needed to insert I-Frames more frequently so we don't lose full seconds
    // when popping frames from the front
    encoder_ctx.set_gop(30);

    let encoder_params = ffmpeg::codec::Parameters::new();

    encoder_ctx.set_parameters(encoder_params)?;

    let mut opts = ffmpeg::Dictionary::new();
    opts.set("preset", "p7");
    opts.set("rc", "vbr");
    opts.set("cq", "0");
    opts.set("lossless", "1");

    let encoder = encoder_ctx.open_with(opts)?;

    Ok(encoder)
}

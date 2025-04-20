use anyhow::Result;
use ffmpeg_next::{self as ffmpeg};

use crate::RawVideoFrame;

use super::buffer::VideoBuffer;

pub const ONE_MICROS: usize = 1_000_000;
pub const GOP_SIZE: u32 = 30;

pub trait VideoEncoder {
    fn new(width: u32, height: u32, max_buffer_seconds: u32, encoder_name: &str) -> Result<Self>
    where
        Self: Sized;
    fn process(&mut self, frame: &RawVideoFrame) -> Result<(), ffmpeg::Error>;
    fn drain(&mut self) -> Result<(), ffmpeg::Error>;
    fn reset(&mut self) -> Result<()>;
    fn get_buffer(&self) -> &VideoBuffer;
    fn drop_encoder(&mut self);
    fn get_encoder(&self) -> &Option<ffmpeg::codec::encoder::Video>;
}

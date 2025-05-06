use anyhow::Result;
use ffmpeg_next::{self as ffmpeg};
use ringbuf::HeapCons;

use crate::{application_config::QualityPreset, RawVideoFrame};

use super::buffer::VideoFrameData;

pub const ONE_MICROS: usize = 1_000_000;
pub const GOP_SIZE: u32 = 30;

pub trait VideoEncoder: Send {
    fn new(width: u32, height: u32, quality: QualityPreset) -> Result<Self>
    where
        Self: Sized;
    fn process(&mut self, frame: &RawVideoFrame) -> Result<(), ffmpeg::Error>;
    fn drain(&mut self) -> Result<(), ffmpeg::Error>;
    fn reset(&mut self) -> Result<()>;
    fn drop_encoder(&mut self);
    fn get_encoder(&self) -> &Option<ffmpeg::codec::encoder::Video>;
    fn take_encoded_recv(&mut self) -> Option<HeapCons<(i64, VideoFrameData)>>;
}

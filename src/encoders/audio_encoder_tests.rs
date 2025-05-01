use crate::{encoders::video_encoder::ONE_MICROS, RawAudioFrame};

use super::audio_encoder::*;

use ffmpeg_next::{
    self as ffmpeg,
    codec::packet::Packet,
    format,
    frame::{self},
    Rational,
};
use std::collections::VecDeque;

/// A fake audio encoder for testing.
#[derive(Clone)]
pub struct FakeAudioEncoder {
    pub channels: u16,
    pub frame_size: i32,
    pub rate: u32,
    pub sent_frames: Vec<Vec<f32>>,
    pub queued_packets: VecDeque<Vec<f32>>,
}

impl FakeAudioEncoder {
    /// Create a new fake. By default no packets are queued.
    pub fn new(channels: u16, frame_size: i32, rate: u32) -> Self {
        Self {
            channels,
            frame_size,
            rate,
            sent_frames: Vec::new(),
            queued_packets: VecDeque::new(),
        }
    }

    /// In your test you can push `Vec<u8>`s here; they’ll come back in `receive_packet`.
    pub fn push_packet(&mut self, data: Vec<f32>) {
        self.queued_packets.push_back(data);
    }
}

impl AudioEncoderImpl for FakeAudioEncoder {
    type Error = ffmpeg::Error;

    fn codec(&self) -> Option<ffmpeg_next::Codec> {
        todo!()
    }

    fn time_base(&self) -> Rational {
        todo!()
    }

    fn channels(&self) -> u16 {
        self.channels
    }

    fn frame_size(&self) -> i32 {
        self.frame_size
    }

    fn format(&self) -> format::Sample {
        format::Sample::F32(ffmpeg_next::format::sample::Type::Packed)
    }

    fn channel_layout(&self) -> ffmpeg_next::channel_layout::ChannelLayout {
        ffmpeg_next::channel_layout::ChannelLayout::STEREO
    }

    fn rate(&self) -> u32 {
        self.rate
    }

    fn send_frame(&mut self, frame: &frame::Audio) -> Result<(), Self::Error> {
        let buf = frame.plane(0).to_vec();
        self.push_packet(buf.to_vec());
        self.sent_frames.push(buf);
        Ok(())
    }

    fn receive_packet(&mut self, pkt: &mut Packet) -> Result<(), Self::Error> {
        if let Some(data) = self.queued_packets.pop_front() {
            let as_u8: &[u8] = bytemuck::cast_slice(&data);
            *pkt = Packet::copy(as_u8);
            let pts = (self.sent_frames.len() - 1) as i64 * self.frame_size as i64;
            pkt.set_pts(Some(pts));
            pkt.set_dts(Some(pts));
            Ok(())
        } else {
            Err(ffmpeg::Error::Exit)
        }
    }

    fn send_eof(&mut self) -> Result<(), Self::Error> {
        Ok(())
    }
}

fn create_default_frame_encoder() -> FakeAudioEncoder {
    // These numbers match the default opus encoder values
    FakeAudioEncoder::new(
        /*channesl=*/ 2, /*frame_size=*/ 960, /*rate=*/ 48000,
    )
}

#[test]
fn test_process_drain() {
    let fake = create_default_frame_encoder();
    let mut audio_encoder = AudioEncoder::new_with_encoder(|| Ok(fake.clone()), 10).unwrap();

    let mut raw = [
        RawAudioFrame {
            timestamp: 1,
            samples: vec![0.0; 1024],
        },
        RawAudioFrame {
            timestamp: 2,
            samples: vec![0.0; 1024],
        },
    ];

    for frame in raw.iter_mut() {
        audio_encoder.process(frame).unwrap();
    }

    audio_encoder.drain().unwrap();
    audio_encoder.drop_encoder();

    let buffer = audio_encoder.get_buffer();

    assert!(buffer.get_frames().is_empty());
    assert!(buffer.get_capture_times().is_empty());
}

#[test]
fn test_process_no_trim() {
    let fake = create_default_frame_encoder();
    let mut audio_encoder = AudioEncoder::new_with_encoder(|| Ok(fake.clone()), 10).unwrap();

    let mut raw = [
        RawAudioFrame {
            timestamp: 1,
            samples: vec![0.0; 1024],
        },
        RawAudioFrame {
            timestamp: 2,
            samples: vec![0.0; 1024],
        },
    ];

    for frame in raw.iter_mut() {
        audio_encoder.process(frame).unwrap();
    }

    let encoder = audio_encoder.get_encoder().as_ref().unwrap();
    let buffer = audio_encoder.get_buffer();

    assert_eq!(encoder.sent_frames.len(), 2);
    assert_eq!(buffer.get_frames().len(), 2);
    assert_eq!(*buffer.get_capture_times(), vec![1, 2]);
    assert_eq!(
        buffer.get_frames().keys().copied().collect::<Vec<_>>(),
        vec![0, 960]
    );
}

#[test]
fn test_process_duplicate_timestamps_when_leftover_is_multiple_of_sample() {
    let fake = create_default_frame_encoder();
    let mut audio_encoder = AudioEncoder::new_with_encoder(|| Ok(fake.clone()), 60).unwrap();

    let mut raw_frames = vec![];
    let mut actual_capture_times = vec![];

    for i in 1..=15 {
        raw_frames.push(RawAudioFrame {
            timestamp: i,
            samples: vec![0.0; 1024],
        });

        actual_capture_times.push(i);
    }

    // This frame should be duplicated
    actual_capture_times.push(15);

    for frame in raw_frames.iter_mut() {
        audio_encoder.process(frame).unwrap();
    }

    let encoder = audio_encoder.get_encoder().as_ref().unwrap();
    let buffer = audio_encoder.get_buffer();

    assert_eq!(encoder.sent_frames.len(), actual_capture_times.len());
    assert_eq!(buffer.get_frames().len(), actual_capture_times.len());

    assert_eq!(*buffer.get_capture_times(), actual_capture_times);
    assert_eq!(
        *buffer
            .get_frames()
            .keys()
            .copied()
            .collect::<Vec<_>>()
            .last()
            .unwrap(),
        actual_capture_times.len() as i64 * fake.frame_size() as i64 - fake.frame_size() as i64,
    );
}

#[test]
fn test_process_trimming() {
    let fake = create_default_frame_encoder();
    let max_frames = 15;
    let mut audio_encoder = AudioEncoder::new_with_encoder(|| Ok(fake.clone()), 5).unwrap();

    let mut raw_frames = vec![];
    // 5 second window
    let actual_capture_times = vec![
        11 * ONE_MICROS as i64,
        12 * ONE_MICROS as i64,
        13 * ONE_MICROS as i64,
        14 * ONE_MICROS as i64,
        15 * ONE_MICROS as i64,
        15 * ONE_MICROS as i64,
    ];

    for i in 1..=max_frames {
        raw_frames.push(RawAudioFrame {
            timestamp: i * ONE_MICROS as i64,
            samples: vec![0.0; 1024],
        });
    }

    for frame in raw_frames.iter_mut() {
        audio_encoder.process(frame).unwrap();
    }

    let encoder = audio_encoder.get_encoder().as_ref().unwrap();
    let buffer = audio_encoder.get_buffer();

    println!("{:?}", buffer.get_capture_times());
    assert_eq!(encoder.sent_frames.len() as i64, max_frames + 1); // Duplicate last frame
    assert_eq!(buffer.get_frames().len(), actual_capture_times.len());

    assert_eq!(*buffer.get_capture_times(), actual_capture_times);
    assert_eq!(
        *buffer
            .get_frames()
            .keys()
            .copied()
            .collect::<Vec<_>>()
            .last()
            .unwrap(),
        (max_frames + 1) * fake.frame_size() as i64 - fake.frame_size() as i64,
    );
}

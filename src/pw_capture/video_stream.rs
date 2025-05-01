use std::{
    os::fd::{FromRawFd, OwnedFd, RawFd},
    sync::{atomic::AtomicBool, Arc},
    time::SystemTime,
};

use pipewire::{
    self as pw,
    context::Context,
    main_loop::MainLoop,
    spa::utils::Direction,
    stream::{Stream, StreamFlags, StreamState},
};
use pw::{properties::properties, spa};

use ringbuf::{traits::Producer, HeapProd};
use spa::pod::Pod;
use tokio::sync::mpsc;

use crate::{RawVideoFrame, Terminate};

pub struct VideoCapture {
    video_ready: Arc<AtomicBool>,
    audio_ready: Arc<AtomicBool>,
}

#[derive(Clone, Copy, Default)]
struct UserData {
    video_format: spa::param::video::VideoInfoRaw,
}

impl VideoCapture {
    pub fn new(video_ready: Arc<AtomicBool>, audio_ready: Arc<AtomicBool>) -> Self {
        Self {
            video_ready,
            audio_ready,
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub fn run(
        &self,
        pipewire_fd: RawFd,
        stream_node: u32,
        mut ringbuf_producer: HeapProd<RawVideoFrame>,
        termination_recv: pw::channel::Receiver<Terminate>,
        saving: Arc<AtomicBool>,
        start_time: SystemTime,
        resolution_negotiation_channel: mpsc::Sender<(u32, u32)>,
    ) -> Result<(), pipewire::Error> {
        let pw_loop = MainLoop::new(None)?;
        let terminate_loop = pw_loop.clone();

        let _recv = termination_recv.attach(pw_loop.loop_(), move |_| {
            log::debug!("Terminating video capture loop");
            terminate_loop.quit();
        });

        let pw_context = Context::new(&pw_loop)?;
        let core = pw_context.connect_fd(unsafe { OwnedFd::from_raw_fd(pipewire_fd) }, None)?;

        let data = UserData::default();

        let _listener = core
            .add_listener_local()
            .info(|i| log::info!("VIDEO CORE:\n{0:#?}", i))
            .error(|e, f, g, h| log::error!("{0},{1},{2},{3}", e, f, g, h))
            .done(|d, _| log::info!("DONE: {0}", d))
            .register();

        // Set up video stream
        let video_stream = Stream::new(
            &core,
            "waycap-video",
            properties! {
                *pw::keys::MEDIA_TYPE => "Video",
                *pw::keys::MEDIA_CATEGORY => "Capture",
                *pw::keys::MEDIA_ROLE => "Screen",
            },
        )?;

        let ready_clone = Arc::clone(&self.video_ready);
        let audio_ready_clone = Arc::clone(&self.audio_ready);
        let _video_stream = video_stream
            .add_local_listener_with_user_data(data)
            .state_changed(move |_, _, old, new| {
                log::debug!("Video Stream State Changed: {0:?} -> {1:?}", old, new);
                ready_clone.store(
                    new == StreamState::Streaming,
                    std::sync::atomic::Ordering::Release,
                );
            })
            .param_changed(move |_, user_data, id, param| {
                let Some(param) = param else {
                    return;
                };
                if id != pw::spa::param::ParamType::Format.as_raw() {
                    return;
                }

                let (media_type, media_subtype) =
                    match pw::spa::param::format_utils::parse_format(param) {
                        Ok(v) => v,
                        Err(_) => return,
                    };

                if media_type != pw::spa::param::format::MediaType::Video
                    || media_subtype != pw::spa::param::format::MediaSubtype::Raw
                {
                    return;
                }

                user_data
                    .video_format
                    .parse(param)
                    .expect("Failed to parse param");

                log::debug!(
                    "  format: {} ({:?})",
                    user_data.video_format.format().as_raw(),
                    user_data.video_format.format()
                );

                resolution_negotiation_channel
                    .blocking_send((
                        user_data.video_format.size().width,
                        user_data.video_format.size().height,
                    ))
                    .unwrap();

                log::debug!(
                    "  size: {}x{}",
                    user_data.video_format.size().width,
                    user_data.video_format.size().height
                );
                log::debug!(
                    "  framerate: {}/{}",
                    user_data.video_format.framerate().num,
                    user_data.video_format.framerate().denom
                );
            })
            .process(move |stream, _| {
                match stream.dequeue_buffer() {
                    None => log::debug!("out of buffers"),
                    Some(mut buffer) => {
                        // Wait until audio is streaming before we try to process
                        if !audio_ready_clone.load(std::sync::atomic::Ordering::Acquire)
                            || saving.load(std::sync::atomic::Ordering::Acquire)
                        {
                            return;
                        }

                        let datas = buffer.datas_mut();
                        if datas.is_empty() {
                            return;
                        }

                        let time_us = if let Ok(elapsed) = start_time.elapsed() {
                            elapsed.as_micros() as i64
                        } else {
                            0
                        };

                        // send frame data to encoder
                        let data = &mut datas[0];
                        if let Some(frame) = data.data() {
                            if ringbuf_producer
                                .try_push(RawVideoFrame {
                                    bytes: frame.to_vec(),
                                    timestamp: time_us,
                                })
                                .is_err()
                            {
                                log::error!(
                                    "Error sending video frame at: {:?}. Ring buf full?",
                                    time_us
                                );
                            }
                        }
                    }
                }
            })
            .register()?;

        let video_spa_obj = pw::spa::pod::object!(
            pw::spa::utils::SpaTypes::ObjectParamFormat,
            pw::spa::param::ParamType::EnumFormat,
            pw::spa::pod::property!(
                pw::spa::param::format::FormatProperties::MediaType,
                Id,
                pw::spa::param::format::MediaType::Video
            ),
            pw::spa::pod::property!(
                pw::spa::param::format::FormatProperties::MediaSubtype,
                Id,
                pw::spa::param::format::MediaSubtype::Raw
            ),
            pw::spa::pod::property!(
                pw::spa::param::format::FormatProperties::VideoFormat,
                Choice,
                Enum,
                Id,
                pw::spa::param::video::VideoFormat::xRGB,
                pw::spa::param::video::VideoFormat::RGB,
                pw::spa::param::video::VideoFormat::RGB,
                pw::spa::param::video::VideoFormat::RGBA,
                pw::spa::param::video::VideoFormat::RGBx,
                pw::spa::param::video::VideoFormat::BGRx,
                pw::spa::param::video::VideoFormat::I420,
            ),
            pw::spa::pod::property!(
                pw::spa::param::format::FormatProperties::VideoSize,
                Choice,
                Range,
                Rectangle,
                pw::spa::utils::Rectangle {
                    width: 2560,
                    height: 1440
                }, // Default
                pw::spa::utils::Rectangle {
                    width: 1,
                    height: 1
                }, // Min
                pw::spa::utils::Rectangle {
                    width: 4096,
                    height: 4096
                } // Max
            ),
            pw::spa::pod::property!(
                pw::spa::param::format::FormatProperties::VideoFramerate,
                Choice,
                Range,
                Fraction,
                pw::spa::utils::Fraction { num: 240, denom: 1 }, // Default
                pw::spa::utils::Fraction { num: 0, denom: 1 },   // Min
                pw::spa::utils::Fraction { num: 244, denom: 1 }  // Max
            ),
        );

        let video_spa_values: Vec<u8> = pw::spa::pod::serialize::PodSerializer::serialize(
            std::io::Cursor::new(Vec::new()),
            &pw::spa::pod::Value::Object(video_spa_obj),
        )
        .unwrap()
        .0
        .into_inner();

        let mut video_params = [Pod::from_bytes(&video_spa_values).unwrap()];

        video_stream.connect(
            Direction::Input,
            Some(stream_node),
            StreamFlags::AUTOCONNECT | StreamFlags::MAP_BUFFERS,
            &mut video_params,
        )?;

        log::debug!("Video Stream: {0:?}", video_stream);

        pw_loop.run();
        Ok(())
    }
}

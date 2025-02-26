use std::{
    os::fd::{FromRawFd, OwnedFd, RawFd},
    time::SystemTime,
};

use log::{debug, error, info};
use pipewire::{
    self as pw,
    context::Context,
    main_loop::MainLoop,
    spa::{
        param::format::{MediaSubtype, MediaType},
        utils::Direction,
    },
    stream::{Stream, StreamFlags},
};
use pw::{properties::properties, spa};

use spa::pod::Pod;

#[derive(Default, Debug)]
pub struct Frame {}

pub struct PipewireCapture {
    main_loop: MainLoop,
}

#[derive(Clone, Copy)]
struct UserData {
    video_format: spa::param::video::VideoInfoRaw,
    audio_format: spa::param::audio::AudioInfoRaw,
    start_time: SystemTime,
}

impl PipewireCapture {
    pub fn new<F1, F2>(
        pipewire_fd: RawFd,
        stream_node: u32,
        process_video_callback: F1,
        process_audio_callback: F2,
    ) -> Result<Self, pipewire::Error>
    where
        F1: Fn(Vec<u8>, i64) + Send + 'static,
        F2: Fn(Vec<u8>, i64) + Send + 'static,
    {
        pw::init();
        let pw_loop = MainLoop::new(None)?;
        let pw_context = Context::new(&pw_loop)?;
        let core = pw_context.connect_fd(unsafe { OwnedFd::from_raw_fd(pipewire_fd) }, None)?;
        let audio_core = pw_context.connect(None)?;

        let data = UserData {
            video_format: Default::default(),
            audio_format: Default::default(),
            start_time: SystemTime::now(),
        };

        let _listener = core
            .add_listener_local()
            .info(|i| info!("{0:#?}", i))
            .error(|e, f, g, h| error!("{0},{1},{2},{3}", e, f, g, h))
            .done(|d, _| info!("DONE: {0}", d))
            .register();

        // Set up video stream
        let video_stream = Stream::new(
            &core,
            "auto-screen-recorder-video",
            properties! {
                *pw::keys::MEDIA_TYPE => "Video",
                *pw::keys::MEDIA_CATEGORY => "Capture",
                *pw::keys::MEDIA_ROLE => "Screen",
            },
        )?;

        let _video_stream_listener = video_stream
            .add_local_listener_with_user_data(data)
            .state_changed(|_, _, old, new| {
                debug!("Video Stream State Changed: {0:?} -> {1:?}", old, new);
            })
            .param_changed(|_, user_data, id, param| {
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
                    .expect("Faield to parse param");

                debug!(
                    "  format: {} ({:?})",
                    user_data.video_format.format().as_raw(),
                    user_data.video_format.format()
                );
                debug!(
                    "  size: {}x{}",
                    user_data.video_format.size().width,
                    user_data.video_format.size().height
                );
                debug!(
                    "  framerate: {}/{}",
                    user_data.video_format.framerate().num,
                    user_data.video_format.framerate().denom
                );
            })
            .process(move |stream, udata| {
                match stream.dequeue_buffer() {
                    None => debug!("out of buffers"),
                    Some(mut buffer) => {
                        let time_ms = if let Ok(elapsed) = udata.start_time.elapsed() {
                            elapsed.as_micros() as i64
                        } else {
                            0
                        };

                        let datas = buffer.datas_mut();
                        if datas.is_empty() {
                            return;
                        }

                        // send frame data to encoder
                        let data = &mut datas[0];
                        process_video_callback(data.data().unwrap().to_vec(), time_ms);
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

        debug!("Video Stream: {0:?}", video_stream);

        // Audio Stream
        let audio_stream = pw::stream::Stream::new(
            &audio_core,
            "auto-screen-recorder-audio",
            properties! {
            *pw::keys::MEDIA_TYPE => "Audio",
            *pw::keys::MEDIA_CATEGORY => "Capture",
            *pw::keys::MEDIA_ROLE => "Music", },
        )?;

        let _audio_stream_listener = audio_stream
            .add_local_listener_with_user_data(data)
            .state_changed(|_, _, old, new| {
                debug!("Audio Stream State Changed: {0:?} -> {1:?}", old, new);
            })
            .param_changed(|_, udata, id, param| {
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

                // only accept raw audio
                if media_type != MediaType::Audio || media_subtype != MediaSubtype::Raw {
                    return;
                }

                udata
                    .audio_format
                    .parse(param)
                    .expect("Failed to parse audio params");

                debug!(
                    "Capturing Rate:{} channels:{}, format: {}",
                    udata.audio_format.rate(),
                    udata.audio_format.channels(),
                    udata.audio_format.format().as_raw()
                );
            })
            .process(move |stream, udata| match stream.dequeue_buffer() {
                None => debug!("Out of audio buffers"),
                Some(mut buffer) => {
                    let datas = buffer.datas_mut();
                    if datas.is_empty() {
                        return;
                    }

                    let time_ms = if let Ok(elapsed) = udata.start_time.elapsed() {
                        elapsed.as_micros() as i64
                    } else {
                        0
                    };

                    let data = &mut datas[0];

                    if let Some(samples) = data.data() {
                        process_audio_callback(samples.to_vec(), time_ms);
                    }
                }
            })
            .register()?;

        let mut audio_info = pw::spa::param::audio::AudioInfoRaw::new();
        audio_info.set_format(pw::spa::param::audio::AudioFormat::F32LE);
        let audio_spa_obj = pw::spa::pod::Object {
            type_: pw::spa::utils::SpaTypes::ObjectParamFormat.as_raw(),
            id: pw::spa::param::ParamType::EnumFormat.as_raw(),
            properties: audio_info.into(),
        };

        let audio_spa_values: Vec<u8> = pw::spa::pod::serialize::PodSerializer::serialize(
            std::io::Cursor::new(Vec::new()),
            &pw::spa::pod::Value::Object(audio_spa_obj),
        )
        .unwrap()
        .0
        .into_inner();

        let mut audio_params = [Pod::from_bytes(&audio_spa_values).unwrap()];
        audio_stream.connect(
            Direction::Input,
            Some(stream_node),
            StreamFlags::AUTOCONNECT | StreamFlags::MAP_BUFFERS | StreamFlags::RT_PROCESS,
            &mut audio_params,
        )?;

        pw_loop.run();

        Ok(Self { main_loop: pw_loop })
    }
}

impl Drop for PipewireCapture {
    fn drop(&mut self) {
        self.main_loop.quit();

        unsafe {
            pw::deinit();
        }
    }
}

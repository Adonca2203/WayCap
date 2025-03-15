use std::{
    os::fd::{FromRawFd, OwnedFd, RawFd},
    process::Command,
    sync::{atomic::AtomicBool, Arc},
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
    stream::{Stream, StreamFlags, StreamState},
};
use pw::{properties::properties, spa};

use spa::pod::Pod;
use tokio::sync::mpsc;

pub struct PipewireCapture {
    main_loop: MainLoop,
}

struct SharedState {
    video_ready: AtomicBool,
    audio_ready: AtomicBool,
    start_time: SystemTime,
}

impl Default for SharedState {
    fn default() -> Self {
        Self {
            video_ready: false.into(),
            audio_ready: false.into(),
            start_time: SystemTime::now(),
        }
    }
}

#[derive(Clone, Copy)]
struct UserData {
    video_format: spa::param::video::VideoInfoRaw,
    audio_format: spa::param::audio::AudioInfoRaw,
}

impl Default for UserData {
    fn default() -> Self {
        Self {
            video_format: Default::default(),
            audio_format: Default::default(),
        }
    }
}

impl PipewireCapture {
    pub fn new(
        pipewire_fd: RawFd,
        stream_node: u32,
        process_video_callback: mpsc::Sender<(Vec<u8>, i64)>,
        process_audio_callback: mpsc::Sender<(Vec<f32>, i64)>,
        use_mic: bool,
    ) -> Result<Self, pipewire::Error> {
        pw::init();
        let pw_loop = MainLoop::new(None)?;
        let pw_context = Context::new(&pw_loop)?;
        let core = pw_context.connect_fd(unsafe { OwnedFd::from_raw_fd(pipewire_fd) }, None)?;

        let audio_core = pw_context.connect(None)?;

        let data = UserData::default();
        let shared_state = Arc::new(SharedState::default());

        let _listener = core
            .add_listener_local()
            .info(|i| info!("VIDEO CORE:\n{0:#?}", i))
            .error(|e, f, g, h| error!("{0},{1},{2},{3}", e, f, g, h))
            .done(|d, _| info!("DONE: {0}", d))
            .register();

        let _audio_core_listener = audio_core
            .add_listener_local()
            .info(|i| info!("AUDIO CORE:\n{0:#?}", i))
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

        let _video_stream_shared_data_listener = video_stream
            .add_local_listener_with_user_data(shared_state.clone())
            .state_changed(|_, udata, old, new| {
                debug!("Video Stream State Changed: {0:?} -> {1:?}", old, new);
                udata.video_ready.store(
                    new == StreamState::Streaming,
                    std::sync::atomic::Ordering::Release,
                );
            })
            .process(move |stream, udata| {
                match stream.dequeue_buffer() {
                    None => debug!("out of buffers"),
                    Some(mut buffer) => {
                        // Wait until audio is streaming before we try to process
                        if !udata.audio_ready.load(std::sync::atomic::Ordering::Acquire) {
                            return;
                        }

                        let datas = buffer.datas_mut();
                        if datas.is_empty() {
                            return;
                        }

                        let time_ms = if let Ok(elapsed) = udata.start_time.elapsed() {
                            elapsed.as_micros() as i64
                        } else {
                            0
                        };

                        // send frame data to encoder
                        let data = &mut datas[0];
                        if let Some(frame) = data.data() {
                            process_video_callback
                                .blocking_send((frame.to_vec(), time_ms))
                                .unwrap();
                        }
                    }
                }
            })
            .register()?;

        let _video_stream_listener = video_stream
            .add_local_listener_with_user_data(data)
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
            *pw::keys::MEDIA_ROLE => "Music",
            },
        )?;

        let _audio_stream_shared_data_listener = audio_stream
            .add_local_listener_with_user_data(shared_state)
            .state_changed(|_, udata, old, new| {
                debug!("Audio Stream State Changed: {0:?} -> {1:?}", old, new);
                udata.audio_ready.store(
                    new == StreamState::Streaming,
                    std::sync::atomic::Ordering::Release,
                );
            })
            .process(move |stream, udata| match stream.dequeue_buffer() {
                None => debug!("Out of audio buffers"),
                Some(mut buffer) => {
                    let datas = buffer.datas_mut();
                    if datas.is_empty() {
                        return;
                    }

                    // Wait until video is streaming before we try to process
                    if !udata.video_ready.load(std::sync::atomic::Ordering::Acquire) {
                        return;
                    }

                    let time_ms = if let Ok(elapsed) = udata.start_time.elapsed() {
                        elapsed.as_micros() as i64
                    } else {
                        0
                    };

                    let data = &mut datas[0];
                    let n_samples = data.chunk().size() / (std::mem::size_of::<f32>()) as u32;

                    if let Some(samples) = data.data() {
                        let samples_f32: &[f32] = bytemuck::cast_slice(samples);
                        let audio_samples = &samples_f32[..n_samples as usize];
                        process_audio_callback
                            .blocking_send((audio_samples.to_vec(), time_ms))
                            .unwrap();
                    }
                }
            })
            .register()?;

        let _audio_stream_listener = audio_stream
            .add_local_listener_with_user_data(data)
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
            .register()?;

        let audio_spa_obj = pw::spa::pod::object! {
            pw::spa::utils::SpaTypes::ObjectParamFormat,
            pw::spa::param::ParamType::EnumFormat,
            pw::spa::pod::property!(
                pw::spa::param::format::FormatProperties::MediaType,
                Id,
                pw::spa::param::format::MediaType::Audio
                ),
            pw::spa::pod::property!(
                pw::spa::param::format::FormatProperties::MediaSubtype,
                Id,
                pw::spa::param::format::MediaSubtype::Raw
            ),
            pw::spa::pod::property!(
                pw::spa::param::format::FormatProperties::AudioFormat,
                Id,
                pw::spa::param::audio::AudioFormat::F32LE
            )
        };

        let audio_spa_values: Vec<u8> = pw::spa::pod::serialize::PodSerializer::serialize(
            std::io::Cursor::new(Vec::new()),
            &pw::spa::pod::Value::Object(audio_spa_obj),
        )
        .unwrap()
        .0
        .into_inner();

        let mut audio_params = [Pod::from_bytes(&audio_spa_values).unwrap()];

        let default_sink_id = if !use_mic {
            get_default_sink_node_id()
        } else {
            Some(stream_node)
        };

        debug!("Default sink id: {:?}", default_sink_id);
        audio_stream.connect(
            Direction::Input,
            default_sink_id,
            StreamFlags::AUTOCONNECT | StreamFlags::MAP_BUFFERS | StreamFlags::RT_PROCESS,
            &mut audio_params,
        )?;

        debug!("Audio Stream: {:?}", audio_stream);

        pw_loop.run();

        Ok(Self { main_loop: pw_loop })
    }
}

fn get_default_sink_node_id() -> Option<u32> {
    let output = Command::new("sh")
        .arg("-c")
        .arg(r#"pactl list sinks | awk -v sink="$(pactl info | grep 'Default Sink' | cut -d' ' -f3)" '$0 ~ "Name: " sink { found=1 } found && /object.id/ { print $NF; exit }'"#)
        .output()
        .expect("Failed to execute command");

    let stdout = String::from_utf8_lossy(&output.stdout);

    let cleaned = stdout.replace('"', "");

    cleaned.trim().parse::<u32>().ok()
}

impl Drop for PipewireCapture {
    fn drop(&mut self) {
        self.main_loop.quit();

        unsafe {
            pw::deinit();
        }
    }
}

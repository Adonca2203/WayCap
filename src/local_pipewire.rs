use std::os::fd::{FromRawFd, OwnedFd, RawFd};

use log::{debug, error, info};
use pipewire::{
    self as pw,
    context::Context,
    main_loop::MainLoop,
    spa::utils::Direction,
    stream::{Stream, StreamFlags},
};
use pw::{properties::properties, spa};

use spa::pod::Pod;

#[derive(Default, Debug)]
pub struct Frame {}

pub struct PipewireCapture {
    main_loop: MainLoop,
}

struct UserData {
    format: spa::param::video::VideoInfoRaw,
}

impl PipewireCapture {
    pub fn new<F>(
        pipewire_fd: RawFd,
        stream_node: u32,
        callback: F,
    ) -> Result<Self, pipewire::Error>
    where
        F: Fn(Vec<u8>) + Send + 'static,
    {
        pw::init();
        let pw_loop = MainLoop::new(None)?;
        let pw_context = Context::new(&pw_loop)?;
        let core = pw_context.connect_fd(unsafe { OwnedFd::from_raw_fd(pipewire_fd) }, None)?;

        let mut data = UserData {
            format: Default::default(),
        };

        data.format.set_format(spa::param::video::VideoFormat::YUY2);

        let _listener = core
            .add_listener_local()
            .info(|i| info!("{0:#?}", i))
            .error(|e, f, g, h| error!("{0},{1},{2},{3}", e, f, g, h))
            .done(|d, _| info!("DONE: {0}", d))
            .register();

        let stream = Stream::new(
            &core,
            "test-screencap",
            properties! {
                *pw::keys::MEDIA_TYPE => "Video",
                *pw::keys::MEDIA_CATEGORY => "Capture",
                *pw::keys::MEDIA_ROLE => "Screen"
            },
        )?;
        debug!("Stream: {0:?}", stream);

        let _stream_listener = stream
            .add_local_listener_with_user_data(data)
            .state_changed(|_, _, old, new| debug!("State changed: {0:?} -> {1:?}", old, new))
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
                    .format
                    .parse(param)
                    .expect("Faield to parse param");

                println!("got video format:");
                println!(
                    "  format: {} ({:?})",
                    user_data.format.format().as_raw(),
                    user_data.format.format()
                );
                println!(
                    "  size: {}x{}",
                    user_data.format.size().width,
                    user_data.format.size().height
                );
                println!(
                    "  framerate: {}/{}",
                    user_data.format.framerate().num,
                    user_data.format.framerate().denom
                );
            })
            .process(move |stream, _| {
                match stream.dequeue_buffer() {
                    None => println!("out of buffers"),
                    Some(mut buffer) => {
                        let datas = buffer.datas_mut();
                        if datas.is_empty() {
                            return;
                        }

                        // copy frame data to screen
                        let data = &mut datas[0];
                        println!("got a frame of size {}", data.chunk().size());

                        callback(data.data().unwrap().to_vec());
                    }
                }
            })
            .register()?;

        let obj = pw::spa::pod::object!(
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
                pw::spa::param::video::VideoFormat::YUY2,
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
                pw::spa::utils::Fraction { num: 60, denom: 1 }, // Default
                pw::spa::utils::Fraction { num: 0, denom: 1 },  // Min
                pw::spa::utils::Fraction { num: 144, denom: 1 }  // Max
            ),
        );

        let values: Vec<u8> = pw::spa::pod::serialize::PodSerializer::serialize(
            std::io::Cursor::new(Vec::new()),
            &pw::spa::pod::Value::Object(obj),
        )
        .unwrap()
        .0
        .into_inner();

        let mut params = [Pod::from_bytes(&values).unwrap()];

        stream.connect(
            Direction::Input,
            Some(stream_node),
            StreamFlags::AUTOCONNECT | StreamFlags::MAP_BUFFERS,
            &mut params,
        )?;

        debug!("Stream: {0:?}", stream);

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

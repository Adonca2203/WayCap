use std::{cell::RefCell, ffi::CString, ptr::null_mut};

use crate::application_config::QualityPreset;
use anyhow::anyhow;
use drm_fourcc::DrmFourcc;
use ffmpeg_next::{
    self as ffmpeg,
    ffi::{
        av_buffer_create, av_buffer_default_free, av_buffer_ref, av_buffer_unref,
        av_hwdevice_ctx_create, av_hwframe_ctx_alloc, av_hwframe_ctx_init, av_hwframe_get_buffer,
        av_hwframe_transfer_data, AVBufferRef, AVDRMFrameDescriptor, AVHWDeviceContext,
        AVHWFramesContext, AVPixelFormat,
    },
    software::scaling::{Context as Scaler, Flags},
    Rational,
};
use log::error;
use ringbuf::{
    traits::{Producer, Split},
    HeapCons, HeapProd, HeapRb,
};

use super::{
    buffer::VideoFrameData,
    video_encoder::{VideoEncoder, GOP_SIZE},
};

thread_local! {
    static SCALER: RefCell<Option<Scaler>> = const { RefCell::new(None) };
}

pub struct VaapiEncoder {
    encoder: Option<ffmpeg::codec::encoder::Video>,
    width: u32,
    height: u32,
    encoder_name: String,
    quality: QualityPreset,
    encoded_frame_recv: Option<HeapCons<(i64, VideoFrameData)>>,
    encoded_frame_sender: Option<HeapProd<(i64, VideoFrameData)>>,
    filter_graph: Option<ffmpeg::filter::Graph>,
}

impl VideoEncoder for VaapiEncoder {
    fn new(width: u32, height: u32, quality: QualityPreset) -> anyhow::Result<Self>
    where
        Self: Sized,
    {
        let encoder_name = "h264_vaapi";
        let encoder = Self::create_encoder(width, height, encoder_name, &quality)?;
        let video_ring_buffer = HeapRb::<(i64, VideoFrameData)>::new(120);
        let (video_ring_sender, video_ring_receiver) = video_ring_buffer.split();
        let filter_graph = Some(Self::create_filter_graph(&encoder, width, height)?);

        Ok(Self {
            encoder: Some(encoder),
            width,
            height,
            encoder_name: encoder_name.to_string(),
            quality,
            encoded_frame_recv: Some(video_ring_receiver),
            encoded_frame_sender: Some(video_ring_sender),
            filter_graph,
        })
    }

    fn process(&mut self, frame: &crate::RawVideoFrame) -> Result<(), ffmpeg::Error> {
        if let Some(ref mut encoder) = self.encoder {
            if let Some(fd) = frame.dmabuf_fd {
                log::debug!(
                    "DMA Frame with fd: {}, size: {}, offset: {}, stride: {}",
                    fd,
                    frame.size,
                    frame.offset,
                    frame.stride
                );

                let mut drm_frame = ffmpeg::util::frame::Video::new(
                    ffmpeg_next::format::Pixel::DRM_PRIME,
                    encoder.width(),
                    encoder.height(),
                );
                unsafe {
                    // Create DRM descriptor that points to the DMA buffer
                    let drm_desc =
                        Box::into_raw(Box::new(std::mem::zeroed::<AVDRMFrameDescriptor>()));

                    (*drm_desc).nb_objects = 1;
                    (*drm_desc).objects[0].fd = fd;
                    (*drm_desc).objects[0].size = 0;
                    (*drm_desc).objects[0].format_modifier = 0;

                    (*drm_desc).nb_layers = 1;
                    (*drm_desc).layers[0].format = DrmFourcc::Argb8888 as u32;
                    (*drm_desc).layers[0].nb_planes = 1;
                    (*drm_desc).layers[0].planes[0].object_index = 0;
                    (*drm_desc).layers[0].planes[0].offset = frame.offset as isize;
                    (*drm_desc).layers[0].planes[0].pitch = frame.stride as isize;

                    // Attach descriptor to frame
                    (*drm_frame.as_mut_ptr()).data[0] = drm_desc as *mut u8;
                    (*drm_frame.as_mut_ptr()).buf[0] = av_buffer_create(
                        drm_desc as *mut u8,
                        std::mem::size_of::<AVDRMFrameDescriptor>(),
                        Some(av_buffer_default_free),
                        null_mut(),
                        0,
                    );

                    (*drm_frame.as_mut_ptr()).hw_frames_ctx =
                        av_buffer_ref((*encoder.as_ptr()).hw_frames_ctx);
                }

                drm_frame.set_pts(Some(*frame.get_timestamp()));
                self.filter_graph
                    .as_mut()
                    .unwrap()
                    .get("in")
                    .unwrap()
                    .source()
                    .add(&drm_frame)
                    .unwrap();

                let mut filtered = ffmpeg::util::frame::Video::empty();
                if self
                    .filter_graph
                    .as_mut()
                    .unwrap()
                    .get("out")
                    .unwrap()
                    .sink()
                    .frame(&mut filtered)
                    .is_ok()
                {
                    encoder.send_frame(&filtered)?;
                }
            } else {
                // Convert BGRA to NV12 then transfer it to a hw frame and send it to the
                // encoder
                //
                // TODO: deprecate this path?
                SCALER.with(|scaler_cell| {
                    let mut scaler = scaler_cell.borrow_mut();
                    if scaler.is_none() {
                        *scaler = Some(
                            Scaler::get(
                                ffmpeg::format::Pixel::BGRA,
                                encoder.width(),
                                encoder.height(),
                                ffmpeg::format::Pixel::NV12,
                                encoder.width(),
                                encoder.height(),
                                Flags::BILINEAR,
                            )
                            .unwrap(),
                        );
                    }

                    let mut src_frame = ffmpeg::util::frame::video::Video::new(
                        ffmpeg_next::format::Pixel::BGRA,
                        encoder.width(),
                        encoder.height(),
                    );
                    src_frame.data_mut(0).copy_from_slice(frame.get_bytes());

                    let mut dst_frame = ffmpeg::util::frame::Video::new(
                        ffmpeg::format::Pixel::NV12,
                        encoder.width(),
                        encoder.height(),
                    );

                    scaler
                        .as_mut()
                        .unwrap()
                        .run(&src_frame, &mut dst_frame)
                        .unwrap();

                    let mut vaapi_frame = ffmpeg::util::frame::video::Video::new(
                        encoder.format(),
                        encoder.width(),
                        encoder.height(),
                    );

                    unsafe {
                        let err = av_hwframe_get_buffer(
                            (*encoder.as_ptr()).hw_frames_ctx,
                            vaapi_frame.as_mut_ptr(),
                            0,
                        );

                        if err < 0 {
                            error!("Error getting the hw frame buffer: {:?}", err);
                        }

                        let err = av_hwframe_transfer_data(
                            vaapi_frame.as_mut_ptr(),
                            dst_frame.as_ptr(),
                            0,
                        );

                        if err < 0 {
                            error!("Error transferring the frame data to hw frame: {:?}", err);
                        }
                    }

                    vaapi_frame.set_pts(Some(*frame.get_timestamp()));

                    encoder.send_frame(&vaapi_frame).unwrap();
                });
            }

            let mut packet = ffmpeg::codec::packet::Packet::empty();
            if encoder.receive_packet(&mut packet).is_ok() {
                if let Some(data) = packet.data() {
                    if let Some(ref mut sender) = self.encoded_frame_sender {
                        if sender
                            .try_push((
                                packet.dts().unwrap_or(0),
                                VideoFrameData::new(
                                    data.to_vec(),
                                    packet.is_key(),
                                    packet.pts().unwrap_or(0),
                                ),
                            ))
                            .is_err()
                        {
                            log::error!("Could not send encoded packet to the ringbuf");
                        }
                    }
                };
            }
        }
        Ok(())
    }

    /// Drain the filter graph and encoder of any remaining frames it is processing
    fn drain(&mut self) -> Result<(), ffmpeg::Error> {
        if let Some(ref mut encoder) = self.encoder {
            // Drain the filter graph
            let mut filtered = ffmpeg::util::frame::Video::empty();
            while self
                .filter_graph
                .as_mut()
                .unwrap()
                .get("out")
                .unwrap()
                .sink()
                .frame(&mut filtered)
                .is_ok()
            {
                encoder.send_frame(&filtered)?;
            }

            // Drain encoder
            encoder.send_eof()?;
            let mut packet = ffmpeg::codec::packet::Packet::empty();
            while encoder.receive_packet(&mut packet).is_ok() {
                if let Some(data) = packet.data() {
                    if let Some(ref mut sender) = self.encoded_frame_sender {
                        if sender
                            .try_push((
                                packet.dts().unwrap_or(0),
                                VideoFrameData::new(
                                    data.to_vec(),
                                    packet.is_key(),
                                    packet.pts().unwrap_or(0),
                                ),
                            ))
                            .is_err()
                        {
                            log::error!("Could not send encoded packet to the ringbuf");
                        }
                    }
                };
                packet = ffmpeg::codec::packet::Packet::empty();
            }
        }
        Ok(())
    }

    fn reset(&mut self) -> anyhow::Result<()> {
        self.drop_encoder();
        let new_encoder =
            Self::create_encoder(self.width, self.height, &self.encoder_name, &self.quality)?;

        let new_filter_graph = Self::create_filter_graph(&new_encoder, self.width, self.height)?;

        self.encoder = Some(new_encoder);
        self.filter_graph = Some(new_filter_graph);
        Ok(())
    }

    fn get_encoder(&self) -> &Option<ffmpeg::codec::encoder::Video> {
        &self.encoder
    }

    fn drop_encoder(&mut self) {
        self.encoder.take();
        self.filter_graph.take();
    }

    fn take_encoded_recv(&mut self) -> Option<HeapCons<(i64, VideoFrameData)>> {
        self.encoded_frame_recv.take()
    }
}

impl VaapiEncoder {
    fn create_encoder(
        width: u32,
        height: u32,
        encoder: &str,
        quality: &QualityPreset,
    ) -> anyhow::Result<ffmpeg::codec::encoder::Video> {
        let encoder_codec =
            ffmpeg::codec::encoder::find_by_name(encoder).ok_or(ffmpeg::Error::EncoderNotFound)?;

        let mut encoder_ctx = ffmpeg::codec::context::Context::new_with_codec(encoder_codec)
            .encoder()
            .video()?;

        encoder_ctx.set_width(width);
        encoder_ctx.set_height(height);
        encoder_ctx.set_format(ffmpeg::format::Pixel::VAAPI);
        // Configuration inspiration from
        // https://git.dec05eba.com/gpu-screen-recorder/tree/src/capture/xcomposite_drm.c?id=8cbdb596ebf79587a432ed40583630b6cd39ed88
        let mut vaapi_device = Self::create_vaapi_device()?;
        let mut frame_ctx = Self::create_vaapi_frame_ctx(vaapi_device)?;

        unsafe {
            let hw_frame_context = &mut *((*frame_ctx).data as *mut AVHWFramesContext);
            hw_frame_context.width = width as i32;
            hw_frame_context.height = height as i32;
            hw_frame_context.sw_format = AVPixelFormat::AV_PIX_FMT_NV12;
            hw_frame_context.format = encoder_ctx.format().into();
            hw_frame_context.device_ref = av_buffer_ref(vaapi_device);
            hw_frame_context.device_ctx = (*vaapi_device).data as *mut AVHWDeviceContext;
            // Decides buffer size if we do not pop frame from the encoder we cannot
            // enqueue more than these many -- maybe adjust but for now setting it to
            // doble target fps
            hw_frame_context.initial_pool_size = 120;

            let err = av_hwframe_ctx_init(frame_ctx);
            if err < 0 {
                return Err(anyhow!(
                    "Error trying to initialize hw frame context: {:?}",
                    err
                ));
            }

            (*encoder_ctx.as_mut_ptr()).hw_device_ctx = av_buffer_ref(vaapi_device);
            (*encoder_ctx.as_mut_ptr()).hw_frames_ctx = av_buffer_ref(frame_ctx);

            av_buffer_unref(&mut vaapi_device);
            av_buffer_unref(&mut frame_ctx);
        }

        // These should be part of a config file
        encoder_ctx.set_time_base(Rational::new(1, 1_000_000));

        // Needed to insert I-Frames more frequently so we don't lose full seconds
        // when popping frames from the front
        encoder_ctx.set_gop(GOP_SIZE);

        let encoder_params = ffmpeg::codec::Parameters::new();

        let opts = Self::get_encoder_params(quality);

        encoder_ctx.set_parameters(encoder_params)?;
        let encoder = encoder_ctx.open_with(opts)?;
        Ok(encoder)
    }

    fn create_vaapi_frame_ctx(device: *mut AVBufferRef) -> anyhow::Result<*mut AVBufferRef> {
        unsafe {
            let frame = av_hwframe_ctx_alloc(device);

            if frame.is_null() {
                return Err(anyhow!("Could not create vaapi frame context"));
            }

            Ok(frame)
        }
    }

    fn create_vaapi_device() -> anyhow::Result<*mut AVBufferRef> {
        unsafe {
            let mut device: *mut AVBufferRef = null_mut();
            let device_path = CString::new("/dev/dri/renderD128").unwrap();
            let ret = av_hwdevice_ctx_create(
                &mut device,
                ffmpeg_next::ffi::AVHWDeviceType::AV_HWDEVICE_TYPE_VAAPI,
                device_path.as_ptr(),
                null_mut(),
                0,
            );
            if ret < 0 {
                return Err(anyhow!("Failed to create VAAPI device: Error code {}", ret));
            }

            Ok(device)
        }
    }

    fn get_encoder_params(quality: &QualityPreset) -> ffmpeg::Dictionary {
        let mut opts = ffmpeg::Dictionary::new();
        opts.set("vsync", "vfr");
        opts.set("rc", "VBR");
        match quality {
            QualityPreset::Low => {
                opts.set("qp", "30");
            }
            QualityPreset::Medium => {
                opts.set("qp", "25");
            }
            QualityPreset::High => {
                opts.set("qp", "20");
            }
            QualityPreset::Ultra => {
                opts.set("qp", "15");
            }
        }
        opts
    }

    fn create_filter_graph(
        encoder: &ffmpeg::codec::encoder::Video,
        width: u32,
        height: u32,
    ) -> anyhow::Result<ffmpeg::filter::Graph> {
        let mut graph = ffmpeg::filter::Graph::new();

        let args = format!(
            "video_size={}x{}:pix_fmt=bgra:time_base=1/1000000",
            width, height
        );

        let mut input = graph.add(&ffmpeg::filter::find("buffer").unwrap(), "in", &args)?;

        let mut hwmap = graph.add(
            &ffmpeg::filter::find("hwmap").unwrap(),
            "hwmap",
            "mode=read+write:derive_device=vaapi",
        )?;

        let scale_args = format!("w={}:h={}:format=nv12:out_range=tv", width, height);
        let mut scale = graph.add(
            &ffmpeg::filter::find("scale_vaapi").unwrap(),
            "scale",
            &scale_args,
        )?;

        let mut out = graph.add(&ffmpeg::filter::find("buffersink").unwrap(), "out", "")?;
        unsafe {
            let dev = (*encoder.as_ptr()).hw_device_ctx;

            (*hwmap.as_mut_ptr()).hw_device_ctx = av_buffer_ref(dev);
        }

        input.link(0, &mut hwmap, 0);
        hwmap.link(0, &mut scale, 0);
        scale.link(0, &mut out, 0);

        graph.validate()?;
        log::trace!("Graph\n{}", graph.dump());

        Ok(graph)
    }
}

impl Drop for VaapiEncoder {
    fn drop(&mut self) {
        if let Err(e) = self.drain() {
            log::error!("Error while draining vaapi encoder during drop: {:?}", e);
        }
        self.drop_encoder();
    }
}

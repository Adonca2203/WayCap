use std::{collections::BTreeMap, sync::Arc, thread::JoinHandle, time::Duration};

use crossbeam::channel::select;
use waycap_rs::types::video_frame::EncodedVideoFrame;

use crate::modes::AppMode;

pub struct RecordMode {
    /// writer thread handle that owns the ffmpeg Output
    writer_handle: Option<JoinHandle<()>>,
    recording: bool,
    file_name: String,
}

impl AppMode for RecordMode {
    async fn init(&mut self, ctx: &mut crate::app_context::AppContext) -> anyhow::Result<()> {
        self.file_name = format!(
            "{}_{}.mp4",
            ctx.hint.clone(),
            chrono::Local::now().timestamp()
        );

        let mut output = ffmpeg_next::format::output(&self.file_name)?;

        ctx.capture.with_video_encoder(|enc| {
            if let Some(encoder) = enc {
                let video_codec = encoder.codec().unwrap();
                let mut video_stream = output.add_stream(video_codec).unwrap();
                video_stream.set_time_base(encoder.time_base());
                video_stream.set_parameters(encoder);
            }
        });

        ctx.capture.with_audio_encoder(|enc| {
            if let Some(encoder) = enc {
                let audio_codec = encoder.codec().unwrap();
                let mut audio_stream = output.add_stream(audio_codec).unwrap();
                audio_stream.set_time_base(encoder.time_base());
                audio_stream.set_parameters(encoder);
            }
        });

        output.write_header()?;

        let video_recv = ctx.capture.get_video_receiver();
        let audio_recv = ctx.capture.get_audio_receiver()?;

        let stop = Arc::clone(&ctx.stop);

        let writer = std::thread::spawn(move || {
            let mut video_frames_buffer: BTreeMap<i64, EncodedVideoFrame> = BTreeMap::new();

            let mut first_video_pts: Option<i64> = None;
            let mut first_audio_pts: Option<i64> = None;

            loop {
                if stop.load(std::sync::atomic::Ordering::Acquire) {
                    while let Ok(encoded_frame) = video_recv.try_recv() {
                        video_frames_buffer.insert(encoded_frame.dts, encoded_frame);
                    }

                    while let Ok(encoded_frame) = audio_recv.try_recv() {
                        if first_audio_pts.is_none() {
                            first_audio_pts = Some(encoded_frame.pts);
                        }

                        let pts_offset = if let Some(pts) = first_audio_pts {
                            encoded_frame.pts - pts
                        } else {
                            0
                        };

                        let mut packet =
                            ffmpeg_next::codec::packet::Packet::copy(&encoded_frame.data);
                        packet.set_pts(Some(pts_offset));
                        packet.set_dts(Some(pts_offset));
                        packet.set_stream(1);

                        if let Err(e) = packet.write_interleaved(&mut output) {
                            log::error!("Error writing final audio packet: {:?}", e);
                        }
                    }

                    if !video_frames_buffer.is_empty() {
                        for frame in video_frames_buffer.values() {
                            let pts_offset = if let Some(pts) = first_video_pts {
                                frame.pts - pts
                            } else {
                                0
                            };

                            let dts_offset = if let Some(pts) = first_video_pts {
                                frame.dts - pts
                            } else {
                                0
                            };

                            let mut packet = ffmpeg_next::codec::packet::Packet::copy(&frame.data);
                            packet.set_pts(Some(pts_offset));
                            packet.set_dts(Some(dts_offset));
                            packet.set_stream(0);
                            if let Err(e) = packet.write_interleaved(&mut output) {
                                log::error!("Error writing final video packet: {:?}", e);
                            }
                        }
                        video_frames_buffer.clear();
                    }

                    // Finish saving file on stop
                    if let Err(e) = output.write_trailer() {
                        log::error!("Error writing trailer: {:?}", e);
                    }
                    break;
                }

                select! {
                    recv(video_recv) -> msg => match msg {
                        Ok(encoded_frame) => {
                            if first_video_pts.is_none() {
                                first_video_pts = Some(encoded_frame.pts);
                            }

                            if encoded_frame.is_keyframe && !video_frames_buffer.is_empty() {
                                for frame in video_frames_buffer.values() {
                                    let pts_offset = if let Some(pts) = first_video_pts {
                                        frame.pts - pts
                                    } else {
                                        0
                                    };

                                    let dts_offset = if let Some(pts) = first_video_pts {
                                        frame.dts - pts
                                    } else {
                                        0
                                    };

                                    let mut packet =
                                        ffmpeg_next::codec::packet::Packet::copy(&frame.data);
                                    packet.set_pts(Some(pts_offset));
                                    packet.set_dts(Some(dts_offset));
                                    packet.set_stream(0);
                                    if let Err(e) = packet.write_interleaved(&mut output) {
                                        log::error!("Could not write video packet: {:?}", e);
                                    }
                                }
                                video_frames_buffer.clear();
                            }
                            video_frames_buffer.insert(encoded_frame.dts, encoded_frame);
                        }
                        Err(_) => {
                            std::thread::sleep(Duration::from_millis(10));
                        }
                    },
                    recv(audio_recv) -> msg => match msg {
                        Ok(encoded_frame) => {
                            if first_audio_pts.is_none() {
                                first_audio_pts = Some(encoded_frame.pts);
                            }
                            let offset = if let Some(pts) = first_audio_pts {
                                encoded_frame.pts - pts
                            } else {
                                0
                            };

                            let mut packet =
                                ffmpeg_next::codec::packet::Packet::copy(&encoded_frame.data);
                            packet.set_pts(Some(offset));
                            packet.set_dts(Some(offset));
                            packet.set_stream(1);

                            if let Err(e) = packet.write_interleaved(&mut output) {
                                log::error!("Could not write audio packet: {:?}", e);
                            }
                        }
                        Err(_) => {
                            std::thread::sleep(Duration::from_millis(10));
                        }
                    },
                    default(Duration::from_millis(50)) => {
                        // no packets currently, loop to re-check stop flag
                    }
                }
            }
        });

        self.writer_handle = Some(writer);
        self.recording = true;

        Ok(())
    }

    async fn on_save(&mut self, ctx: &mut crate::app_context::AppContext) -> anyhow::Result<()> {
        ctx.saving.store(true, std::sync::atomic::Ordering::Release);
        ctx.stop.store(true, std::sync::atomic::Ordering::Release);
        ctx.capture.controls().pause();

        if let Some(handle) = self.writer_handle.take() {
            match handle.join() {
                Ok(_) => {}
                Err(e) => {
                    log::error!("Error joining writer thread: {e:?}");
                }
            }
        }

        self.recording = false;
        Ok(())
    }

    async fn on_exit(&mut self, ctx: &mut crate::app_context::AppContext) -> anyhow::Result<()> {
        ctx.stop.store(true, std::sync::atomic::Ordering::Release);
        ctx.capture.controls().pause();

        if let Some(handle) = self.writer_handle.take() {
            match handle.join() {
                Ok(_) => {}
                Err(e) => {
                    log::error!("Error joining writer thread during exit: {e:?}");
                }
            }
        }

        // If we were recording when we exited delete the file as we did not exit cleanly
        if std::path::Path::new(&self.file_name).exists() && self.recording {
            match std::fs::remove_file(&self.file_name) {
                Ok(_) => {
                    log::info!("Deleted incomplete recording file: {}", self.file_name);
                }
                Err(e) => {
                    log::error!(
                        "Failed to delete incomplete recording file {}: {}",
                        self.file_name,
                        e
                    );
                }
            }
        }
        Ok(())
    }
}

impl RecordMode {
    pub async fn new() -> Self {
        Self {
            writer_handle: None,
            recording: false,
            file_name: String::new(),
        }
    }
}

use ffmpeg_next::{self as ffmpeg, Rational};

pub fn create_video_encoder(
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

    encoder_ctx.set_width(width);
    encoder_ctx.set_height(height);
    encoder_ctx.set_format(ffmpeg::format::Pixel::NV12);
    encoder_ctx.set_frame_rate(Some(Rational::new(target_fps as i32, 1)));

    // These should be part of a config file
    encoder_ctx.set_bit_rate(12_000_000);
    encoder_ctx.set_max_bit_rate(16_000_000);
    encoder_ctx.set_time_base(Rational::new(1, 1_000_000));

    // Needed to insert I-Frames more frequently so we don't lose full seconds
    // when popping frames from the front
    encoder_ctx.set_gop(30);

    let encoder_params = ffmpeg::codec::Parameters::new();

    encoder_ctx.set_parameters(encoder_params)?;
    let encoder = encoder_ctx.open()?;

    Ok(encoder)
}

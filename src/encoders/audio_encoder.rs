use ffmpeg_next::{self as ffmpeg, Rational};

pub fn create_opus_encoder() -> Result<ffmpeg::codec::encoder::Audio, ffmpeg::Error> {
    let encoder_codec = ffmpeg::codec::encoder::find(ffmpeg_next::codec::Id::OPUS)
        .ok_or(ffmpeg::Error::EncoderNotFound)?;

    let mut encoder_ctx = ffmpeg::codec::context::Context::new_with_codec(encoder_codec)
        .encoder()
        .audio()?;

    encoder_ctx.set_rate(48000);
    encoder_ctx.set_bit_rate(128_000);
    encoder_ctx.set_format(ffmpeg::format::Sample::F32(
        ffmpeg_next::format::sample::Type::Packed,
    ));
    encoder_ctx.set_time_base(Rational::new(1, 48000));
    encoder_ctx.set_frame_rate(Some(Rational::new(1, 48000)));
    encoder_ctx.set_channel_layout(ffmpeg::channel_layout::ChannelLayout::STEREO);

    let mut encoder = encoder_ctx.open()?;

    // Opus frame size is based on n channels so need to update it
    unsafe {
        (*encoder.as_mut_ptr()).frame_size =
            (encoder.frame_size() as i32 * encoder.channels() as i32) as i32;
    }

    Ok(encoder)
}

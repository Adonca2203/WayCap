use waycap_rs::types::video_frame::EncodedVideoFrame;

use super::buffer::*;

fn new_video_frame(data: Vec<u8>, pts: i64, keyframe: bool, dts: i64) -> EncodedVideoFrame {
    EncodedVideoFrame {
        data,
        is_keyframe: keyframe,
        pts,
        dts,
    }
}

#[test]
fn test_video_frame_data_getters() {
    let video_frame_data = EncodedVideoFrame {
        data: vec![1],
        pts: 1,
        is_keyframe: true,
        dts: 1,
    };

    assert_eq!(video_frame_data.pts, 1);
    assert_eq!(video_frame_data.data, vec![1]);
    assert!(video_frame_data.is_keyframe);
}

#[test]
fn test_video_buffer_no_trim() {
    let mut buffer = ShadowCaptureVideoBuffer::new(10);

    buffer.insert(1, new_video_frame(vec![1], 1, true, 1));
    buffer.insert(2, new_video_frame(vec![2], 3, false, 3));
    buffer.insert(3, new_video_frame(vec![3], 6, true, 6));

    assert_eq!(*buffer.get_last_gop_start().unwrap(), 3);

    assert_eq!(buffer.get_frames().len(), 3);

    buffer.reset();

    assert!(buffer.get_frames().is_empty());
}

#[test]
fn test_video_buffer_trimming() {
    let mut buffer = ShadowCaptureVideoBuffer::new(10);

    // Insert frames directly without storing in array first
    buffer.insert(0, new_video_frame(vec![1], 0, true, 0));
    buffer.insert(1, new_video_frame(vec![1], 3, false, 3));
    buffer.insert(2, new_video_frame(vec![1], 5, false, 5));
    buffer.insert(3, new_video_frame(vec![1], 7, true, 7));
    buffer.insert(4, new_video_frame(vec![1], 9, false, 9));
    buffer.insert(5, new_video_frame(vec![1], 11, false, 11));
    // This keyframe (PTS 13) should become the oldest after trimming,
    // as it's the first keyframe after the PTS 9 cut-off.
    buffer.insert(6, new_video_frame(vec![1], 13, true, 13));
    buffer.insert(7, new_video_frame(vec![1], 15, false, 15));
    buffer.insert(8, new_video_frame(vec![1], 17, false, 17));
    buffer.insert(9, new_video_frame(vec![1], 19, true, 19));

    assert_eq!(buffer.get_frames().len(), 4);
    assert_eq!(*buffer.get_last_gop_start().unwrap(), 9);
    buffer.reset();
    assert!(buffer.get_frames().is_empty());
}

#[test]
fn test_video_buffer_stress_realistic() {
    // 5 second buffer at 60fps with variable GOP sizes
    let mut buffer = ShadowCaptureVideoBuffer::new(5_000_000);

    let mut dts = 0i64;
    let mut pts = 0i64;
    let frame_duration_us = 16667; // ~60fps (1/60 second in microseconds)
    let mut frames_inserted = 0;

    // Simulate 10 seconds of video
    for _ in 0..10 {
        for frame_in_second in 0..60 {
            // Keyframe every 30 frames (0.5 seconds at 60fps)
            let is_keyframe = frames_inserted % 30 == 0;

            // Simulate B-frames: some frames have PTS ahead of DTS
            let pts_offset = if !is_keyframe && frame_in_second % 4 == 2 {
                frame_duration_us * 2
            } else if !is_keyframe && frame_in_second % 4 == 3 {
                -frame_duration_us
            } else {
                0
            };

            let current_pts = pts + pts_offset;

            buffer.insert(
                dts,
                new_video_frame(
                    vec![1, 2, 3],
                    current_pts,
                    is_keyframe,
                    current_pts,
                ),
            );

            dts += frame_duration_us;
            pts += frame_duration_us;
            frames_inserted += 1;
        }
    }

    assert!(buffer.get_frames().len() <= 270); // 4.5 Seconds since we trimmed last GOP (30 frames)

    // Verify the time window is within bounds
    if let (Some(oldest), Some(newest)) = (buffer.oldest_pts(), buffer.newest_pts()) {
        let duration_us = newest - oldest;
        println!(
            "Buffer duration: {:.2} seconds",
            duration_us as f64 / 1_000_000.0
        );
        assert!(duration_us >= 4_400_000 && duration_us <= 4_600_000); // Should be close to max (~4.5 seconds)
    }

    assert!(buffer.get_last_gop_start().is_some());
}

#[test]
fn test_audio_buffer_no_trim() {
    let mut audio_buffer = ShadowCaptureAudioBuffer::new(10);
    let dummy_data = [
        (1, vec![1]),
        (2, vec![1]),
        (3, vec![1]),
        (4, vec![1]),
        (5, vec![1]),
    ];

    for (pts, data) in dummy_data {
        audio_buffer.insert(pts, data);
        audio_buffer.insert_capture_time(pts);
    }

    assert_eq!(*audio_buffer.get_capture_times(), vec![1, 2, 3, 4, 5]);
    assert_eq!(audio_buffer.get_frames().len(), 5);

    audio_buffer.reset();

    assert!(audio_buffer.get_capture_times().is_empty());
    assert!(audio_buffer.get_frames().is_empty());
}

#[test]
fn test_audio_buffer_trimming() {
    let mut audio_buffer = ShadowCaptureAudioBuffer::new(10);
    let dummy_data = [
        (1, vec![1]),
        (3, vec![1]),
        (5, vec![1]),
        (7, vec![1]),
        (9, vec![1]), // Should be our first frame
        (11, vec![1]),
        (13, vec![1]),
        (15, vec![1]),
        (17, vec![1]),
        (19, vec![1]),
    ];

    for (pts, data) in dummy_data {
        audio_buffer.insert(pts, data);
        audio_buffer.insert_capture_time(pts);
    }

    assert_eq!(audio_buffer.get_capture_times()[0], 9);
    assert_eq!(audio_buffer.get_frames().len(), 6);

    audio_buffer.reset();

    assert!(audio_buffer.get_capture_times().is_empty());
    assert!(audio_buffer.get_frames().is_empty());
}

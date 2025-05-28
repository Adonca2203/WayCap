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

    let newest_pts = buffer.newest_pts().unwrap();
    assert_eq!(newest_pts, 6);

    let oldest_pts = buffer.oldest_pts().unwrap();
    assert_eq!(oldest_pts, 1);

    assert_eq!(buffer.get_frames().len(), 3);

    buffer.reset();

    assert!(buffer.get_frames().is_empty());
    assert!(buffer.newest_pts().is_none());
    assert!(buffer.oldest_pts().is_none());
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

    let oldest = buffer.oldest_pts().unwrap();
    assert_eq!(oldest, 13);
    let newest = buffer.newest_pts().unwrap();
    assert_eq!(newest, 19);
    assert_eq!(buffer.get_frames().len(), 4);
    assert_eq!(*buffer.get_last_gop_start().unwrap(), 9);
    buffer.reset();
    assert!(buffer.get_frames().is_empty());
    assert!(buffer.newest_pts().is_none());
    assert!(buffer.oldest_pts().is_none());
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

use super::buffer::*;

#[test]
fn test_video_frame_data_getters() {
    let video_frame_data = VideoFrameData::new(vec![1], true, 1);

    assert_eq!(*video_frame_data.get_pts(), 1);
    assert_eq!(*video_frame_data.get_raw_bytes(), vec![1]);
    assert!(*video_frame_data.is_key());
}

#[test]
fn test_video_buffer_no_trim() {
    let dummy_frames = [
        VideoFrameData::new(vec![1], true, 1),
        VideoFrameData::new(vec![2], false, 3),
        VideoFrameData::new(vec![3], true, 6),
    ];

    let mut buffer = VideoBuffer::new(10);

    buffer.insert(1, dummy_frames[0].clone());
    buffer.insert(2, dummy_frames[1].clone());
    buffer.insert(3, dummy_frames[2].clone());

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
    let mut buffer = VideoBuffer::new(10);

    let dummy_frames = [
        VideoFrameData::new(vec![1], true, 0),
        VideoFrameData::new(vec![1], false, 3),
        VideoFrameData::new(vec![1], false, 5),
        VideoFrameData::new(vec![1], true, 7),
        VideoFrameData::new(vec![1], false, 9),
        VideoFrameData::new(vec![1], false, 11),
        // This keyframe (PTS 13) should become the oldest after trimming,
        // as it's the first keyframe after the PTS 9 cut-off.
        VideoFrameData::new(vec![1], true, 13),
        VideoFrameData::new(vec![1], false, 15),
        VideoFrameData::new(vec![1], false, 17),
        VideoFrameData::new(vec![1], true, 19),
    ];

    for (iter, frame) in dummy_frames.iter().enumerate() {
        buffer.insert(iter as i64, frame.clone());
    }

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
    let mut audio_buffer = AudioBuffer::new(10);
    let dummy_data = [
        (1, vec![1]),
        (2, vec![1]),
        (3, vec![1]),
        (4, vec![1]),
        (5, vec![1]),
    ];

    for (pts, data) in dummy_data {
        audio_buffer.insert_frame(pts, data);
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
    let mut audio_buffer = AudioBuffer::new(10);
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
        audio_buffer.insert_frame(pts, data);
        audio_buffer.insert_capture_time(pts);
    }

    assert_eq!(audio_buffer.get_capture_times()[0], 9);
    assert_eq!(audio_buffer.get_frames().len(), 6);

    audio_buffer.reset();

    assert!(audio_buffer.get_capture_times().is_empty());
    assert!(audio_buffer.get_frames().is_empty());
}

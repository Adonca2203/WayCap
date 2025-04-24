use std::collections::BTreeMap;

use log::warn;

/// Represents a single encoded video frame
#[derive(Clone, Debug)]
pub struct VideoFrameData {
    frame_bytes: Vec<u8>,
    pts: i64,
    is_key: bool,
}

impl VideoFrameData {
    pub fn new(frame_bytes: Vec<u8>, is_key: bool, pts: i64) -> Self {
        Self {
            frame_bytes,
            is_key,
            pts,
        }
    }

    pub fn get_raw_bytes(&self) -> &Vec<u8> {
        &self.frame_bytes
    }

    pub fn get_pts(&self) -> &i64 {
        &self.pts
    }

    pub fn is_key(&self) -> &bool {
        &self.is_key
    }
}

/// Rolling buffer which holds up to the last `max_time` seconds of video frames.
///
/// The buffer is ordered by decoding timestamp (DTS) and maintains complete GOPs (groups of pictures),
/// ensuring that no partial GOPs are kept when trimming for ease of muxing and playback.
#[derive(Clone)]
pub struct VideoBuffer {
    frames: BTreeMap<i64, VideoFrameData>,

    /// Maximum duration (in seconds) that the buffer should retain.
    /// Once the difference between the newest and oldest frame exceeds this, older GOPs are trimmed.
    max_time: usize,

    /// List of DTS values corresponding to key frames, ordered by insertion.
    /// Used to identify GOP boundaries for trimming purposes.
    key_frame_keys: Vec<i64>,
}

impl VideoBuffer {
    /// Creates a new `FrameBuffer` with a specified maximum duration.
    ///
    /// # Arguments
    ///
    /// * `max_time` - Maximum duration (in seconds) of video frames to retain in the buffer.
    pub fn new(max_time: usize) -> Self {
        Self {
            frames: BTreeMap::new(),
            max_time,
            key_frame_keys: Vec::new(),
        }
    }

    /// Inserts a new video frame into the buffer, keeping the buffer size within `max_time`.
    ///
    /// If the inserted frame is a key frame, its timestamp is recorded to track GOP boundaries.
    /// After insertion, older frames are trimmed if the total duration exceeds `max_time`.
    ///
    /// # Arguments
    ///
    /// * `timestamp` - The decoding timestamp (DTS) of the frame.
    /// * `frame` - A [`VideoFrameData`] representing an encoded frame.
    pub fn insert(&mut self, timestamp: i64, frame: VideoFrameData) {
        if frame.is_key {
            self.key_frame_keys.push(timestamp);
        }

        self.frames.insert(timestamp, frame);

        // Trim old GOPs if buffer exceeds max_time
        while let (Some(oldest), Some(newest)) = (self.oldest_pts(), self.newest_pts()) {
            if newest - oldest >= self.max_time as i64 {
                self.trim_oldest_gop();
            } else {
                break;
            }
        }
    }

    /// Returns the presentation timestamp (PTS) of the newest frame in the buffer.
    ///
    /// Returns `None` if the buffer is empty.
    pub fn newest_pts(&self) -> Option<i64> {
        self.frames.values().map(|frame| frame.pts).max()
    }

    /// Returns the presentation timestamp (PTS) of the oldest frame in the buffer.
    ///
    /// Returns `None` if the buffer is empty.
    pub fn oldest_pts(&self) -> Option<i64> {
        self.frames.values().map(|frame| frame.pts).min()
    }

    /// Returns the decoding timestamp (DTS) of the most recent key frame (start of the last GOP).
    ///
    /// Returns `None` if no key frames have been inserted.
    pub fn get_last_gop_start(&self) -> Option<&i64> {
        self.key_frame_keys.last()
    }

    /// Removes the oldest group of pictures (GOP) from the buffer.
    ///
    /// A GOP is considered complete when there is at least one subsequent key frame.
    /// This method trims all frames up to (but not including) the second key frame,
    /// ensuring only complete GOPs are retained.
    ///
    /// Does nothing if there is only one or no key frame recorded.
    fn trim_oldest_gop(&mut self) {
        if self.key_frame_keys.len() <= 1 {
            warn!("Tried to remove oldest GOP without enough keyframes to determine the range. Is the max time too low?");
            return;
        }

        let stop_dts = self.key_frame_keys[1]; // First complete GOP ends at second key frame

        let mut dts_to_remove = Vec::new();
        for (&pts, _) in self.frames.range(..stop_dts) {
            dts_to_remove.push(pts);
        }

        for dts in dts_to_remove {
            self.frames.remove(&dts);
        }

        // Remove deleted key frame
        self.key_frame_keys.remove(0);
    }

    pub fn get_frames(&self) -> &BTreeMap<i64, VideoFrameData> {
        &self.frames
    }

    pub fn reset(&mut self) {
        self.frames.clear();
        self.key_frame_keys.clear();
    }
}

#[derive(Clone)]
pub struct AudioBuffer {
    frames: BTreeMap<i64, Vec<u8>>,

    /// Maximum duration (in seconds) that the buffer should retain.
    /// Once the difference between the newest and oldest frame exceeds this, older GOPs are trimmed.
    max_time: usize,

    capture_times: Vec<i64>,
}

impl AudioBuffer {
    pub fn new(max_time: usize) -> Self {
        Self {
            frames: BTreeMap::new(),
            max_time,
            capture_times: Vec::new(),
        }
    }

    /// Inserts a new audio frame into the buffer, keeping the buffer size within `max_time`.
    ///
    /// It converts the encoder PTS into real world micro seconds to keep track of elapsed time
    ///
    /// # Arguments
    ///
    /// * `timestamp` - The presentation timestamp (PTS) of the frame according to the audio
    ///   encoder.
    /// * `frame` - A [`AudioFrameData`] representing an encoded frame.
    pub fn insert_frame(&mut self, timestamp: i64, frame: Vec<u8>) {
        self.frames.insert(timestamp, frame);

        while let (Some(oldest), Some(newest)) =
            (self.capture_times.first(), self.capture_times.last())
        {
            if newest - oldest >= self.max_time as i64 {
                if let Some(oldest_frame) = self.frames.first_entry() {
                    oldest_frame.remove();
                    self.capture_times.remove(0);
                }
            } else {
                break;
            }
        }
    }

    pub fn get_capture_times(&self) -> &Vec<i64> {
        &self.capture_times
    }

    pub fn get_frames(&self) -> &BTreeMap<i64, Vec<u8>> {
        &self.frames
    }

    pub fn insert_capture_time(&mut self, time: i64) {
        self.capture_times.push(time);
    }

    pub fn reset(&mut self) {
        self.frames.clear();
        self.capture_times.clear();
    }
}

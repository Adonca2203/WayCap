use std::collections::BTreeMap;

use waycap_rs::types::video_frame::EncodedVideoFrame;

/// Represents a time window between Presentation Time Stamps.
/// Used in Shadow Buffers to cache
struct TimeWindow {
    min_time: Option<i64>,
    max_time: Option<i64>,
}

impl TimeWindow {
    pub fn new() -> Self {
        Self {
            min_time: None,
            max_time: None,
        }
    }

    pub fn insert_time(&mut self, time: i64) {
        self.min_time = Some(self.min_time.map_or(time, |min| min.min(time)));
        self.max_time = Some(self.max_time.map_or(time, |max| max.max(time)));
    }

    pub fn get_elapsed(&self) -> Option<i64> {
        match (self.min_time, self.max_time) {
            (Some(min), Some(max)) => Some(max - min),
            _ => None,
        }
    }

    pub fn reset(&mut self) {
        self.min_time = None;
        self.max_time = None;
    }
}

/// Rolling buffer which holds up to the last `max_time` seconds of video frames.
///
/// The buffer is ordered by decoding timestamp (DTS) and maintains complete GOPs (groups of pictures),
/// ensuring that no partial GOPs are kept when trimming for ease of muxing and playback.
pub struct ShadowCaptureVideoBuffer {
    frames: BTreeMap<i64, EncodedVideoFrame>,

    /// Maximum duration (in seconds) that the buffer should retain.
    /// Once the difference between the newest and oldest frame exceeds this, older GOPs are trimmed.
    max_time: usize,

    /// List of DTS values corresponding to key frames, ordered by insertion.
    /// Used to identify GOP boundaries for trimming purposes.
    key_frame_keys: Vec<i64>,

    /// Cached time window. Updated every call to `trim_oldest_gop()`
    time_window: TimeWindow,
}

impl ShadowCaptureVideoBuffer {
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
            time_window: TimeWindow::new(),
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
    pub fn insert(&mut self, timestamp: i64, frame: EncodedVideoFrame) {
        if frame.is_keyframe {
            self.key_frame_keys.push(timestamp);
        }

        self.time_window.insert_time(frame.pts);
        self.frames.insert(timestamp, frame);

        // Trim old GOPs if buffer exceeds max_time
        match self.time_window.get_elapsed() {
            Some(elapsed) => {
                if elapsed >= self.max_time as i64 {
                    self.trim_oldest_gop();
                }
            }
            None => {
                log::warn!("Time window imporperly set");
            }
        }
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
            log::warn!("Tried to remove oldest GOP without enough keyframes to determine the range. Is the max time too low?");
            return;
        }

        let stop_dts = self.key_frame_keys[1]; // First complete GOP ends at second key frame

        self.frames.retain(|&dts, _| dts >= stop_dts);

        // Remove deleted key frame
        self.key_frame_keys.remove(0);
        self.recalculate_pts();
    }

    #[cfg(test)]
    pub fn oldest_pts(&self) -> Option<i64> {
        self.time_window.min_time
    }

    #[cfg(test)]
    pub fn newest_pts(&self) -> Option<i64> {
        self.time_window.max_time
    }

    fn recalculate_pts(&mut self) {
        self.time_window.reset();

        for frame in self.frames.values() {
            self.time_window.insert_time(frame.pts);
        }
    }

    pub fn get_frames(&self) -> &BTreeMap<i64, EncodedVideoFrame> {
        &self.frames
    }

    pub fn reset(&mut self) {
        self.frames.clear();
        self.key_frame_keys.clear();
    }
}

#[derive(Clone)]
pub struct ShadowCaptureAudioBuffer {
    frames: BTreeMap<i64, Vec<u8>>,

    /// Maximum duration (in seconds) that the buffer should retain.
    /// Once the difference between the newest and oldest frame exceeds this, older GOPs are trimmed.
    max_time: usize,

    capture_times: Vec<i64>,
}

impl ShadowCaptureAudioBuffer {
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
    pub fn insert(&mut self, timestamp: i64, frame: Vec<u8>) {
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

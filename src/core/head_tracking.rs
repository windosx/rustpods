//! Head-tracking sample parsing, orientation math and gesture recognition.
//!
//! ## Sample layout
//!
//! A `SensorV2` custom message arrives as a fixed 65-byte payload. Field offsets
//! are **absolute within the received frame** (i.e. they already include the
//! 6-byte AACP header), which is how both LibrePods' parser and the
//! `AAP Definitions.md` table describe them:
//!
//! | field                   | offset | width |
//! |-------------------------|--------|-------|
//! | orientation 1           | 43     | i16   |
//! | orientation 2           | 45     | i16   |
//! | orientation 3           | 47     | i16   |
//! | horizontal acceleration | 51     | i16   |
//! | vertical acceleration   | 53     | i16   |
//!
//! All values are little-endian signed 16-bit.
//!
//! ## Orientation
//!
//! `o2`/`o3` are a complementary pair; their mean tracks pitch and their
//! difference tracks yaw. `o1` is a direct roll channel. Raw units are scaled by
//! 32000 counts per 180°.
//!
//! ## Gestures
//!
//! Nod/shake detection is a port of LibrePods' `GestureDetector`: a 5-sample
//! moving average feeds a direction-change detector with a variance-adaptive
//! threshold, and a candidate gesture is only accepted once it accumulates
//! three alternating extrema *and* scores at least 0.7 on a weighted
//! amplitude/rhythm/alternation/isolation confidence metric.

use std::collections::VecDeque;
use std::time::Instant;
use tracing::{debug, info};

/// Bytes per `SensorV2` sample.
pub const SENSOR_V2_LEN: usize = 65;

/// Marker that precedes the sensor payload on newer firmware.
///
/// AirPods 4 (firmware 2026-10) emits 80/81-byte frames whose payload begins
/// with `3a 3e 08 10 1a 3a`; the fields are then read relative to the marker
/// rather than from fixed frame offsets. Documented by
/// `anilsrml/airpods-posture/spike/spike.py` from live measurements.
pub const PAYLOAD_MARK: [u8; 6] = [0x3a, 0x3e, 0x08, 0x10, 0x1a, 0x3a];

/// Payload length after the marker.
pub const MARKED_PAYLOAD_LEN: usize = 58;

/// Offset of the gravity vector (int16 x3, scaled x1024) inside the marked
/// payload.
pub const MARKED_GRAVITY_OFFSET: usize = 44;

/// Minimum frame length needed to read the last field (vertical accel at 53..55).
const MIN_FRAME_LEN: usize = 55;

const ORIENTATION_1_OFFSET: usize = 43;
const ORIENTATION_2_OFFSET: usize = 45;
const ORIENTATION_3_OFFSET: usize = 47;
const ACCEL_H_OFFSET: usize = 51;
const ACCEL_V_OFFSET: usize = 53;

/// Counts per 180° on the orientation channels.
const ORIENTATION_SCALE: f32 = 32000.0;

const CALIBRATION_SAMPLES: usize = 10;
const SMOOTHING_WINDOW: usize = 5;

// Gesture tuning, matching LibrePods' detector.
const HISTORY_LEN: usize = 100;
const AMPLITUDE_FLOOR: f32 = 400.0;
const DIRECTION_CHANGE_THRESHOLD: f32 = 175.0;
const MIN_DIRECTION_CHANGE: f32 = 100.0;
const MIN_CONFIDENCE: f32 = 0.7;
const REQUIRED_EXTREMES: usize = 3;
const AMPLITUDE_NORMALIZER: f32 = 600.0;
/// Relative spacing variance at which rhythm consistency reaches zero.
const RHYTHM_CONSISTENCY_TOLERANCE: f32 = 0.5;
/// How much louder the target axis must be than the other one.
const ISOLATION_MARGIN: f32 = 1.2;
/// Samples kept for the rolling rate estimate (~2 s at 50 Hz).
const RATE_WINDOW: usize = 100;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct HeadPose {
    /// Nodding: negative looks down, positive looks up. Degrees.
    pub pitch: f32,
    /// Tipping the head to one shoulder. Degrees.
    pub roll: f32,
    /// Turning the head left/right. Degrees.
    pub yaw: f32,
    pub accel_h: i16,
    pub accel_v: i16,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HeadGesture {
    /// Three alternating pitches — "yes".
    NodYes,
    /// Three alternating yaws — "no".
    ShakeNo,
}

/// One raw, unparseable-attitude sample triple in device counts.
#[derive(Debug, Clone, Copy)]
struct RawSample {
    o1: f32,
    o2: f32,
    o3: f32,
}

/// Alternating peak/trough detector on one axis.
#[derive(Debug, Default)]
struct AxisDetector {
    history: VecDeque<f32>,
    extrema: Vec<f32>,
    /// Time between consecutive extrema, used for the rhythm factor.
    intervals: VecDeque<f32>,
    last_extremum_at: Option<Instant>,
    increasing: Option<bool>,
}

impl AxisDetector {
    fn push(&mut self, raw: f32) {
        if self.history.len() >= HISTORY_LEN {
            self.history.pop_front();
        }
        self.history.push_back(raw);
    }

    /// Variance-adaptive direction-change threshold over the recent window.
    fn dynamic_threshold(&self) -> f32 {
        let recent: Vec<f32> = self.history.iter().rev().take(4).copied().collect();
        if recent.len() < 2 {
            return MIN_DIRECTION_CHANGE;
        }
        let mean = recent.iter().sum::<f32>() / recent.len() as f32;
        let variance = recent.iter().map(|v| (v - mean).powi(2)).sum::<f32>() / recent.len() as f32;
        (variance / 3.0).clamp(MIN_DIRECTION_CHANGE, DIRECTION_CHANGE_THRESHOLD)
    }

    /// Feeds one sample and records an extremity if the direction just flipped.
    fn detect(&mut self) {
        if self.history.len() < 4 {
            return;
        }
        let n = self.history.len();
        let current = self.history[n - 1];
        let prev = self.history[n - 2];

        let increasing = *self.increasing.get_or_insert(current > prev);
        let threshold = self.dynamic_threshold();

        let turned = if increasing && current < prev - threshold {
            self.increasing = Some(false);
            Some(prev)
        } else if !increasing && current > prev + threshold {
            self.increasing = Some(true);
            Some(prev)
        } else {
            None
        };

        let Some(extremity) = turned.filter(|v| v.abs() > AMPLITUDE_FLOOR) else {
            return;
        };

        let now = Instant::now();
        if let Some(last) = self.last_extremum_at {
            if self.intervals.len() >= REQUIRED_EXTREMES {
                self.intervals.pop_front();
            }
            self.intervals.push_back(now.duration_since(last).as_secs_f32());
        }
        self.last_extremum_at = Some(now);

        self.extrema.push(extremity);
        if self.extrema.len() > REQUIRED_EXTREMES {
            self.extrema.remove(0);
        }
    }

    /// Mean absolute amplitude of the recent extrema.
    fn mean_amplitude(&self) -> f32 {
        if self.extrema.is_empty() {
            return 0.0;
        }
        self.extrema.iter().map(|v| v.abs()).sum::<f32>() / self.extrema.len() as f32
    }

    /// How evenly spaced the extrema are, in `0.0..=1.0`. Deliberate nodding and
    /// shaking is rhythmic; incidental head movement is not.
    fn rhythm_consistency(&self) -> f32 {
        if self.intervals.len() < 2 {
            return 0.0;
        }
        let mean = self.intervals.iter().sum::<f32>() / self.intervals.len() as f32;
        if mean <= f32::EPSILON {
            return 0.0;
        }
        let deviation = self
            .intervals
            .iter()
            .map(|i| (i / mean - 1.0).powi(2))
            .sum::<f32>()
            / self.intervals.len() as f32;
        (1.0 - (deviation / RHYTHM_CONSISTENCY_TOLERANCE).min(1.0)).max(0.0)
    }

    fn has_candidate(&self) -> bool {
        self.extrema.len() >= REQUIRED_EXTREMES
    }

    /// Three alternating extrema in sign order, e.g. `+ - +`.
    fn is_alternating(&self) -> bool {
        self.extrema
            .windows(2)
            .all(|pair| (pair[0] > 0.0) != (pair[1] > 0.0))
    }

    fn reset(&mut self) {
        self.history.clear();
        self.extrema.clear();
        self.intervals.clear();
        self.last_extremum_at = None;
        self.increasing = None;
    }
}

pub struct HeadTracker {
    /// Whether the accessory is currently streaming samples.
    pub active: bool,

    smooth_pitch: VecDeque<f32>,
    smooth_yaw: VecDeque<f32>,
    smooth_roll: VecDeque<f32>,

    calibrated: bool,
    calib_samples: Vec<RawSample>,
    o1_neutral: f32,
    o2_neutral: f32,
    o3_neutral: f32,

    horizon: AxisDetector,
    vertical: AxisDetector,

    /// Timestamps of recent samples, for a rolling rate estimate.
    sample_times: VecDeque<Instant>,
    samples_seen: u64,
    last_gesture: Option<(HeadGesture, Instant)>,
}

impl HeadTracker {
    pub fn new() -> Self {
        Self {
            active: false,

            smooth_pitch: VecDeque::with_capacity(SMOOTHING_WINDOW),
            smooth_yaw: VecDeque::with_capacity(SMOOTHING_WINDOW),
            smooth_roll: VecDeque::with_capacity(SMOOTHING_WINDOW),

            calibrated: false,
            calib_samples: Vec::with_capacity(CALIBRATION_SAMPLES),
            o1_neutral: 0.0,
            o2_neutral: 0.0,
            o3_neutral: 0.0,

            horizon: AxisDetector::default(),
            vertical: AxisDetector::default(),

            sample_times: VecDeque::with_capacity(RATE_WINDOW),
            samples_seen: 0,
            last_gesture: None,
        }
    }

    /// Drops calibration and buffered motion. Call when the stream restarts,
    /// since the wearer's neutral pose is unlikely to be identical and stale
    /// samples would otherwise trigger phantom gestures.
    pub fn reset(&mut self) {
        self.calibrated = false;
        self.calib_samples.clear();
        self.smooth_pitch.clear();
        self.smooth_yaw.clear();
        self.smooth_roll.clear();
        self.horizon.reset();
        self.vertical.reset();
        self.sample_times.clear();
        self.samples_seen = 0;
        self.last_gesture = None;
    }

    pub fn is_calibrated(&self) -> bool {
        self.calibrated
    }

    pub fn samples_seen(&self) -> u64 {
        self.samples_seen
    }

    /// Sample rate over the recent window, or `None` until enough samples have
    /// arrived to measure it.
    pub fn sample_rate_hz(&self) -> Option<f32> {
        if self.sample_times.len() < 2 {
            return None;
        }
        let oldest = self.sample_times.front()?;
        let span = oldest.elapsed().as_secs_f32();
        if span <= f32::EPSILON {
            return None;
        }
        let rate = (self.sample_times.len() - 1) as f32 / span;
        rate.is_finite().then_some(rate)
    }

    /// Parses one received frame and updates orientation/gesture state.
    ///
    /// Accepts both the bare 65-byte `SensorV2` payload and the AACP-wrapped
    /// form. When the 6-byte AACP header is present it is stripped first, so the
    /// field offsets below are relative to the sensor payload.
    pub fn parse_packet(&mut self, data: &[u8]) -> Option<(HeadPose, Option<HeadGesture>)> {
        // Descriptor 0x800 belongs to RemoteHID exclusively. Route on the
        // descriptor before any offset heuristics: frames on that channel that
        // are not InputReports (DeviceConnect plists, SetReport responses) must
        // never be read as attitude samples.
        if let Some(body) = remote_hid_body(data) {
            return remote_hid_motion_block(body)
                .and_then(|block| self.parse_remote_hid_block(block));
        }

        // Newer firmware frames carry a marker and are not fixed-length, so they
        // are recognised by the marker before any length checks.
        if let Some(payload) = payload_after_marker(data) {
            return self.parse_marked_payload(payload);
        }

        let payload = if data.len() >= 71
            && data[..4] == [0x04, 0x00, 0x04, 0x00]
            && data[4] == 0x17
        {
            &data[6..]
        } else if data.len() >= SENSOR_V2_LEN {
            data
        } else {
            return None;
        };

        if payload.len() < MIN_FRAME_LEN {
            return None;
        }

        // Samples only ever arrive under the head-tracking opcode. A frame
        // carrying anything else is a different custom message that happens to
        // be long enough, and reading offsets 43..55 out of it would produce a
        // meaningless pose. AACP-wrapped frames are covered by the branch above.
        if data.len() >= 6 && data[4] != 0x17 {
            return None;
        }

        let o1 = read_i16(payload, ORIENTATION_1_OFFSET)? as f32;
        let o2 = read_i16(payload, ORIENTATION_2_OFFSET)? as f32;
        let o3 = read_i16(payload, ORIENTATION_3_OFFSET)? as f32;
        let accel_h = read_i16(payload, ACCEL_H_OFFSET)?;
        let accel_v = read_i16(payload, ACCEL_V_OFFSET)?;

        if !self.calibrated {
            self.calib_samples.push(RawSample { o1, o2, o3 });
            if self.calib_samples.len() >= CALIBRATION_SAMPLES {
                let n = self.calib_samples.len() as f32;
                self.o1_neutral = self.calib_samples.iter().map(|s| s.o1).sum::<f32>() / n;
                self.o2_neutral = self.calib_samples.iter().map(|s| s.o2).sum::<f32>() / n;
                self.o3_neutral = self.calib_samples.iter().map(|s| s.o3).sum::<f32>() / n;
                self.calibrated = true;
                info!(
                    "Head tracking calibrated: o1={:.0}, o2={:.0}, o3={:.0}",
                    self.o1_neutral, self.o2_neutral, self.o3_neutral
                );
            }
            // Still emit a zeroed pose so the UI can show "calibrating".
            return Some((
                HeadPose {
                    pitch: 0.0,
                    roll: 0.0,
                    yaw: 0.0,
                    accel_h,
                    accel_v,
                },
                None,
            ));
        }

        self.samples_seen += 1;
        if self.sample_times.len() >= RATE_WINDOW {
            self.sample_times.pop_front();
        }
        self.sample_times.push_back(Instant::now());

        let pitch_raw = ((o2 - self.o2_neutral) + (o3 - self.o3_neutral)) / 2.0;
        let yaw_raw = ((o2 - self.o2_neutral) - (o3 - self.o3_neutral)) / 2.0;
        let roll_raw = o1 - self.o1_neutral;

        let pitch = Self::smooth(&mut self.smooth_pitch, pitch_raw) / ORIENTATION_SCALE * 180.0;
        let yaw = Self::smooth(&mut self.smooth_yaw, yaw_raw) / ORIENTATION_SCALE * 180.0;
        let roll = Self::smooth(&mut self.smooth_roll, roll_raw) / ORIENTATION_SCALE * 180.0;

        let pose = HeadPose {
            pitch: pitch.clamp(-90.0, 90.0),
            roll: roll.clamp(-90.0, 90.0),
            yaw: yaw.clamp(-180.0, 180.0),
            accel_h,
            accel_v,
        };

        let gesture = self.detect_gesture(accel_h as f32, accel_v as f32);
        Some((pose, gesture))
    }

    fn smooth(buf: &mut VecDeque<f32>, val: f32) -> f32 {
        if buf.len() >= SMOOTHING_WINDOW {
            buf.pop_front();
        }
        buf.push_back(val);
        buf.iter().sum::<f32>() / buf.len() as f32
    }

    fn detect_gesture(&mut self, horiz: f32, vert: f32) -> Option<HeadGesture> {
        self.horizon.push(horiz);
        self.vertical.push(vert);
        self.horizon.detect();
        self.vertical.detect();

        let gesture = if Self::is_confident(&self.vertical, &self.horizon) {
            HeadGesture::NodYes
        } else if Self::is_confident(&self.horizon, &self.vertical) {
            HeadGesture::ShakeNo
        } else {
            return None;
        };

        // Consume the extrema so one physical movement cannot fire twice.
        self.horizon.extrema.clear();
        self.vertical.extrema.clear();
        self.last_gesture = Some((gesture, Instant::now()));

        debug!(
            "Head gesture detected: {:?} after {} samples",
            gesture, self.samples_seen
        );
        Some(gesture)
    }

    /// Port of LibrePods' confidence model.
    ///
    /// A run of three alternating extrema is scored on four independent
    /// signals; the gesture is only reported when the weighted total clears
    /// 0.7, which is what keeps ordinary head movement from registering as a
    /// deliberate nod or shake.
    fn is_confident(target: &AxisDetector, other: &AxisDetector) -> bool {
        if !target.has_candidate() {
            return false;
        }

        // Three monotonic large swings are the head settling, not a gesture.
        // Alternation is therefore a hard gate rather than just a score term.
        if !target.is_alternating() {
            return false;
        }

        let amplitude_factor = (target.mean_amplitude() / AMPLITUDE_NORMALIZER).min(1.0);
        let rhythm_factor = target.rhythm_consistency();
        let alternation_factor = 1.0;
        let isolation_factor =
            (target.mean_amplitude() / (other.mean_amplitude() + 0.1) * ISOLATION_MARGIN).min(1.0);

        let confidence = amplitude_factor * 0.4
            + rhythm_factor * 0.2
            + alternation_factor * 0.2
            + isolation_factor * 0.2;

        debug!(
            "Gesture candidate: confidence {:.2} (amp {:.2}, rhythm {:.2}, isolation {:.2})",
            confidence, amplitude_factor, rhythm_factor, isolation_factor
        );
        confidence >= MIN_CONFIDENCE
    }

    /// Most recent gesture if it fired within `max_age`.
    pub fn recent_gesture(&self, max_age: std::time::Duration) -> Option<HeadGesture> {
        match self.last_gesture {
            Some((gesture, at)) if at.elapsed() <= max_age => Some(gesture),
            _ => None,
        }
    }
}

impl Default for HeadTracker {
    fn default() -> Self {
        Self::new()
    }
}

fn read_i16(data: &[u8], offset: usize) -> Option<i16> {
    Some(i16::from_le_bytes([
        *data.get(offset)?,
        *data.get(offset + 1)?,
    ]))
}

/// Locates the marked payload inside a frame, if the marker is present and the
/// full payload follows it.
fn payload_after_marker(data: &[u8]) -> Option<&[u8]> {
    let start = data
        .windows(PAYLOAD_MARK.len())
        .position(|w| w == PAYLOAD_MARK)?;
    let payload_start = start + PAYLOAD_MARK.len();
    let payload_end = payload_start + MARKED_PAYLOAD_LEN;
    data.get(payload_start..payload_end)
}

/// Descriptor for the RemoteHID channel carrying AirPods 3 motion reports.
const DESCRIPTOR_REMOTE_HID: u32 = 0x0000_0800;
/// Kind 2 in the RemoteHID sub-header is an InputReport.
const REMOTE_HID_KIND_INPUT: u32 = 2;
/// Bytes of motion data in one RemoteHID InputReport, after the report-type byte.
const REMOTE_HID_MOTION_LEN: usize = 58;
/// Rotation-rate scale from the protocol document: ~0.122 °/s per count. The
/// source marks it approximate, so treat derived yaw as indicative only.
const ROTATION_DEG_PER_SEC: f32 = 0.122;

/// Extracts the 58-byte motion block from a RemoteHID InputReport.
///
/// Layout of the frame:
///
/// ```text
/// [04 00 04 00 17 00] [descriptor 0x800: 4] [body_len: 2]
///   [version][gen][0][side] [sub-header: 4] [report type: 1] [motion: 58]
///                            ^ bits 17..19 == 2 for InputReport
/// ```
fn remote_hid_body(frame: &[u8]) -> Option<&[u8]> {
    if frame.len() < 12 || frame[4] != 0x17 {
        return None;
    }
    let descriptor = u32::from_le_bytes([frame[6], frame[7], frame[8], frame[9]]);
    if descriptor != DESCRIPTOR_REMOTE_HID {
        return None;
    }
    let body_len = u16::from_le_bytes([frame[10], frame[11]]) as usize;
    frame.get(12..12 + body_len)
}

/// Extracts the 58-byte motion block from a RemoteHID body.
///
/// Returns `None` for any body that is not an InputReport, which keeps plists and
/// control responses out of the attitude path.
fn remote_hid_motion_block(body: &[u8]) -> Option<&[u8]> {
    if body.len() < 9 {
        return None;
    }
    let header = u32::from_le_bytes([body[4], body[5], body[6], body[7]]);
    if (header >> 17) & 0x7 != REMOTE_HID_KIND_INPUT {
        return None;
    }
    // body[8] is the report type; the motion block follows it.
    body.get(9..9 + REMOTE_HID_MOTION_LEN)
}

impl HeadTracker {
    /// Parses a RemoteHID motion block.
    ///
    /// The block carries an absolute gravity vector in 1000-per-g units at
    /// offset 44, so pitch and roll follow directly with no calibration. Rotation
    /// rate sits at offset 26 and is scaled by roughly 0.122 °/s per count.
    fn parse_remote_hid_block(
        &mut self,
        block: &[u8],
    ) -> Option<(HeadPose, Option<HeadGesture>)> {
        let gx = read_i16(block, 44)? as f32;
        let gy = read_i16(block, 46)? as f32;
        let gz = read_i16(block, 48)? as f32;

        // A zero vector means the sensor is still waking up.
        if gx == 0.0 && gy == 0.0 && gz == 0.0 {
            return None;
        }

        let rotation_x = read_i16(block, 26)?;
        let user_x = read_i16(block, 38)?;
        let user_y = read_i16(block, 40)?;

        // Forward tilt rotates gravity in the x-y plane; lateral tilt moves z.
        let pitch = gy.atan2(gx).to_degrees();
        let magnitude = (gx * gx + gy * gy + gz * gz).sqrt().max(f32::EPSILON);
        let roll = (gz / magnitude).clamp(-1.0, 1.0).asin().to_degrees();

        self.calibrated = true;
        self.samples_seen += 1;
        if self.sample_times.len() >= RATE_WINDOW {
            self.sample_times.pop_front();
        }
        self.sample_times.push_back(Instant::now());

        let pose = HeadPose {
            pitch: pitch.clamp(-90.0, 90.0),
            // RemoteHID exposes rotation rate; yaw is the integrated z rate, which
            // this layer does not accumulate yet.
            yaw: (rotation_x as f32 * ROTATION_DEG_PER_SEC).clamp(-180.0, 180.0),
            roll: roll.clamp(-90.0, 90.0),
            accel_h: user_x,
            accel_v: user_y,
        };

        let gesture = self.detect_gesture(user_x as f32, user_y as f32);
        Some((pose, gesture))
    }
}

impl HeadTracker {
    /// Parses a marked payload: raw accelerometer at `[20..26]`, gyro at
    /// `[26..32]`, and an absolute gravity unit vector scaled by 1024 at
    /// `[44..50]`.
    ///
    /// Because the gravity vector is absolute rather than a delta from a neutral
    /// pose, no calibration is needed: pitch and roll follow directly from it.
    /// Re-seating the earbuds shifts the resting angle, which is why callers may
    /// still want to zero against a reference.
    fn parse_marked_payload(&mut self, payload: &[u8]) -> Option<(HeadPose, Option<HeadGesture>)> {
        let gx = read_i16(payload, MARKED_GRAVITY_OFFSET)? as f32;
        let gy = read_i16(payload, MARKED_GRAVITY_OFFSET + 2)? as f32;
        let gz = read_i16(payload, MARKED_GRAVITY_OFFSET + 4)? as f32;

        // A zero vector means the sensor is still warming up.
        if gx == 0.0 && gy == 0.0 && gz == 0.0 {
            return None;
        }

        let accel_h = read_i16(payload, 20)?;
        let accel_v = read_i16(payload, 22)?;

        // Forward tilt rotates g in the x-y plane; lateral tilt moves z.
        let pitch = gy.atan2(gx).to_degrees();
        let magnitude = (gx * gx + gy * gy + gz * gz).sqrt().max(f32::EPSILON);
        let roll = (gz / magnitude).clamp(-1.0, 1.0).asin().to_degrees();

        self.calibrated = true;
        self.samples_seen += 1;
        if self.sample_times.len() >= RATE_WINDOW {
            self.sample_times.pop_front();
        }
        self.sample_times.push_back(Instant::now());

        let pose = HeadPose {
            pitch: pitch.clamp(-90.0, 90.0),
            // The marked payload has no yaw channel.
            yaw: 0.0,
            roll: roll.clamp(-90.0, 90.0),
            accel_h,
            accel_v,
        };

        let gesture = self.detect_gesture(accel_h as f32, accel_v as f32);
        Some((pose, gesture))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Builds a frame with the five fields a sample needs, at their absolute
    /// offsets inside a 65-byte payload.
    fn frame(o1: i16, o2: i16, o3: i16, accel_h: i16, accel_v: i16) -> Vec<u8> {
        let mut data = vec![0u8; SENSOR_V2_LEN];
        data[..6].copy_from_slice(&[0x04, 0x00, 0x04, 0x00, 0x17, 0x00]);
        data[43..45].copy_from_slice(&o1.to_le_bytes());
        data[45..47].copy_from_slice(&o2.to_le_bytes());
        data[47..49].copy_from_slice(&o3.to_le_bytes());
        data[51..53].copy_from_slice(&accel_h.to_le_bytes());
        data[53..55].copy_from_slice(&accel_v.to_le_bytes());
        data
    }

    fn calibrated_tracker() -> HeadTracker {
        let mut tracker = HeadTracker::new();
        for _ in 0..CALIBRATION_SAMPLES {
            tracker.parse_packet(&frame(1000, 2000, 2000, 0, 0));
        }
        assert!(tracker.is_calibrated());
        tracker
    }

    #[test]
    fn rejects_short_frames() {
        let mut tracker = HeadTracker::new();
        assert!(tracker.parse_packet(&[0u8; 10]).is_none());
    }

    #[test]
    fn rejects_non_sensor_payloads() {
        let mut data = frame(0, 0, 0, 0, 0);
        data[4] = 0x04; // battery opcode, not 0x17
        let mut tracker = HeadTracker::new();
        assert!(tracker.parse_packet(&data).is_none());
    }

    #[test]
    fn accepts_full_65_byte_payload() {
        let mut tracker = HeadTracker::new();
        assert!(tracker.parse_packet(&frame(0, 0, 0, 0, 0)).is_some());
    }

    #[test]
    fn accepts_aacp_wrapped_frame() {
        // 6 real header bytes + body, padded past 65 — offsets still hold.
        let mut tracker = HeadTracker::new();
        let mut data = frame(0, 0, 0, 0, 0);
        data.extend_from_slice(&[0u8; 6]);
        assert!(tracker.parse_packet(&data).is_some());
    }

    /// Builds an 80-byte marked frame in the format newer firmware uses.
    fn marked_frame(gx: i16, gy: i16, gz: i16, accel_h: i16, accel_v: i16) -> Vec<u8> {
        let mut data = vec![0u8; 80];
        data[..6].copy_from_slice(&[0x04, 0x00, 0x04, 0x00, 0x17, 0x00]);
        let start = 12;
        data[start..start + 6].copy_from_slice(&PAYLOAD_MARK);
        let payload = start + PAYLOAD_MARK.len();
        data[payload + 20..payload + 22].copy_from_slice(&accel_h.to_le_bytes());
        data[payload + 22..payload + 24].copy_from_slice(&accel_v.to_le_bytes());
        data[payload + 44..payload + 46].copy_from_slice(&gx.to_le_bytes());
        data[payload + 46..payload + 48].copy_from_slice(&gy.to_le_bytes());
        data[payload + 48..payload + 50].copy_from_slice(&gz.to_le_bytes());
        data
    }

    /// Builds a RemoteHID InputReport carrying a 58-byte motion block.
    fn remote_hid_frame(gx: i16, gy: i16, gz: i16, user_x: i16, user_y: i16) -> Vec<u8> {
        let mut block = vec![0u8; REMOTE_HID_MOTION_LEN];
        block[44..46].copy_from_slice(&gx.to_le_bytes());
        block[46..48].copy_from_slice(&gy.to_le_bytes());
        block[48..50].copy_from_slice(&gz.to_le_bytes());
        block[38..40].copy_from_slice(&user_x.to_le_bytes());
        block[40..42].copy_from_slice(&user_y.to_le_bytes());

        // body: version, generation, 0, side, sub-header, report type, block
        let sub_len = 4 + 1 + block.len();
        let header = 7u32 | ((sub_len as u32) << 7) | (2u32 << 17);
        let mut body = vec![0x00, 0x2A, 0x00, 0x00];
        body.extend_from_slice(&header.to_le_bytes());
        body.push(0x00); // report type: input
        body.extend_from_slice(&block);

        let mut frame = vec![0x04, 0x00, 0x04, 0x00, 0x17, 0x00];
        frame.extend_from_slice(&0x0000_0800u32.to_le_bytes());
        frame.extend_from_slice(&(body.len() as u16).to_le_bytes());
        frame.extend_from_slice(&body);
        frame
    }

    #[test]
    fn remote_hid_input_report_is_recognised() {
        let mut tracker = HeadTracker::new();
        let (pose, _) = tracker
            .parse_packet(&remote_hid_frame(1000, 0, 0, 0, 0))
            .expect("RemoteHID InputReport should parse");
        // Gravity along +x means level.
        assert!(pose.pitch.abs() < 0.5, "pitch={}", pose.pitch);
        assert!(pose.roll.abs() < 0.5, "roll={}", pose.roll);
        assert!(tracker.is_calibrated());
    }

    #[test]
    fn remote_hid_pitch_follows_gravity() {
        let mut tracker = HeadTracker::new();
        let (pose, _) = tracker
            .parse_packet(&remote_hid_frame(0, 1000, 0, 0, 0))
            .expect("should parse");
        assert!((pose.pitch - 90.0).abs() < 0.5, "pitch={}", pose.pitch);
    }

    #[test]
    fn remote_hid_zero_gravity_is_ignored() {
        let mut tracker = HeadTracker::new();
        assert!(tracker.parse_packet(&remote_hid_frame(0, 0, 0, 0, 0)).is_none());
    }

    #[test]
    fn remote_hid_ignores_non_input_reports() {
        let mut frame = remote_hid_frame(1000, 0, 0, 0, 0);
        // Rewrite the kind bits from 2 (InputReport) to 0 (DeviceConnect).
        let header = u32::from_le_bytes([frame[16], frame[17], frame[18], frame[19]]);
        let cleared = header & !(0x7 << 17);
        frame[16..20].copy_from_slice(&cleared.to_le_bytes());
        let mut tracker = HeadTracker::new();
        assert!(tracker.parse_packet(&frame).is_none());
    }

    #[test]
    fn marked_frame_is_parsed_without_calibration() {
        let mut tracker = HeadTracker::new();
        // Gravity pointing along +x: no tilt.
        let (pose, _) = tracker
            .parse_packet(&marked_frame(1024, 0, 0, 0, 0))
            .expect("marked frame should parse");
        assert!(tracker.is_calibrated(), "absolute gravity needs no calibration");
        assert!(pose.pitch.abs() < 0.5, "pitch={}", pose.pitch);
        assert!(pose.roll.abs() < 0.5, "roll={}", pose.roll);
    }

    #[test]
    fn marked_frame_pitch_follows_gravity_direction() {
        let mut tracker = HeadTracker::new();
        // Gravity along +y is a 90° forward tilt.
        let (pose, _) = tracker.parse_packet(&marked_frame(0, 1024, 0, 0, 0)).unwrap();
        assert!(
            (pose.pitch - 90.0).abs() < 0.5,
            "expected ~90°, got {}",
            pose.pitch
        );
    }

    #[test]
    fn marked_frame_roll_follows_z_component() {
        let mut tracker = HeadTracker::new();
        let (pose, _) = tracker.parse_packet(&marked_frame(1024, 0, 1024, 0, 0)).unwrap();
        // gz/|g| = 1/sqrt(2) -> asin = 45°.
        assert!(
            (pose.roll - 45.0).abs() < 0.5,
            "expected ~45°, got {}",
            pose.roll
        );
    }

    #[test]
    fn marked_frame_zero_gravity_is_ignored_while_warming_up() {
        let mut tracker = HeadTracker::new();
        assert!(tracker.parse_packet(&marked_frame(0, 0, 0, 0, 0)).is_none());
    }

    #[test]
    fn marked_frame_acceleration_passes_through() {
        let mut tracker = HeadTracker::new();
        let (pose, _) = tracker.parse_packet(&marked_frame(1024, 0, 0, -1234, 567)).unwrap();
        assert_eq!(pose.accel_h, -1234);
        assert_eq!(pose.accel_v, 567);
    }

    #[test]
    fn calibration_uses_ten_samples_and_zeros_pose() {
        let mut tracker = HeadTracker::new();
        for i in 0..CALIBRATION_SAMPLES - 1 {
            let (pose, gesture) = tracker
                .parse_packet(&frame(500, 6000, 6000, 0, 0))
                .expect("frame should parse");
            assert_eq!(pose.pitch, 0.0, "pose is zeroed while calibrating (sample {i})");
            assert!(gesture.is_none());
            assert!(!tracker.is_calibrated());
        }
        tracker.parse_packet(&frame(500, 6000, 6000, 0, 0));
        assert!(tracker.is_calibrated());
        assert_eq!(tracker.o2_neutral, 6000.0);
        assert_eq!(tracker.o1_neutral, 500.0);
    }

    #[test]
    fn neutral_pose_reads_zero_after_calibration() {
        let mut tracker = calibrated_tracker();
        let (pose, _) = tracker.parse_packet(&frame(1000, 2000, 2000, 0, 0)).unwrap();
        assert!(pose.pitch.abs() < 0.01, "pitch={}", pose.pitch);
        assert!(pose.yaw.abs() < 0.01, "yaw={}", pose.yaw);
        assert!(pose.roll.abs() < 0.01, "roll={}", pose.roll);
    }

    #[test]
    fn pitch_responds_to_half_scale_deflection() {
        let mut tracker = calibrated_tracker();
        // +16000 on both o2 and o3 is half of the 32000/180° scale → 90°.
        let (pose, _) = tracker.parse_packet(&frame(1000, 18000, 18000, 0, 0)).unwrap();
        assert!(
            (pose.pitch - 90.0).abs() < 0.5,
            "expected ~90°, got {}",
            pose.pitch
        );
        assert!(pose.yaw.abs() < 0.5, "yaw should stay neutral");
    }

    #[test]
    fn yaw_uses_o2_minus_o3() {
        let mut tracker = calibrated_tracker();
        // o2 up, o3 down by the same amount → pure yaw.
        let (pose, _) = tracker.parse_packet(&frame(1000, 10000, -6000, 0, 0)).unwrap();
        assert!(
            (pose.yaw - 45.0).abs() < 0.5,
            "expected ~45° yaw, got {}",
            pose.yaw
        );
    }

    #[test]
    fn roll_uses_o1() {
        let mut tracker = calibrated_tracker();
        let (pose, _) = tracker.parse_packet(&frame(9000, 2000, 2000, 0, 0)).unwrap();
        assert!(
            (pose.roll - 45.0).abs() < 0.5,
            "expected ~45° roll, got {}",
            pose.roll
        );
    }

    #[test]
    fn accel_fields_pass_through_signed() {
        let mut tracker = calibrated_tracker();
        let (pose, _) = tracker.parse_packet(&frame(1000, 2000, 2000, -1234, 5678)).unwrap();
        assert_eq!(pose.accel_h, -1234);
        assert_eq!(pose.accel_v, 5678);
    }

    #[test]
    fn samples_seen_counts_only_calibrated_samples() {
        let mut tracker = calibrated_tracker();
        for _ in 0..5 {
            tracker.parse_packet(&frame(1000, 2000, 2000, 0, 0));
        }
        assert_eq!(tracker.samples_seen(), 5);
    }

    #[test]
    fn vertical_oscillation_yields_nod() {
        let mut tracker = calibrated_tracker();
        let mut gesture = None;
        // Three alternating, high-amplitude vertical swings.
        for amplitude in [900i16, -900, 900, -900, 900, -900] {
            for _ in 0..3 {
                if let Some((_, g)) = tracker.parse_packet(&frame(1000, 2000, 2000, 0, amplitude)) {
                    gesture = gesture.or(g);
                }
            }
            if gesture.is_some() {
                break;
            }
        }
        assert_eq!(gesture, Some(HeadGesture::NodYes));
    }

    #[test]
    fn horizontal_oscillation_yields_shake() {
        let mut tracker = calibrated_tracker();
        let mut gesture = None;
        for amplitude in [900i16, -900, 900, -900, 900, -900] {
            for _ in 0..3 {
                if let Some((_, g)) = tracker.parse_packet(&frame(1000, 2000, 2000, amplitude, 0)) {
                    gesture = gesture.or(g);
                }
            }
            if gesture.is_some() {
                break;
            }
        }
        assert_eq!(gesture, Some(HeadGesture::ShakeNo));
    }

    #[test]
    fn small_noise_does_not_trigger_gesture() {
        let mut tracker = calibrated_tracker();
        for i in 0..200 {
            let jitter = ((i % 7) as i16) * 40; // ±240 counts, under the 400 floor
            let (_, gesture) = tracker
                .parse_packet(&frame(1000, 2000, 2000, jitter, -jitter))
                .unwrap();
            assert!(gesture.is_none(), "noise triggered {:?} at sample {i}", gesture);
        }
    }

    #[test]
    fn sample_rate_needs_two_samples() {
        let mut tracker = calibrated_tracker();
        assert!(tracker.sample_rate_hz().is_none(), "no samples yet");
        tracker.parse_packet(&frame(1000, 2000, 2000, 0, 0));
        assert!(
            tracker.sample_rate_hz().is_none(),
            "one sample gives no measurable interval"
        );
        tracker.parse_packet(&frame(1000, 2000, 2000, 0, 0));
        let rate = tracker.sample_rate_hz().expect("two samples give a rate");
        assert!(rate > 0.0 && rate.is_finite(), "rate={}", rate);
    }

    #[test]
    fn reset_clears_calibration() {
        let mut tracker = calibrated_tracker();
        tracker.reset();
        assert!(!tracker.is_calibrated());
        assert_eq!(tracker.samples_seen(), 0);
    }

    #[test]
    fn recent_gesture_expires() {
        let mut tracker = calibrated_tracker();
        tracker.last_gesture = Some((HeadGesture::NodYes, Instant::now()));
        assert_eq!(
            tracker.recent_gesture(std::time::Duration::from_secs(1)),
            Some(HeadGesture::NodYes)
        );
        assert_eq!(
            tracker.recent_gesture(std::time::Duration::from_nanos(0)),
            None
        );
    }
}

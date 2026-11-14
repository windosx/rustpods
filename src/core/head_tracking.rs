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

/// The absolute pitch/roll anchor the integrator otherwise lacks.
///
/// The integrated pitch is unobservable drift: nothing ever pulls it back
/// toward truth, so every residual rate error accumulates forever - the user
/// watched the compass wander away from their actual head posture over
/// minutes. The gravity vector is the fix, and no axis learning is needed:
/// the *change* of gravity from its captured neutral direction is a plain
/// rotation, whose components about the head's lateral axis (pitch) and
/// forward axis (roll) are exactly the absolute angles. Decomposing with
/// the integrator's own tracked axes keeps every bud seating supported -
/// whatever device orientation "level" is, it is whatever gravity was when
/// the stream started, which is also where the integrals start at zero.
///
/// Yaw gets no correction from gravity (a yaw rotation leaves the gravity
/// vector unchanged). Instead it decays gently toward the session centre -
/// see [`AttitudeIntegrator::YAW_RECENTER_TAU_S`]. The nod counter measures
/// yaw *swing* against an adapting reference, not an absolute heading, so a
/// slow recentering does not disturb it.
#[derive(Debug, Clone, Default)]
struct GravityAnchor {
    /// Mean device-frame gravity direction over the first seconds of
    /// streaming; `None` until captured. Defines "zero" pitch/roll.
    neutral: Option<[f32; 3]>,
    /// Mean gravity magnitude at capture time, in device counts. The live
    /// samples are sanity-checked against it, since a magnitude far off the
    /// captured norm means the sensor is not in a state worth trusting.
    neutral_norm: f32,
    /// Accumulator for the neutral capture.
    acc: [f32; 3],
    /// Accumulated raw magnitudes for [`Self::neutral_norm`].
    acc_norm: f32,
    /// Samples accumulated so far.
    n: u32,
}

impl GravityAnchor {
    /// Samples averaged into the neutral direction (~1 s at 100 Hz). The
    /// wearer's posture when tracking starts is the zero they will read.
    const CAPTURE_SAMPLES: u32 = 100;
    /// Gravity magnitude sanity band around the captured norm, as factors;
    /// outside it the sensor is waking up, saturated, or the bud is out of
    /// an ear, and no correction should fire.
    const NORM_BAND: (f32, f32) = (0.5, 2.0);
    /// How strongly each sample pulls the integrated angle onto the gravity
    /// observation. At ~100 Hz this is a time constant near 2 s: fast enough
    /// that drift cannot accumulate, slow enough that nod dynamics stay
    /// dominated by the gyro (a 0.3 s nod receives only ~13% pull).
    const PULL: f32 = 0.006;

    /// Feeds one gravity sample plus the current integrated pitch/roll (the
    /// lateral/forward unit vectors, device frame) and returns the corrected
    /// pair. Before the neutral is captured it passes the integrals through.
    fn correct(
        &mut self,
        gravity: [f32; 3],
        pitch_deg: f32,
        roll_deg: f32,
        lateral: [f32; 3],
        forward: [f32; 3],
    ) -> (f32, f32) {
        let len = len3(gravity);
        if len <= 1e-3 {
            return (pitch_deg, roll_deg);
        }
        let unit = scale3(gravity, 1.0 / len);

        let Some(neutral) = self.neutral else {
            self.acc = add3(self.acc, unit);
            self.acc_norm += len;
            self.n += 1;
            if self.n >= Self::CAPTURE_SAMPLES {
                self.neutral = Some(normalize3(self.acc));
                self.neutral_norm = self.acc_norm / self.n as f32;
            }
            return (pitch_deg, roll_deg);
        };

        // The magnitude has to stay inside the sanity band around what was
        // captured: outside it the sensor is waking up, saturated, or the bud
        // is out of an ear, and applying the observation would steer the
        // attitude with garbage.
        let ratio = len / self.neutral_norm;
        if !(Self::NORM_BAND.0..=Self::NORM_BAND.1).contains(&ratio) {
            return (pitch_deg, roll_deg);
        }

        // Rotation taking neutral to current, as an axis-scaled vector.
        // cross(a,b) = |a||b| * sin(angle) * right(a->b); its projection on
        // a head axis is the rotation about that axis. Sign: nodding down
        // (pitch < 0) rotates the *device* negatively about the lateral
        // axis, so gravity in the device frame rotates positively about it,
        // and cross(neutral, unit) points along +lateral - hence the minus,
        // matching the integrator's pitch/roll conventions.
        let dr = cross3(neutral, unit);
        let cos = dot3(neutral, unit).clamp(-1.0, 1.0);
        // Near the 180° flip the decomposition is meaningless; skip.
        if cos < 0.0 {
            return (pitch_deg, roll_deg);
        }
        let sin_pitch = dot3(dr, lateral);
        let sin_roll = dot3(dr, forward);
        if sin_pitch.abs() > 1.0 || sin_roll.abs() > 1.0 {
            return (pitch_deg, roll_deg);
        }
        let obs_pitch = -sin_pitch.asin().to_degrees();
        let obs_roll = -sin_roll.asin().to_degrees();

        (
            pitch_deg + (obs_pitch - pitch_deg) * Self::PULL,
            roll_deg + (obs_roll - roll_deg) * Self::PULL,
        )
    }
}

/// Fused attitude and instantaneous angular rate sample.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct AttitudeSample {
    pub yaw: f32,
    pub pitch: f32,
    pub roll: f32,
    pub yaw_rate: f32,
    pub pitch_rate: f32,
    pub roll_rate: f32,
}

impl AttitudeSample {
    pub fn angles(&self) -> [f32; 3] {
        [self.yaw, self.pitch, self.roll]
    }
}

/// Fuses gyroscope angular rate with the gravity vector into yaw/pitch/roll.
///
/// The math follows `starman-tech/mousepods` `MotionEngine.step`, which is the
/// implementation verified against this hardware:
///
/// * gravity gives the vertical axis, low-pass filtered so noise does not steer
///   the frame;
/// * the gyro rate is projected onto that vertical for yaw, and onto the
///   horizontal residual for pitch and roll;
/// * each component is integrated over time to produce degrees.
///
/// A head resting against a shoulder is a *rotation*, so it shows up as a large
/// integral here even though the raw gravity vector barely changes.
#[derive(Debug, Clone, Default)]
struct AttitudeIntegrator {
    /// Low-pass filtered gravity unit vector; `None` until the first sample.
    up: Option<[f32; 3]>,
    yaw_deg: f32,
    pitch_deg: f32,
    roll_deg: f32,
    /// Learned lateral axis, so pitch keeps a stable reference when the head is
    /// tilted far enough that the default axis is nearly vertical.
    lateral: Option<[f32; 3]>,
    last_sample: Option<Instant>,

    /// Estimated gyro bias, in counts per second.
    ///
    /// A MEMS gyro reports a small non-zero rate even when still, and integrating
    /// that constant offset makes the compass creep in one direction forever -
    /// which is what repeated head shaking produced: the needle drifted
    /// anticlockwise a little more each time.
    bias: [f32; 3],
    /// Recent rates and gravity vectors, for the stillness test.
    window: VecDeque<[f32; 3]>,
    gravity_window: VecDeque<[f32; 3]>,
    /// How long the head has been still enough to trust as a bias sample.
    still_seconds: f32,
    /// Whether a first bias estimate has been taken. The first one is adopted
    /// whole; later ones are blended in slowly.
    bias_seeded: bool,

    /// Learned gravity-to-head mapping for the pitch correction.
    ///
    /// The integrated pitch is unobservable drift: nothing ever pulls it back
    /// toward truth, so every residual rate error accumulates forever - the
    /// user watched the compass wander away from their actual head posture
    /// over minutes. The raw gravity vector, by contrast, is absolute: it
    /// always points down, whatever the buds' seating. It is however in
    /// *device* axes with unknown orientation, so the correction learns the
    /// linear map from device gravity to head pitch/roll (signs and zero
    /// offsets) from real motion, and only once that map is stable does it
    /// gently pull the integrals back. Before then the behaviour is exactly
    /// the old ungoverned integral.
    /// wheels the neutral capture and the pull. See [`GravityAnchor`].
    grav: GravityAnchor,
}
impl AttitudeIntegrator {
    const UP_SMOOTHING: f32 = 0.15;

    /// Time constant of the yaw pull toward the session centre, in seconds.
    ///
    /// There is no magnetometer, so absolute heading is unobservable: gravity
    /// cannot see a yaw rotation at all, and the bias learner only runs while
    /// the head is perfectly still. Any residual rate error - temperature
    /// drift, the scale being approximate - integrated forever, which is the
    /// "the longer it runs, the worse the offset" failure. A gentle leak makes
    /// the error bounded instead of cumulative: a constant residual rate ends
    /// at `rate * tau` degrees, and while the wearer rests the needle settles
    /// back to centre the way macOS spatial audio recentres its stage. The
    /// constant is long enough that deliberate turns dominate it (a 3 s turn
    /// keeps ~97% of its angle) and short enough that drift cannot accumulate
    /// over minutes.
    const YAW_RECENTER_TAU_S: f32 = 90.0;

    /// Peak-to-peak rate range under which the head counts as still, in counts/s.
    /// The reference implementation uses 3.0 deg/s, converted here with the same
    /// scale the integrator applies.
    const STILL_RANGE_COUNTS: f32 = 3.0 / ROTATION_DEG_PER_SEC;
    /// Gravity may drift by at most this much while "still", as a cosine.
    const STILL_GRAVITY_DOT: f32 = 0.999_7;
    /// Stillness has to persist this long before the average is trusted.
    const STILL_SECONDS: f32 = 0.4;
    /// How much of a new bias estimate to adopt after the first one.
    const BIAS_BLEND: f32 = 0.03;
    /// Samples kept for the stillness test (~0.4 s at 100 Hz).
    const BIAS_WINDOW: usize = 40;
    /// Largest first estimate to accept, in counts/s; a bigger one means the
    /// "still" judgement was wrong and a real rotation is in progress.
    const BIAS_SEED_LIMIT: f32 = 20.0 / ROTATION_DEG_PER_SEC;
    /// Largest correction to an existing estimate, in counts/s.
    const BIAS_TRACK_LIMIT: f32 = 5.0 / ROTATION_DEG_PER_SEC;

    fn new() -> Self {
        Self::default()
    }

    fn reset(&mut self) {
        *self = Self::default();
    }

    /// Updates the gyro bias estimate while the head is still.
    ///
    /// A constant offset in the rate integrates into an ever-growing angle, so
    /// without this the compass creeps even when nothing moves. The test follows
    /// the reference implementation: the rate must stay within a narrow
    /// peak-to-peak band and gravity must not rotate, for a sustained period.
    fn update_bias(&mut self, gyro: [f32; 3], up: [f32; 3], dt: f32) {
        if self.window.len() >= Self::BIAS_WINDOW {
            self.window.pop_front();
            self.gravity_window.pop_front();
        }
        self.window.push_back(gyro);
        self.gravity_window.push_back(up);

        if self.window.len() < 8 {
            return;
        }

        let mut range = 0.0f32;
        for axis in 0..3 {
            let max = self
                .window
                .iter()
                .map(|s| s[axis])
                .fold(f32::NEG_INFINITY, f32::max);
            let min = self
                .window
                .iter()
                .map(|s| s[axis])
                .fold(f32::INFINITY, f32::min);
            range = range.max(max - min);
        }

        let gravity_moved = match (self.gravity_window.front(), self.gravity_window.back()) {
            (Some(first), Some(last)) => dot3(*first, *last) < Self::STILL_GRAVITY_DOT,
            _ => true,
        };

        if range < Self::STILL_RANGE_COUNTS && !gravity_moved {
            self.still_seconds += dt;
        } else {
            self.still_seconds = 0.0;
        }

        if self.still_seconds < Self::STILL_SECONDS {
            return;
        }

        let n = self.window.len() as f32;
        let mean = [
            self.window.iter().map(|s| s[0]).sum::<f32>() / n,
            self.window.iter().map(|s| s[1]).sum::<f32>() / n,
            self.window.iter().map(|s| s[2]).sum::<f32>() / n,
        ];

        let jump = len3(sub3(mean, self.bias));
        let limit = if self.bias_seeded {
            Self::BIAS_TRACK_LIMIT
        } else {
            Self::BIAS_SEED_LIMIT
        };
        if jump >= limit {
            // A slow, steady rotation is not a bias; refuse to learn from it.
            return;
        }

        let blend = if self.bias_seeded { Self::BIAS_BLEND } else { 1.0 };
        self.bias = add3(
            self.bias,
            scale3(sub3(mean, self.bias), blend),
        );
        self.bias_seeded = true;
    }

    /// Feeds one sample. `gyro` is the rotation rate at offsets 26..32 and
    /// `gravity` the vector at 44..50, both in device counts.
    fn update(&mut self, gyro: [f32; 3], gravity: [f32; 3]) -> Option<AttitudeSample> {
        let g_len = len3(gravity);
        if g_len <= 1e-3 {
            return None;
        }
        let unit = scale3(gravity, 1.0 / g_len);
        self.up = Some(match self.up {
            None => unit,
            Some(prev) => normalize3(add3(
                prev,
                scale3(sub3(unit, prev), Self::UP_SMOOTHING),
            )),
        });
        let up = self.up?;

        // Time since the previous sample, capped so a stalled stream does not
        // produce one huge jump when it resumes.
        let now = Instant::now();
        let dt = self
            .last_sample
            .map(|t| now.duration_since(t).as_secs_f32().min(0.25))
            .unwrap_or(0.0);
        self.last_sample = Some(now);
        if dt <= 0.0 {
            return Some(AttitudeSample {
                yaw: self.yaw_deg,
                pitch: self.pitch_deg,
                roll: self.roll_deg,
                yaw_rate: 0.0,
                pitch_rate: 0.0,
                roll_rate: 0.0,
            });
        }

        self.update_bias(gyro, up, dt);
        let corrected = sub3(gyro, self.bias);

        // Yaw: rotation about the vertical.
        let yaw_rate = dot3(corrected, up);

        // Pitch and roll live in the plane perpendicular to gravity. The lateral
        // axis is the device's own x axis projected down, kept from the previous
        // sample when the projection degenerates.
        let horizontal = sub3(corrected, scale3(up, yaw_rate));
        let lateral = match self.lateral {
            Some(prev) if len3(reject3([1.0, 0.0, 0.0], up)) < 0.25 => prev,
            _ => normalize3(reject3([1.0, 0.0, 0.0], up)),
        };
        self.lateral = Some(lateral);
        let forward = cross3(up, lateral);

        let pitch_rate = dot3(horizontal, lateral);
        let roll_rate = dot3(horizontal, forward);

        self.yaw_deg += yaw_rate * ROTATION_DEG_PER_SEC * dt;
        self.pitch_deg += pitch_rate * ROTATION_DEG_PER_SEC * dt;
        self.roll_deg += roll_rate * ROTATION_DEG_PER_SEC * dt;

        // Yaw has no absolute reference (gravity cannot observe it), so a
        // residual rate error would integrate forever. The leak bounds it and
        // recentres the stage while the head rests; pitch/roll need no such
        // decay because the gravity anchor already pulls them onto truth.
        self.yaw_deg *= 1.0 - dt / Self::YAW_RECENTER_TAU_S;

        // Absolute correction: gravity says where down is, so after the
        // neutral direction is captured the integrated pitch/roll get pulled
        // onto the gravity observation instead of wandering forever. Runs
        // through the same lateral/forward frame the integration used, so
        // every bud seating is handled without calibration.
        let lateral_v = self.lateral.unwrap_or([1.0, 0.0, 0.0]);
        let forward_v = cross3(up, lateral_v);
        let (pitch_corr, roll_corr) = self.grav.correct(
            gravity,
            self.pitch_deg,
            self.roll_deg,
            lateral_v,
            forward_v,
        );
        self.pitch_deg = pitch_corr;
        self.roll_deg = roll_corr;

        // Keep the display bounded; the head cannot turn further than this.
        self.yaw_deg = wrap_deg(self.yaw_deg);
        self.pitch_deg = self.pitch_deg.clamp(-90.0, 90.0);
        self.roll_deg = self.roll_deg.clamp(-90.0, 90.0);

        let yaw_rate_dps = yaw_rate * ROTATION_DEG_PER_SEC;
        let pitch_rate_dps = pitch_rate * ROTATION_DEG_PER_SEC;
        let roll_rate_dps = roll_rate * ROTATION_DEG_PER_SEC;

        Some(AttitudeSample {
            yaw: self.yaw_deg,
            pitch: self.pitch_deg,
            roll: self.roll_deg,
            yaw_rate: yaw_rate_dps,
            pitch_rate: pitch_rate_dps,
            roll_rate: roll_rate_dps,
        })
    }
}

fn add3(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}

fn sub3(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn scale3(a: [f32; 3], k: f32) -> [f32; 3] {
    [a[0] * k, a[1] * k, a[2] * k]
}

fn dot3(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn cross3(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

fn len3(a: [f32; 3]) -> f32 {
    dot3(a, a).sqrt()
}

fn normalize3(a: [f32; 3]) -> [f32; 3] {
    let l = len3(a);
    if l > 1e-6 {
        scale3(a, 1.0 / l)
    } else {
        [0.0, 0.0, 1.0]
    }
}

/// Removes the component of `v` parallel to `axis`.
fn reject3(v: [f32; 3], axis: [f32; 3]) -> [f32; 3] {
    sub3(v, scale3(axis, dot3(v, axis)))
}

fn wrap_deg(deg: f32) -> f32 {
    let mut d = deg % 360.0;
    if d > 180.0 {
        d -= 360.0;
    } else if d < -180.0 {
        d += 360.0;
    }
    d
}

#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct HeadPose {
    /// Nodding: negative looks down, positive looks up. Degrees.
    pub pitch: f32,
    /// Tipping the head to one shoulder. Degrees.
    pub roll: f32,
    /// Turning the head left/right. Degrees.
    pub yaw: f32,
    pub accel_h: i16,
    pub accel_v: i16,
    /// Turning velocity left/right. Degrees per second (positive turns right).
    pub yaw_rate: f32,
    /// Nodding velocity. Degrees per second (positive tilts up).
    pub pitch_rate: f32,
    /// Tilting velocity. Degrees per second (positive tilts right).
    pub roll_rate: f32,
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
    ///
    /// Both the floor and the ceiling follow the user's gesture sensitivity
    /// (see `nod_counter::badge_amplitude_scale`): higher sensitivity means
    /// smaller direction changes already register.
    fn dynamic_threshold(&self) -> f32 {
        let scale = crate::core::nod_counter::badge_amplitude_scale();
        let recent: Vec<f32> = self.history.iter().rev().take(4).copied().collect();
        if recent.len() < 2 {
            return MIN_DIRECTION_CHANGE * scale;
        }
        let mean = recent.iter().sum::<f32>() / recent.len() as f32;
        let variance = recent.iter().map(|v| (v - mean).powi(2)).sum::<f32>() / recent.len() as f32;
        (variance / 3.0).clamp(
            MIN_DIRECTION_CHANGE * scale,
            DIRECTION_CHANGE_THRESHOLD * scale,
        )
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

        let amplitude_gate = AMPLITUDE_FLOOR * crate::core::nod_counter::badge_amplitude_scale();
        let Some(extremity) = turned.filter(|v| v.abs() > amplitude_gate) else {
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

    /// Integrated attitude, in degrees.
    ///
    /// The gyroscope reports angular rate, not angle, so orientation is the
    /// integral of the rate projected onto gravity-derived axes. Reading the
    /// gravity vector as an angle (atan2 of two components) only gives the
    /// static tilt of one plane and barely moves when the head turns - which is
    /// exactly what a head resting on a shoulder looked like.
    attitude: AttitudeIntegrator,

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
            attitude: AttitudeIntegrator::new(),

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
        self.attitude.reset();
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
    /// Frame shapes are told apart before any offsets are read:
    /// RemoteHID (descriptor `0x800`) and the marker-prefixed payloads are
    /// recognised first, then the `SensorV2` fallback, which comes in two
    /// variants - the bare payload (fields at absolute offsets 43..55) and
    /// the AACP-wrapped form (`04 00 04 00 17` prefix, fields at the same
    /// offsets *after* the 6-byte header is stripped). The length classes
    /// decide which one applies; the header of a wrapped frame is never
    /// counted as part of the payload.
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
                    yaw_rate: 0.0,
                    pitch_rate: 0.0,
                    roll_rate: 0.0,
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
            yaw_rate: 0.0,
            pitch_rate: 0.0,
            roll_rate: 0.0,
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
pub const REMOTE_HID_MOTION_LEN: usize = 58;
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

        let gyro = [
            read_i16(block, 26)? as f32,
            read_i16(block, 28)? as f32,
            read_i16(block, 30)? as f32,
        ];
        let gravity = [gx, gy, gz];
        let user_x = read_i16(block, 38)?;
        let user_y = read_i16(block, 40)?;

        // Orientation is the integral of the gyroscope rate, not the gravity
        // vector itself. Reading gravity as an angle only tracks the static tilt
        // of one plane and barely reacts to a head turn.
        let attitude = self.attitude.update(gyro, gravity);

        self.calibrated = true;
        self.samples_seen += 1;
        if self.sample_times.len() >= RATE_WINDOW {
            self.sample_times.pop_front();
        }
        self.sample_times.push_back(Instant::now());

        let att = attitude.unwrap_or_default();

        // Dump raw and integrated values occasionally so the gyro scale can be
        // calibrated against real movement. `0.122 deg/s per count` comes from
        // the protocol document, which marks it approximate.
        if self.samples_seen.is_multiple_of(100) {
            debug!(
                "attitude raw gyro=[{:.0},{:.0},{:.0}] grav=[{:.0},{:.0},{:.0}] -> yaw={:.1} pitch={:.1} roll={:.1} rates=[{:.1},{:.1},{:.1}]",
                gyro[0], gyro[1], gyro[2], gx, gy, gz, att.yaw, att.pitch, att.roll, att.yaw_rate, att.pitch_rate, att.roll_rate
            );
        }
        let pose = HeadPose {
            pitch: att.pitch,
            yaw: att.yaw,
            roll: att.roll,
            accel_h: user_x,
            accel_v: user_y,
            yaw_rate: att.yaw_rate,
            pitch_rate: att.pitch_rate,
            roll_rate: att.roll_rate,
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
            yaw_rate: 0.0,
            pitch_rate: 0.0,
            roll_rate: 0.0,
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
    fn integrator_turns_rotation_rate_into_angle() {
        // A steady yaw rate about the vertical must accumulate into an angle.
        // This is the behaviour the old atan2(gravity) code could not produce:
        // it reported the static tilt only, so a head turn barely moved.
        let mut integ = AttitudeIntegrator::new();
        // Gravity along +z means the head is upright.
        let gravity = [0.0, 0.0, 1000.0];
        // Rotation of 5000 counts/s about z, held for ~0.1 s per sample.
        let gyro = [0.0, 0.0, 5000.0];
        integ.update(gyro, gravity);
        // Feed repeatedly; each call advances by the real elapsed time, so assert
        // on the direction rather than an exact value.
        for _ in 0..50 {
            std::thread::sleep(std::time::Duration::from_millis(2));
            integ.update(gyro, gravity);
        }
        let [yaw, _, _] = integ.update(gyro, gravity).unwrap().angles();
        assert!(yaw > 0.0, "yaw should integrate upward, got {}", yaw);
    }

    #[test]
    fn constant_gyro_offset_is_learned_as_bias_and_stops_the_drift() {
        // This is the drift you saw: the gyro reports a small constant rate while
        // the head is still, and integrating it walks the needle round the dial.
        // The bias estimator must absorb it so the angle settles.
        let mut integ = AttitudeIntegrator::new();
        let gravity = [0.0, 0.0, 1000.0];
        // A steady offset well inside the stillness band (2 deg/s worth).
        let offset = 2.0 / ROTATION_DEG_PER_SEC;
        let gyro = [0.0, 0.0, offset];

        // Stillness has to persist for 0.4 s before the average is trusted, so
        // feed well past that. 100 samples at 5 ms is 0.5 s.
        for _ in 0..100 {
            std::thread::sleep(std::time::Duration::from_millis(5));
            integ.update(gyro, gravity);
        }
        assert!(integ.bias_seeded, "the offset should have been learned");
        let learned_yaw = integ.yaw_deg;

        // Another half second must not keep accumulating.
        for _ in 0..100 {
            std::thread::sleep(std::time::Duration::from_millis(5));
            integ.update(gyro, gravity);
        }
        let drift = (integ.yaw_deg - learned_yaw).abs();
        assert!(
            drift < 1.0,
            "yaw kept drifting by {} deg after the bias was learned",
            drift
        );
    }

    #[test]
    fn yaw_drift_under_a_constant_residual_rate_stays_bounded() {
        // The live failure (2026-11-09): the longer the stream ran, the further
        // the needle sat from the real heading. Yaw has no absolute reference,
        // so any rate error the stillness learner misses integrates forever.
        // With the recentering leak a constant residual rate must end in a
        // bounded steady state (rate * tau), not a walk to the wrap.
        let mut integ = AttitudeIntegrator::new();
        let gravity = [0.0, 0.0, 1000.0];
        // ~2.4 deg/s of phantom rotation: above the stillness band, so the
        // bias learner never runs, and below the seed limit it would reject.
        let residual = [0.0, 0.0, 20.0];

        // Capture the neutral first.
        for _ in 0..60 {
            integ.update([0.0, 0.0, 0.0], gravity);
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        // ~4 s of continuous residual rate. Ungoverned this is ~10 deg and
        // growing without bound; governed it must stay far below the steady
        // state (2.4 deg/s * 90 s = 220 deg) because the leak fights it.
        for _ in 0..800 {
            integ.update(residual, gravity);
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        let yaw = integ.yaw_deg;
        assert!(
            yaw.abs() < 20.0,
            "yaw walked to {} deg under a constant residual rate; the leak is not biting",
            yaw
        );
    }

    #[test]
    fn displaced_yaw_recenters_while_the_head_rests() {
        // After a turn, the stage must settle back to centre while the wearer
        // faces forward again - the macOS spatial-audio behaviour, and the
        // reason drift cannot accumulate across a session.
        let mut integ = AttitudeIntegrator::new();
        let gravity = [0.0, 0.0, 1000.0];
        for _ in 0..60 {
            integ.update([0.0, 0.0, 0.0], gravity);
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        // Turn the head: ~0.3 s of yaw rate.
        for _ in 0..60 {
            integ.update([0.0, 0.0, 2000.0], gravity);
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        let turned = integ.yaw_deg;
        assert!(turned.abs() > 3.0, "the turn should register, got {}", turned);

        // Rest for ~6 s: the leak must visibly shrink the angle toward zero.
        for _ in 0..1200 {
            integ.update([0.0, 0.0, 0.0], gravity);
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert!(
            integ.yaw_deg.abs() < turned.abs() * 0.95,
            "yaw stayed at {} after resting (was {}); it never recentred",
            integ.yaw_deg,
            turned
        );
    }

    #[test]
    fn integrator_ignores_a_zero_gravity_vector() {
        let mut integ = AttitudeIntegrator::new();
        assert!(integ.update([100.0, 0.0, 0.0], [0.0, 0.0, 0.0]).is_none());
    }

    #[test]
    fn integrator_does_not_move_without_rotation() {
        let mut integ = AttitudeIntegrator::new();
        let gravity = [0.0, 0.0, 1000.0];
        integ.update([0.0, 0.0, 0.0], gravity);
        for _ in 0..20 {
            std::thread::sleep(std::time::Duration::from_millis(2));
            integ.update([0.0, 0.0, 0.0], gravity);
        }
        let [yaw, pitch, roll] = integ.update([0.0, 0.0, 0.0], gravity).unwrap().angles();
        assert!(yaw.abs() < 0.01, "yaw drifted to {}", yaw);
        assert!(pitch.abs() < 0.01, "pitch drifted to {}", pitch);
        assert!(roll.abs() < 0.01, "roll drifted to {}", roll);
    }

    #[test]
    fn tilt_about_the_lateral_axis_moves_pitch_not_roll() {
        let mut integ = AttitudeIntegrator::new();
        let gravity = [0.0, 0.0, 1000.0];
        // Rotating about x (the lateral axis) is a nod when upright.
        let gyro = [4000.0, 0.0, 0.0];
        integ.update(gyro, gravity);
        for _ in 0..40 {
            std::thread::sleep(std::time::Duration::from_millis(2));
            integ.update(gyro, gravity);
        }
        let [_yaw, pitch, roll] = integ.update(gyro, gravity).unwrap().angles();
        assert!(pitch.abs() > 0.0, "pitch should move, got {}", pitch);
        assert!(
            roll.abs() < pitch.abs().max(0.5),
            "roll ({}) should stay smaller than pitch ({})",
            roll,
            pitch
        );
    }

    #[test]
    fn a_constant_gyro_bias_cannot_drift_pitch_past_the_gravity_anchor() {
        // The live failure (2026-11-09): the compass wandered away from the
        // actual head posture over minutes of use. Root cause: pitch was a
        // pure integral, and the bias learner only runs while perfectly
        // still - any residual rate error accumulated forever. With the
        // anchor, a sustained bias ends in a bounded steady state: the pull
        // cancels the bias drift rate.
        let mut integ = AttitudeIntegrator::new();
        let gravity = [0.0, 0.0, 1000.0];
        // Bias that old bias-learning misses: above the still-window blend
        // it would integrate forever. ~16 deg/s of phantom rotation.
        let bias = [130.0, 0.0, 0.0];

        // First second: capture the neutral gravity direction.
        for _ in 0..100 {
            integ.update([0.0, 0.0, 0.0], gravity);
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        // Then inject the bias for 30 simulated seconds of head motion that
        // never sits still long enough for the bias learner.
        for _ in 0..600 {
            integ.update(bias, gravity);
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        let [_yaw, pitch, _roll] = integ.update(bias, gravity).unwrap().angles();
        // Ungoverned, 30 s x 16 deg/s = 480 deg of drift (clamped to 90).
        // Governed, the pull bounds it near the bias/pull ratio (~7 deg for
        // this rate); give it generous room either way - the assertion is
        // that the anchor bites at all.
        assert!(
            pitch.abs() < 30.0,
            "pitch ran away to {} deg under a constant bias",
            pitch
        );
    }

    #[test]
    fn gravity_anchor_pulls_a_displaced_pitch_back() {
        let mut integ = AttitudeIntegrator::new();
        let gravity = [0.0, 0.0, 1000.0];
        for _ in 0..100 {
            integ.update([0.0, 0.0, 0.0], gravity);
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        // Physically nod down and hold: the device rotates -25° about the
        // lateral axis, so gravity in the device frame rotates +25° about
        // it: z tilts into -y ([0,-sin25,cos25]). The gyro sweeps -25 over
        // ~1 s and then stops; the anchor must pull the integral onto the
        // gravity observation, not fight the true pose.
        let held_gravity = [
            0.0,
            (-25.0f32).to_radians().sin() * 1000.0,
            (-25.0f32).to_radians().cos() * 1000.0,
        ];
        let gyro_rate = -25.0 / 0.122; // negative x: pitch decreases
        for _ in 0..50 {
            integ.update([gyro_rate, 0.0, 0.0], held_gravity);
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        for _ in 0..400 {
            integ.update([0.0, 0.0, 0.0], held_gravity);
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        let [_yaw, pitch, _roll] = integ.update([0.0, 0.0, 0.0], held_gravity).unwrap().angles();
        assert!(
            (pitch + 25.0).abs() < 6.0,
            "held -25 deg pose must read near -25, got {}",
            pitch
        );
    }

    #[test]
    fn gravity_anchor_ignores_an_unsane_gravity_magnitude() {
        // Outside the 0.5-2.0 band around the captured norm the sensor is
        // waking up, saturated, or the bud is out of an ear; the observation
        // must not steer the attitude.
        let mut anchor = GravityAnchor::default();
        let neutral = [0.0, 0.0, 1000.0];
        let lateral = [1.0, 0.0, 0.0];
        let forward = [0.0, 1.0, 0.0];
        // Capture (~1 s worth).
        let mut pitch = 0.0;
        for _ in 0..100 {
            let (p, _r) = anchor.correct(neutral, pitch, 0.0, lateral, forward);
            pitch = p;
        }
        // A tilted vector with a sane magnitude (~1000) pulls the pitch.
        let tilted = [0.0, 500.0, 866.0]; // 30 degrees off vertical, |g| ~= 1000
        let (moved, _) = anchor.correct(tilted, 0.0, 0.0, lateral, forward);
        assert!(
            moved.abs() > f32::EPSILON,
            "a sane tilted gravity vector must correct the pitch"
        );
        // The same tilt at 3x magnitude is out of band: no correction.
        let (unchanged, _) = anchor.correct(
            [0.0, 1500.0, 2598.0],
            0.0,
            0.0,
            lateral,
            forward,
        );
        assert_eq!(unchanged, 0.0, "an out-of-band magnitude must be ignored");
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
    fn integrator_reports_angular_velocity_in_degrees_per_second() {
        let mut integ = AttitudeIntegrator::new();
        let gravity = [0.0, 0.0, 1000.0];
        // Feed initial sample to anchor time.
        integ.update([0.0, 0.0, 0.0], gravity);
        std::thread::sleep(std::time::Duration::from_millis(10));
        // Rotating 5000 counts/s about z.
        // 5000 counts/s * 0.122 deg/s per count = ~610 deg/s.
        let sample = integ.update([0.0, 0.0, 5000.0], gravity).unwrap();
        assert!(
            (sample.yaw_rate - 610.0).abs() < 50.0,
            "yaw_rate should be near 610 deg/s, got {}",
            sample.yaw_rate
        );
    }

    #[test]
    fn remote_hid_pose_starts_level_and_stays_there_without_rotation() {
        // Orientation now comes from integrating the gyroscope, so a single
        // static frame - regardless of which way gravity points - reports the
        // integrated attitude, which starts at zero. The old parser read the
        // gravity vector as an angle, and that is what made a head turn look like
        // it did almost nothing.
        let mut tracker = HeadTracker::new();
        let (pose, _) = tracker
            .parse_packet(&remote_hid_frame(0, 1000, 0, 0, 0))
            .expect("should parse");
        assert!(pose.pitch.abs() < 0.5, "pitch={}", pose.pitch);
        assert!(pose.roll.abs() < 0.5, "roll={}", pose.roll);
        assert!(pose.yaw.abs() < 0.5, "yaw={}", pose.yaw);
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

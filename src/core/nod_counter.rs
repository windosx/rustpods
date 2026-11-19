//! Counts deliberate nods from the integrated pitch signal.
//!
//! The existing [`AxisDetector`](crate::core::head_tracking) recognises *a*
//! nod or shake from accelerometer oscillation, which is what the compass
//! badge shows. Answering a multiple-choice question needs something that
//! detector does not provide: a **count** of separate nods, committed once the
//! wearer stops. That is what this module adds.
//!
//! The signal is `HeadPose.pitch` (negative looks down). A nod is a dip below
//! the resting pitch followed by a return to it, so the detector is a
//! hysteresis state machine around a slowly tracked resting baseline:
//!
//! * `pitch < baseline − depth` arms the count (head is down),
//! * `pitch > baseline − return` commits one nod (head came back up),
//! * `QUIET_COMMIT_SECS` of silence after the last nod commits the total.
//!
//! The baseline only adapts while no count is pending, so pausing between
//! nods cannot drag the reference downwards and make the next nod harder to
//! reach. Turning the head (yaw) while a count is pending cancels it - that is
//! the "shake to dismiss" gesture, and it also stops a glance sideways from
//! being mistaken for the start of a nod sequence.

/// Gestural sensitivity, shared process-wide: 1 means only deliberate, large
/// movements count, 5 means light movements already do. Surfaced as a combo
/// on the preferences page; drives both the nod counter's thresholds and the
/// badge detector's amplitude gates.
pub const SENSITIVITY_DEFAULT: u8 = 3;
pub const SENSITIVITY_MIN: u8 = 1;
pub const SENSITIVITY_MAX: u8 = 5;

static SENSITIVITY: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(SENSITIVITY_DEFAULT);

/// Clamps and stores the process-wide sensitivity, returning the clamped
/// value.
pub fn set_sensitivity(value: u8) -> u8 {
    let value = value.clamp(SENSITIVITY_MIN, SENSITIVITY_MAX);
    SENSITIVITY.store(value, std::sync::atomic::Ordering::Relaxed);
    value
}

/// The current process-wide sensitivity.
pub fn sensitivity() -> u8 {
    SENSITIVITY.load(std::sync::atomic::Ordering::Relaxed)
}

/// How deep a nod must dip below the resting pitch, in degrees, per level.
fn nod_depth_for(sensitivity: u8) -> f32 {
    [9.0, 7.0, 4.8, 3.6, 2.6][(sensitivity - SENSITIVITY_MIN) as usize]
}

/// How large a yaw swing, in degrees, reads as a head shake per level:
/// higher sensitivity means a smaller turn already cancels.
fn yaw_cancel_for(sensitivity: u8) -> f32 {
    [18.0, 14.0, 10.0, 7.0, 5.0][(sensitivity - SENSITIVITY_MIN) as usize]
}

/// Multiplier for the badge detector's amplitude thresholds (accel counts).
/// Above 1 is stricter than the historical default, below 1 more eager.
pub fn badge_amplitude_scale() -> f32 {
    [2.0, 1.4, 1.0, 0.7, 0.5][(sensitivity() - SENSITIVITY_MIN) as usize]
}

/// Minimum spacing between counted nods, in seconds; filters the return-stroke
/// overshoot and settling of the preceding nod so one nod cannot count twice.
const MIN_NOD_INTERVAL: f32 = 0.30;
/// Silence after the last nod that commits the count, in seconds.
///
/// This must outlast the natural pause between two deliberate nods: a wearer
/// counting out "option two" regularly holds still between nods while they
/// check the count on the card.
const QUIET_COMMIT_SECS: f32 = 2.5;
/// Resting pitch is tracked with an EMA; this is its convergence rate in 1/s.
const BASELINE_RATE: f32 = 1.5;
/// The shake must dominate the nod motion by this ratio to cancel it.
const YAW_CANCEL_RATIO: f32 = 1.5;
/// Swing decay time constant in seconds: how long motion stays "recent".
const SWING_TAU: f32 = 0.8;
/// Largest integration step accepted, in seconds; bigger gaps reset hysteresis.
const MAX_DT: f32 = 0.25;
/// Peak-to-peak pitch motion within [`Self::BASELINE_WINDOW`] above which the
/// baseline is forbidden to move.
const BASELINE_WINDOW_SWING: f32 = 2.0;
/// Length of that recent-motion window, in seconds.
const BASELINE_WINDOW: f32 = 0.6;
/// A return stroke counts once the head has climbed this far back from the
/// dip's lowest point, as a fraction of the dip depth below baseline.
/// 0.35 allows crisp, prompt detection as soon as the upward rebound starts.
const RETURN_FROM_DIP: f32 = 0.35;
/// Rate of the always-on reference EMA, in 1/s. Unlike the gated baseline it
/// never freezes, so over a nod oscillation it settles on the cycle midline
/// (a 1 s-period wave is attenuated ~7x) - that midline is where a
/// mis-anchored baseline must be pulled back to.
const REF_BASELINE_RATE: f32 = 1.0;
/// When the head stays below the dip threshold this long while nothing is
/// counted yet, the baseline is anchored above the wearer's actual answering
/// posture (live: they nod while looking down at the RustPods card, ~19°
/// below the screen-watching baseline, and no up-stroke ever reaches the old
/// return band). Re-anchor to the reference EMA.
const REANCHOR_AFTER_SECS: f32 = 1.5;
/// A wearer who finishes a sequence parked below the dip threshold still
/// gets their commit - this many extra seconds on top of the quiet window.
const PARKED_COMMIT_GRACE: f32 = 1.0;

/// What one fed sample produced.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum NodEvent {
    /// One more nod was counted; carries the running total.
    Counted(usize),
    /// The wearer stopped nodding; carries the final total.
    Commit(usize),
    /// A head shake (or large turn) cancelled the pending count.
    Cancelled,
    /// A distinct head shake (alternating left-right turn) without a pending nod count.
    Shaken,
}

/// State of an alternating head shake gesture.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ShakePhase {
    None,
    TurnedLeft,
    TurnedRight,
}

/// Counts nod cycles on the pitch axis. Feed it every pose sample while a
/// question is pending, and [`NodCounter::reset`] it when none is.
///
/// Thresholds follow the process-wide sensitivity (see [`set_sensitivity`]):
/// construct with [`NodCounter::with_sensitivity`] where the level is known,
/// and rebuild when it changes.
#[derive(Debug, Clone)]
pub struct NodCounter {
    /// The sensitivity level these thresholds were built from, so a reset
    /// restores the same behaviour instead of silently reverting to the
    /// default level.
    sensitivity: u8,
    /// How far below the resting pitch a nod must dip, in degrees.
    depth: f32,
    /// Yaw swing, in degrees, above which a pending count is a head shake.
    yaw_cancel: f32,
    /// Resting pitch in degrees; re-seeded from the first sample after a reset.
    baseline: Option<f32>,
    /// Always-on reference EMA. It tracks the pitch unconditionally - through
    /// nods, holds and posture changes - and serves as the re-anchoring
    /// target when the gated baseline is provably in the wrong place.
    ref_ema: Option<f32>,
    /// Resting yaw, the reference the yaw swing is measured against.
    yaw_ref: Option<f32>,
    /// Recent pitch motion magnitude, decays exponentially.
    pitch_swing: f32,
    /// Recent yaw motion magnitude, decays exponentially.
    yaw_swing: f32,
    /// Ring of recent `(time, pitch, yaw)` samples for the baseline gate's
    /// peak-to-peak test and head shake recognition; older than [`BASELINE_WINDOW`] falls off.
    window: Vec<(f32, f32, f32)>,
    /// Running clock, seconds since construction.
    clock: f32,
    /// Lowest pitch reached since the current dip armed; the count
    /// threshold is measured from here, not from the baseline.
    dip_low: f32,
    /// Highest pitch since the last count (or since construction); the arm
    /// threshold is measured from here. See the hysteresis comment in
    /// [`Self::update`].
    peak: Option<f32>,
    /// Hysteresis state: is the head currently below the dip threshold?
    down: bool,
    /// Seconds the head has been continuously below the dip threshold; the
    /// re-anchor trigger.
    down_secs: f32,
    /// Nods counted so far in the current sequence.
    count: usize,
    /// Seconds since the last counted nod.
    since_count: f32,
    /// Seconds since the last emitted head shake event.
    since_shake: f32,
    /// Pending direction of an alternating head shake.
    shake_phase: ShakePhase,
    /// Time elapsed in the current shake phase.
    shake_timer: f32,
    /// Deepest pitch excursion below the baseline since the last
    /// [`Self::take_telemetry`]. A 1 Hz heartbeat would otherwise miss a
    /// 300 ms nod entirely, and "nods never count" is undiagnosable without
    /// knowing the depth the counter actually saw.
    tel_min_dev: f32,
    /// Largest yaw excursion from the yaw reference since the last
    /// [`Self::take_telemetry`], for the same reason.
    tel_max_yaw_dev: f32,
}

impl Default for NodCounter {
    fn default() -> Self {
        Self::new()
    }
}

impl NodCounter {
    /// A counter at the default sensitivity.
    pub fn new() -> Self {
        Self::with_sensitivity(SENSITIVITY_DEFAULT)
    }

    /// A counter with the thresholds for the given sensitivity level.
    pub fn with_sensitivity(sensitivity: u8) -> Self {
        let sensitivity = sensitivity.clamp(SENSITIVITY_MIN, SENSITIVITY_MAX);
        let depth = nod_depth_for(sensitivity);
        Self {
            sensitivity,
            depth,
            yaw_cancel: yaw_cancel_for(sensitivity),
            baseline: None,
            ref_ema: None,
            yaw_ref: None,
            pitch_swing: 0.0,
            yaw_swing: 0.0,
            window: Vec::new(),
            clock: 0.0,
            dip_low: 0.0,
            peak: None,
            down: false,
            down_secs: 0.0,
            count: 0,
            // A fresh counter has no recent count to debounce against, so the
            // first nod may arm immediately - otherwise a wearer answering
            // right away loses the nod that crosses within the first 150 ms.
            since_count: MIN_NOD_INTERVAL,
            since_shake: 0.5,
            shake_phase: ShakePhase::None,
            shake_timer: 0.0,
            tel_min_dev: 0.0,
            tel_max_yaw_dev: 0.0,
        }
    }

    /// Clears everything; the next sample re-seeds the baselines.
    pub fn reset(&mut self) {
        *self = Self::with_sensitivity(self.sensitivity);
    }

    /// Clears the gesture state but keeps the tracked resting pose.
    ///
    /// Used when a question ends: the next question then starts from the
    /// baseline the wearer settled into, not from whatever pose they happen
    /// to hold at the instant it registers. A full re-seed caught mid-nod
    /// anchors the thresholds to the bottom of the nod - seen live, every
    /// following nod was then unreachable for minutes.
    pub fn reset_count(&mut self) {
        self.down = false;
        self.peak = None;
        self.count = 0;
        self.since_count = MIN_NOD_INTERVAL;
        self.since_shake = 0.5;
        self.shake_phase = ShakePhase::None;
        self.shake_timer = 0.0;
        self.window.clear();
    }

    /// Feeds one sample while nothing is pending, keeping only the resting
    /// pose warm: the swing memory decays and the baselines follow the
    /// wearer, but nothing is armed, counted or cancelled.
    ///
    /// The baseline moves unconditionally here (unlike [`Self::update`]'s
    /// stillness gate): a wearer who shifts to a new resting pose while no
    /// question is up must not keep a stale reference, and the drag a nod
    /// itself causes cancels out over its down-and-up stroke. By the time a
    /// question registers, the baseline already sits on their rest.
    pub fn observe_idle(&mut self, pitch_deg: f32, yaw_deg: f32, dt: f32) {
        let dt = dt.clamp(0.0, MAX_DT);
        let decay = (-dt / SWING_TAU).exp();
        self.pitch_swing *= decay;
        self.yaw_swing *= decay;
        let baseline = *self.baseline.get_or_insert(pitch_deg);
        let yaw_ref = *self.yaw_ref.get_or_insert(yaw_deg);
        let ref_ema = *self.ref_ema.get_or_insert(pitch_deg);
        self.yaw_ref = Some(yaw_ref + (yaw_deg - yaw_ref) * (BASELINE_RATE * dt).min(1.0));
        self.baseline =
            Some(baseline + (pitch_deg - baseline) * (BASELINE_RATE * dt).min(1.0));
        self.ref_ema =
            Some(ref_ema + (pitch_deg - ref_ema) * (REF_BASELINE_RATE * dt).min(1.0));
    }

    /// Nods counted so far in the current sequence.
    pub fn count(&self) -> usize {
        self.count
    }

    /// Motion extremes since the last call: `(deepest pitch excursion below
    /// the baseline, largest yaw excursion from the yaw reference)`, reset on
    /// read. Feeds the pending-question heartbeat.
    pub fn take_telemetry(&mut self) -> (f32, f32) {
        let out = (self.tel_min_dev, self.tel_max_yaw_dev);
        self.tel_min_dev = 0.0;
        self.tel_max_yaw_dev = 0.0;
        out
    }

    /// `(baseline, pitch swing, count, armed)` at this instant, for the
    /// pending-question heartbeat.
    pub fn snapshot(&self) -> (Option<f32>, f32, usize, bool) {
        (self.baseline, self.pitch_swing, self.count, self.down)
    }

    /// Feeds one sample (`dt` in seconds) and returns what it produced.
    ///
    /// Call this for every pose sample while a question is pending. Commit is
    /// emitted once, after `QUIET_COMMIT_SECS` of silence following the last
    /// nod; after that the counter resets itself and is ready for the next
    /// question without an explicit reset.
    pub fn update(&mut self, pitch_deg: f32, yaw_deg: f32, dt: f32) -> Option<NodEvent> {
        // A stream gap must not leave stale hysteresis state behind: force the
        // state machine back to neutral. Baselines and the running count
        // survive, so a hiccup does not discard real work.
        let dt = dt.clamp(0.0, MAX_DT);
        if dt >= MAX_DT {
            self.down = false;
            // Re-seed the peak from the post-gap pitch: the old peak is from
            // a stale context, and a nod straddling the gap must re-arm on
            // fresh motion.
            self.peak = None;
        }
        self.since_count += dt;
        self.since_shake += dt;
        self.shake_timer += dt;
        if self.shake_timer > 0.6 {
            self.shake_phase = ShakePhase::None;
        }
        self.clock += dt;

        let decay = (-dt / SWING_TAU).exp();
        self.pitch_swing *= decay;
        self.yaw_swing *= decay;

        // Seed the baselines on the first sample after a reset: whatever pose
        // the wearer holds when the question arrives is the resting pose.
        let baseline = *self.baseline.get_or_insert(pitch_deg);
        let yaw_ref = *self.yaw_ref.get_or_insert(yaw_deg);
        // The reference EMA never freezes and never gates: over a nod
        // oscillation it settles on the cycle midline, which is exactly where
        // a mis-anchored baseline must be pulled back to. Slow enough that a
        // single nod moves it little (BASELINE_RATE=1.5/s over a 0.3 s nod
        // shifts it ~30% of the excursion's midpoint deficit).
        let ref_ema = *self.ref_ema.get_or_insert(pitch_deg);
        self.ref_ema = Some(ref_ema + (pitch_deg - ref_ema) * (REF_BASELINE_RATE * dt).min(1.0));

        self.tel_min_dev = self.tel_min_dev.min(pitch_deg - baseline);
        self.tel_max_yaw_dev = self.tel_max_yaw_dev.max((yaw_deg - yaw_ref).abs());

        self.pitch_swing = self.pitch_swing.max((pitch_deg - baseline).abs());
        self.yaw_swing = self.yaw_swing.max((yaw_deg - yaw_ref).abs());

        // A deliberate head turn while a count is pending means "dismiss":
        // cancel, and never let it half-count as a nod.
        if self.count > 0
            && self.yaw_swing > self.yaw_cancel
            && self.yaw_swing > self.pitch_swing * YAW_CANCEL_RATIO
        {
            self.reset();
            self.since_shake = 0.0;
            return Some(NodEvent::Cancelled);
        }

        // The resting pitch only adapts while nothing is pending *and* the
        // head has been genuinely quiet: peak-to-peak over the last
        // BASELINE_WINDOW seconds must be under the gate. A decaying-swing
        // gate read 0.2° *during* a live nod (the decay outruns the nod),
        // let the baseline chase the dip, and the return stroke never
        // reached the band it had dragged down with it.
        self.window.push((self.clock, pitch_deg, yaw_deg));
        self.window.retain(|(t, _, _)| self.clock - t <= BASELINE_WINDOW);
        let (w_min, w_max, y_min, y_max) = self.window.iter().fold(
            (f32::MAX, f32::MIN, f32::MAX, f32::MIN),
            |(p_lo, p_hi, y_lo, y_hi), (_, p, y)| {
                (p_lo.min(*p), p_hi.max(*p), y_lo.min(*y), y_hi.max(*y))
            },
        );
        if self.count == 0 && w_max - w_min < BASELINE_WINDOW_SWING {
            self.baseline =
                Some(baseline + (pitch_deg - baseline) * (BASELINE_RATE * dt).min(1.0));
        }
        if self.count == 0 && y_max - y_min < BASELINE_WINDOW_SWING {
            self.yaw_ref =
                Some(yaw_ref + (yaw_deg - yaw_ref) * (BASELINE_RATE * dt).min(1.0));
        }

        // Deliberate head shake without a pending nod count:
        // Alternating yaw excursion across the resting yaw with yaw dominating pitch.
        if self.count == 0 && !self.down && self.since_shake >= 0.4 {
            let yaw_dev = yaw_deg - yaw_ref;
            let thresh = self.yaw_cancel * 0.55;

            match self.shake_phase {
                ShakePhase::None => {
                    if self.yaw_swing > self.pitch_swing * YAW_CANCEL_RATIO {
                        if yaw_dev <= -thresh {
                            self.shake_phase = ShakePhase::TurnedLeft;
                            self.shake_timer = 0.0;
                        } else if yaw_dev >= thresh {
                            self.shake_phase = ShakePhase::TurnedRight;
                            self.shake_timer = 0.0;
                        }
                    }
                }
                ShakePhase::TurnedLeft => {
                    if yaw_dev >= thresh && self.yaw_swing > self.pitch_swing * YAW_CANCEL_RATIO {
                        self.shake_phase = ShakePhase::None;
                        self.since_shake = 0.0;
                        self.window.clear();
                        return Some(NodEvent::Shaken);
                    }
                }
                ShakePhase::TurnedRight => {
                    if yaw_dev <= -thresh && self.yaw_swing > self.pitch_swing * YAW_CANCEL_RATIO {
                        self.shake_phase = ShakePhase::None;
                        self.since_shake = 0.0;
                        self.window.clear();
                        return Some(NodEvent::Shaken);
                    }
                }
            }
        }

        // Symmetric peak/dip hysteresis. A nod is a *relative* motion: the
        // arm threshold is DEPTH below the recent peak, the count threshold
        // is half a DEPTH above the recent dip - both anchored on the
        // oscillation itself, never on the rest pose. Earlier revisions
        // anchored the return on the baseline (live: the head never returns
        // above rest, so no nod counted) or the climb-out on the rest or
        // last-count pitch (live: a wearer answering in a lowered posture
        // never gets back there either, so the second nod never armed; and
        // with a window anchor the same climb re-armed and counted twice).
        // Peak/dip needs no posture anchor at all.
        self.peak = self.peak.map_or(Some(pitch_deg), |p| Some(p.max(pitch_deg)));
        if !self.down {
            if pitch_deg < self.peak.unwrap_or(pitch_deg) - self.depth
                && self.since_count >= MIN_NOD_INTERVAL
            {
                self.down = true;
                self.down_secs = 0.0;
                self.dip_low = pitch_deg;
            }
        } else {
            self.down_secs += dt;
            self.dip_low = self.dip_low.min(pitch_deg);
            // The peak rides along while armed: without this, a slow
            // continuous nodding motion inflates the gap between peak and
            // current pitch until every next cycle re-arms immediately.
            if let Some(p) = self.peak.as_mut() {
                *p = (*p).max(pitch_deg);
            }
            // Head parked below the dip threshold with nothing counted: the
            // baseline is anchored above the wearer's actual answering
            // posture (they nod while reading the question card, ~19° below
            // the screen-watching baseline - live telemetry, 2026-11-09).
            // Re-anchor onto the always-on reference EMA, which has settled
            // on the midline of their actual answering oscillation.
            if self.count == 0 && self.down_secs > REANCHOR_AFTER_SECS
                && let Some(reference) = self.ref_ema {
                    self.baseline = Some(reference);
                    self.down_secs = 0.0;
                    self.down = false;
                    self.peak = Some(pitch_deg);
                    self.dip_low = pitch_deg;
                    return None;
                }
            // Return stroke: climbed halfway out of the dip.
            if pitch_deg > self.dip_low + self.depth * RETURN_FROM_DIP {
                self.count += 1;
                self.since_count = 0.0;
                // Reset the reference: the next dip must fall DEPTH below
                // the peak this stroke reached - the stroke's own top.
                self.peak = Some(pitch_deg);
                self.down = false;
                return Some(NodEvent::Counted(self.count));
            }
        }

        // The wearer has stopped nodding: commit whatever was counted. The
        // head must be back near the baseline first - a slow, deliberate nod
        // can spend longer than the quiet window on the way down, and
        // committing mid-dip would truncate the count. A wearer who finishes
        // a sequence parked below the dip threshold (still `down`) gets the
        // commit after the quiet window plus the grace period instead of
        // never.
        let quiet_long_enough = if self.down {
            self.since_count > QUIET_COMMIT_SECS + PARKED_COMMIT_GRACE
        } else {
            self.since_count > QUIET_COMMIT_SECS
        };
        if self.count > 0 && quiet_long_enough {
            let total = self.count;
            self.reset();
            return Some(NodEvent::Commit(total));
        }

        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DT: f32 = 0.01;

    /// Feeds a straight-line ramp between two pitch values at 100 Hz.
    fn ramp(counter: &mut NodCounter, from: f32, to: f32, secs: f32, yaw: f32) -> Vec<NodEvent> {
        let steps = (secs / DT).round() as i32;
        let mut events = Vec::new();
        for i in 1..=steps {
            let t = i as f32 / steps as f32;
            let pitch = from + (to - from) * t;
            events.extend(counter.update(pitch, yaw, DT));
        }
        events
    }

    fn hold(counter: &mut NodCounter, pitch: f32, secs: f32, yaw: f32) -> Vec<NodEvent> {
        let steps = (secs / DT).round() as i32;
        let mut events = Vec::new();
        for _ in 0..steps {
            events.extend(counter.update(pitch, yaw, DT));
        }
        events
    }

    /// One nod: down over 0.15 s, hold briefly, back up over 0.15 s.
    fn nod(counter: &mut NodCounter, depth: f32) -> Vec<NodEvent> {
        nod_from(counter, 0.0, depth)
    }

    /// One nod from an arbitrary resting pitch `rest` down to `depth`.
    /// A nod is relative to wherever the head currently is; a helper that
    /// always started at 0.0 injected a phantom 19° up-jump when the head
    /// was held lower, and the counter dutifully counted it.
    fn nod_from(counter: &mut NodCounter, rest: f32, depth: f32) -> Vec<NodEvent> {
        let mut events = ramp(counter, rest, depth, 0.15, 0.0);
        events.extend(hold(counter, depth, 0.05, 0.0));
        events.extend(ramp(counter, depth, rest, 0.15, 0.0));
        events
    }

    #[test]
    fn three_nods_commit_three() {
        let mut counter = NodCounter::new();
        let mut events = Vec::new();
        for _ in 0..3 {
            events.extend(nod(&mut counter, -10.0));
            events.extend(hold(&mut counter, 0.0, 0.3, 0.0));
        }
        events.extend(hold(&mut counter, 0.0, 2.8, 0.0));

        let counts: Vec<_> = events
            .iter()
            .filter_map(|e| match e {
                NodEvent::Counted(n) => Some(*n),
                _ => None,
            })
            .collect();
        assert_eq!(counts, vec![1, 2, 3]);
        assert!(
            events.contains(&NodEvent::Commit(3)),
            "expected a commit of 3, got {events:?}"
        );
        // After the commit the counter is ready for the next question.
        assert_eq!(counter.count(), 0);
    }

    #[test]
    fn single_nod_commits_one() {
        let mut counter = NodCounter::new();
        let mut events = nod(&mut counter, -10.0);
        events.extend(hold(&mut counter, 0.0, 2.8, 0.0));
        assert_eq!(
            events.last(),
            Some(&NodEvent::Commit(1)),
            "a lone nod must still commit"
        );
    }

    #[test]
    fn two_nods_with_a_counting_pause_commit_two() {
        let mut counter = NodCounter::new();
        let mut events = nod(&mut counter, -10.0);
        assert!(events.contains(&NodEvent::Counted(1)), "{events:?}");
        // The wearer checks the count on the card before deciding to nod
        // again: this pause sits between the old 1.4 s window and the new
        // one, and live it truncated every such answer to "option one".
        events.extend(hold(&mut counter, 0.0, 2.0, 0.0));
        assert!(
            !events.iter().any(|e| matches!(e, NodEvent::Commit(_))),
            "a 2 s pause between nods must not commit early: {events:?}"
        );
        events.extend(nod(&mut counter, -10.0));
        events.extend(hold(&mut counter, 0.0, 2.8, 0.0));
        assert!(
            events.contains(&NodEvent::Commit(2)),
            "the second nod must land in the same sequence: {events:?}"
        );
    }

    #[test]
    fn idle_observations_never_count() {
        let mut counter = NodCounter::new();
        // Vigorous nodding while nothing is pending must arm nothing.
        for i in 0..100 {
            let pitch = if i % 20 < 10 { -10.0 } else { 0.0 };
            counter.observe_idle(pitch, 0.0, DT);
        }
        assert_eq!(counter.count(), 0);
        assert!(!counter.down);
    }

    #[test]
    fn idle_observations_keep_the_baseline_warm() {
        let mut counter = NodCounter::new();
        // Nothing pending: the wearer shifts to resting 12 degrees down.
        // The idle baseline follows unconditionally.
        for _ in 0..400 {
            counter.observe_idle(-12.0, 0.0, DT);
        }
        assert_eq!(counter.count(), 0);
        // A question registers; the wearer nods relative to their rest pose.
        let mut events = ramp(&mut counter, -12.0, -22.0, 0.15, 0.0);
        events.extend(hold(&mut counter, -22.0, 0.05, 0.0));
        events.extend(ramp(&mut counter, -22.0, -12.0, 0.15, 0.0));
        events.extend(hold(&mut counter, -12.0, 2.8, 0.0));
        assert!(
            events.contains(&NodEvent::Commit(1)),
            "a nod relative to the idle-warmed baseline must count: {events:?}"
        );
    }

    #[test]
    fn reset_count_keeps_the_warm_baseline() {
        let mut counter = NodCounter::new();
        for _ in 0..400 {
            counter.observe_idle(-12.0, 0.0, DT);
        }
        // A question came and went: the count clears, the resting pose
        // survives, and the next question starts from the same reference.
        counter.update(-22.0, 0.0, DT);
        counter.reset_count();
        assert_eq!(counter.count(), 0);
        assert!(!counter.down);
        let mut events = ramp(&mut counter, -12.0, -22.0, 0.15, 0.0);
        events.extend(hold(&mut counter, -22.0, 0.05, 0.0));
        events.extend(ramp(&mut counter, -22.0, -12.0, 0.15, 0.0));
        events.extend(hold(&mut counter, -12.0, 2.8, 0.0));
        assert!(
            events.contains(&NodEvent::Commit(1)),
            "the next question must reuse the warm baseline: {events:?}"
        );
    }

    #[test]
    fn nods_in_a_lowered_answering_posture_still_count() {
        // Live telemetry (2026-11-09, Q1): the wearer reads the question
        // card ~19° below the screen-watching baseline and nods there. The
        // old gated baseline never adapted (the nod keeps the swing gate
        // closed), so no up-stroke ever reached the return band and not one
        // nod counted for the whole 172 s window.
        let mut counter = NodCounter::new();
        // Question arrives while looking at the screen: baseline 0.
        hold(&mut counter, 0.0, 0.5, 0.0);
        // The wearer lowers their head to read the card, then nods there.
        hold(&mut counter, -19.0, 2.0, 0.0);
        let mut events = nod_from(&mut counter, -19.0, -26.0);
        events.extend(hold(&mut counter, -19.0, 0.3, 0.0));
        events.extend(nod_from(&mut counter, -19.0, -26.0));
        events.extend(hold(&mut counter, -19.0, 2.8, 0.0));
        assert!(
            events.contains(&NodEvent::Commit(2)),
            "two nods made while looking down must commit: {events:?}"
        );
    }

    #[test]
    fn a_fast_shallow_nod_from_rest_counts_without_returning_above_rest() {
        // Live telemetry 02:38:17: the wearer rests at +2°, nods quickly to
        // about -7° and back to -2°. The old rules needed the head back above
        // baseline - return_band; a real return stroke stops AT the rest
        // pose, below that line, so the nod never counted even though the
        // dip was plainly past the depth threshold.
        let mut counter = NodCounter::new();
        hold(&mut counter, 2.0, 0.5, 0.0);
        let mut events = ramp(&mut counter, 2.0, -7.0, 0.3, 0.0);
        events.extend(hold(&mut counter, -7.0, 0.2, 0.0));
        events.extend(ramp(&mut counter, -7.0, -2.0, 0.5, 0.0));
        assert!(
            events.contains(&NodEvent::Counted(1)),
            "a nod whose return stops at the rest pose must still count: {events:?}"
        );
        // And with a pause between two such nods, both count.
        events.extend(hold(&mut counter, -2.0, 0.4, 0.0));
        events.extend(ramp(&mut counter, -2.0, -11.0, 0.3, 0.0));
        events.extend(hold(&mut counter, -11.0, 0.2, 0.0));
        events.extend(ramp(&mut counter, -11.0, -2.0, 0.5, 0.0));
        events.extend(hold(&mut counter, -2.0, 2.8, 0.0));
        assert!(
            events.contains(&NodEvent::Commit(2)),
            "the second nod in the lowered posture must complete the pair: {events:?}"
        );
    }

    #[test]
    fn a_nod_cannot_drag_the_baseline_down() {
        // The old swing-decay gate read 0.2-0.5° DURING the nod (decay
        // outruns the motion), so the baseline chased the dip: live it moved
        // +2.0 → -5.0 within one nod, pulling the return band below the
        // return stroke. The peak-to-peak window gate must hold the
        // baseline flat through the dip.
        let mut counter = NodCounter::new();
        hold(&mut counter, 2.0, 0.5, 0.0);
        let before = counter.snapshot().0;
        ramp(&mut counter, 2.0, -7.0, 0.3, 0.0);
        hold(&mut counter, -7.0, 0.2, 0.0);
        let during = counter.snapshot().0;
        let (Some(before), Some(during)) = (before, during) else {
            panic!("baselines must be seeded");
        };
        assert!(
            (during - before).abs() < 1.0,
            "baseline moved {:.1}° during a nod - it must stay put",
            during - before
        );
    }

    #[test]
    fn telemetry_captures_the_nod_depth() {
        let mut counter = NodCounter::new();
        nod(&mut counter, -10.0);
        let (min_dev, max_yaw) = counter.take_telemetry();
        assert!(
            min_dev <= -6.0,
            "a full nod must show as a deep excursion: {min_dev}"
        );
        assert_eq!(max_yaw, 0.0);
        // Reading resets the window.
        let (min_dev, _) = counter.take_telemetry();
        assert_eq!(min_dev, 0.0);
    }

    #[test]
    fn continuous_nodding_does_not_commit_until_quiet() {
        let mut counter = NodCounter::new();
        let mut events = Vec::new();
        // Four nods back to back with no pause beyond the return stroke.
        for _ in 0..4 {
            events.extend(nod(&mut counter, -10.0));
        }
        assert!(
            !events.contains(&NodEvent::Commit(4)),
            "must not commit while nodding continues"
        );
        let counted = events
            .iter()
            .filter(|e| matches!(e, NodEvent::Counted(_)))
            .count();
        assert_eq!(counted, 4);

        events.extend(hold(&mut counter, 0.0, 2.8, 0.0));
        assert!(
            events.contains(&NodEvent::Commit(4)),
            "silence must commit the running total"
        );
    }

    #[test]
    fn small_noise_never_counts() {
        let mut counter = NodCounter::new();
        // Jitter well under NOD_DEPTH for ten seconds.
        for i in 0..1000 {
            let jitter = if i % 2 == 0 { -1.5 } else { 1.5 };
            assert_eq!(counter.update(jitter, 0.0, DT), None);
        }
        assert_eq!(counter.count(), 0);
    }

    #[test]
    fn shallow_dips_do_not_count() {
        let mut counter = NodCounter::new();
        for _ in 0..4 {
            let events = nod(&mut counter, -4.0);
            assert!(
                events.is_empty(),
                "a dip shallower than NOD_DEPTH must not count: {events:?}"
            );
            hold(&mut counter, 0.0, 0.3, 0.0);
        }
        assert_eq!(counter.count(), 0);
    }

    #[test]
    fn head_shake_cancels_the_pending_count() {
        let mut counter = NodCounter::new();
        nod(&mut counter, -10.0);
        assert_eq!(counter.count(), 1);

        // Turn the head side to side while pitch stays still.
        let mut events = Vec::new();
        for _ in 0..6 {
            events.extend(ramp(&mut counter, 0.0, 0.0, 0.15, 15.0));
            events.extend(ramp(&mut counter, 0.0, 0.0, 0.15, -15.0));
        }
        assert!(
            events.contains(&NodEvent::Cancelled),
            "a yaw swing while pending must cancel: {events:?}"
        );
        assert_eq!(counter.count(), 0);
        // And the cancelled state must not produce a commit later.
        let tail = hold(&mut counter, 0.0, 2.0, 0.0);
        assert!(tail.is_empty(), "cancelled count must not commit: {tail:?}");
    }

    #[test]
    fn slow_nods_are_not_committed_mid_dip() {
        let mut counter = NodCounter::new();
        // First slow nod (0.9 s down, 0.2 s up), then start a second one.
        let mut events = ramp(&mut counter, 0.0, -10.0, 0.9, 0.0);
        events.extend(ramp(&mut counter, -10.0, 0.0, 0.2, 0.0));
        events.extend(hold(&mut counter, 0.0, 0.35, 0.0));
        assert!(events.contains(&NodEvent::Counted(1)));
        events.extend(ramp(&mut counter, 0.0, -10.0, 0.9, 0.0));
        assert_eq!(counter.count(), 1, "second dip still in progress");

        // Hold the head down: more than QUIET_COMMIT_SECS now elapsed since
        // the first count. The old rule committed right here, truncating the
        // wearer's intent to option 1 while their head was still nodding.
        events.extend(hold(&mut counter, -10.0, 0.4, 0.0));
        assert!(
            !events.iter().any(|e| matches!(e, NodEvent::Commit(_))),
            "must not commit while mid-dip: {events:?}"
        );

        // Finish the nod and rest: the full count commits.
        events.extend(ramp(&mut counter, -10.0, 0.0, 0.2, 0.0));
        events.extend(hold(&mut counter, 0.0, 2.8, 0.0));
        assert!(
            events.contains(&NodEvent::Commit(2)),
            "expected commit of 2 after the slow nods: {events:?}"
        );
    }

    #[test]
    fn counting_survives_a_stream_gap() {
        let mut counter = NodCounter::new();
        nod(&mut counter, -10.0);
        assert_eq!(counter.count(), 1);
        // A huge dt (tracking stutter) resets hysteresis but not the count.
        let _ = counter.update(0.0, 0.0, 5.0);
        assert_eq!(counter.count(), 1);
        let mut events = nod(&mut counter, -10.0);
        events.extend(hold(&mut counter, 0.0, 2.8, 0.0));
        assert!(
            events.contains(&NodEvent::Commit(2)),
            "the second nod after a gap must still commit: {events:?}"
        );
    }

    #[test]
    fn sensitivity_5_counts_a_shallow_nod_the_default_rejects() {
        // A 4-degree dip: below the level-3 depth (6°) but past level 5
        // (3.5°). The two counters must disagree exactly as labelled.
        let nod_at = |sensitivity: u8| {
            let mut counter = NodCounter::with_sensitivity(sensitivity);
            let mut events = ramp(&mut counter, 0.0, -4.0, 0.15, 0.0);
            events.extend(hold(&mut counter, -4.0, 0.05, 0.0));
            events.extend(ramp(&mut counter, -4.0, 0.0, 0.15, 0.0));
            events.extend(hold(&mut counter, 0.0, 2.8, 0.0));
            events
        };
        assert_eq!(nod_at(3).len(), 0, "level 3 must ignore a 4° nod");
        assert!(
            nod_at(5).contains(&NodEvent::Commit(1)),
            "level 5 must count a 4° nod"
        );
    }

    #[test]
    fn sensitivity_1_requires_deliberate_nods() {
        let mut counter = NodCounter::with_sensitivity(1);
        // Level 1 needs a 10° dip: a standard-depth 6° nod is ignored.
        let mut events = nod(&mut counter, -6.0);
        events.extend(hold(&mut counter, 0.0, 2.8, 0.0));
        assert!(events.is_empty(), "level 1 must ignore a 6° nod: {events:?}");
        // A deliberate 12° nod still counts.
        let mut events = nod(&mut counter, -12.0);
        events.extend(hold(&mut counter, 0.0, 2.8, 0.0));
        assert!(events.contains(&NodEvent::Commit(1)), "{events:?}");
    }

    #[test]
    fn reset_keeps_the_counter_sensitivity() {
        let mut counter = NodCounter::with_sensitivity(5);
        counter.update(-4.0, 0.0, 0.01);
        counter.reset();
        // Still level 5: a fresh 4° nod must count again after the reset.
        let mut events = ramp(&mut counter, 0.0, -4.0, 0.15, 0.0);
        events.extend(ramp(&mut counter, -4.0, 0.0, 0.15, 0.0));
        events.extend(hold(&mut counter, 0.0, 2.8, 0.0));
        assert!(
            events.contains(&NodEvent::Commit(1)),
            "reset must not revert the sensitivity: {events:?}"
        );
    }

    #[test]
    fn baseline_reseeds_where_the_question_finds_the_head() {
        let mut counter = NodCounter::new();
        // The wearer is already looking 15 degrees down when the question pops.
        hold(&mut counter, -15.0, 0.5, 0.0);
        // One nod: down to -25 and back to the resting -15.
        let mut events = ramp(&mut counter, -15.0, -25.0, 0.15, 0.0);
        events.extend(hold(&mut counter, -25.0, 0.05, 0.0));
        events.extend(ramp(&mut counter, -25.0, -15.0, 0.15, 0.0));
        events.extend(hold(&mut counter, -15.0, 2.8, 0.0));
        assert!(
            events.contains(&NodEvent::Commit(1)),
            "a nod relative to the current head pose must count: {events:?}"
        );
    }

    #[test]
    fn finishing_parked_below_the_dip_threshold_still_commits() {
        // The wearer nods once and then leaves their head below the dip
        // threshold instead of returning to rest - the count must still
        // commit after the quiet window plus the grace period, not never.
        let mut counter = NodCounter::new();
        hold(&mut counter, 0.0, 0.5, 0.0);
        let mut events = nod(&mut counter, -10.0);
        assert!(events.contains(&NodEvent::Counted(1)), "{events:?}");
        // Parked below the threshold: still `down`, no return stroke.
        events.extend(hold(
            &mut counter,
            -12.0,
            QUIET_COMMIT_SECS + PARKED_COMMIT_GRACE + 0.2,
            0.0,
        ));
        assert!(
            events.contains(&NodEvent::Commit(1)),
            "a sequence finished parked below the threshold must commit: {events:?}"
        );
    }

    #[test]
    fn shaking_head_without_prior_nod_emits_shaken() {
        let mut counter = NodCounter::new();
        // Settle at neutral resting yaw 0.0.
        hold(&mut counter, 0.0, 0.5, 0.0);
        // Shake head: turn left to -12°, then right to +12°, then back to 0°
        let mut events = Vec::new();
        events.extend(ramp(&mut counter, 0.0, 0.0, 0.15, -12.0));
        events.extend(ramp(&mut counter, 0.0, 0.0, 0.20, 12.0));
        events.extend(ramp(&mut counter, 0.0, 0.0, 0.15, 0.0));
        assert!(
            events.contains(&NodEvent::Shaken),
            "alternating yaw swings must produce Shaken event: {events:?}"
        );
    }

    #[test]
    fn unidirectional_glance_does_not_emit_shaken() {
        let mut counter = NodCounter::new();
        hold(&mut counter, 0.0, 0.5, 0.0);
        // Turn right to +15° and stay there (a glance, not a shake)
        let mut events = Vec::new();
        events.extend(ramp(&mut counter, 0.0, 0.0, 0.20, 15.0));
        events.extend(hold(&mut counter, 0.0, 0.5, 15.0));
        assert!(
            !events.contains(&NodEvent::Shaken),
            "unidirectional glance must not trigger Shaken: {events:?}"
        );
    }
}


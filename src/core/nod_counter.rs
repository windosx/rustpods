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
    [10.0, 8.0, 6.0, 4.5, 3.5][(sensitivity - SENSITIVITY_MIN) as usize]
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

/// Minimum spacing between counted nods, in seconds; filters single-sample bounce.
const MIN_NOD_INTERVAL: f32 = 0.15;
/// Silence after the last nod that commits the count, in seconds.
const QUIET_COMMIT_SECS: f32 = 1.4;
/// Resting pitch is tracked with an EMA; this is its convergence rate in 1/s.
const BASELINE_RATE: f32 = 1.5;
/// The baseline only adapts while the head is essentially still - pitch swing
/// below this. A deadband on the deviation cannot do this job: an EMA chasing
/// a ramp lags by slope/rate (a fast nod at ~27°/s already lags ~4° within a
/// tenth of a second), which silently deepens the very thresholds the
/// sensitivity level is supposed to set. Nod motion must never drag the
/// reference along.
const BASELINE_STILL_SWING: f32 = 1.5;
/// The shake must dominate the nod motion by this ratio to cancel it.
const YAW_CANCEL_RATIO: f32 = 1.5;
/// Swing decay time constant in seconds: how long motion stays "recent".
const SWING_TAU: f32 = 0.8;
/// Largest integration step accepted, in seconds; bigger gaps reset hysteresis.
const MAX_DT: f32 = 0.25;

/// What one fed sample produced.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum NodEvent {
    /// One more nod was counted; carries the running total.
    Counted(usize),
    /// The wearer stopped nodding; carries the final total.
    Commit(usize),
    /// A head shake (or large turn) cancelled the pending count.
    Cancelled,
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
    /// How close to the resting pitch the head must come back for the dip to
    /// count; a third of the depth.
    return_band: f32,
    /// Yaw swing, in degrees, above which a pending count is a head shake.
    yaw_cancel: f32,
    /// Resting pitch in degrees; re-seeded from the first sample after a reset.
    baseline: Option<f32>,
    /// Resting yaw, the reference the yaw swing is measured against.
    yaw_ref: Option<f32>,
    /// Recent pitch motion magnitude, decays exponentially.
    pitch_swing: f32,
    /// Recent yaw motion magnitude, decays exponentially.
    yaw_swing: f32,
    /// Hysteresis state: is the head currently below the dip threshold?
    down: bool,
    /// Nods counted so far in the current sequence.
    count: usize,
    /// Seconds since the last counted nod.
    since_count: f32,
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
            return_band: depth / 3.0,
            yaw_cancel: yaw_cancel_for(sensitivity),
            baseline: None,
            yaw_ref: None,
            pitch_swing: 0.0,
            yaw_swing: 0.0,
            down: false,
            count: 0,
            // A fresh counter has no recent count to debounce against, so the
            // first nod may arm immediately - otherwise a wearer answering
            // right away loses the nod that crosses within the first 150 ms.
            since_count: MIN_NOD_INTERVAL,
        }
    }

    /// Clears everything; the next sample re-seeds the baselines.
    pub fn reset(&mut self) {
        *self = Self::with_sensitivity(self.sensitivity);
    }

    /// Nods counted so far in the current sequence.
    pub fn count(&self) -> usize {
        self.count
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
        }
        self.since_count += dt;

        let decay = (-dt / SWING_TAU).exp();
        self.pitch_swing *= decay;
        self.yaw_swing *= decay;

        // Seed the baselines on the first sample after a reset: whatever pose
        // the wearer holds when the question arrives is the resting pose.
        let baseline = *self.baseline.get_or_insert(pitch_deg);
        let yaw_ref = *self.yaw_ref.get_or_insert(yaw_deg);

        self.pitch_swing = self.pitch_swing.max((pitch_deg - baseline).abs());
        self.yaw_swing = self.yaw_swing.max((yaw_deg - yaw_ref).abs());
        self.yaw_ref = Some(yaw_ref + (yaw_deg - yaw_ref) * BASELINE_RATE * dt);

        // A deliberate head turn while a count is pending means "dismiss":
        // cancel, and never let it half-count as a nod.
        if self.count > 0
            && self.yaw_swing > self.yaw_cancel
            && self.yaw_swing > self.pitch_swing * YAW_CANCEL_RATIO
        {
            self.reset();
            return Some(NodEvent::Cancelled);
        }

        // The resting pitch only adapts while nothing is pending *and* the
        // head is near-still. Once a nod has been counted, freezing the
        // baseline keeps the next nod equally easy to reach even if the
        // wearer rests slightly dipped.
        if self.count == 0 && self.pitch_swing < BASELINE_STILL_SWING {
            self.baseline =
                Some(baseline + (pitch_deg - baseline) * (BASELINE_RATE * dt).min(1.0));
        }

        // Hysteresis: arm on the way down, count on the way back up.
        if !self.down {
            if self.since_count >= MIN_NOD_INTERVAL && pitch_deg < baseline - self.depth {
                self.down = true;
            }
        } else if pitch_deg > baseline - self.return_band {
            self.count += 1;
            self.since_count = 0.0;
            // Back to neutral: the next nod must dip below the threshold
            // again, otherwise the whole return stroke counts sample by
            // sample.
            self.down = false;
            return Some(NodEvent::Counted(self.count));
        }

        // The wearer has stopped nodding: commit whatever was counted. The
        // head must be back near the baseline first - a slow, deliberate nod
        // can spend longer than the quiet window on the way down, and
        // committing mid-dip would truncate the count.
        if self.count > 0 && !self.down && self.since_count > QUIET_COMMIT_SECS {
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
        let mut events = ramp(counter, 0.0, depth, 0.15, 0.0);
        events.extend(hold(counter, depth, 0.05, 0.0));
        events.extend(ramp(counter, depth, 0.0, 0.15, 0.0));
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
        events.extend(hold(&mut counter, 0.0, 1.6, 0.0));

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
        events.extend(hold(&mut counter, 0.0, 1.6, 0.0));
        assert_eq!(
            events.last(),
            Some(&NodEvent::Commit(1)),
            "a lone nod must still commit"
        );
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

        events.extend(hold(&mut counter, 0.0, 1.6, 0.0));
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
        events.extend(hold(&mut counter, 0.0, 1.6, 0.0));
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
        events.extend(hold(&mut counter, 0.0, 1.6, 0.0));
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
            events.extend(hold(&mut counter, 0.0, 1.6, 0.0));
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
        events.extend(hold(&mut counter, 0.0, 1.6, 0.0));
        assert!(events.is_empty(), "level 1 must ignore a 6° nod: {events:?}");
        // A deliberate 12° nod still counts.
        let mut events = nod(&mut counter, -12.0);
        events.extend(hold(&mut counter, 0.0, 1.6, 0.0));
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
        events.extend(hold(&mut counter, 0.0, 1.6, 0.0));
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
        events.extend(hold(&mut counter, -15.0, 1.6, 0.0));
        assert!(
            events.contains(&NodEvent::Commit(1)),
            "a nod relative to the current head pose must count: {events:?}"
        );
    }
}


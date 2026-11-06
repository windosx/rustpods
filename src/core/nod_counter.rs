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
//! * `pitch < baseline − NOD_DEPTH` arms the count (head is down),
//! * `pitch > baseline − NOD_RETURN` commits one nod (head came back up),
//! * `QUIET_COMMIT_SECS` of silence after the last nod commits the total.
//!
//! The baseline only adapts while no count is pending, so pausing between
//! nods cannot drag the reference downwards and make the next nod harder to
//! reach. Turning the head (yaw) while a count is pending cancels it - that is
//! the "shake to dismiss" gesture, and it also stops a glance sideways from
//! being mistaken for the start of a nod sequence.

/// How far below the resting pitch a nod must dip, in degrees.
const NOD_DEPTH: f32 = 6.0;
/// How close to the resting pitch the head must come back for the dip to count.
const NOD_RETURN: f32 = 2.0;
/// Minimum spacing between counted nods, in seconds; filters single-sample bounce.
const MIN_NOD_INTERVAL: f32 = 0.15;
/// Silence after the last nod that commits the count, in seconds.
const QUIET_COMMIT_SECS: f32 = 1.4;
/// Resting pitch is tracked with an EMA; this is its convergence rate in 1/s
/// while the head is near the baseline.
const BASELINE_RATE: f32 = 1.5;
/// Outside this deadband (degrees) the baseline adapts at the slow drift
/// rate instead: a deliberate slow nod must not drag its own reference down
/// with it, or the dip never crosses the threshold at all.
const BASELINE_DEADBAND: f32 = 3.0;
/// Baseline convergence rate while the head is away from the baseline.
const BASELINE_DRIFT_RATE: f32 = 0.1;
/// Yaw swing, in degrees, above which a pending count is read as a head shake.
const YAW_CANCEL_SWING: f32 = 10.0;
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
#[derive(Debug, Clone)]
pub struct NodCounter {
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
    pub fn new() -> Self {
        Self {
            baseline: None,
            yaw_ref: None,
            pitch_swing: 0.0,
            yaw_swing: 0.0,
            down: false,
            count: 0,
            since_count: 0.0,
        }
    }

    /// Clears everything; the next sample re-seeds the baselines.
    pub fn reset(&mut self) {
        *self = Self::new();
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
            && self.yaw_swing > YAW_CANCEL_SWING
            && self.yaw_swing > self.pitch_swing * YAW_CANCEL_RATIO
        {
            self.reset();
            return Some(NodEvent::Cancelled);
        }

        // The resting pitch only adapts while nothing is pending. Once a nod
        // has been counted, freezing the baseline keeps the next nod equally
        // easy to reach even if the wearer rests slightly dipped.
        if self.count == 0 {
            let deviation = pitch_deg - baseline;
            let rate = if deviation.abs() < BASELINE_DEADBAND {
                BASELINE_RATE
            } else {
                BASELINE_DRIFT_RATE
            };
            self.baseline = Some(baseline + deviation * (rate * dt).min(1.0));
        }

        // Hysteresis: arm on the way down, count on the way back up.
        if !self.down {
            if self.since_count >= MIN_NOD_INTERVAL && pitch_deg < baseline - NOD_DEPTH {
                self.down = true;
            }
        } else if pitch_deg > baseline - NOD_RETURN {
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

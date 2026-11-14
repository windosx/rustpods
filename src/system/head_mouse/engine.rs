use super::one_euro::OneEuroFilter;
use crate::system::config::HeadMouseParams;
use std::time::Instant;

/// Button action emitted by gestures or clicks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MouseButton {
    Left,
    Right,
    Middle,
}

/// Output computed for one motion update.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct MouseStepOutput {
    /// Desired pixel displacement (dx, dy). Relative mode only; absolute mode
    /// reports its position through [`Self::abs_pos`] instead.
    pub delta: (f32, f32),
    /// Desired scroll lines (positive scrolls up).
    pub scroll: f32,
    /// Triggered click event, if any.
    pub click: Option<MouseButton>,
    /// Absolute cursor position in the virtual device's 0..1023 space, when
    /// the engine ran in absolute mode. The compositor maps that space onto
    /// the whole screen, so no screen-size knowledge is needed here.
    pub abs_pos: Option<(f32, f32)>,
}

/// Discards values within [-dz, dz] and subtracts the threshold for larger movements,
/// preventing jerky jumps as the velocity crosses the deadzone.
#[inline]
pub fn soft_deadzone(v: f32, dz: f32) -> f32 {
    if v.abs() <= dz {
        0.0
    } else {
        v.signum() * (v.abs() - dz)
    }
}

/// Non-linear speed acceleration curve.
/// At low speeds (~0-10 deg/s), multiplier is close to 1.0 (fine cursor control).
/// At high speeds (> 60 deg/s), multiplier scales up to (1.0 + accel * 2.5) for quick flicking.
#[inline]
pub fn calculate_gain(speed_dps: f32, px_per_deg: f32, acceleration: f32) -> f32 {
    let factor = (speed_dps / 60.0).min(2.5);
    px_per_deg * (1.0 + acceleration * factor)
}

/// Coordinate space of the virtual device's absolute axes. The compositor
/// maps 0..[`ABS_SPACE_MAX`] across the whole screen.
pub const ABS_SPACE_MAX: f32 = 1023.0;
/// Head yaw that spans the full screen width in absolute mode, in degrees.
pub const ABS_YAW_RANGE_DEG: f32 = 45.0;
/// Head pitch that spans the full screen height in absolute mode, in degrees.
pub const ABS_PITCH_RANGE_DEG: f32 = 30.0;
/// Time constant of the chase toward the absolute target, in seconds.
const ABS_CHASE_TAU_S: f32 = 0.08;

/// Maps a head pose to a normalized screen target in `0..=1` for absolute
/// mode: x grows to the right with yaw, y grows downward as pitch lowers.
/// Non-finite input maps to the centre.
pub fn absolute_target(yaw_deg: f32, pitch_deg: f32) -> (f32, f32) {
    let yaw = if yaw_deg.is_finite() { yaw_deg } else { 0.0 };
    let pitch = if pitch_deg.is_finite() { pitch_deg } else { 0.0 };
    let x = (yaw / ABS_YAW_RANGE_DEG).clamp(-1.0, 1.0) * 0.5 + 0.5;
    let y = -(pitch / ABS_PITCH_RANGE_DEG).clamp(-1.0, 1.0) * 0.5 + 0.5;
    (x, y)
}

/// Detects quick sideways head tilts (roll gestures) to trigger left/right clicks.
#[derive(Debug, Clone)]
pub struct RollClickDetector {
    pub enabled: bool,
    pub trigger_deg: f32,
    pub max_duration_s: f32,
    roll_ang: f32,
    latched: bool,
    start_time: Option<Instant>,
}

impl Default for RollClickDetector {
    fn default() -> Self {
        Self {
            enabled: false,
            trigger_deg: 15.0,
            max_duration_s: 0.6,
            roll_ang: 0.0,
            latched: false,
            start_time: None,
        }
    }
}

impl RollClickDetector {
    pub fn new(enabled: bool) -> Self {
        Self {
            enabled,
            ..Default::default()
        }
    }

    pub fn reset(&mut self) {
        self.roll_ang = 0.0;
        self.latched = false;
        self.start_time = None;
    }

    /// Feeds roll rate (in deg/s) and returns a click if a quick tilt threshold is met.
    pub fn update(&mut self, roll_rate_dps: f32, dt: f32, now: Instant) -> Option<MouseButton> {
        if !self.enabled || dt <= 0.0 {
            return None;
        }

        // Leaky integration with ~0.4s decay time constant.
        self.roll_ang = self.roll_ang * (-dt / 0.4).exp() + roll_rate_dps * dt;
        let abs_ang = self.roll_ang.abs();

        if self.latched {
            // Unlatch when head returns near level.
            if abs_ang < self.trigger_deg * 0.5 {
                self.latched = false;
            }
            return None;
        }

        if abs_ang > 4.0 && self.start_time.is_none() {
            self.start_time = Some(now);
        } else if abs_ang < 2.0 {
            self.start_time = None;
        }

        if abs_ang >= self.trigger_deg {
            if let Some(t0) = self.start_time {
                let duration = now.duration_since(t0).as_secs_f32();
                if duration <= self.max_duration_s {
                    self.latched = true;
                    self.start_time = None;
                    // Tilt right (positive) -> Left Click; Tilt left (negative) -> Right Click
                    return Some(if self.roll_ang > 0.0 {
                        MouseButton::Left
                    } else {
                        MouseButton::Right
                    });
                }
            }
            self.latched = true;
            self.start_time = None;
        }

        None
    }
}

/// Detects stationary hover (dwell click) for accessibility.
#[derive(Debug, Clone)]
pub struct DwellClickDetector {
    pub enabled: bool,
    pub time_s: f32,
    pub radius_px: f32,
    pub min_move_px: f32,
    anchor: Option<(f32, f32)>,
    moved: f32,
    dwell_t0: Option<Instant>,
    fired: bool,
}

impl Default for DwellClickDetector {
    fn default() -> Self {
        Self {
            enabled: false,
            time_s: 1.0,
            radius_px: 12.0,
            min_move_px: 60.0,
            anchor: None,
            moved: 0.0,
            dwell_t0: None,
            fired: false,
        }
    }
}

impl DwellClickDetector {
    pub fn new(enabled: bool) -> Self {
        Self {
            enabled,
            ..Default::default()
        }
    }

    pub fn reset(&mut self) {
        self.anchor = None;
        self.moved = 0.0;
        self.dwell_t0 = None;
        self.fired = false;
    }

    pub fn update(&mut self, pos_x: f32, pos_y: f32, now: Instant) -> Option<MouseButton> {
        if !self.enabled {
            return None;
        }

        let Some(anchor) = self.anchor else {
            self.anchor = Some((pos_x, pos_y));
            self.dwell_t0 = Some(now);
            return None;
        };

        let dx = pos_x - anchor.0;
        let dy = pos_y - anchor.1;
        let dist = (dx * dx + dy * dy).sqrt();

        if dist > self.radius_px {
            self.moved += dist;
            self.anchor = Some((pos_x, pos_y));
            self.dwell_t0 = Some(now);
            self.fired = false;
        } else if !self.fired
            && self.moved >= self.min_move_px
            && now.duration_since(self.dwell_t0.unwrap_or(now)).as_secs_f32() >= self.time_s
        {
            self.fired = true;
            self.moved = 0.0;
            return Some(MouseButton::Left);
        }

        None
    }
}

/// Motion translation engine combining 1-Euro smoothing, deadzone filtering,
/// dynamic acceleration, and gesture detection.
#[derive(Debug, Clone)]
pub struct MouseEngine {
    pub yaw_filter: OneEuroFilter,
    pub pitch_filter: OneEuroFilter,
    pub roll_click: RollClickDetector,
    pub dwell_click: DwellClickDetector,
    // Virtual position tracking for dwell click calculation
    virtual_pos: (f32, f32),
}

impl MouseEngine {
    pub fn new(smoothing: f32) -> Self {
        Self {
            yaw_filter: OneEuroFilter::from_smoothing(smoothing),
            pitch_filter: OneEuroFilter::from_smoothing(smoothing),
            roll_click: RollClickDetector::default(),
            dwell_click: DwellClickDetector::default(),
            virtual_pos: (0.0, 0.0),
        }
    }

    pub fn set_smoothing(&mut self, smoothing: f32) {
        self.yaw_filter = OneEuroFilter::from_smoothing(smoothing);
        self.pitch_filter = OneEuroFilter::from_smoothing(smoothing);
    }

    pub fn reset(&mut self) {
        self.yaw_filter.reset();
        self.pitch_filter.reset();
        self.roll_click.reset();
        self.dwell_click.reset();
        self.virtual_pos = (0.0, 0.0);
    }

    /// Steps the engine forward with a new motion sample (relative mode).
    ///
    /// - `yaw_rate`: Turning velocity in deg/s (positive turns right).
    /// - `pitch_rate`: Tilting velocity in deg/s (positive looks up).
    /// - `roll_rate`: Tilting velocity in deg/s (positive tilts right).
    /// - `dt`: Elapsed time in seconds.
    /// - `params`: the head-mouse tuning copied out of the config.
    ///
    /// Eight arguments is the honest shape here: the three rates and `dt` are
    /// the motion, `now` drives the gesture detectors' wall clock, and
    /// `params`/`scroll_mode` are the two knobs that differ per call. Bundling
    /// them further would hide what each argument means.
    #[allow(clippy::too_many_arguments)]
    pub fn step(
        &mut self,
        yaw_rate: f32,
        pitch_rate: f32,
        roll_rate: f32,
        dt: f32,
        now: Instant,
        params: &HeadMouseParams,
        scroll_mode: bool,
    ) -> MouseStepOutput {
        if dt <= 0.0 {
            return MouseStepOutput::default();
        }

        // 1. 1-Euro adaptive low-pass filter
        let yaw_f = self.yaw_filter.filter(yaw_rate, dt);
        let pitch_f = self.pitch_filter.filter(pitch_rate, dt);

        // 2. Roll click gesture
        if let Some(btn) = self.roll_click.update(roll_rate, dt, now) {
            return MouseStepOutput {
                click: Some(btn),
                ..MouseStepOutput::default()
            };
        }

        // 3. Scroll mode
        let iy = if params.invert_y { -1.0 } else { 1.0 };
        let ix = if params.invert_x { -1.0 } else { 1.0 };

        if scroll_mode {
            // Looking up/down scrolls lines (positive lines scroll up)
            let scroll_lines = iy * pitch_f * dt * 0.25 * 4.0;
            return MouseStepOutput {
                scroll: scroll_lines,
                ..MouseStepOutput::default()
            };
        }

        // 4. Soft deadzone
        let vy = soft_deadzone(yaw_f, params.deadzone);
        let vp = soft_deadzone(pitch_f, params.deadzone);
        let speed = (vy * vy + vp * vp).sqrt();

        // 5. Dynamic acceleration
        let gain = calculate_gain(speed, params.speed, params.accel);

        // Looking right (yaw > 0) -> +X cursor movement
        // Looking up -> -Y screen movement (towards top of display)
        let dx = ix * vy * gain * dt;
        let dy = iy * vp * gain * dt;

        self.virtual_pos.0 += dx;
        self.virtual_pos.1 += dy;

        // 6. Dwell click
        let click = self.dwell_click.update(self.virtual_pos.0, self.virtual_pos.1, now);

        MouseStepOutput {
            delta: (dx, dy),
            scroll: 0.0,
            click,
            abs_pos: None,
        }
    }

    /// Steps the engine forward in absolute mode: the head angle picks a
    /// position on screen directly instead of accumulating displacement.
    ///
    /// The target position is computed from the (gravity-anchored) pitch and
    /// yaw angles and mapped into the virtual device's 0..1023 space; the
    /// compositor stretches that space over the whole screen, so no screen
    /// size is needed here. The cursor chases the target with a short time
    /// constant, so a pose jump glides instead of teleporting.
    pub fn step_absolute(
        &mut self,
        yaw_deg: f32,
        pitch_deg: f32,
        roll_rate: f32,
        dt: f32,
        now: Instant,
        params: &HeadMouseParams,
    ) -> MouseStepOutput {
        if dt <= 0.0 {
            return MouseStepOutput::default();
        }

        // Roll click still applies in absolute mode: it is a gesture on the
        // roll rate, not on the cursor path.
        if let Some(btn) = self.roll_click.update(roll_rate, dt, now) {
            return MouseStepOutput {
                click: Some(btn),
                ..MouseStepOutput::default()
            };
        }

        // Yaw within +/-ABS_YAW_RANGE_DEG spans the full width; positive yaw
        // (turn right) moves the cursor right. Positive pitch (look up) moves
        // the cursor up: screen y grows downward, hence the negation inside
        // [`absolute_target`] (which also guards non-finite input).
        let (target_x, target_y) = absolute_target(yaw_deg, pitch_deg);

        // Chase the target with a fixed time constant.
        let k = 1.0 - (-dt / ABS_CHASE_TAU_S).exp();
        let vx = self.virtual_pos.0 + (target_x * ABS_SPACE_MAX - self.virtual_pos.0) * k;
        let vy = self.virtual_pos.1 + (target_y * ABS_SPACE_MAX - self.virtual_pos.1) * k;
        self.virtual_pos = (vx, vy);

        // Dwell click sees the same coordinates the device emits, so its
        // radius keeps meaning "a fraction of the screen".
        let click = if params.dwell_click {
            self.dwell_click.update(vx, vy, now)
        } else {
            None
        };

        MouseStepOutput {
            delta: (0.0, 0.0),
            scroll: 0.0,
            click,
            abs_pos: Some((vx, vy)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deadzone_suppresses_small_rates_and_smooths_above() {
        assert_eq!(soft_deadzone(1.0, 2.0), 0.0);
        assert_eq!(soft_deadzone(-1.5, 2.0), 0.0);
        assert!((soft_deadzone(3.0, 2.0) - 1.0).abs() < 1e-4);
        assert!((soft_deadzone(-4.0, 2.0) - (-2.0)).abs() < 1e-4);
    }

    #[test]
    fn gain_increases_with_velocity() {
        let base_gain = calculate_gain(0.0, 20.0, 1.0);
        let high_speed_gain = calculate_gain(60.0, 20.0, 1.0);
        assert_eq!(base_gain, 20.0);
        assert!(high_speed_gain > base_gain);
    }

    #[test]
    fn roll_click_triggers_on_rapid_tilt() {
        let mut detector = RollClickDetector::new(true);
        let mut now = Instant::now();
        // Rapid tilt right at 80 deg/s for 0.3s
        let mut clicked = None;
        for _ in 0..30 {
            now += std::time::Duration::from_millis(10);
            if let Some(btn) = detector.update(80.0, 0.01, now) {
                clicked = Some(btn);
                break;
            }
        }
        assert_eq!(clicked, Some(MouseButton::Left));
    }

    #[test]
    fn dwell_click_triggers_after_moving_then_holding_still() {
        let mut detector = DwellClickDetector::new(true);
        let mut now = Instant::now();
        // Initial anchor at (0, 0)
        detector.update(0.0, 0.0, now);

        // Move by 100px (exceeds min_move_px = 60)
        now += std::time::Duration::from_millis(100);
        detector.update(100.0, 0.0, now);

        // Hold still at (100.0, 0.0) for 1.1s (exceeds time_s = 1.0)
        now += std::time::Duration::from_millis(1100);
        let click = detector.update(100.0, 0.0, now);
        assert_eq!(click, Some(MouseButton::Left));

        // Subsequent update while still at the same spot should not re-fire until moved again
        now += std::time::Duration::from_millis(50);
        assert_eq!(detector.update(100.0, 0.0, now), None);
    }

    #[test]
    fn engine_scroll_mode_translates_pitch_to_scroll_lines() {
        let mut engine = MouseEngine::new(0.35);
        let now = Instant::now();
        let dt = 0.01;
        let params = crate::system::config::AppConfig::default().head_mouse_params();
        // Looking up at 30 deg/s
        let out = engine.step(0.0, 30.0, 0.0, dt, now, &params, true);
        assert_eq!(out.delta, (0.0, 0.0), "cursor should not move in scroll mode");
        assert!(out.scroll > 0.0, "pitching up should scroll up (positive lines), got {}", out.scroll);
        assert!(out.abs_pos.is_none(), "scroll mode is relative, not absolute");
    }

    #[test]
    fn absolute_target_is_centred_and_monotonic() {
        // Zero pose -> screen centre.
        let (cx, cy) = absolute_target(0.0, 0.0);
        assert!((cx - 0.5).abs() < 1e-4, "centre x, got {}", cx);
        assert!((cy - 0.5).abs() < 1e-4, "centre y, got {}", cy);
        // Turning right moves right; looking up moves up (y shrinks).
        let (rx, _ry) = absolute_target(ABS_YAW_RANGE_DEG, 0.0);
        assert!((rx - 1.0).abs() < 1e-4, "right edge at +yaw range, got {}", rx);
        let (lx, _) = absolute_target(-ABS_YAW_RANGE_DEG, 0.0);
        assert!((lx - 0.0).abs() < 1e-4, "left edge at -yaw range, got {}", lx);
        let (_, uy) = absolute_target(0.0, ABS_PITCH_RANGE_DEG);
        assert!((uy - 0.0).abs() < 1e-4, "top edge when looking up, got {}", uy);
        let (_, dy) = absolute_target(0.0, -ABS_PITCH_RANGE_DEG);
        assert!((dy - 1.0).abs() < 1e-4, "bottom edge when looking down, got {}", dy);
        // Monotonic inside the range.
        let (a, _) = absolute_target(10.0, 0.0);
        let (b, _) = absolute_target(20.0, 0.0);
        assert!(b > a, "more yaw must move further right");
    }

    #[test]
    fn engine_absolute_mode_chases_the_target_without_teleporting() {
        let mut engine = MouseEngine::new(0.35);
        let params = crate::system::config::AppConfig::default().head_mouse_params();
        let now = Instant::now();
        // One sample of a full-range yaw: the chase must move part of the way,
        // not the whole distance at once.
        let first = engine.step_absolute(ABS_YAW_RANGE_DEG, 0.0, 0.0, 0.01, now, &params);
        let (x, _) = first.abs_pos.expect("absolute mode reports a position");
        assert!(x > 0.0 && x < ABS_SPACE_MAX, "first step should be partial, got {}", x);
        // Holding the pose converges onto the right edge.
        let mut last = first;
        for i in 1..100 {
            last = engine.step_absolute(
                ABS_YAW_RANGE_DEG,
                0.0,
                0.0,
                0.01,
                now + std::time::Duration::from_millis(i * 10),
                &params,
            );
        }
        let (x, _) = last.abs_pos.expect("absolute mode reports a position");
        assert!(x > ABS_SPACE_MAX * 0.95, "chase should converge, got {}", x);
        assert_eq!(last.delta, (0.0, 0.0), "absolute mode never reports a delta");
    }
}

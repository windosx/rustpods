use super::engine::{MouseButton, MouseEngine};
use super::{UinputMouseDevice, UinputStatus};
use crate::core::head_tracking::HeadPose;
use crate::system::config::{AppConfig, HeadMouseMode};
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::{Arc, LazyLock, Mutex, RwLock};
use std::time::{Duration, Instant};
use tokio::sync::{mpsc, watch};
use tracing::{debug, error, info, warn};

/// How long the virtual button stays pressed on a click. The release happens
/// on a later render tick, so the async controller is never blocked on the
/// press duration (the old inline `thread::sleep` stalled a tokio worker).
const CLICK_HOLD_MS: u128 = 15;

/// Runtime operational state of the head mouse.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HeadMouseState {
    Disabled,
    PermissionDenied,
    Disconnected,
    Active,
    Paused,
    ScrollMode,
}

impl HeadMouseState {
    fn to_u8(self) -> u8 {
        match self {
            Self::Disabled => 0,
            Self::PermissionDenied => 1,
            Self::Disconnected => 2,
            Self::Active => 3,
            Self::Paused => 4,
            Self::ScrollMode => 5,
        }
    }

    fn from_u8(val: u8) -> Self {
        match val {
            0 => Self::Disabled,
            1 => Self::PermissionDenied,
            2 => Self::Disconnected,
            3 => Self::Active,
            4 => Self::Paused,
            5 => Self::ScrollMode,
            _ => Self::Disabled,
        }
    }
}

/// Commands accepted by the head mouse controller.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HeadMouseCommand {
    TogglePause,
    SetPaused(bool),
    ToggleScroll,
    SetScroll(bool),
    Click(MouseButton),
    Recenter,
}

/// Global hub for dispatching head mouse commands and observing status.
pub struct HeadMouseHub {
    cmd_tx: mpsc::UnboundedSender<HeadMouseCommand>,
    cmd_rx: Mutex<Option<mpsc::UnboundedReceiver<HeadMouseCommand>>>,
    state: AtomicU8,
}

pub static HEAD_MOUSE: LazyLock<HeadMouseHub> = LazyLock::new(HeadMouseHub::new);

impl HeadMouseHub {
    fn new() -> Self {
        let (cmd_tx, cmd_rx) = mpsc::unbounded_channel();
        Self {
            cmd_tx,
            cmd_rx: Mutex::new(Some(cmd_rx)),
            state: AtomicU8::new(HeadMouseState::Disabled.to_u8()),
        }
    }

    pub fn send_command(&self, cmd: HeadMouseCommand) {
        let _ = self.cmd_tx.send(cmd);
    }

    pub fn state(&self) -> HeadMouseState {
        HeadMouseState::from_u8(self.state.load(Ordering::Relaxed))
    }

    pub fn is_active(&self) -> bool {
        self.state() == HeadMouseState::Active
    }

    pub fn is_paused(&self) -> bool {
        self.state() == HeadMouseState::Paused
    }

    pub fn is_scroll(&self) -> bool {
        self.state() == HeadMouseState::ScrollMode
    }

    fn set_state(&self, s: HeadMouseState) {
        self.state.store(s.to_u8(), Ordering::Relaxed);
    }

    fn take_receiver(&self) -> Option<mpsc::UnboundedReceiver<HeadMouseCommand>> {
        self.cmd_rx.lock().unwrap().take()
    }
}

/// Background loop that runs the head mouse engine and 120 Hz output rendering.
pub async fn run_head_mouse_controller(
    mut pose_rx: watch::Receiver<Option<HeadPose>>,
    config: Arc<RwLock<AppConfig>>,
) {
    let mut cmd_rx = match HEAD_MOUSE.take_receiver() {
        Some(rx) => rx,
        None => {
            warn!("Head mouse command receiver already claimed; aborting redundant runner");
            return;
        }
    };

    let mut device: Option<UinputMouseDevice> = None;
    let mut engine = MouseEngine::new(0.35);
    // Tuning copied out of the config; only the small [`HeadMouseParams`] is
    // cloned, never the whole AppConfig, and the 120 Hz render tick reads a
    // single bool instead of taking a full snapshot.
    let mut params = AppConfig::default().head_mouse_params();
    let mut last_smoothing: Option<f32> = None;
    let mut engine_mode = params.mode;
    // A click whose press went out and whose release is pending, so the
    // press-hold never blocks an async worker.
    let mut pending_release: Option<(MouseButton, Instant)> = None;

    let mut paused = false;
    let mut scroll_mode = false;
    let mut last_sample_time: Option<Instant> = None;
    let mut last_render_time = Instant::now();
    let mut current_vel: (f32, f32) = (0.0, 0.0);

    let mut render_ticker = tokio::time::interval(Duration::from_micros(8333)); // ~120 Hz

    info!("Head mouse background controller started");

    // Releases a pending click, if one is in flight.
    macro_rules! flush_release {
        () => {
            if let Some((btn, _)) = pending_release.take() {
                if let Some(dev) = device.as_mut() {
                    let _ = dev.release(btn);
                }
            }
        };
    }

    loop {
        tokio::select! {
            // 1. Hardware motion sample arrival
            changed = pose_rx.changed() => {
                if changed.is_err() {
                    break;
                }

                let pose = *pose_rx.borrow();
                let now = Instant::now();
                params = config
                    .read()
                    .map(|c| c.head_mouse_params())
                    .unwrap_or(params);

                if !params.enabled {
                    HEAD_MOUSE.set_state(HeadMouseState::Disabled);
                    flush_release!();
                    device = None;
                    current_vel = (0.0, 0.0);
                    continue;
                }

                // Check device connectivity
                let Some(pose) = pose else {
                    HEAD_MOUSE.set_state(HeadMouseState::Disconnected);
                    flush_release!();
                    current_vel = (0.0, 0.0);
                    continue;
                };

                // Lazy initialize uinput device
                if device.is_none() {
                    let probe = UinputStatus::probe();
                    if !probe.is_available() {
                        HEAD_MOUSE.set_state(HeadMouseState::PermissionDenied);
                        continue;
                    }
                    match UinputMouseDevice::create() {
                        Ok(dev) => {
                            device = Some(dev);
                            engine_mode = params.mode;
                        }
                        Err(e) => {
                            error!("Failed to initialize uinput device: {}", e);
                            HEAD_MOUSE.set_state(HeadMouseState::PermissionDenied);
                            continue;
                        }
                    }
                }

                // A mode change must reset the engine: relative mode accumulates
                // raw pixels while absolute mode chases a position in the
                // 0..1023 virtual space, and the two must never mix in one
                // accumulator.
                if engine_mode != params.mode {
                    engine.reset();
                    engine_mode = params.mode;
                }

                // Update operational state
                if paused {
                    HEAD_MOUSE.set_state(HeadMouseState::Paused);
                    current_vel = (0.0, 0.0);
                    continue;
                } else if scroll_mode {
                    HEAD_MOUSE.set_state(HeadMouseState::ScrollMode);
                } else {
                    HEAD_MOUSE.set_state(HeadMouseState::Active);
                }

                let dt = last_sample_time
                    .map(|t| now.duration_since(t).as_secs_f32().min(0.1))
                    .unwrap_or(0.01);
                last_sample_time = Some(now);

                // Rebuild the 1-Euro filters only when the smoothing knob
                // actually changed: recreating them every sample wiped their
                // history, which silently disabled smoothing altogether.
                if last_smoothing != Some(params.smoothing) {
                    engine.set_smoothing(params.smoothing);
                    last_smoothing = Some(params.smoothing);
                }
                engine.roll_click.enabled = params.roll_click;
                engine.dwell_click.enabled = params.dwell_click;

                let output = match params.mode {
                    HeadMouseMode::Absolute => {
                        engine.step_absolute(pose.yaw, pose.pitch, pose.roll_rate, dt, now, &params)
                    }
                    HeadMouseMode::Relative => {
                        engine.step(
                            pose.yaw_rate,
                            pose.pitch_rate,
                            pose.roll_rate,
                            dt,
                            now,
                            &params,
                            scroll_mode,
                        )
                    }
                };

                if let Some(dev) = device.as_mut() {
                    // Handle gesture clicks (roll click or dwell click): press
                    // now, release on a later render tick.
                    if let Some(btn) = output.click {
                        let _ = dev.press(btn);
                        pending_release = Some((btn, now));
                    }
                    // Handle scroll
                    if output.scroll.abs() > 0.001 {
                        let _ = dev.scroll(output.scroll);
                    }
                    // Handle absolute positioning
                    if let Some((x, y)) = output.abs_pos {
                        let _ = dev.move_abs(x, y);
                    }
                }

                // Update translational velocity in pixels/second (relative mode
                // only; absolute mode has no per-tick velocity to interpolate).
                if dt > 0.0 && !scroll_mode && params.mode == HeadMouseMode::Relative {
                    current_vel = (output.delta.0 / dt, output.delta.1 / dt);
                } else {
                    current_vel = (0.0, 0.0);
                }
            }

            // 2. 120 Hz render tick for smooth motion and gap fading
            _ = render_ticker.tick() => {
                let now = Instant::now();
                let dt = now.duration_since(last_render_time).as_secs_f32().min(0.05);
                last_render_time = now;

                let enabled = config
                    .read()
                    .map(|c| c.head_mouse_enabled)
                    .unwrap_or(false);
                if !enabled || paused || scroll_mode {
                    continue;
                }

                if let Some(dev) = device.as_mut() {
                    // Release a click whose press-hold has run its course.
                    if let Some((btn, t0)) = pending_release
                        && now.duration_since(t0).as_millis() >= CLICK_HOLD_MS {
                            let _ = dev.release(btn);
                            pending_release = None;
                        }

                    if params.mode == HeadMouseMode::Relative
                        && let Some(t_last) = last_sample_time {
                            let age = now.duration_since(t_last).as_secs_f32();
                            // Fade out velocity if Bluetooth packets stall past 80 ms
                            let fade = if age < 0.08 {
                                1.0
                            } else {
                                (1.0 - (age - 0.08) / 0.1).clamp(0.0, 1.0)
                            };

                            let dx = current_vel.0 * dt * fade;
                            let dy = current_vel.1 * dt * fade;

                            if dx.abs() > 0.01 || dy.abs() > 0.01 {
                                let _ = dev.move_rel(dx, dy);
                            }
                        }
                }
            }

            // 3. User commands (stem press, UI toggles, hotkeys)
            Some(cmd) = cmd_rx.recv() => {
                debug!("Head mouse received command: {:?}", cmd);
                match cmd {
                    HeadMouseCommand::TogglePause => {
                        paused = !paused;
                        current_vel = (0.0, 0.0);
                        engine.reset();
                        info!("Head mouse pause toggled: paused = {}", paused);
                    }
                    HeadMouseCommand::SetPaused(p) => {
                        paused = p;
                        current_vel = (0.0, 0.0);
                        engine.reset();
                        info!("Head mouse set paused = {}", paused);
                    }
                    HeadMouseCommand::ToggleScroll => {
                        scroll_mode = !scroll_mode;
                        current_vel = (0.0, 0.0);
                        info!("Head mouse scroll mode toggled: scroll = {}", scroll_mode);
                    }
                    HeadMouseCommand::SetScroll(s) => {
                        scroll_mode = s;
                        current_vel = (0.0, 0.0);
                        info!("Head mouse set scroll = {}", scroll_mode);
                    }
                    HeadMouseCommand::Click(btn) => {
                        if let Some(dev) = device.as_mut() {
                            let _ = dev.press(btn);
                            pending_release = Some((btn, Instant::now()));
                        }
                    }
                    HeadMouseCommand::Recenter => {
                        current_vel = (0.0, 0.0);
                        engine.reset();
                        info!("Head mouse recentered");
                    }
                }
            }
        }
    }

    info!("Head mouse controller stopped");
}

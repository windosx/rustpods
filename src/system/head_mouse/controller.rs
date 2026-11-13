use super::engine::{MouseButton, MouseEngine};
use super::uinput::{UinputMouseDevice, UinputStatus};
use crate::core::head_tracking::HeadPose;
use crate::system::config::AppConfig;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::{Arc, LazyLock, Mutex, RwLock};
use std::time::{Duration, Instant};
use tokio::sync::{mpsc, watch};
use tracing::{debug, error, info, warn};

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

    pub fn label(self) -> &'static str {
        match self {
            Self::Disabled => "已停用",
            Self::PermissionDenied => "未授权 /dev/uinput",
            Self::Disconnected => "等待耳机连接",
            Self::Active => "正在运行",
            Self::Paused => "已暂停",
            Self::ScrollMode => "滚轮滚动模式",
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

    let mut paused = false;
    let mut scroll_mode = false;
    let mut last_sample_time: Option<Instant> = None;
    let mut last_render_time = Instant::now();
    let mut current_vel: (f32, f32) = (0.0, 0.0);

    let mut render_ticker = tokio::time::interval(Duration::from_micros(8333)); // ~120 Hz

    info!("Head mouse background controller started");

    loop {
        tokio::select! {
            // 1. Hardware motion sample arrival
            changed = pose_rx.changed() => {
                if changed.is_err() {
                    break;
                }

                let pose = *pose_rx.borrow();
                let now = Instant::now();
                let cfg = { config.read().unwrap().clone() };

                if !cfg.head_mouse_enabled {
                    HEAD_MOUSE.set_state(HeadMouseState::Disabled);
                    device = None;
                    current_vel = (0.0, 0.0);
                    continue;
                }

                // Check device connectivity
                let Some(pose) = pose else {
                    HEAD_MOUSE.set_state(HeadMouseState::Disconnected);
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
                        Ok(dev) => device = Some(dev),
                        Err(e) => {
                            error!("Failed to initialize uinput device: {}", e);
                            HEAD_MOUSE.set_state(HeadMouseState::PermissionDenied);
                            continue;
                        }
                    }
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

                engine.set_smoothing(cfg.head_mouse_smoothing);
                engine.roll_click.enabled = cfg.head_mouse_roll_click;
                engine.dwell_click.enabled = cfg.head_mouse_dwell_click;

                let output = engine.step(
                    pose.yaw_rate,
                    pose.pitch_rate,
                    pose.roll_rate,
                    dt,
                    now,
                    cfg.head_mouse_speed,
                    cfg.head_mouse_accel,
                    cfg.head_mouse_deadzone,
                    cfg.head_mouse_invert_x,
                    cfg.head_mouse_invert_y,
                    scroll_mode,
                );

                if let Some(dev) = device.as_mut() {
                    // Handle gesture clicks (roll click or dwell click)
                    if let Some(btn) = output.click {
                        let _ = dev.click(btn);
                    }
                    // Handle scroll
                    if output.scroll.abs() > 0.001 {
                        let _ = dev.scroll(output.scroll);
                    }
                }

                // Update translational velocity in pixels/second
                if dt > 0.0 && !scroll_mode {
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

                let cfg = { config.read().unwrap().clone() };
                if !cfg.head_mouse_enabled || paused || scroll_mode {
                    continue;
                }

                if let Some(dev) = device.as_mut() {
                    if let Some(t_last) = last_sample_time {
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
                            let _ = dev.click(btn);
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

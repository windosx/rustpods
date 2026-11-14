use super::engine::MouseButton;
use evdev::{
    uinput::{VirtualDevice, VirtualDeviceBuilder},
    AbsInfo, AbsoluteAxisType, AttributeSet, EventType, InputEvent, Key, RelativeAxisType,
    UinputAbsSetup,
};
use std::fs::OpenOptions;
use std::io;
use std::path::Path;
use tracing::{debug, info};

/// Upper bound of the virtual absolute axes; the compositor stretches
/// 0..=`ABS_MAX` across the whole screen.
pub const ABS_MAX: i32 = 1023;

/// Status of the Linux /dev/uinput subsystem on the host machine.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UinputStatus {
    /// Fully accessible and writable.
    Available,
    /// /dev/uinput exists but current user lacks write permission.
    PermissionDenied,
    /// /dev/uinput device file does not exist on this system.
    NotFound,
    /// Other IO error.
    Unavailable,
}

impl UinputStatus {
    pub fn probe() -> Self {
        let path = Path::new("/dev/uinput");
        if !path.exists() {
            return Self::NotFound;
        }
        match OpenOptions::new().write(true).open(path) {
            Ok(_) => Self::Available,
            Err(e) if e.kind() == io::ErrorKind::PermissionDenied => Self::PermissionDenied,
            Err(_) => Self::Unavailable,
        }
    }

    pub fn is_available(&self) -> bool {
        matches!(self, Self::Available)
    }
}

/// Linux virtual mouse device powered by /dev/uinput.
///
/// The device carries BOTH relative and absolute axes: relative for the
/// velocity-driven cursor, absolute for the head-angle-driven pointing mode.
/// On the absolute axes the compositor maps the 0..[`ABS_MAX`] space onto the
/// screen, which is what lets absolute mode work without knowing the screen
/// size.
pub struct UinputMouseDevice {
    device: VirtualDevice,
    accum_x: f32,
    accum_y: f32,
    accum_scroll: f32,
}

impl UinputMouseDevice {
    /// Attempts to create the virtual mouse device.
    pub fn create() -> io::Result<Self> {
        let mut keys = AttributeSet::<Key>::new();
        keys.insert(Key::BTN_LEFT);
        keys.insert(Key::BTN_RIGHT);
        keys.insert(Key::BTN_MIDDLE);

        let mut rel_axes = AttributeSet::<RelativeAxisType>::new();
        rel_axes.insert(RelativeAxisType::REL_X);
        rel_axes.insert(RelativeAxisType::REL_Y);
        rel_axes.insert(RelativeAxisType::REL_WHEEL);
        rel_axes.insert(RelativeAxisType::REL_HWHEEL);

        let abs_x = UinputAbsSetup::new(
            AbsoluteAxisType::ABS_X,
            AbsInfo::new(ABS_MAX / 2, 0, ABS_MAX, 0, 0, 0),
        );
        let abs_y = UinputAbsSetup::new(
            AbsoluteAxisType::ABS_Y,
            AbsInfo::new(ABS_MAX / 2, 0, ABS_MAX, 0, 0, 0),
        );

        let device = VirtualDeviceBuilder::new()?
            .name("RustPods Head Mouse")
            .with_keys(&keys)?
            .with_relative_axes(&rel_axes)?
            .with_absolute_axis(&abs_x)?
            .with_absolute_axis(&abs_y)?
            .build()?;

        info!("Created virtual mouse device via /dev/uinput");

        Ok(Self {
            device,
            accum_x: 0.0,
            accum_y: 0.0,
            accum_scroll: 0.0,
        })
    }

    /// Emits relative cursor movement. Sub-pixel remainders are accumulated across calls.
    pub fn move_rel(&mut self, dx: f32, dy: f32) -> io::Result<()> {
        self.accum_x += dx;
        self.accum_y += dy;

        let ix = self.accum_x.trunc() as i32;
        let iy = self.accum_y.trunc() as i32;

        self.accum_x -= ix as f32;
        self.accum_y -= iy as f32;

        if ix != 0 || iy != 0 {
            let mut events = Vec::with_capacity(2);
            if ix != 0 {
                events.push(InputEvent::new(EventType::RELATIVE, RelativeAxisType::REL_X.0, ix));
            }
            if iy != 0 {
                events.push(InputEvent::new(EventType::RELATIVE, RelativeAxisType::REL_Y.0, iy));
            }
            self.device.emit(&events)?;
        }

        Ok(())
    }

    /// Emits an absolute cursor position, clamped to the 0..[`ABS_MAX`] space
    /// the axes were declared with.
    pub fn move_abs(&mut self, x: f32, y: f32) -> io::Result<()> {
        let ix = (x.round() as i32).clamp(0, ABS_MAX);
        let iy = (y.round() as i32).clamp(0, ABS_MAX);
        let events = [
            InputEvent::new(EventType::ABSOLUTE, AbsoluteAxisType::ABS_X.0, ix),
            InputEvent::new(EventType::ABSOLUTE, AbsoluteAxisType::ABS_Y.0, iy),
        ];
        self.device.emit(&events)
    }

    /// Emits vertical scroll events.
    pub fn scroll(&mut self, lines: f32) -> io::Result<()> {
        self.accum_scroll += lines;
        let n = self.accum_scroll.trunc() as i32;
        if n != 0 {
            self.accum_scroll -= n as f32;
            let event = InputEvent::new(EventType::RELATIVE, RelativeAxisType::REL_WHEEL.0, n);
            self.device.emit(&[event])?;
        }
        Ok(())
    }

    /// Emits a button press. The matching [`Self::release`] comes later from
    /// the controller's render tick, so no async worker is ever blocked on the
    /// press duration.
    pub fn press(&mut self, button: MouseButton) -> io::Result<()> {
        self.emit_button(button, 1)
    }

    /// Emits a button release.
    pub fn release(&mut self, button: MouseButton) -> io::Result<()> {
        self.emit_button(button, 0)
    }

    fn emit_button(&mut self, button: MouseButton, value: i32) -> io::Result<()> {
        let key = match button {
            MouseButton::Left => Key::BTN_LEFT,
            MouseButton::Right => Key::BTN_RIGHT,
            MouseButton::Middle => Key::BTN_MIDDLE,
        };
        let event = InputEvent::new(EventType::KEY, key.0, value);
        self.device.emit(&[event])?;
        debug!("Emitted mouse button {:?} = {}", button, value);
        Ok(())
    }
}

/// The udev rule script to make /dev/uinput permanently accessible to the input group.
pub const UINPUT_SETUP_SCRIPT: &str = r#"echo 'KERNEL=="uinput", MODE="0660", GROUP="input"' > /etc/udev/rules.d/99-uinput.rules && udevadm control --reload-rules && udevadm trigger /dev/uinput && chmod 0660 /dev/uinput && chgrp input /dev/uinput"#;

/// Attempts to authorize /dev/uinput using Polkit's `pkexec` GUI authentication.
pub fn grant_uinput_permission_pkexec() -> Result<(), String> {
    use std::process::Command;
    let status = Command::new("pkexec")
        .arg("sh")
        .arg("-c")
        .arg(UINPUT_SETUP_SCRIPT)
        .status()
        .map_err(|e| format!("无法启动 pkexec: {}", e))?;

    if status.success() && UinputStatus::probe().is_available() {
        Ok(())
    } else {
        Err("Polkit 授权未完成或被取消".to_string())
    }
}

/// Attempts to authorize /dev/uinput using a user-provided sudo password.
pub fn grant_uinput_permission_with_sudo(password: &str) -> Result<(), String> {
    use std::io::Write;
    use std::process::{Command, Stdio};

    let mut child = Command::new("sudo")
        .arg("-S")
        .arg("sh")
        .arg("-c")
        .arg(UINPUT_SETUP_SCRIPT)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("无法执行 sudo: {}", e))?;

    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(password.as_bytes());
        let _ = stdin.write_all(b"\n");
    }

    let output = child.wait_with_output().map_err(|e| format!("等待授权进程结束失败: {}", e))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        if stderr.contains("incorrect password") || stderr.contains("Sorry, try again") {
            return Err("密码错误，请重新输入".to_string());
        }
        return Err(format!("授权失败: {}", stderr.trim()));
    }

    if UinputStatus::probe().is_available() {
        Ok(())
    } else {
        Err("授权已执行但 /dev/uinput 尚未开放写权限，请尝试重新登录".to_string())
    }
}

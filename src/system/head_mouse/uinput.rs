use super::engine::MouseButton;
use evdev::{
    uinput::{VirtualDevice, VirtualDeviceBuilder},
    AttributeSet, EventType, InputEvent, Key, RelativeAxisType,
};
use std::fs::OpenOptions;
use std::io;
use std::path::Path;
use tracing::{debug, info};

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

    pub fn description(&self) -> &'static str {
        match self {
            Self::Available => "正常可用",
            Self::PermissionDenied => "无访问权限 (需要加入 input 用户组或配置 udev 规则)",
            Self::NotFound => "未找到 /dev/uinput 驱动节点",
            Self::Unavailable => "不可用",
        }
    }
}

/// Linux virtual mouse device powered by /dev/uinput.
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

        let device = VirtualDeviceBuilder::new()?
            .name("RustPods Head Mouse")
            .with_keys(&keys)?
            .with_relative_axes(&rel_axes)?
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

    /// Emits a mouse click (press then release).
    pub fn click(&mut self, button: MouseButton) -> io::Result<()> {
        let key = match button {
            MouseButton::Left => Key::BTN_LEFT,
            MouseButton::Right => Key::BTN_RIGHT,
            MouseButton::Middle => Key::BTN_MIDDLE,
        };

        let press = InputEvent::new(EventType::KEY, key.0, 1);
        let release = InputEvent::new(EventType::KEY, key.0, 0);

        self.device.emit(&[press])?;
        std::thread::sleep(std::time::Duration::from_millis(15));
        self.device.emit(&[release])?;

        debug!("Emitted mouse click: {:?}", button);
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

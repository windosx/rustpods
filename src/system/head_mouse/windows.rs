use super::engine::MouseButton;
use std::io;
use tracing::{debug, info};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    SendInput, INPUT, INPUT_0, INPUT_MOUSE, MOUSEEVENTF_ABSOLUTE, MOUSEEVENTF_LEFTDOWN,
    MOUSEEVENTF_LEFTUP, MOUSEEVENTF_MIDDLEDOWN, MOUSEEVENTF_MIDDLEUP, MOUSEEVENTF_MOVE,
    MOUSEEVENTF_RIGHTDOWN, MOUSEEVENTF_RIGHTUP, MOUSEEVENTF_VIRTUALDESK, MOUSEEVENTF_WHEEL,
    MOUSEINPUT,
};

pub const ABS_MAX: i32 = 1023;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UinputStatus {
    Available,
    PermissionDenied,
    NotFound,
    Unavailable,
}

impl UinputStatus {
    pub fn probe() -> Self {
        Self::Available
    }

    pub fn is_available(&self) -> bool {
        true
    }
}

pub struct UinputMouseDevice {
    accum_x: f32,
    accum_y: f32,
    accum_scroll: f32,
}

impl UinputMouseDevice {
    pub fn create() -> io::Result<Self> {
        info!("Initialized Windows virtual mouse backend via SendInput");
        Ok(Self {
            accum_x: 0.0,
            accum_y: 0.0,
            accum_scroll: 0.0,
        })
    }

    pub fn move_rel(&mut self, dx: f32, dy: f32) -> io::Result<()> {
        self.accum_x += dx;
        self.accum_y += dy;

        let ix = self.accum_x.trunc() as i32;
        let iy = self.accum_y.trunc() as i32;

        self.accum_x -= ix as f32;
        self.accum_y -= iy as f32;

        if ix != 0 || iy != 0 {
            send_mouse_input(MOUSEEVENTF_MOVE, ix, iy, 0)?;
        }
        Ok(())
    }

    pub fn move_abs(&mut self, x: f32, y: f32) -> io::Result<()> {
        let clamped_x = x.clamp(0.0, ABS_MAX as f32);
        let clamped_y = y.clamp(0.0, ABS_MAX as f32);

        // Normalize 0..ABS_MAX to 0..65535 for MOUSEEVENTF_ABSOLUTE
        let norm_x = ((clamped_x / (ABS_MAX as f32)) * 65535.0).round() as i32;
        let norm_y = ((clamped_y / (ABS_MAX as f32)) * 65535.0).round() as i32;

        send_mouse_input(
            MOUSEEVENTF_MOVE | MOUSEEVENTF_ABSOLUTE | MOUSEEVENTF_VIRTUALDESK,
            norm_x,
            norm_y,
            0,
        )
    }

    pub fn scroll(&mut self, lines: f32) -> io::Result<()> {
        self.accum_scroll += lines;
        let n = self.accum_scroll.trunc() as i32;
        if n != 0 {
            self.accum_scroll -= n as f32;
            // In Windows, WHEEL_DELTA is 120 per line
            send_mouse_input(MOUSEEVENTF_WHEEL, 0, 0, n * 120)?;
        }
        Ok(())
    }

    pub fn press(&mut self, button: MouseButton) -> io::Result<()> {
        let flag = match button {
            MouseButton::Left => MOUSEEVENTF_LEFTDOWN,
            MouseButton::Right => MOUSEEVENTF_RIGHTDOWN,
            MouseButton::Middle => MOUSEEVENTF_MIDDLEDOWN,
        };
        debug!("Emitted mouse button press: {:?}", button);
        send_mouse_input(flag, 0, 0, 0)
    }

    pub fn release(&mut self, button: MouseButton) -> io::Result<()> {
        let flag = match button {
            MouseButton::Left => MOUSEEVENTF_LEFTUP,
            MouseButton::Right => MOUSEEVENTF_RIGHTUP,
            MouseButton::Middle => MOUSEEVENTF_MIDDLEUP,
        };
        debug!("Emitted mouse button release: {:?}", button);
        send_mouse_input(flag, 0, 0, 0)
    }
}

fn send_mouse_input(
    flags: windows::Win32::UI::Input::KeyboardAndMouse::MOUSE_EVENT_FLAGS,
    dx: i32,
    dy: i32,
    data: i32,
) -> io::Result<()> {
    let input = INPUT {
        r#type: INPUT_MOUSE,
        Anonymous: INPUT_0 {
            mi: MOUSEINPUT {
                dx,
                dy,
                mouseData: data as u32,
                dwFlags: flags,
                time: 0,
                dwExtraInfo: 0,
            },
        },
    };
    let sent = unsafe { SendInput(&[input], std::mem::size_of::<INPUT>() as i32) };
    if sent == 1 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

pub fn grant_uinput_permission_pkexec() -> Result<(), String> {
    Ok(())
}

pub fn grant_uinput_permission_with_sudo(_password: &str) -> Result<(), String> {
    Ok(())
}

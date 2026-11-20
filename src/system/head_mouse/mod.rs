pub mod controller;
pub mod engine;
pub mod one_euro;

#[cfg(target_os = "linux")]
pub mod uinput;
#[cfg(target_os = "windows")]
pub mod windows;

pub use controller::{run_head_mouse_controller, HeadMouseCommand, HeadMouseHub, HeadMouseState, HEAD_MOUSE};
pub use engine::{MouseButton, MouseEngine, MouseStepOutput};
pub use one_euro::OneEuroFilter;

#[cfg(target_os = "linux")]
pub use uinput::{
    grant_uinput_permission_pkexec, grant_uinput_permission_with_sudo, UinputMouseDevice,
    UinputStatus, UINPUT_SETUP_SCRIPT,
};

#[cfg(target_os = "windows")]
pub use windows::{
    grant_uinput_permission_pkexec, grant_uinput_permission_with_sudo, UinputMouseDevice,
    UinputStatus,
};
#[cfg(target_os = "windows")]
pub const UINPUT_SETUP_SCRIPT: &str = "";

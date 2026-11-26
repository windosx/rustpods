use crate::core::aacp::commands::NoiseControlMode;
use crate::core::device::AirPodsDeviceState;
use crate::system::config::{save_config, AppConfig};
use crate::ui::UiEvent;
#[cfg(target_os = "linux")]
use ksni::menu::*;
#[cfg(target_os = "linux")]
use ksni::{Icon, ToolTip, Tray};
#[cfg(target_os = "windows")]
#[derive(Debug, Clone, Default)]
pub struct Icon {
    pub width: i32,
    pub height: i32,
    pub data: Vec<u8>,
}
use std::sync::Arc;
use tokio::sync::{mpsc, RwLock};

/// Applies a noise-control mode the way every entry point does: push it to the
/// accessory, then publish the updated device state. The accessory never
/// echoes the mode back, so without this shared publish the window and tray
/// would show the old mode until restart. One helper instead of three copies
/// (tray daemon closure, tray GUI closure, controller).
pub async fn apply_noise_mode(
    aacp: &Arc<crate::core::aacp::AacpManager>,
    device_state: &Arc<RwLock<Option<AirPodsDeviceState>>>,
    ui_tx: &mpsc::UnboundedSender<UiEvent>,
    mode: NoiseControlMode,
) {
    let _ = aacp.set_noise_mode(mode).await;
    let updated = {
        let mut guard = device_state.write().await;
        match guard.as_mut() {
            Some(state) => {
                state.noise_mode = mode;
                Some(state.clone())
            }
            None => None,
        }
    };
    if let Some(state) = updated {
        let _ = ui_tx.send(UiEvent::DeviceStateUpdated(state));
    }
}

/// Applies an ear-detection toggle the way every entry point does: persist it,
/// push it to the accessory, and publish the updated device state.
pub async fn apply_ear_detection_toggle(
    aacp: &Arc<crate::core::aacp::AacpManager>,
    config: &Arc<std::sync::RwLock<AppConfig>>,
    device_state: &Arc<RwLock<Option<AirPodsDeviceState>>>,
    ui_tx: &mpsc::UnboundedSender<UiEvent>,
    enabled: bool,
) {
    if let Ok(mut c) = config.write() {
        c.ear_detection_enabled = enabled;
        save_config(&c);
    }
    let _ = aacp.set_ear_detection(enabled).await;
    let updated = {
        let mut guard = device_state.write().await;
        match guard.as_mut() {
            Some(state) => {
                state.ear_detection_enabled = enabled;
                Some(state.clone())
            }
            None => None,
        }
    };
    if let Some(state) = updated {
        let _ = ui_tx.send(UiEvent::DeviceStateUpdated(state));
    }
}

pub struct RustPodsTray {
    pub device_state: Arc<RwLock<Option<AirPodsDeviceState>>>,
    pub on_open_window: Arc<dyn Fn() + Send + Sync>,
    pub on_set_noise_mode: Arc<dyn Fn(NoiseControlMode) + Send + Sync>,
    pub on_toggle_ear_detection: Arc<dyn Fn(bool) + Send + Sync>,
    /// Requests a graceful application shutdown. `std::process::exit` here
    /// would skip every Drop - most importantly the HRTF client's, which
    /// restores the default audio sink on the way out.
    pub on_quit: Arc<dyn Fn() + Send + Sync>,
}

#[cfg(target_os = "linux")]
impl Tray for RustPodsTray {
    fn id(&self) -> String {
        "rustpods".to_string()
    }

    fn title(&self) -> String {
        "RustPods".to_string()
    }

    fn icon_name(&self) -> String {
        let state_guard = self.device_state.try_read();
        let state_opt = state_guard.ok().and_then(|g| g.clone());
        let is_connected = state_opt.as_ref().map(|s| s.is_connected).unwrap_or(false);
        if is_connected {
            // Must return empty string so the desktop status bar renders icon_pixmap (circular battery)
            "".to_string()
        } else {
            "io.github.rustpods.RustPods".to_string()
        }
    }

    fn icon_pixmap(&self) -> Vec<Icon> {
        let state_guard = self.device_state.try_read();
        let state_opt = state_guard.ok().and_then(|g| g.clone());

        let (level, charging, connected) = if let Some(state) = state_opt {
            let min_bud = match (state.battery_left, state.battery_right) {
                (Some(l), Some(r)) => Some(l.min(r)),
                (Some(l), None) => Some(l),
                (None, Some(r)) => Some(r),
                (None, None) => None,
            };
            let charging = state.battery_left_charging || state.battery_right_charging;
            (min_bud, charging, state.is_connected)
        } else {
            (None, false, false)
        };

        vec![
            generate_circular_battery_icon(24, level, charging, connected),
            generate_circular_battery_icon(32, level, charging, connected),
            generate_circular_battery_icon(48, level, charging, connected),
            generate_circular_battery_icon(64, level, charging, connected),
        ]
    }

    fn tool_tip(&self) -> ToolTip {
        let state_guard = self.device_state.try_read();
        let state_opt = state_guard.ok().and_then(|g| g.clone());

        if let Some(state) = state_opt {
            if !state.is_connected {
                return ToolTip {
                    icon_name: "io.github.rustpods.RustPods".to_string(),
                    icon_pixmap: vec![],
                    title: format!("{} {}", state.name, rust_i18n::t!("tray.tag_disconnected")),
                    description: rust_i18n::t!("tray.not_connected").to_string(),
                };
            }
            let l = state
                .battery_left
                .map(|v| format!("{}%", v))
                .unwrap_or_else(|| "--".to_string());
            let r = state
                .battery_right
                .map(|v| format!("{}%", v))
                .unwrap_or_else(|| "--".to_string());
            let c = state
                .battery_case
                .map(|v| format!("{}%", v))
                .unwrap_or_else(|| rust_i18n::t!("tray.case_closed").to_string());
            let charge_sym = if state.battery_left_charging || state.battery_right_charging {
                format!(" [{}]", rust_i18n::t!("battery.charging"))
            } else {
                String::new()
            };

            ToolTip {
                icon_name: "io.github.rustpods.RustPods".to_string(),
                icon_pixmap: vec![],
                title: format!("{} (AirPods){}", state.name, charge_sym),
                description: rust_i18n::t!(
                    "tray.tip_description",
                    left = l,
                    right = r,
                    case = c
                )
                .to_string(),
            }
        } else {
            ToolTip {
                icon_name: "io.github.rustpods.RustPods".to_string(),
                icon_pixmap: vec![],
                title: "RustPods".to_string(),
                description: "未连接 AirPods".to_string(),
            }
        }
    }

    fn activate(&mut self, _x: i32, _y: i32) {
        (self.on_open_window)();
    }

    fn menu(&self) -> Vec<MenuItem<Self>> {
        let state_guard = self.device_state.try_read();
        let state_opt = state_guard.ok().and_then(|g| g.clone());

        let mut menu: Vec<MenuItem<Self>> = Vec::new();

        if let Some(state) = state_opt {
            let header_text = if !state.is_connected {
                format!("{} {}", state.name, rust_i18n::t!("tray.tag_disconnected"))
            } else if !state.is_wearing_any()
                && (state.left_in_ear == crate::core::aacp::events::EarStatus::InCase
                    || state.right_in_ear == crate::core::aacp::events::EarStatus::InCase)
            {
                format!("{} {}", state.name, rust_i18n::t!("tray.tag_in_case"))
            } else {
                format!("{} {}", state.name, rust_i18n::t!("tray.tag_connected"))
            };
            menu.push(
                StandardItem {
                    label: header_text,
                    enabled: false,
                    ..Default::default()
                }
                .into(),
            );

            // Battery item (only when connected; disconnected BLE proximity readings
            // are reserved for the case-open popup)
            if state.is_connected {
                let l_str = state
                    .battery_left
                    .map(|v| format!("{}%", v))
                    .unwrap_or_else(|| "--".to_string());
                let r_str = state
                    .battery_right
                    .map(|v| format!("{}%", v))
                    .unwrap_or_else(|| "--".to_string());
                let c_str = state
                    .battery_case
                    .map(|v| format!("{}%", v))
                    .unwrap_or_else(|| rust_i18n::t!("tray.case_closed").to_string());

                let batt_text = rust_i18n::t!(
                    "tray.battery_line",
                    left = l_str,
                    right = r_str,
                    case = c_str
                )
                .to_string();
                menu.push(
                    StandardItem {
                        label: batt_text,
                        enabled: false,
                        ..Default::default()
                    }
                    .into(),
                );
            }

            menu.push(MenuItem::Separator);

            // Noise Control submenu if supported
            if state.capabilities.has_anc {
                let current_mode = state.noise_mode;
                let mut mode_items: Vec<MenuItem<Self>> = Vec::new();

                let on_mode_anc = self.on_set_noise_mode.clone();
                mode_items.push(
                    StandardItem {
                        label: format!(
                            "{} {}",
                            if current_mode == NoiseControlMode::NoiseCancellation {
                                "●"
                            } else {
                                "○"
                            },
                            rust_i18n::t!("noise.anc")
                        ),
                        activate: Box::new(move |_| {
                            on_mode_anc(NoiseControlMode::NoiseCancellation);
                        }),
                        ..Default::default()
                    }
                    .into(),
                );

                let on_mode_trans = self.on_set_noise_mode.clone();
                mode_items.push(
                    StandardItem {
                        label: format!(
                            "{} {}",
                            if current_mode == NoiseControlMode::Transparency {
                                "●"
                            } else {
                                "○"
                            },
                            rust_i18n::t!("noise.transparency")
                        ),
                        activate: Box::new(move |_| {
                            on_mode_trans(NoiseControlMode::Transparency);
                        }),
                        ..Default::default()
                    }
                    .into(),
                );

                if state.capabilities.has_adaptive_audio {
                    let on_mode_adapt = self.on_set_noise_mode.clone();
                    mode_items.push(
                        StandardItem {
                            label: format!(
                                "{} {}",
                                if current_mode == NoiseControlMode::Adaptive {
                                    "●"
                                } else {
                                    "○"
                                },
                                rust_i18n::t!("noise.adaptive")
                            ),
                            activate: Box::new(move |_| {
                                on_mode_adapt(NoiseControlMode::Adaptive);
                            }),
                            ..Default::default()
                        }
                        .into(),
                    );
                }

                let on_mode_off = self.on_set_noise_mode.clone();
                mode_items.push(
                    StandardItem {
                        label: format!(
                            "{} {}",
                            if current_mode == NoiseControlMode::Off {
                                "●"
                            } else {
                                "○"
                            },
                            rust_i18n::t!("noise.off")
                        ),
                        activate: Box::new(move |_| {
                            on_mode_off(NoiseControlMode::Off);
                        }),
                        ..Default::default()
                    }
                    .into(),
                );

                menu.push(
                    SubMenu {
                        label: rust_i18n::t!("tray.noise_control").into(),
                        submenu: mode_items,
                        ..Default::default()
                    }
                    .into(),
                );
            }

            // In-ear detection toggle
            let on_toggle = self.on_toggle_ear_detection.clone();
            let ear_enabled = state.ear_detection_enabled;
            menu.push(
                CheckmarkItem {
                    label: rust_i18n::t!("tray.ear_detection").into(),
                    checked: ear_enabled,
                    activate: Box::new(move |_| {
                        on_toggle(!ear_enabled);
                    }),
                    ..Default::default()
                }
                .into(),
            );
        } else {
            menu.push(
                StandardItem {
                    label: rust_i18n::t!("tray.not_connected").into(),
                    enabled: false,
                    ..Default::default()
                }
                .into(),
            );
        }

        menu.push(MenuItem::Separator);

        let on_open = self.on_open_window.clone();
        menu.push(
            StandardItem {
                label: rust_i18n::t!("tray.open_app").into(),
                activate: Box::new(move |_| {
                    on_open();
                }),
                ..Default::default()
            }
            .into(),
        );

        let on_quit = self.on_quit.clone();
        menu.push(
            StandardItem {
                label: rust_i18n::t!("tray.quit").into(),
                activate: Box::new(move |_| {
                    on_quit();
                }),
                ..Default::default()
            }
            .into(),
        );

        menu
    }
}

pub fn generate_circular_battery_icon(
    size: usize,
    level: Option<u8>,
    charging: bool,
    connected: bool,
) -> Icon {
    let mut data = Vec::with_capacity(size * size * 4);

    let cx = (size as f32) / 2.0;
    let cy = (size as f32) / 2.0;
    let outer_r = cx - 1.2;
    let inner_r = cx - (size as f32 * 0.22).max(3.5);

    // Unknown level draws the track only: pretending 100% (the old
    // unwrap_or(100)) showed a full green ring before any battery report.
    let pct = level.unwrap_or(0) as f32 / 100.0;

    // Color determination (ARGB)
    let arc_color = if charging {
        [255, 52, 199, 89] // Electric charging green
    } else if pct > 0.40 {
        [255, 48, 209, 88] // High battery green
    } else if pct > 0.20 {
        [255, 255, 214, 10] // Medium battery yellow
    } else {
        [255, 255, 69, 58] // Low battery red
    };

    let track_color = if connected {
        [80, 200, 200, 200] // Translucent light ring
    } else {
        [40, 140, 140, 140] // Dim disconnected ring
    };

    for y in 0..size {
        for x in 0..size {
            let dx = x as f32 + 0.5 - cx;
            let dy = y as f32 + 0.5 - cy;
            let dist = (dx * dx + dy * dy).sqrt();

            if dist >= inner_r && dist <= outer_r {
                // Angle from top (12 o'clock), clockwise [0..2*PI]
                let angle = dy.atan2(dx) + std::f32::consts::FRAC_PI_2;
                let norm_angle = (angle + 2.0 * std::f32::consts::PI) % (2.0 * std::f32::consts::PI);
                let progress_angle = pct * 2.0 * std::f32::consts::PI;

                if connected && norm_angle <= progress_angle {
                    data.extend_from_slice(&arc_color);
                } else {
                    data.extend_from_slice(&track_color);
                }
            } else if dist < inner_r - 1.5 {
                if connected && charging {
                    // Draw central lightning bolt ⚡
                    if is_in_lightning_glyph(x, y, size) {
                        data.extend_from_slice(&[255, 52, 199, 89]);
                    } else {
                        data.extend_from_slice(&[0, 0, 0, 0]);
                    }
                } else if connected && level.is_some() {
                    let core_r = inner_r * 0.45;
                    if dist <= core_r {
                        let alpha = ((1.0 - dist / core_r) * 200.0) as u8;
                        data.push(alpha);
                        data.push(arc_color[1]);
                        data.push(arc_color[2]);
                        data.push(arc_color[3]);
                    } else {
                        data.extend_from_slice(&[0, 0, 0, 0]);
                    }
                } else {
                    data.extend_from_slice(&[0, 0, 0, 0]);
                }
            } else {
                data.extend_from_slice(&[0, 0, 0, 0]); // Transparent
            }
        }
    }

    Icon {
        width: size as i32,
        height: size as i32,
        data,
    }
}

fn is_in_lightning_glyph(x: usize, y: usize, size: usize) -> bool {
    let scale = size as f32 / 64.0;
    let norm_x = x as f32 / scale;
    let norm_y = y as f32 / scale;

    if norm_y < 22.0 || norm_y > 42.0 || norm_x < 26.0 || norm_x > 38.0 {
        return false;
    }
    if (31.0..=33.0).contains(&norm_y) && (27.0..=37.0).contains(&norm_x) {
        return true;
    }
    if norm_y < 32.0 {
        let expected_x = 34.0 - (norm_y - 22.0) * 0.7;
        (norm_x - expected_x).abs() <= 2.2
    } else {
        let expected_x = 37.0 - (norm_y - 31.0) * 0.7;
        (norm_x - expected_x).abs() <= 2.2
    }
}

#[cfg(target_os = "windows")]
pub fn create_hicon_from_icon(
    icon: &Icon,
) -> Option<windows::Win32::UI::WindowsAndMessaging::HICON> {
    use windows::Win32::Graphics::Gdi::{CreateBitmap, DeleteObject};
    use windows::Win32::UI::WindowsAndMessaging::{CreateIconIndirect, ICONINFO};

    let width = icon.width;
    let height = icon.height;
    if width <= 0 || height <= 0 || icon.data.len() != (width * height * 4) as usize {
        return None;
    }

    // Convert ARGB to BGRA
    let mut bgra = Vec::with_capacity(icon.data.len());
    for [a, r, g, b] in icon.data.as_chunks::<4>().0 {
        bgra.push(*b);
        bgra.push(*g);
        bgra.push(*r);
        bgra.push(*a);
    }

    unsafe {
        let hbm_color = CreateBitmap(width, height, 1, 32, Some(bgra.as_ptr() as *const _));
        let mask_bytes = vec![0u8; ((width + 15) / 16 * 2 * height) as usize];
        let hbm_mask = CreateBitmap(width, height, 1, 1, Some(mask_bytes.as_ptr() as *const _));

        let icon_info = ICONINFO {
            fIcon: true.into(),
            xHotspot: 0,
            yHotspot: 0,
            hbmMask: hbm_mask,
            hbmColor: hbm_color,
        };

        let hicon = CreateIconIndirect(&icon_info).ok();
        let _ = DeleteObject(hbm_color);
        let _ = DeleteObject(hbm_mask);
        hicon
    }
}

#[cfg(target_os = "windows")]
const WM_TRAY_CALLBACK: u32 = windows::Win32::UI::WindowsAndMessaging::WM_USER + 100;
#[cfg(target_os = "windows")]
pub const WM_TRAY_SHOW_MAIN: u32 = windows::Win32::UI::WindowsAndMessaging::WM_USER + 101;
#[cfg(target_os = "windows")]
const TRAY_ICON_ID: u32 = 1;
#[cfg(target_os = "windows")]
const TIMER_ID: usize = 1;

#[cfg(target_os = "windows")]
const ID_OPEN_WINDOW: u32 = 2001;
#[cfg(target_os = "windows")]
const ID_NOISE_OFF: u32 = 2002;
#[cfg(target_os = "windows")]
const ID_NOISE_TRANSPARENCY: u32 = 2003;
#[cfg(target_os = "windows")]
const ID_NOISE_ADAPTIVE: u32 = 2004;
#[cfg(target_os = "windows")]
const ID_NOISE_ANC: u32 = 2005;
#[cfg(target_os = "windows")]
const ID_EAR_DETECTION: u32 = 2006;
#[cfg(target_os = "windows")]
const ID_QUIT: u32 = 2007;

#[cfg(target_os = "windows")]
unsafe extern "system" fn tray_wndproc(
    hwnd: windows::Win32::Foundation::HWND,
    msg: u32,
    wparam: windows::Win32::Foundation::WPARAM,
    lparam: windows::Win32::Foundation::LPARAM,
) -> windows::Win32::Foundation::LRESULT {
    use windows::Win32::Foundation::LRESULT;
    use windows::Win32::UI::WindowsAndMessaging::*;

    unsafe {
        let tray_ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *const RustPodsTray;
        if tray_ptr.is_null() {
            return DefWindowProcW(hwnd, msg, wparam, lparam);
        }
        let tray = &*tray_ptr;

        match msg {
            WM_TRAY_CALLBACK => {
                let event = lparam.0 as u32;
                if event == WM_LBUTTONUP || event == WM_LBUTTONDBLCLK {
                    (tray.on_open_window)();
                } else if event == WM_RBUTTONUP {
                    show_tray_menu(hwnd, tray);
                }
                LRESULT(0)
            }
            WM_TRAY_SHOW_MAIN => {
                (tray.on_open_window)();
                LRESULT(0)
            }
            WM_COMMAND => {
                let cmd_id = (wparam.0 & 0xffff) as u32;
                handle_tray_command(cmd_id, tray);
                LRESULT(0)
            }
            WM_TIMER => {
                update_tray_icon_and_tip(hwnd, tray);
                LRESULT(0)
            }
            WM_CLOSE => {
                let _ = DestroyWindow(hwnd);
                LRESULT(0)
            }
            WM_DESTROY => {
                PostQuitMessage(0);
                LRESULT(0)
            }
            _ => DefWindowProcW(hwnd, msg, wparam, lparam),
        }
    }
}

#[cfg(target_os = "windows")]
unsafe fn show_tray_menu(hwnd: windows::Win32::Foundation::HWND, tray: &RustPodsTray) {
    use windows::core::{w, PCWSTR};
    use windows::Win32::Foundation::{LPARAM, POINT, WPARAM};
    use windows::Win32::UI::WindowsAndMessaging::*;

    unsafe {
        let mut pt = POINT::default();
        let _ = GetCursorPos(&mut pt);
        let _ = SetForegroundWindow(hwnd);

        let hmenu = match CreatePopupMenu() {
            Ok(m) => m,
            Err(_) => return,
        };

        let state = tray.device_state.try_read().ok().and_then(|g| g.clone());
        let is_connected = state.as_ref().map(|s| s.is_connected).unwrap_or(false);
        let has_anc = state.as_ref().map(|s| s.capabilities.has_anc).unwrap_or(false);
        let current_noise = state.as_ref().map(|s| s.noise_mode).unwrap_or(NoiseControlMode::Off);
        let ear_detect_enabled = state.as_ref().map(|s| s.ear_detection_enabled).unwrap_or(true);

        let _ = AppendMenuW(hmenu, MF_STRING, ID_OPEN_WINDOW as usize, w!("打开 RustPods"));
        let _ = AppendMenuW(hmenu, MF_SEPARATOR, 0, PCWSTR::null());

        if is_connected && has_anc
            && let Ok(noise_menu) = CreatePopupMenu()
        {
                let check_off = if current_noise == NoiseControlMode::Off { MF_CHECKED } else { MF_UNCHECKED };
                let check_trans = if current_noise == NoiseControlMode::Transparency { MF_CHECKED } else { MF_UNCHECKED };
                let check_adapt = if current_noise == NoiseControlMode::Adaptive { MF_CHECKED } else { MF_UNCHECKED };
                let check_anc = if current_noise == NoiseControlMode::NoiseCancellation { MF_CHECKED } else { MF_UNCHECKED };

                let _ = AppendMenuW(noise_menu, MF_STRING | check_off, ID_NOISE_OFF as usize, w!("关闭"));
                let _ = AppendMenuW(noise_menu, MF_STRING | check_trans, ID_NOISE_TRANSPARENCY as usize, w!("通透模式"));
                let _ = AppendMenuW(noise_menu, MF_STRING | check_adapt, ID_NOISE_ADAPTIVE as usize, w!("自适应音频"));
                let _ = AppendMenuW(noise_menu, MF_STRING | check_anc, ID_NOISE_ANC as usize, w!("降噪"));

            let _ = AppendMenuW(hmenu, MF_POPUP, noise_menu.0 as usize, w!("噪声控制"));
        }

        if is_connected {
            let ear_flags = if ear_detect_enabled { MF_CHECKED } else { MF_UNCHECKED };
            let _ = AppendMenuW(hmenu, MF_STRING | ear_flags, ID_EAR_DETECTION as usize, w!("自动入耳检测"));
            let _ = AppendMenuW(hmenu, MF_SEPARATOR, 0, PCWSTR::null());
        }

        let _ = AppendMenuW(hmenu, MF_STRING, ID_QUIT as usize, w!("退出"));

        let _ = TrackPopupMenu(hmenu, TPM_RIGHTBUTTON | TPM_BOTTOMALIGN, pt.x, pt.y, 0, hwnd, None);
        let _ = PostMessageW(hwnd, WM_NULL, WPARAM(0), LPARAM(0));
        let _ = DestroyMenu(hmenu);
    }
}

#[cfg(target_os = "windows")]
fn handle_tray_command(cmd_id: u32, tray: &RustPodsTray) {
    match cmd_id {
        ID_OPEN_WINDOW => (tray.on_open_window)(),
        ID_NOISE_OFF => (tray.on_set_noise_mode)(NoiseControlMode::Off),
        ID_NOISE_TRANSPARENCY => (tray.on_set_noise_mode)(NoiseControlMode::Transparency),
        ID_NOISE_ADAPTIVE => (tray.on_set_noise_mode)(NoiseControlMode::Adaptive),
        ID_NOISE_ANC => (tray.on_set_noise_mode)(NoiseControlMode::NoiseCancellation),
        ID_EAR_DETECTION => {
            let current = tray
                .device_state
                .try_read()
                .ok()
                .and_then(|g| g.clone())
                .map(|s| s.ear_detection_enabled)
                .unwrap_or(true);
            (tray.on_toggle_ear_detection)(!current);
        }
        ID_QUIT => (tray.on_quit)(),
        _ => {}
    }
}

#[cfg(target_os = "windows")]
unsafe fn update_tray_icon_and_tip(hwnd: windows::Win32::Foundation::HWND, tray: &RustPodsTray) {
    use windows::Win32::UI::Shell::{
        Shell_NotifyIconW, NIF_ICON, NIF_TIP, NIM_MODIFY, NOTIFYICONDATAW,
    };
    use windows::Win32::UI::WindowsAndMessaging::{DestroyIcon, GetPropW, SetPropW, HICON};

    let state_opt = tray.device_state.try_read().ok().and_then(|g| g.clone());

    let (level, charging, connected, tip_text) = if let Some(ref state) = state_opt {
        let min_bud = match (state.battery_left, state.battery_right) {
            (Some(l), Some(r)) => Some(l.min(r)),
            (Some(l), None) => Some(l),
            (None, Some(r)) => Some(r),
            (None, None) => None,
        };
        let charging = state.battery_left_charging || state.battery_right_charging;
        let mut tip = format!("{}: ", state.name);
        if state.is_connected {
            tip.push_str("已连接");
            if let Some(l) = state.battery_left {
                tip.push_str(&format!(" | 左: {}%", l));
            }
            if let Some(r) = state.battery_right {
                tip.push_str(&format!(" | 右: {}%", r));
            }
            if let Some(c) = state.battery_case {
                tip.push_str(&format!(" | 盒: {}%", c));
            }
        } else {
            tip.push_str("未连接");
        }
        (min_bud, charging, state.is_connected, tip)
    } else {
        (None, false, false, "RustPods (未连接)".to_string())
    };

    let icon_data = generate_circular_battery_icon(32, level, charging, connected);
    unsafe {
        if let Some(new_hicon) = create_hicon_from_icon(&icon_data) {
            let mut nid = NOTIFYICONDATAW {
                cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
                hWnd: hwnd,
                uID: TRAY_ICON_ID,
                uFlags: NIF_ICON | NIF_TIP,
                hIcon: new_hicon,
                ..Default::default()
            };

            let wide: Vec<u16> = tip_text.encode_utf16().chain(std::iter::once(0)).collect();
            let copy_len = wide.len().min(nid.szTip.len());
            nid.szTip[..copy_len].copy_from_slice(&wide[..copy_len]);

            let _ = Shell_NotifyIconW(NIM_MODIFY, &nid);

            let old_handle = GetPropW(hwnd, windows::core::w!("RustPodsHicon"));
            let _ = SetPropW(
                hwnd,
                windows::core::w!("RustPodsHicon"),
                windows::Win32::Foundation::HANDLE(new_hicon.0),
            );
            if !old_handle.is_invalid() && !old_handle.0.is_null() {
                let _ = DestroyIcon(HICON(old_handle.0));
            }
        }
    }
}

#[cfg(target_os = "windows")]
pub fn spawn_windows_tray(tray: Arc<RustPodsTray>, has_gui: bool) {
    std::thread::Builder::new()
        .name("windows-tray".to_string())
        .spawn(move || unsafe {
            use windows::core::w;
            use windows::Win32::Foundation::HINSTANCE;
            use windows::Win32::UI::Shell::{
                Shell_NotifyIconW, NIF_ICON, NIF_MESSAGE, NIF_TIP, NIM_ADD, NIM_DELETE,
                NOTIFYICONDATAW,
            };
            use windows::Win32::UI::WindowsAndMessaging::*;

            let class_name = w!("RustPodsTrayClass");
            let window_title = if has_gui {
                w!("RustPodsTrayWindow")
            } else {
                w!("RustPodsDaemonTrayWindow")
            };
            let hinstance = HINSTANCE::default();

            let wnd_class = WNDCLASSW {
                lpfnWndProc: Some(tray_wndproc),
                hInstance: hinstance,
                lpszClassName: class_name,
                ..Default::default()
            };
            let _ = RegisterClassW(&wnd_class);

            let hwnd = match CreateWindowExW(
                WINDOW_EX_STYLE::default(),
                class_name,
                window_title,
                WS_OVERLAPPEDWINDOW,
                CW_USEDEFAULT,
                CW_USEDEFAULT,
                CW_USEDEFAULT,
                CW_USEDEFAULT,
                None,
                None,
                hinstance,
                None,
            ) {
                Ok(h) => h,
                Err(_) => return,
            };

            let tray_raw = Arc::into_raw(tray);
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, tray_raw as isize);

            let initial_icon = generate_circular_battery_icon(32, None, false, false);
            let initial_hicon = create_hicon_from_icon(&initial_icon);
            if let Some(hicon) = initial_hicon {
                let _ = SetPropW(
                    hwnd,
                    w!("RustPodsHicon"),
                    windows::Win32::Foundation::HANDLE(hicon.0),
                );
            }

            let mut nid = NOTIFYICONDATAW {
                cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
                hWnd: hwnd,
                uID: TRAY_ICON_ID,
                uFlags: NIF_ICON | NIF_MESSAGE | NIF_TIP,
                uCallbackMessage: WM_TRAY_CALLBACK,
                hIcon: initial_hicon.unwrap_or_default(),
                ..Default::default()
            };
            let tip = "RustPods\0".encode_utf16().collect::<Vec<u16>>();
            nid.szTip[..tip.len()].copy_from_slice(&tip);

            let _ = Shell_NotifyIconW(NIM_ADD, &nid);

            let _ = SetTimer(hwnd, TIMER_ID, 1000, None);

            let mut msg = MSG::default();
            while GetMessageW(&mut msg, None, 0, 0).as_bool() {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }

            let _ = Shell_NotifyIconW(NIM_DELETE, &nid);
            if let Ok(cleanup_handle) = RemovePropW(hwnd, w!("RustPodsHicon"))
                && !cleanup_handle.is_invalid() && !cleanup_handle.0.is_null()
            {
                let _ = DestroyIcon(HICON(cleanup_handle.0));
            }
            let _ = Arc::from_raw(tray_raw);
        })
        .expect("Failed to spawn windows-tray thread");
}

#[cfg(target_os = "windows")]
pub fn close_windows_tray() {
    use windows::core::w;
    use windows::Win32::Foundation::{LPARAM, WPARAM};
    use windows::Win32::UI::Shell::{Shell_NotifyIconW, NIM_DELETE, NOTIFYICONDATAW};
    use windows::Win32::UI::WindowsAndMessaging::{FindWindowW, PostMessageW, WM_CLOSE};

    for title in [w!("RustPodsTrayWindow"), w!("RustPodsDaemonTrayWindow")] {
        if let Ok(hwnd) = unsafe { FindWindowW(w!("RustPodsTrayClass"), title) }
            && !hwnd.is_invalid()
            && !hwnd.0.is_null()
        {
            let nid = NOTIFYICONDATAW {
                cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
                hWnd: hwnd,
                uID: TRAY_ICON_ID,
                ..Default::default()
            };
            unsafe {
                let _ = Shell_NotifyIconW(NIM_DELETE, &nid);
                let _ = PostMessageW(hwnd, WM_CLOSE, WPARAM(0), LPARAM(0));
            }
        }
    }
}

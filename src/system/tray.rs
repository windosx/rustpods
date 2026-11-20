use crate::core::aacp::commands::NoiseControlMode;
use crate::core::device::AirPodsDeviceState;
use crate::system::config::{save_config, AppConfig};
use crate::ui::UiEvent;
#[cfg(target_os = "linux")]
use ksni::menu::*;
#[cfg(target_os = "linux")]
use ksni::{Icon, ToolTip, Tray};
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

            // Battery item
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

#[cfg(target_os = "linux")]
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
                if charging {
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

use crate::core::aacp::commands::NoiseControlMode;
use crate::core::device::AirPodsDeviceState;
use ksni::menu::*;
use ksni::{Icon, ToolTip, Tray};
use std::sync::Arc;
use tokio::sync::RwLock;

pub struct RustPodsTray {
    pub device_state: Arc<RwLock<Option<AirPodsDeviceState>>>,
    pub on_open_window: Arc<dyn Fn() + Send + Sync>,
    pub on_set_noise_mode: Arc<dyn Fn(NoiseControlMode) + Send + Sync>,
    pub on_toggle_ear_detection: Arc<dyn Fn(bool) + Send + Sync>,
}

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
            "audio-headphones".to_string()
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
                .unwrap_or_else(|| "闭盖".to_string());
            let charge_sym = if state.battery_left_charging || state.battery_right_charging {
                " [充电中]"
            } else {
                ""
            };

            ToolTip {
                icon_name: "audio-headphones".to_string(),
                icon_pixmap: vec![],
                title: format!("{} (AirPods){}", state.name, charge_sym),
                description: format!("左耳: {}  右耳: {}  充电盒: {}", l, r, c),
            }
        } else {
            ToolTip {
                icon_name: "audio-headphones".to_string(),
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
                format!("{} (未连接)", state.name)
            } else if !state.is_wearing_any()
                && (state.left_in_ear == crate::core::aacp::events::EarStatus::InCase
                    || state.right_in_ear == crate::core::aacp::events::EarStatus::InCase)
            {
                format!("{} (充电盒中)", state.name)
            } else {
                format!("{} (已连接)", state.name)
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
                .unwrap_or_else(|| "闭盖".to_string());

            let batt_text = format!("左耳: {} | 右耳: {} | 充电盒: {}", l_str, r_str, c_str);
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
                            "{} 降噪 (Noise Cancellation)",
                            if current_mode == NoiseControlMode::NoiseCancellation {
                                "● "
                            } else {
                                "○ "
                            }
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
                            "{} 通透模式 (Transparency)",
                            if current_mode == NoiseControlMode::Transparency {
                                "● "
                            } else {
                                "○ "
                            }
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
                                "{} 自适应 (Adaptive)",
                                if current_mode == NoiseControlMode::Adaptive {
                                    "● "
                                } else {
                                    "○ "
                                }
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
                            "{} 关闭 (Off)",
                            if current_mode == NoiseControlMode::Off {
                                "● "
                            } else {
                                "○ "
                            }
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
                        label: "噪声控制".into(),
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
                    label: "自动入耳检测".into(),
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
                    label: "未连接 AirPods".into(),
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
                label: "打开 RustPods 设置...".into(),
                activate: Box::new(move |_| {
                    on_open();
                }),
                ..Default::default()
            }
            .into(),
        );

        menu.push(
            StandardItem {
                label: "退出应用".into(),
                activate: Box::new(|_| {
                    std::process::exit(0);
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
    if norm_y >= 31.0 && norm_y <= 33.0 && norm_x >= 27.0 && norm_x <= 37.0 {
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

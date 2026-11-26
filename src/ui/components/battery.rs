use crate::core::aacp::events::EarStatus;
use crate::core::device::AirPodsDeviceState;
use crate::ui::theme::{control_card, ApplePalette};
use gpui_kit::component::Icon;
use gpui_kit::{
    canvas, div, point, px, App, Bounds, FontWeight, Hsla, IntoElement, ParentElement as _,
    PathBuilder, Pixels, Point, Styled as _, Window,
};
use std::f32::consts::{PI, TAU};

pub fn render_battery_card(state: &AirPodsDeviceState, p: &ApplePalette) -> impl IntoElement {
    div()
        .w_full()
        .flex()
        .flex_row()
        .gap(px(12.))
        .child(render_single_battery_column(
            &rust_i18n::t!("battery.left"),
            state.battery_left,
            state.battery_left_charging,
            state.left_in_ear,
            false,
            state.is_connected,
            p,
        ))
        .child(render_single_battery_column(
            &rust_i18n::t!("battery.right"),
            state.battery_right,
            state.battery_right_charging,
            state.right_in_ear,
            false,
            state.is_connected,
            p,
        ))
        .child(render_single_battery_column(
            &rust_i18n::t!("battery.case"),
            state.battery_case,
            state.battery_case_charging,
            EarStatus::InCase,
            true,
            state.is_connected,
            p,
        ))
}

fn effective_battery_display(
    level_opt: Option<u8>,
    charging: bool,
    is_connected: bool,
) -> (Option<u8>, bool) {
    if is_connected {
        (level_opt, charging)
    } else {
        (None, false)
    }
}

fn render_single_battery_column(
    title: &str,
    level_opt: Option<u8>,
    charging: bool,
    ear_status: EarStatus,
    is_case: bool,
    is_connected: bool,
    p: &ApplePalette,
) -> impl IntoElement {
    let (display_level, display_charging) =
        effective_battery_display(level_opt, charging, is_connected);
    let pct_str = format_center_text(display_level);
    let status_str =
        format_status_string(display_level, display_charging, ear_status, is_case, is_connected);
    let ring_color = ring_color_for(display_level, display_charging, p);
    let track_color = p.track_bg;
    let status_color = if display_charging {
        p.accent_green
    } else {
        p.text_secondary
    };

    control_card(p)
        .flex_1()
        .min_w_0()
        .py(px(16.))
        .px(px(12.))
        .flex()
        .flex_col()
        .items_center()
        .gap(px(10.))
        .child(
            div()
                .text_size(px(13.))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(p.text_primary)
                .child(title.to_string()),
        )
        .child(
            div()
                .relative()
                .w(px(84.))
                .h(px(84.))
                .flex()
                .items_center()
                .justify_center()
                .child(
                    canvas(
                        move |bounds: Bounds<Pixels>, _window: &mut Window, _cx: &mut App| bounds,
                        move |_bounds, bounds: Bounds<Pixels>, window: &mut Window, _cx: &mut App| {
                            paint_battery_ring(
                                window,
                                bounds,
                                display_level,
                                track_color,
                                ring_color,
                            );
                        },
                    )
                    .absolute()
                    .size_full(),
                )
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .items_center()
                        .justify_center()
                        .child(
                            div()
                                .text_size(px(21.))
                                .font_weight(FontWeight::BOLD)
                                .text_color(p.text_primary)
                                .child(pct_str),
                        )
                        .when(display_charging, |this| {
                            this.child(
                                Icon::new(gpui_kit::assets::IconName::Zap)
                                    .size(px(11.))
                                    .text_color(p.accent_green),
                            )
                        }),
                ),
        )
        .child(
            div()
                .text_size(px(11.5))
                .text_color(status_color)
                .child(status_str),
        )
}

fn paint_battery_ring(
    window: &mut Window,
    bounds: Bounds<Pixels>,
    level: Option<u8>,
    track_color: Hsla,
    ring_color: Hsla,
) {
    let cx = bounds.origin.x.as_f32() + bounds.size.width.as_f32() * 0.5;
    let cy = bounds.origin.y.as_f32() + bounds.size.height.as_f32() * 0.5;
    let radius = (bounds.size.width.as_f32().min(bounds.size.height.as_f32()) * 0.5 - 8.0).max(12.0);
    let stroke_w = 6.5_f32;

    // Background track ring
    let mut track = PathBuilder::stroke(px(stroke_w));
    const TRACK_SEGMENTS: usize = 64;
    for i in 0..=TRACK_SEGMENTS {
        let a = (i as f32 / TRACK_SEGMENTS as f32) * TAU;
        let pt = point(px(cx + radius * a.cos()), px(cy + radius * a.sin()));
        if i == 0 {
            track.move_to(pt);
        } else {
            track.line_to(pt);
        }
    }
    if let Ok(path) = track.build() {
        window.paint_path(path, track_color);
    }

    let Some(level_val) = level else {
        return;
    };
    let pct = (level_val as f32 / 100.0).clamp(0.0, 1.0);
    if pct <= 0.0 {
        return;
    }

    let start_angle = -PI / 2.0;
    let sweep = pct * TAU;
    let segments = ((pct * 72.0).ceil() as usize).max(2);
    let mut arc = PathBuilder::stroke(px(stroke_w));
    let mut start_pt = Point::default();
    let mut end_pt = Point::default();

    for i in 0..=segments {
        let a = start_angle + (i as f32 / segments as f32) * sweep;
        let pt = point(px(cx + radius * a.cos()), px(cy + radius * a.sin()));
        if i == 0 {
            start_pt = pt;
            arc.move_to(pt);
        } else {
            arc.line_to(pt);
        }
        if i == segments {
            end_pt = pt;
        }
    }
    if let Ok(path) = arc.build() {
        window.paint_path(path, ring_color);
    }

    // Rounded endcaps
    let cap_r = stroke_w * 0.5;
    paint_disc(window, start_pt, cap_r, ring_color);
    paint_disc(window, end_pt, cap_r, ring_color);
}

fn paint_disc(window: &mut Window, center: Point<Pixels>, radius: f32, color: Hsla) {
    let mut disc = PathBuilder::fill();
    const STEPS: usize = 16;
    for i in 0..STEPS {
        let a = (i as f32 / STEPS as f32) * TAU;
        let pt = point(
            px(center.x.as_f32() + radius * a.cos()),
            px(center.y.as_f32() + radius * a.sin()),
        );
        if i == 0 {
            disc.move_to(pt);
        } else {
            disc.line_to(pt);
        }
    }
    disc.close();
    if let Ok(path) = disc.build() {
        window.paint_path(path, color);
    }
}

fn ring_color_for(level: Option<u8>, charging: bool, p: &ApplePalette) -> Hsla {
    if charging {
        return p.accent_green;
    }
    match level {
        Some(v) if v > 40 => p.accent_green,
        Some(v) if v > 20 => p.accent_amber,
        _ => p.accent_red,
    }
}

fn format_center_text(level: Option<u8>) -> String {
    match level {
        Some(l) => l.to_string(),
        None => "--".to_string(),
    }
}

fn format_status_string(
    level: Option<u8>,
    charging: bool,
    ear_status: EarStatus,
    is_case: bool,
    is_connected: bool,
) -> String {
    if !is_connected {
        return if level.is_none() {
            if is_case {
                rust_i18n::t!("battery.sleeping").to_string()
            } else {
                rust_i18n::t!("battery.waiting_connection").to_string()
            }
        } else {
            rust_i18n::t!("battery.disconnected_status").to_string()
        };
    }

    if level.is_none() {
        return if is_case {
            rust_i18n::t!("battery.sleeping").to_string()
        } else {
            rust_i18n::t!("battery.waiting_connection").to_string()
        };
    }

    if is_case {
        return if charging {
            rust_i18n::t!("battery.charging").to_string()
        } else {
            rust_i18n::t!("battery.in_use").to_string()
        };
    }

    match ear_status {
        EarStatus::Disconnected => {
            if charging {
                format!(
                    "{} • {}",
                    rust_i18n::t!("battery.charging"),
                    rust_i18n::t!("battery.disconnected_status")
                )
            } else {
                rust_i18n::t!("battery.disconnected_status").to_string()
            }
        }
        EarStatus::InCase => {
            if charging {
                format!(
                    "{} • {}",
                    rust_i18n::t!("battery.charging"),
                    rust_i18n::t!("battery.in_case_status")
                )
            } else {
                rust_i18n::t!("battery.in_case_status").to_string()
            }
        }
        EarStatus::InEar => {
            let charge_str = if charging {
                rust_i18n::t!("battery.charging")
            } else {
                rust_i18n::t!("battery.in_use")
            };
            format!("{} • {}", charge_str, rust_i18n::t!("battery.worn"))
        }
        EarStatus::OutOfEar => {
            let charge_str = if charging {
                rust_i18n::t!("battery.charging")
            } else {
                rust_i18n::t!("battery.in_use")
            };
            format!("{} • {}", charge_str, rust_i18n::t!("battery.unworn"))
        }
    }
}

use gpui_kit::prelude::FluentBuilder as _;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disconnected_device_hides_battery_level_and_charging() {
        assert_eq!(effective_battery_display(Some(85), true, false), (None, false));
        assert_eq!(effective_battery_display(Some(0), false, false), (None, false));
        assert_eq!(effective_battery_display(Some(85), true, true), (Some(85), true));
        assert_eq!(format_center_text(None), "--");
    }

    #[test]
    fn disconnected_device_never_shows_in_use_worn_or_charging() {
        let s = format_status_string(Some(85), false, EarStatus::InEar, false, false);
        assert_eq!(s, rust_i18n::t!("battery.disconnected_status"));

        let s_charging = format_status_string(Some(85), true, EarStatus::InCase, false, false);
        assert_eq!(s_charging, rust_i18n::t!("battery.disconnected_status"));

        let s_case = format_status_string(Some(90), false, EarStatus::InCase, true, false);
        assert_eq!(s_case, rust_i18n::t!("battery.disconnected_status"));

        let s_none = format_status_string(None, false, EarStatus::Disconnected, false, false);
        assert_eq!(s_none, rust_i18n::t!("battery.waiting_connection"));

        let s_case_none = format_status_string(None, false, EarStatus::InCase, true, false);
        assert_eq!(s_case_none, rust_i18n::t!("battery.sleeping"));
    }

    #[test]
    fn connected_device_formats_ear_and_case_states_properly() {
        let worn = format_status_string(Some(80), false, EarStatus::InEar, false, true);
        assert_eq!(
            worn,
            format!(
                "{} • {}",
                rust_i18n::t!("battery.in_use"),
                rust_i18n::t!("battery.worn")
            )
        );

        let unworn = format_status_string(Some(80), false, EarStatus::OutOfEar, false, true);
        assert_eq!(
            unworn,
            format!(
                "{} • {}",
                rust_i18n::t!("battery.in_use"),
                rust_i18n::t!("battery.unworn")
            )
        );

        let in_case_full = format_status_string(Some(100), false, EarStatus::InCase, false, true);
        assert_eq!(in_case_full, rust_i18n::t!("battery.in_case_status"));

        let in_case_charging =
            format_status_string(Some(60), true, EarStatus::InCase, false, true);
        assert_eq!(
            in_case_charging,
            format!(
                "{} • {}",
                rust_i18n::t!("battery.charging"),
                rust_i18n::t!("battery.in_case_status")
            )
        );

        let single_disconnected =
            format_status_string(Some(80), false, EarStatus::Disconnected, false, true);
        assert_eq!(
            single_disconnected,
            rust_i18n::t!("battery.disconnected_status")
        );
    }
}

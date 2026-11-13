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
            p,
        ))
        .child(render_single_battery_column(
            &rust_i18n::t!("battery.right"),
            state.battery_right,
            state.battery_right_charging,
            state.right_in_ear,
            false,
            p,
        ))
        .child(render_single_battery_column(
            &rust_i18n::t!("battery.case"),
            state.battery_case,
            state.battery_case_charging,
            EarStatus::InCase,
            true,
            p,
        ))
}

fn render_single_battery_column(
    title: &str,
    level_opt: Option<u8>,
    charging: bool,
    ear_status: EarStatus,
    is_case: bool,
    p: &ApplePalette,
) -> impl IntoElement {
    let pct_str = format_center_text(level_opt);
    let status_str = format_status_string(level_opt, charging, ear_status, is_case);
    let ring_color = ring_color_for(level_opt, charging, p);
    let track_color = p.track_bg;
    let status_color = if charging {
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
                                level_opt,
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
                        .when(charging, |this| {
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
) -> String {
    if level.is_none() {
        return if is_case {
            rust_i18n::t!("battery.sleeping").to_string()
        } else {
            rust_i18n::t!("battery.waiting_connection").to_string()
        };
    }
    let charge_str = if charging {
        rust_i18n::t!("battery.charging")
    } else {
        rust_i18n::t!("battery.in_use")
    };
    if is_case {
        charge_str.to_string()
    } else {
        let ear_str = match ear_status {
            EarStatus::InEar => rust_i18n::t!("battery.worn"),
            EarStatus::OutOfEar => rust_i18n::t!("battery.unworn"),
            EarStatus::InCase => rust_i18n::t!("battery.in_case_status"),
            EarStatus::Disconnected => rust_i18n::t!("battery.disconnected_status"),
        };
        format!("{} • {}", charge_str, ear_str)
    }
}

use gpui_kit::prelude::FluentBuilder as _;

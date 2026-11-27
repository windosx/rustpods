use crate::core::head_tracking::HeadGesture;
use crate::ui::state::HeadTrackingViewState;
use crate::ui::theme::{control_card, ApplePalette};
use gpui_kit::component::Icon;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    canvas, div, hsla, point, px, App, Bounds, FontWeight, Hsla, IntoElement, ParentElement as _,
    PathBuilder, Pixels, Point, Styled as _, Window,
};
use std::f32::consts::{PI, TAU};

pub fn render_head_tracking_card(ht: &HeadTrackingViewState, p: &ApplePalette) -> impl IntoElement {
    let ht_snapshot = ht.clone();
    let dial_color = p.text_muted.opacity(0.45);
    let inner_dial_color = p.text_muted.opacity(0.24);
    let tick_color = p.text_muted.opacity(0.38);
    let needle_color = p.accent_blue;

    let (pitch_str, yaw_str, roll_str) = match ht.pose {
        Some(pose) if ht.calibrated => (
            format!("{:.1}°", pose.pitch),
            format!("{:.1}°", pose.yaw),
            format!("{:.1}°", pose.roll),
        ),
        _ => ("—".to_string(), "—".to_string(), "—".to_string()),
    };

    let (gesture_text, gesture_color, gesture_bg) = match ht.gesture {
        Some(HeadGesture::NodYes) => (
            rust_i18n::t!("spatial.nod").to_string(),
            p.accent_green,
            p.accent_green.opacity(0.14),
        ),
        Some(HeadGesture::ShakeNo) => (
            rust_i18n::t!("spatial.shake").to_string(),
            p.accent_blue,
            p.accent_blue.opacity(0.14),
        ),
        None => {
            let hint = match ht.rate_hz {
                Some(rate) => format!("{} · {:.0} Hz", rust_i18n::t!("spatial.gesture_hint"), rate),
                None => rust_i18n::t!("spatial.gesture_hint").to_string(),
            };
            (hint, p.text_secondary, p.pill_bg)
        }
    };

    control_card(p)
        .flex()
        .flex_col()
        .gap(px(12.))
        .child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .justify_between()
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap(px(8.))
                        .child(
                            Icon::new(gpui_kit::assets::IconName::Compass)
                                .size(px(18.))
                                .text_color(p.accent_blue),
                        )
                        .child(
                            div()
                                .text_size(px(15.))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(p.text_primary)
                                .child(rust_i18n::t!("spatial.title")),
                        ),
                )
                .when_some(ht.rate_hz.filter(|_| ht.calibrated), |this, hz| {
                    this.child(
                        div()
                            .px(px(10.))
                            .py(px(3.))
                            .rounded(px(999.))
                            .bg(p.accent_blue.opacity(0.16))
                            .text_size(px(11.5))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(p.accent_blue)
                            .child(format!("{:.0} Hz", hz)),
                    )
                }),
        )
        .child(
            div()
                .text_size(px(12.))
                .text_color(p.text_muted)
                .child(rust_i18n::t!("spatial.desc")),
        )
        .child(
            div()
                .text_size(px(12.5))
                .text_color(p.text_secondary)
                .child(ht.status.clone()),
        )
        .child(
            div()
                .w_full()
                .flex()
                .justify_center()
                .py(px(4.))
                .child(
                    div()
                        .relative()
                        .w(px(300.))
                        .h(px(300.))
                        .child(
                            canvas(
                                move |bounds: Bounds<Pixels>, _w: &mut Window, _cx: &mut App| {
                                    bounds
                                },
                                move |_bounds,
                                      bounds: Bounds<Pixels>,
                                      window: &mut Window,
                                      _cx: &mut App| {
                                    paint_compass(
                                        window,
                                        bounds,
                                        &ht_snapshot,
                                        dial_color,
                                        inner_dial_color,
                                        tick_color,
                                        needle_color,
                                    );
                                },
                            )
                            .size_full(),
                        )
                        // Cardinal labels around the dial
                        .child(cardinal_label(&rust_i18n::t!("spatial.front"), p).top(px(2.)).left(px(142.)))
                        .child(cardinal_label(&rust_i18n::t!("spatial.back"), p).bottom(px(2.)).left(px(142.)))
                        .child(cardinal_label(&rust_i18n::t!("spatial.left"), p).left(px(4.)).top(px(141.)))
                        .child(cardinal_label(&rust_i18n::t!("spatial.right"), p).right(px(4.)).top(px(141.))),
                ),
        )
        .child(
            div()
                .w_full()
                .flex()
                .flex_row()
                .justify_center()
                .gap(px(16.))
                .child(readout_chip(&rust_i18n::t!("spatial.pitch"), &pitch_str, p))
                .child(readout_chip(&rust_i18n::t!("spatial.yaw"), &yaw_str, p))
                .child(readout_chip(&rust_i18n::t!("spatial.roll"), &roll_str, p)),
        )
        .child(
            div()
                .w_full()
                .flex()
                .justify_center()
                .pt(px(2.))
                .child(
                    div()
                        .px(px(14.))
                        .py(px(6.))
                        .rounded(px(999.))
                        .bg(gesture_bg)
                        .text_size(px(12.5))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(gesture_color)
                        .child(gesture_text),
                ),
        )
}

fn cardinal_label(text: &str, p: &ApplePalette) -> gpui_kit::Div {
    div()
        .absolute()
        .text_size(px(12.))
        .font_weight(FontWeight::MEDIUM)
        .text_color(p.text_secondary)
        .child(text.to_string())
}

fn readout_chip(label: &str, value: &str, p: &ApplePalette) -> impl IntoElement {
    div()
        .flex_1()
        .max_w(px(140.))
        .rounded(px(12.))
        .bg(p.card_elevated_bg)
        .border_1()
        .border_color(p.card_border)
        .py(px(10.))
        .px(px(12.))
        .flex()
        .flex_col()
        .items_center()
        .gap(px(3.))
        .child(
            div()
                .text_size(px(11.))
                .text_color(p.text_secondary)
                .child(label.to_string()),
        )
        .child(
            div()
                .text_size(px(16.))
                .font_weight(FontWeight::BOLD)
                .text_color(p.text_primary)
                .child(value.to_string()),
        )
}

fn paint_compass(
    window: &mut Window,
    bounds: Bounds<Pixels>,
    state: &HeadTrackingViewState,
    dial_color: Hsla,
    inner_dial_color: Hsla,
    tick_color: Hsla,
    needle_color: Hsla,
) {
    let w = bounds.size.width.as_f32();
    let h = bounds.size.height.as_f32();
    let cx = bounds.origin.x.as_f32() + w * 0.5;
    let cy = bounds.origin.y.as_f32() + h * 0.5;
    let radius = (w.min(h) * 0.5 - 28.0).max(20.0);

    // Outer dial
    paint_circle_stroke(window, cx, cy, radius, 2.0, dial_color);
    // Inner dial
    paint_circle_stroke(window, cx, cy, radius * 0.62, 1.0, inner_dial_color);

    // 12 radial ticks (every 30 degrees)
    for i in 0..12 {
        let angle = i as f32 * PI / 6.0;
        let inner = radius * 0.88;
        let outer = radius;
        let mut tick = PathBuilder::stroke(px(1.5));
        tick.move_to(point(px(cx + inner * angle.cos()), px(cy + inner * angle.sin())));
        tick.line_to(point(px(cx + outer * angle.cos()), px(cy + outer * angle.sin())));
        if let Ok(path) = tick.build() {
            window.paint_path(path, tick_color);
        }
    }

    let Some(pose) = state.pose else {
        // Idle crosshair
        let cross_color = hsla(220. / 360., 0.1, 0.58, 0.35);
        let mut cross = PathBuilder::stroke(px(1.5));
        cross.move_to(point(px(cx - 12.0), px(cy)));
        cross.line_to(point(px(cx + 12.0), px(cy)));
        cross.move_to(point(px(cx), px(cy - 12.0)));
        cross.line_to(point(px(cx), px(cy + 12.0)));
        if let Ok(path) = cross.build() {
            window.paint_path(path, cross_color);
        }
        return;
    };

    // Drawn at 1:1 with the reported angles (matching headtracking.rs behavior).
    let yaw_rad = pose.yaw.to_radians();
    let pitch_norm = (pose.pitch / 90.0).clamp(-1.0, 1.0);
    let pitch_factor = 1.0 - pitch_norm.abs().powf(0.6) * 0.45;
    let roll_rad = pose.roll.to_radians().clamp(-1.4, 1.4);

    // Rotate relative vector by roll * 0.5 around center (cx, cy).
    let rot = roll_rad * 0.5;
    let rotate_pt = |dx: f32, dy: f32| -> Point<Pixels> {
        let rx = dx * rot.cos() - dy * rot.sin();
        let ry = dx * rot.sin() + dy * rot.cos();
        point(px(cx + rx), px(cy + ry))
    };

    let tip_len = radius * 0.78 * pitch_factor;
    let tip_dx = tip_len * yaw_rad.sin();
    let tip_dy = -tip_len * yaw_rad.cos();

    let mut shaft = PathBuilder::stroke(px(3.2));
    shaft.move_to(point(px(cx), px(cy)));
    shaft.line_to(rotate_pt(tip_dx, tip_dy));
    if let Ok(path) = shaft.build() {
        window.paint_path(path, needle_color);
    }

    // Arrowhead
    let head = 12.0_f32;
    let back = yaw_rad + PI;
    let left_a = back - 0.45;
    let right_a = back + 0.45;

    let mut arrow = PathBuilder::fill();
    arrow.move_to(rotate_pt(tip_dx, tip_dy));
    arrow.line_to(rotate_pt(
        tip_dx + head * left_a.sin(),
        tip_dy - head * left_a.cos(),
    ));
    arrow.line_to(rotate_pt(
        tip_dx + head * right_a.sin(),
        tip_dy - head * right_a.cos(),
    ));
    arrow.close();
    if let Ok(path) = arrow.build() {
        window.paint_path(path, needle_color);
    }

    // Center dot
    paint_circle_fill(window, cx, cy, 4.2, needle_color);
}

fn paint_circle_stroke(
    window: &mut Window,
    cx: f32,
    cy: f32,
    radius: f32,
    width: f32,
    color: Hsla,
) {
    let mut pb = PathBuilder::stroke(px(width));
    const SEGMENTS: usize = 72;
    for i in 0..=SEGMENTS {
        let a = (i as f32 / SEGMENTS as f32) * TAU;
        let pt = point(px(cx + radius * a.cos()), px(cy + radius * a.sin()));
        if i == 0 {
            pb.move_to(pt);
        } else {
            pb.line_to(pt);
        }
    }
    if let Ok(path) = pb.build() {
        window.paint_path(path, color);
    }
}

fn paint_circle_fill(window: &mut Window, cx: f32, cy: f32, radius: f32, color: Hsla) {
    let mut pb = PathBuilder::fill();
    const SEGMENTS: usize = 20;
    for i in 0..SEGMENTS {
        let a = (i as f32 / SEGMENTS as f32) * TAU;
        let pt = point(px(cx + radius * a.cos()), px(cy + radius * a.sin()));
        if i == 0 {
            pb.move_to(pt);
        } else {
            pb.line_to(pt);
        }
    }
    pb.close();
    if let Ok(path) = pb.build() {
        window.paint_path(path, color);
    }
}

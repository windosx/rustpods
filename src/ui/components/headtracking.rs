use crate::core::head_tracking::{HeadGesture, HeadPose};
use gtk4::prelude::*;
use gtk4::{Align, Box, DrawingArea, Label, Orientation};
use std::cell::RefCell;
use std::rc::Rc;

/// Widgets that the UI update loop needs to refresh at stream rate.
pub struct HeadTrackingWidgets {
    pub container: Box,
    pub area: DrawingArea,
    pub pitch_value: Label,
    pub yaw_value: Label,
    pub roll_value: Label,
    pub status_label: Label,
    pub gesture_label: Label,
    pub state: Rc<RefCell<HeadTrackingViewState>>,
}

#[derive(Debug, Clone)]
pub struct HeadTrackingViewState {
    pub pose: Option<HeadPose>,
    pub status: String,
    pub gesture: Option<HeadGesture>,
    pub rate_hz: Option<f32>,
    pub calibrated: bool,
}

impl Default for HeadTrackingViewState {
    fn default() -> Self {
        Self {
            pose: None,
            status: "等待数据流…".to_string(),
            gesture: None,
            rate_hz: None,
            calibrated: false,
        }
    }
}

/// Builds the "空间音频与头部追踪" page.
///
/// The compass is drawn with Cairo rather than a stack of widgets: at ~50
/// samples per second the pose changes too fast for layout, and the arrow needs
/// continuous angles rather than snapped ones.
pub fn build_head_tracking_page() -> HeadTrackingWidgets {
    let container = Box::new(Orientation::Vertical, 14);
    container.set_margin_top(16);
    container.set_margin_bottom(24);
    container.set_margin_start(20);
    container.set_margin_end(20);

    let card = Box::new(Orientation::Vertical, 10);
    card.add_css_class("card");
    card.set_margin_start(16);
    card.set_margin_end(16);
    card.set_margin_top(8);
    card.set_margin_bottom(8);

    let inner = Box::new(Orientation::Vertical, 10);
    inner.set_margin_top(16);
    inner.set_margin_bottom(16);
    inner.set_margin_start(16);
    inner.set_margin_end(16);

    let header_row = Box::new(Orientation::Horizontal, 8);
    let icon = gtk4::Image::from_icon_name("audio-headphones-symbolic");
    header_row.append(&icon);
    let title = Label::new(Some("空间音频与头部追踪"));
    title.add_css_class("heading");
    title.set_halign(Align::Start);
    header_row.append(&title);
    inner.append(&header_row);

    let status_label = Label::new(Some("等待数据流…"));
    status_label.add_css_class("caption");
    status_label.add_css_class("dim-label");
    status_label.set_halign(Align::Start);
    status_label.set_wrap(true);
    inner.append(&status_label);

    // Live compass
    let area = DrawingArea::new();
    area.set_content_width(300);
    area.set_content_height(300);
    area.set_halign(Align::Center);
    area.set_margin_top(8);
    area.set_margin_bottom(8);

    let state = Rc::new(RefCell::new(HeadTrackingViewState::default()));

    {
        let state = state.clone();
        area.set_draw_func(move |_, cr, width, height| {
            draw_compass(cr, width, height, &state.borrow());
        });
    }

    inner.append(&area);

    // Numeric readout
    let readout = Box::new(Orientation::Horizontal, 24);
    readout.set_halign(Align::Center);
    readout.set_margin_top(4);

    let mk_readout = |label: &str| {
        let column = Box::new(Orientation::Vertical, 2);
        column.set_halign(Align::Center);
        let caption = Label::new(Some(label));
        caption.add_css_class("caption");
        caption.add_css_class("dim-label");
        let value = Label::new(Some("—"));
        value.add_css_class("title-3");
        column.append(&caption);
        column.append(&value);
        (column, value)
    };

    let (pitch_col, pitch_value) = mk_readout("俯仰 pitch");
    let (yaw_col, yaw_value) = mk_readout("偏航 yaw");
    let (roll_col, roll_value) = mk_readout("横滚 roll");
    readout.append(&pitch_col);
    readout.append(&yaw_col);
    readout.append(&roll_col);
    inner.append(&readout);

    // Gesture feedback
    let gesture_label = Label::new(Some("点头 / 摇头可触发手势"));
    gesture_label.add_css_class("caption");
    gesture_label.add_css_class("dim-label");
    gesture_label.set_halign(Align::Center);
    gesture_label.set_margin_top(6);
    inner.append(&gesture_label);

    card.append(&inner);
    container.append(&card);

    HeadTrackingWidgets {
        container,
        area,
        pitch_value,
        yaw_value,
        roll_value,
        status_label,
        gesture_label,
        state,
    }
}

fn draw_compass(cr: &gtk4::cairo::Context, width: i32, height: i32, state: &HeadTrackingViewState) {
    let w = width as f64;
    let h = height as f64;
    let cx = w / 2.0;
    let cy = h / 2.0;
    let radius = w.min(h) / 2.0 - 28.0;

    // Dial
    cr.set_line_width(2.0);
    cr.set_source_rgba(0.55, 0.55, 0.60, 0.45);
    cr.arc(cx, cy, radius, 0.0, std::f64::consts::TAU);
    cr.stroke().ok();

    // Inner ring
    cr.set_line_width(1.0);
    cr.set_source_rgba(0.55, 0.55, 0.60, 0.22);
    cr.arc(cx, cy, radius * 0.62, 0.0, std::f64::consts::TAU);
    cr.stroke().ok();

    // Tick marks every 30 degrees, labelled on the primary axes
    for i in 0..12 {
        let angle = i as f64 * std::f64::consts::PI / 6.0;
        let inner = radius * 0.88;
        let outer = radius;
        cr.set_source_rgba(0.55, 0.55, 0.60, 0.35);
        cr.move_to(cx + inner * angle.cos(), cy + inner * angle.sin());
        cr.line_to(cx + outer * angle.cos(), cy + outer * angle.sin());
        cr.stroke().ok();
    }

    // Axis labels
    cr.set_source_rgba(0.6, 0.6, 0.65, 0.75);
    cr.set_font_size(13.0);
    let labels = [
        ("前", 0.0, -1.0),
        ("后", 0.0, 1.0),
        ("左", -1.0, 0.0),
        ("右", 1.0, 0.0),
    ];
    for (text, dx, dy) in labels {
        let extent = cr.text_extents(text).map(|e| (e.width(), e.height())).unwrap_or((0.0, 0.0));
        cr.move_to(
            cx + dx * (radius + 14.0) - extent.0 / 2.0,
            cy + dy * (radius + 14.0) + extent.1 / 2.0,
        );
        cr.show_text(text).ok();
    }

    let Some(pose) = state.pose else {
        // Idle crosshair
        cr.set_source_rgba(0.55, 0.55, 0.60, 0.30);
        cr.set_line_width(1.5);
        cr.move_to(cx - 12.0, cy);
        cr.line_to(cx + 12.0, cy);
        cr.move_to(cx, cy - 12.0);
        cr.line_to(cx, cy + 12.0);
        cr.stroke().ok();
        return;
    };

    // The gyro scale in the protocol document is marked approximate ("~0.122
    // deg/s per LSB"), and the integrated angles come out on the small side, so
    // the dial is drawn with a visible gain. Motion amplification is a rendering
    // choice; it does not change what the sensor reported.
    const DISPLAY_GAIN: f64 = 2.2;

    // Yaw: clockwise from "front", matching how a wearer turns their head.
    let yaw_rad = (pose.yaw as f64 * DISPLAY_GAIN).to_radians().clamp(-3.0, 3.0);
    // Pitch shortens the arrow: looking up pulls the tip toward the centre.
    // The cube-root-ish easing keeps small movements visible instead of
    // flattening everything below 30 degrees.
    let pitch_norm = (pose.pitch as f64 * DISPLAY_GAIN / 90.0).clamp(-1.0, 1.0);
    let pitch_factor = 1.0 - pitch_norm.abs().powf(0.6) * 0.45;
    // Roll tips the whole dial, which is what leaning toward a shoulder does.
    // Without this the compass ignored roll entirely, so that motion looked like
    // nothing was happening.
    let roll_rad = (pose.roll as f64 * DISPLAY_GAIN)
        .to_radians()
        .clamp(-1.4, 1.4);

    cr.save().ok();
    cr.translate(cx, cy);
    cr.rotate(roll_rad * 0.5);
    cr.translate(-cx, -cy);

    cr.set_source_rgba(0.36, 0.66, 1.0, 0.95);
    cr.set_line_width(3.0);
    cr.set_line_cap(gtk4::cairo::LineCap::Round);

    let tip_len = radius * 0.78 * pitch_factor;
    let tip_x = cx + tip_len * yaw_rad.sin();
    let tip_y = cy - tip_len * yaw_rad.cos();

    cr.move_to(cx, cy);
    cr.line_to(tip_x, tip_y);
    cr.stroke().ok();

    // Arrowhead
    let head = 12.0;
    let back = yaw_rad + std::f64::consts::PI;
    let left = back - 0.45;
    let right = back + 0.45;
    cr.set_source_rgba(0.36, 0.66, 1.0, 0.95);
    cr.move_to(tip_x, tip_y);
    cr.line_to(tip_x + head * left.sin(), tip_y - head * left.cos());
    cr.line_to(tip_x + head * right.sin(), tip_y - head * right.cos());
    cr.close_path();
    cr.fill().ok();

    cr.restore().ok();

    // Centre dot
    cr.set_source_rgba(0.36, 0.66, 1.0, 0.9);
    cr.arc(cx, cy, 4.0, 0.0, std::f64::consts::TAU);
    cr.fill().ok();
}

/// Refreshes labels and redraws the compass from the latest pose.
pub fn update_head_tracking_widgets(widgets: &HeadTrackingWidgets) {
    let (pose, status, gesture, rate, calibrated) = {
        let state = widgets.state.borrow();
        (
            state.pose,
            state.status.clone(),
            state.gesture,
            state.rate_hz,
            state.calibrated,
        )
    };

    widgets.status_label.set_text(&status);

    match pose {
        Some(pose) if calibrated => {
            widgets
                .pitch_value
                .set_text(&format!("{:.1}°", pose.pitch));
            widgets.yaw_value.set_text(&format!("{:.1}°", pose.yaw));
            widgets
                .roll_value
                .set_text(&format!("{:.1}°", pose.roll));
        }
        _ => {
            widgets.pitch_value.set_text("—");
            widgets.yaw_value.set_text("—");
            widgets.roll_value.set_text("—");
        }
    }

    match gesture {
        Some(HeadGesture::NodYes) => {
            widgets.gesture_label.set_text("✓ 检测到点头（是）");
            widgets.gesture_label.remove_css_class("dim-label");
            widgets.gesture_label.add_css_class("success");
        }
        Some(HeadGesture::ShakeNo) => {
            widgets.gesture_label.set_text("✗ 检测到摇头（否）");
            widgets.gesture_label.remove_css_class("dim-label");
            widgets.gesture_label.add_css_class("accent");
        }
        None => {
            let hint = match rate {
                Some(rate) => format!("点头 / 摇头可触发手势 · {:.0} Hz", rate),
                None => "点头 / 摇头可触发手势".to_string(),
            };
            widgets.gesture_label.set_text(&hint);
            widgets.gesture_label.remove_css_class("success");
            widgets.gesture_label.remove_css_class("accent");
            widgets.gesture_label.add_css_class("dim-label");
        }
    }

    widgets.area.queue_draw();
}

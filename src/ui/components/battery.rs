use crate::core::aacp::events::EarStatus;
use crate::core::device::AirPodsDeviceState;
use gtk4::prelude::*;
use gtk4::{Align, Box, DrawingArea, Label, Orientation, Overlay};
use std::cell::RefCell;
use std::f64::consts::PI;
use std::rc::Rc;

pub struct SingleBatteryWidgets {
    pub container: Box,
    pub area: DrawingArea,
    pub center_label: Label,
    pub status_label: Label,
    pub state_holder: Rc<RefCell<(Option<u8>, bool)>>,
}

pub struct BatteryViewWidgets {
    pub container: Box,
    pub left: SingleBatteryWidgets,
    pub right: SingleBatteryWidgets,
    pub case: SingleBatteryWidgets,
}

pub fn build_battery_card(state: &AirPodsDeviceState) -> BatteryViewWidgets {
    let main_box = Box::new(Orientation::Horizontal, 12);
    main_box.set_homogeneous(true);
    main_box.set_margin_start(16);
    main_box.set_margin_end(16);
    main_box.set_margin_top(8);
    main_box.set_margin_bottom(8);

    // Left Earbud Card
    let left = build_single_circular_battery_column(
        "左耳 (Left)",
        state.battery_left,
        state.battery_left_charging,
        state.left_in_ear,
        false,
    );

    // Right Earbud Card
    let right = build_single_circular_battery_column(
        "右耳 (Right)",
        state.battery_right,
        state.battery_right_charging,
        state.right_in_ear,
        false,
    );

    // Case Card
    let case = build_single_circular_battery_column(
        "充电盒 (Case)",
        state.battery_case,
        state.battery_case_charging,
        EarStatus::InCase,
        true,
    );

    main_box.append(&left.container);
    main_box.append(&right.container);
    main_box.append(&case.container);

    BatteryViewWidgets {
        container: main_box,
        left,
        right,
        case,
    }
}

fn build_single_circular_battery_column(
    title: &str,
    level_opt: Option<u8>,
    charging: bool,
    ear_status: EarStatus,
    is_case: bool,
) -> SingleBatteryWidgets {
    let card = Box::new(Orientation::Vertical, 8);
    card.add_css_class("card");
    card.set_margin_top(6);
    card.set_margin_bottom(6);
    card.set_margin_start(4);
    card.set_margin_end(4);

    let inner = Box::new(Orientation::Vertical, 8);
    inner.set_margin_top(14);
    inner.set_margin_bottom(14);
    inner.set_margin_start(14);
    inner.set_margin_end(14);

    // Title on top
    let title_label = Label::new(Some(title));
    title_label.add_css_class("heading");
    title_label.set_halign(Align::Center);
    inner.append(&title_label);

    // Circular Battery Ring Gauge (Apple Widget style)
    let state_holder = Rc::new(RefCell::new((level_opt, charging)));

    let overlay = Overlay::new();
    overlay.set_halign(Align::Center);
    overlay.set_valign(Align::Center);

    let drawing_area = DrawingArea::new();
    drawing_area.set_content_width(84);
    drawing_area.set_content_height(84);
    drawing_area.set_halign(Align::Center);
    drawing_area.set_valign(Align::Center);

    let state_draw = state_holder.clone();
    drawing_area.set_draw_func(move |_area, cr, width, height| {
        let cx = (width as f64) / 2.0;
        let cy = (height as f64) / 2.0;
        let r = 34.0;
        let stroke_w = 6.5;

        let (level, is_charging) = *state_draw.borrow();
        let pct = level.unwrap_or(0) as f64 / 100.0;

        // Background track ring
        cr.arc(cx, cy, r, 0.0, 2.0 * PI);
        cr.set_source_rgba(0.5, 0.5, 0.5, 0.16);
        cr.set_line_width(stroke_w);
        let _ = cr.stroke();

        // Foreground animated progress arc
        if level.is_some() && pct > 0.0 {
            let color = if is_charging {
                [0.20, 0.78, 0.35, 1.0] // Electric Green
            } else if pct > 0.40 {
                [0.19, 0.82, 0.35, 1.0] // Vibrant Green
            } else if pct > 0.20 {
                [1.00, 0.84, 0.04, 1.0] // Warning Yellow
            } else {
                [1.00, 0.27, 0.23, 1.0] // Low Battery Red
            };

            let start_angle = -PI / 2.0;
            let end_angle = start_angle + pct * 2.0 * PI;

            cr.set_line_cap(gtk4::cairo::LineCap::Round);
            cr.set_source_rgba(color[0], color[1], color[2], color[3]);
            cr.set_line_width(stroke_w);
            cr.arc(cx, cy, r, start_angle, end_angle);
            let _ = cr.stroke();
        }
    });

    let pct_str = format_center_text(level_opt, charging);
    let center_label = Label::new(Some(&pct_str));
    center_label.add_css_class("title-2");
    center_label.add_css_class("numeric");
    center_label.set_halign(Align::Center);
    center_label.set_valign(Align::Center);

    overlay.set_child(Some(&drawing_area));
    overlay.add_overlay(&center_label);
    inner.append(&overlay);

    // Status subtitle below
    let status_str = format_status_string(level_opt, charging, ear_status, is_case);
    let status_label = Label::new(Some(&status_str));
    status_label.add_css_class("caption");
    status_label.set_halign(Align::Center);
    if charging {
        status_label.add_css_class("success");
    } else {
        status_label.add_css_class("dim-label");
    }
    inner.append(&status_label);

    card.append(&inner);
    SingleBatteryWidgets {
        container: card,
        area: drawing_area,
        center_label,
        status_label,
        state_holder,
    }
}

pub fn update_battery_widgets(widgets: &BatteryViewWidgets, state: &AirPodsDeviceState) {
    // Left
    *widgets.left.state_holder.borrow_mut() =
        (state.battery_left, state.battery_left_charging);
    widgets.left.area.queue_draw();
    widgets
        .left
        .center_label
        .set_text(&format_center_text(state.battery_left, state.battery_left_charging));
    widgets.left.status_label.set_text(&format_status_string(
        state.battery_left,
        state.battery_left_charging,
        state.left_in_ear,
        false,
    ));

    // Right
    *widgets.right.state_holder.borrow_mut() =
        (state.battery_right, state.battery_right_charging);
    widgets.right.area.queue_draw();
    widgets
        .right
        .center_label
        .set_text(&format_center_text(state.battery_right, state.battery_right_charging));
    widgets.right.status_label.set_text(&format_status_string(
        state.battery_right,
        state.battery_right_charging,
        state.right_in_ear,
        false,
    ));

    // Case
    *widgets.case.state_holder.borrow_mut() =
        (state.battery_case, state.battery_case_charging);
    widgets.case.area.queue_draw();
    widgets
        .case
        .center_label
        .set_text(&format_center_text(state.battery_case, state.battery_case_charging));
    widgets.case.status_label.set_text(&format_status_string(
        state.battery_case,
        state.battery_case_charging,
        EarStatus::InCase,
        true,
    ));
}

fn format_center_text(level: Option<u8>, _charging: bool) -> String {
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
            "盒盖休眠中".to_string()
        } else {
            "等待连接".to_string()
        };
    }
    let charge_str = if charging { "充电中" } else { "使用中" };
    if is_case {
        charge_str.to_string()
    } else {
        let ear_str = match ear_status {
            EarStatus::InEar => "已佩戴",
            EarStatus::OutOfEar => "未佩戴",
            EarStatus::InCase => "充电盒中",
            EarStatus::Disconnected => "未连接",
        };
        format!("{} • {}", charge_str, ear_str)
    }
}

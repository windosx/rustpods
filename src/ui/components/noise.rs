use crate::core::aacp::commands::NoiseControlMode;
use crate::core::device::AirPodsDeviceState;
use gtk4::prelude::*;
use gtk4::{Align, Box, Label, Orientation, Scale, ToggleButton};
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::Arc;

pub fn build_noise_control_card<F, S>(
    state: &AirPodsDeviceState,
    on_set_mode: F,
    on_set_adaptive: S,
) -> Box
where
    F: Fn(NoiseControlMode) + Send + Sync + 'static,
    S: Fn(u8) + Send + Sync + 'static,
{
    let container = Box::new(Orientation::Vertical, 8);
    container.add_css_class("card");
    container.set_margin_start(16);
    container.set_margin_end(16);
    container.set_margin_top(8);
    container.set_margin_bottom(8);

    let inner = Box::new(Orientation::Vertical, 10);
    inner.set_margin_top(16);
    inner.set_margin_bottom(16);
    inner.set_margin_start(16);
    inner.set_margin_end(16);

    if state.capabilities.has_anc {
        let title_label = Label::new(Some("噪声控制"));
        title_label.add_css_class("heading");
        title_label.set_halign(Align::Start);
        inner.append(&title_label);

        // Segmented Control
        let seg_box = Box::new(Orientation::Horizontal, 0);
        seg_box.add_css_class("linked");
        seg_box.set_halign(Align::Center);
        seg_box.set_margin_top(6);
        seg_box.set_margin_bottom(6);

        let btn_anc = ToggleButton::with_label("降噪");
        let btn_off = ToggleButton::with_label("关闭");
        let btn_trans = ToggleButton::with_label("通透模式");

        // Radio-group semantics: without a shared group the three buttons
        // toggle independently - two could highlight at once, and clicking
        // the active one deactivated it, leaving no selection and (because
        // the handlers fire on is_active) no command sent.
        btn_off.set_group(Some(&btn_anc));
        btn_trans.set_group(Some(&btn_anc));

        let current_mode = state.noise_mode;
        btn_anc.set_active(current_mode == NoiseControlMode::NoiseCancellation);
        btn_off.set_active(current_mode == NoiseControlMode::Off);
        btn_trans.set_active(current_mode == NoiseControlMode::Transparency);

        seg_box.append(&btn_anc);
        seg_box.append(&btn_off);
        seg_box.append(&btn_trans);

        let on_mode_arc = Arc::new(on_set_mode);

        let m_anc = on_mode_arc.clone();
        btn_anc.connect_toggled(move |btn| {
            if btn.is_active() {
                m_anc(NoiseControlMode::NoiseCancellation);
            }
        });

        let m_off = on_mode_arc.clone();
        btn_off.connect_toggled(move |btn| {
            if btn.is_active() {
                m_off(NoiseControlMode::Off);
            }
        });

        let m_trans = on_mode_arc.clone();
        btn_trans.connect_toggled(move |btn| {
            if btn.is_active() {
                m_trans(NoiseControlMode::Transparency);
            }
        });

        inner.append(&seg_box);

        if state.capabilities.has_adaptive_audio {
            let slider_box = Box::new(Orientation::Vertical, 4);
            let slider_label = Label::new(Some("自适应噪声消除强度"));
            slider_label.add_css_class("caption");
            slider_label.set_halign(Align::Start);
            slider_box.append(&slider_label);

            let scale = Scale::with_range(Orientation::Horizontal, 0.0, 100.0, 5.0);
            scale.set_value(state.adaptive_strength as f64);
            scale.set_draw_value(true);

            // Dragging fires value_changed continuously - one drag emitted
            // dozens of AACP commands, with off-step values from the truncation.
            // Coalesce to a single send 150 ms after the last movement, snapped
            // to a 5-point step.
            let on_adapt_arc = Arc::new(on_set_adaptive);
            let pending: Rc<RefCell<Option<u8>>> = Rc::new(RefCell::new(None));
            let flush_scheduled: Rc<Cell<bool>> = Rc::new(Cell::new(false));
            scale.connect_value_changed({
                let pending = pending.clone();
                let flush_scheduled = flush_scheduled.clone();
                let on_adapt = on_adapt_arc.clone();
                move |s| {
                    *pending.borrow_mut() = Some(((s.value() / 5.0).round() as u8).min(100) * 5);
                    if flush_scheduled.replace(true) {
                        return; // a flush is already queued; it reads the latest value
                    }
                    let pending = pending.clone();
                    let flush_scheduled = flush_scheduled.clone();
                    let on_adapt = on_adapt.clone();
                    gtk4::glib::timeout_add_local_once(
                        std::time::Duration::from_millis(150),
                        move || {
                            flush_scheduled.set(false);
                            if let Some(v) = pending.borrow_mut().take() {
                                on_adapt(v);
                            }
                        },
                    );
                }
            });

            slider_box.append(&scale);
            inner.append(&slider_box);
        }
    } else {
        // Half-in-ear / Non-ANC info banner (e.g. AirPods 3)
        let header_row = Box::new(Orientation::Horizontal, 8);
        let icon = gtk4::Image::from_icon_name("audio-volume-high-symbolic");
        header_row.append(&icon);

        let title_label = Label::new(Some("声学特性与能力"));
        title_label.add_css_class("heading");
        header_row.append(&title_label);
        inner.append(&header_row);

        let desc_label = Label::new(Some(
            "当前设备为半入耳式经典声学架构，支持高品质计算音频与自适应均衡调节。",
        ));
        desc_label.add_css_class("caption");
        desc_label.add_css_class("dim-label");
        desc_label.set_wrap(true);
        desc_label.set_halign(Align::Start);
        inner.append(&desc_label);

        let tags_box = Box::new(Orientation::Horizontal, 8);
        tags_box.set_margin_top(6);

        let tag1 = Label::new(Some("自适应均衡"));
        tag1.add_css_class("card");
        tag1.add_css_class("caption");
        tags_box.append(&tag1);

        let tag2 = Label::new(Some("入耳检测"));
        tag2.add_css_class("card");
        tag2.add_css_class("caption");
        tags_box.append(&tag2);

        let tag3 = Label::new(Some("高清立体声"));
        tag3.add_css_class("card");
        tag3.add_css_class("caption");
        tags_box.append(&tag3);

        inner.append(&tags_box);
    }

    container.append(&inner);
    container
}

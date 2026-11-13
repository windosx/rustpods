use crate::core::aacp::commands::NoiseControlMode;
use crate::ui::state::RustPodsState;
use crate::ui::theme::{control_card, ApplePalette};
use gpui_kit::component::slider::{Slider, SliderState};
use gpui_kit::component::tag::Tag;
use gpui_kit::component::Icon;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    div, px, App, Entity, FontWeight, InteractiveElement as _, IntoElement, ParentElement as _,
    StatefulInteractiveElement as _, Styled as _,
};

pub fn render_noise_control_card(
    state: &RustPodsState,
    app_state: &Entity<RustPodsState>,
    adaptive_slider: &Entity<SliderState>,
    p: &ApplePalette,
    _cx: &App,
) -> impl IntoElement {
    let dev = &state.device;

    if dev.capabilities.has_anc {
        let current_mode = dev.noise_mode;
        let adaptive_strength = dev.adaptive_strength;

        let modes = [
            (NoiseControlMode::NoiseCancellation, "降噪", "anc-btn"),
            (NoiseControlMode::Off, "关闭", "off-btn"),
            (NoiseControlMode::Transparency, "通透模式", "trans-btn"),
        ];

        control_card(p)
            .flex()
            .flex_col()
            .gap(px(14.))
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .text_size(px(15.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(p.text_primary)
                            .child("噪声控制"),
                    )
                    .child(
                        div()
                            .text_size(px(12.))
                            .text_color(p.text_secondary)
                            .child(match current_mode {
                                NoiseControlMode::NoiseCancellation => "降噪已开启",
                                NoiseControlMode::Transparency => "通透模式已开启",
                                NoiseControlMode::Adaptive => "自适应",
                                NoiseControlMode::Off => "已关闭",
                            }),
                    ),
            )
            .child(
                div()
                    .w_full()
                    .flex()
                    .justify_center()
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .p(px(4.))
                            .rounded(px(12.))
                            .bg(p.pill_bg)
                            .border_1()
                            .border_color(p.card_border)
                            .gap(px(4.))
                            .children(modes.into_iter().map(|(mode, label, id)| {
                                let selected = current_mode == mode;
                                let app_click = app_state.clone();
                                div()
                                    .id(id)
                                    .cursor_pointer()
                                    .px(px(18.))
                                    .py(px(7.))
                                    .rounded(px(9.))
                                    .text_size(px(13.))
                                    .font_weight(if selected {
                                        FontWeight::SEMIBOLD
                                    } else {
                                        FontWeight::MEDIUM
                                    })
                                    .bg(if selected {
                                        p.pill_active_bg
                                    } else {
                                        gpui_kit::transparent_black()
                                    })
                                    .text_color(if selected {
                                        p.sidebar_item_active_fg
                                    } else {
                                        p.text_primary
                                    })
                                    .hover(|this| {
                                        if selected {
                                            this
                                        } else {
                                            this.bg(p.sidebar_item_hover)
                                        }
                                    })
                                    .on_click(move |_, _, cx| {
                                        app_click.update(cx, |s, cx| {
                                            s.device.noise_mode = mode;
                                            s.controller.set_noise_mode(mode);
                                            cx.notify();
                                        });
                                    })
                                    .child(label)
                            })),
                    ),
            )
            .when(dev.capabilities.has_adaptive_audio, |this| {
                this.child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(8.))
                        .pt(px(4.))
                        .border_t_1()
                        .border_color(p.separator)
                        .child(
                            div()
                                .flex()
                                .flex_row()
                                .items_center()
                                .justify_between()
                                .child(
                                    div()
                                        .text_size(px(12.5))
                                        .text_color(p.text_secondary)
                                        .child("自适应噪声消除强度"),
                                )
                                .child(
                                    div()
                                        .text_size(px(12.5))
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(p.text_primary)
                                        .child(format!("{}%", adaptive_strength)),
                                ),
                        )
                        .child(Slider::new(adaptive_slider).horizontal()),
                )
            })
            .into_any_element()
    } else {
        control_card(p)
            .flex()
            .flex_col()
            .gap(px(10.))
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(8.))
                    .child(
                        Icon::new(gpui_kit::assets::IconName::Volume2)
                            .size(px(18.))
                            .text_color(p.accent_blue),
                    )
                    .child(
                        div()
                            .text_size(px(15.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(p.text_primary)
                            .child("声学特性与能力"),
                    ),
            )
            .child(
                div()
                    .text_size(px(12.5))
                    .text_color(p.text_secondary)
                    .child("当前设备为半入耳式经典声学架构，支持高品质计算音频与自适应均衡调节。"),
            )
            .child(
                div()
                    .flex()
                    .flex_row()
                    .flex_wrap()
                    .gap(px(8.))
                    .pt(px(2.))
                    .child(Tag::secondary().child("自适应均衡"))
                    .child(Tag::secondary().child("入耳检测"))
                    .child(Tag::secondary().child("高清立体声")),
            )
            .into_any_element()
    }
}

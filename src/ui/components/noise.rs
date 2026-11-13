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
            (NoiseControlMode::NoiseCancellation, rust_i18n::t!("noise.anc"), "anc-btn"),
            (NoiseControlMode::Off, rust_i18n::t!("noise.off"), "off-btn"),
            (NoiseControlMode::Transparency, rust_i18n::t!("noise.transparency"), "trans-btn"),
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
                            .child(rust_i18n::t!("noise.title")),
                    )
                    .child(
                        div()
                            .text_size(px(12.))
                            .text_color(p.text_secondary)
                            .child(match current_mode {
                                NoiseControlMode::NoiseCancellation => rust_i18n::t!("noise.status_anc"),
                                NoiseControlMode::Transparency => rust_i18n::t!("noise.status_transparency"),
                                NoiseControlMode::Adaptive => rust_i18n::t!("noise.status_adaptive"),
                                NoiseControlMode::Off => rust_i18n::t!("noise.status_off"),
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
                                        .child(rust_i18n::t!("noise.adaptive_strength")),
                                )
                                .child(
                                    div()
                                        .text_size(px(12.5))
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(p.text_primary)
                                        .child(format!("{}%", adaptive_strength)),
                                )
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
                            .child(rust_i18n::t!("noise.features_title")),
                    ),
            )
            .child(
                div()
                    .text_size(px(12.5))
                    .text_color(p.text_secondary)
                    .child(rust_i18n::t!("noise.features_desc")),
            )
            .child(
                div()
                    .flex()
                    .flex_row()
                    .flex_wrap()
                    .gap(px(8.))
                    .pt(px(2.))
                    .child(Tag::secondary().child(rust_i18n::t!("noise.feature_adaptive_eq")))
                    .child(Tag::secondary().child(rust_i18n::t!("noise.feature_ear_detect")))
                    .child(Tag::secondary().child(rust_i18n::t!("noise.feature_spatial_audio"))),
            )
            .into_any_element()
    }
}

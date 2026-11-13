use crate::ui::state::RustPodsState;
use crate::ui::theme::{grouped_card, section_header, setting_row, ApplePalette};
use gpui_kit::component::select::{Select, SelectState};
use gpui_kit::component::switch::Switch;
use gpui_kit::component::tag::Tag;
use gpui_kit::component::Icon;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{div, px, Entity, IntoElement, ParentElement as _, Styled as _};

pub fn render_settings_page(
    state: &RustPodsState,
    app_state: &Entity<RustPodsState>,
    pause_select: &Entity<SelectState<Vec<String>>>,
    mic_select: &Entity<SelectState<Vec<String>>>,
    p: &ApplePalette,
) -> impl IntoElement {
    let cfg = state.config_snapshot();
    let dev = &state.device;

    let app_ear = app_state.clone();
    let app_conv = app_state.clone();
    let app_chime = app_state.clone();

    div()
        .w_full()
        .flex()
        .flex_col()
        .gap(px(14.))
        .child(section_header(
            p,
            &rust_i18n::t!("controls.title"),
            Some(&rust_i18n::t!("controls.desc")),
        ))
        .child(
            grouped_card(p)
                .child(setting_row(
                    p,
                    &rust_i18n::t!("controls.ear_detect"),
                    Some(&rust_i18n::t!("controls.ear_detect_desc")),
                    Switch::new("ear-detection-switch")
                        .checked(cfg.ear_detection_enabled)
                        .on_change(move |checked, _, cx| {
                            let val = *checked;
                            app_ear.update(cx, |s, cx| {
                                s.device.ear_detection_enabled = val;
                                s.controller.toggle_ear_detection(val);
                                cx.notify();
                            });
                        }),
                    false,
                ))
                .child(setting_row(
                    p,
                    &rust_i18n::t!("controls.pause_behavior"),
                    Some(&rust_i18n::t!("controls.pause_desc")),
                    Select::new(pause_select).w(px(210.)),
                    true,
                ))
                .child(setting_row(
                    p,
                    &rust_i18n::t!("controls.conv_awareness"),
                    Some(&rust_i18n::t!("controls.conv_desc")),
                    Switch::new("conv-awareness-switch")
                        .checked(cfg.conversational_awareness_enabled)
                        .on_change(move |checked, _, cx| {
                            let val = *checked;
                            app_conv.update(cx, |s, cx| {
                                s.device.conversational_awareness = val;
                                s.controller.toggle_conversational_awareness(val);
                                cx.notify();
                            });
                        }),
                    true,
                ))
                .child(setting_row(
                    p,
                    &rust_i18n::t!("controls.mic"),
                    Some(&rust_i18n::t!("controls.mic_desc")),
                    Select::new(mic_select).w(px(170.)),
                    true,
                ))
                .when(dev.capabilities.has_case_speaker, |this| {
                    this.child(setting_row(
                        p,
                        &rust_i18n::t!("controls.chime"),
                        Some(&rust_i18n::t!("controls.chime_desc")),
                        Switch::new("case-chime-switch")
                            .checked(dev.case_chime_enabled)
                            .on_change(move |checked, _, cx| {
                                let val = *checked;
                                app_chime.update(cx, |s, cx| {
                                    s.device.case_chime_enabled = val;
                                    s.controller.toggle_case_chime(val);
                                    cx.notify();
                                });
                            }),
                        true,
                    ))
                })
                .child(setting_row(
                    p,
                    &rust_i18n::t!("controls.mpris_integration"),
                    Some(&rust_i18n::t!("controls.mpris_desc")),
                    Tag::success().outline().child(
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap(px(4.))
                            .child(Icon::new(gpui_kit::component::IconName::Check).size(px(12.)))
                            .child(rust_i18n::t!("controls.enabled")),
                    ),
                    true,
                )),
        )
}

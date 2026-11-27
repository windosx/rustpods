use crate::ui::components::setting_rows::{
    feedback_banner, page_header, select_row, toggle_row, SettingGroup,
};
use crate::ui::state::RustPodsState;
use crate::ui::theme::{section_header, ApplePalette, SELECT_W_MD};
use gpui_kit::component::select::SelectState;
use gpui_kit::component::tag::Tag;
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

    let header_trailing = if let Some(fb) = &state.settings_feedback {
        Some(feedback_banner(p, &fb.message, fb.is_error).into_any_element())
    } else if !dev.is_connected {
        Some(
            Tag::warning()
                .outline()
                .child(rust_i18n::t!("controls.disconnected_badge"))
                .into_any_element(),
        )
    } else {
        None
    };

    let pause_desc = if cfg.ear_detection_enabled {
        rust_i18n::t!("controls.pause_desc")
    } else {
        rust_i18n::t!("controls.requires_ear_detect")
    };

    let show_conv_awareness = !dev.is_connected || dev.capabilities.has_conversational_awareness;

    div()
        .w_full()
        .flex()
        .flex_col()
        .gap(px(16.))
        .child(page_header(
            p,
            &rust_i18n::t!("controls.title"),
            Some(&if dev.is_connected {
                rust_i18n::t!("controls.desc")
            } else {
                rust_i18n::t!("controls.disconnected_hint")
            }),
            header_trailing,
        ))
        .child(section_header(
            p,
            &rust_i18n::t!("controls.wear_section"),
            Some(&rust_i18n::t!("controls.wear_section_desc")),
        ))
        .child(
            SettingGroup::new(p)
                .row(toggle_row(
                    p,
                    "ear-detection-switch",
                    &rust_i18n::t!("controls.ear_detect"),
                    Some(&rust_i18n::t!("controls.ear_detect_desc")),
                    cfg.ear_detection_enabled,
                    false,
                    move |checked, _, cx| {
                        let val = *checked;
                        app_ear.update(cx, |s, cx| {
                            s.device.ear_detection_enabled = val;
                            s.controller.toggle_ear_detection(val);
                            s.notify_saved();
                            cx.notify();
                        });
                    },
                ))
                .row(select_row(
                    p,
                    &rust_i18n::t!("controls.pause_behavior"),
                    Some(&pause_desc),
                    pause_select,
                    SELECT_W_MD,
                    !cfg.ear_detection_enabled,
                ))
                .build(),
        )
        .child(section_header(
            p,
            &rust_i18n::t!("controls.audio_section"),
            Some(&rust_i18n::t!("controls.audio_section_desc")),
        ))
        .child(
            SettingGroup::new(p)
                .row(select_row(
                    p,
                    &rust_i18n::t!("controls.mic"),
                    Some(&rust_i18n::t!("controls.mic_desc")),
                    mic_select,
                    SELECT_W_MD,
                    false,
                ))
                .row_if(show_conv_awareness, || {
                    toggle_row(
                        p,
                        "conv-awareness-switch",
                        &rust_i18n::t!("controls.conv_awareness"),
                        Some(&rust_i18n::t!("controls.conv_desc")),
                        cfg.conversational_awareness_enabled,
                        false,
                        move |checked, _, cx| {
                            let val = *checked;
                            app_conv.update(cx, |s, cx| {
                                s.device.conversational_awareness = val;
                                s.controller.toggle_conversational_awareness(val);
                                s.notify_saved();
                                cx.notify();
                            });
                        },
                    )
                })
                .row_if(dev.capabilities.has_case_speaker, || {
                    toggle_row(
                        p,
                        "case-chime-switch",
                        &rust_i18n::t!("controls.chime"),
                        Some(&rust_i18n::t!("controls.chime_desc")),
                        dev.case_chime_enabled,
                        false,
                        move |checked, _, cx| {
                            let val = *checked;
                            app_chime.update(cx, |s, cx| {
                                s.device.case_chime_enabled = val;
                                s.controller.toggle_case_chime(val);
                                s.notify_saved();
                                cx.notify();
                            });
                        },
                    )
                })
                .build(),
        )
}

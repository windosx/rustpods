use crate::system::updater::{open_browser, UpdateState};
use crate::ui::state::RustPodsState;
use crate::ui::theme::{apply_apple_theme, grouped_card, section_header, setting_row, ApplePalette};
use gpui_kit::component::button::{Button, ButtonRounded, ButtonVariants as _};
use gpui_kit::component::select::{Select, SelectState};
use gpui_kit::component::switch::Switch;
use gpui_kit::component::tag::Tag;
use gpui_kit::component::{Icon, ThemeMode};
use gpui_kit::{
    div, px, Entity, FontWeight, InteractiveElement as _, IntoElement, ParentElement as _,
    StatefulInteractiveElement as _, Styled as _,
};

pub fn render_preferences_page(
    state: &RustPodsState,
    app_state: &Entity<RustPodsState>,
    language_select: &Entity<SelectState<Vec<String>>>,
    p: &ApplePalette,
) -> impl IntoElement {
    let cfg = state.config_snapshot();
    let dev = &state.device;
    let app_tray = app_state.clone();
    let app_popup = app_state.clone();
    let app_preview = app_state.clone();
    let app_theme_light = app_state.clone();
    let app_theme_dark = app_state.clone();
    let is_dark = state.theme_mode.is_dark();

    let theme_switcher = div()
        .flex()
        .flex_row()
        .items_center()
        .p(px(3.))
        .rounded(px(10.))
        .bg(p.pill_bg)
        .border_1()
        .border_color(p.card_border)
        .gap(px(3.))
        .child(
            div()
                .id("pref-theme-light")
                .cursor_pointer()
                .px(px(12.))
                .py(px(5.))
                .rounded(px(7.))
                .text_size(px(12.))
                .font_weight(if !is_dark {
                    FontWeight::SEMIBOLD
                } else {
                    FontWeight::MEDIUM
                })
                .bg(if !is_dark {
                    p.pill_active_bg
                } else {
                    gpui_kit::transparent_black()
                })
                .text_color(if !is_dark {
                    p.sidebar_item_active_fg
                } else {
                    p.text_primary
                })
                .on_click(move |_, _, cx| {
                    apply_apple_theme(ThemeMode::Light, cx);
                    app_theme_light.update(cx, |s, cx| {
                        s.theme_mode = ThemeMode::Light;
                        cx.notify();
                    });
                })
                .child(rust_i18n::t!("shell.theme_light")),
        )
        .child(
            div()
                .id("pref-theme-dark")
                .cursor_pointer()
                .px(px(12.))
                .py(px(5.))
                .rounded(px(7.))
                .text_size(px(12.))
                .font_weight(if is_dark {
                    FontWeight::SEMIBOLD
                } else {
                    FontWeight::MEDIUM
                })
                .bg(if is_dark {
                    p.pill_active_bg
                } else {
                    gpui_kit::transparent_black()
                })
                .text_color(if is_dark {
                    p.sidebar_item_active_fg
                } else {
                    p.text_primary
                })
                .on_click(move |_, _, cx| {
                    apply_apple_theme(ThemeMode::Dark, cx);
                    app_theme_dark.update(cx, |s, cx| {
                        s.theme_mode = ThemeMode::Dark;
                        cx.notify();
                    });
                })
                .child(rust_i18n::t!("shell.theme_dark")),
        );

    div()
        .w_full()
        .flex()
        .flex_col()
        .gap(px(16.))
        .child(section_header(
            p,
            &rust_i18n::t!("preferences.appearance_title"),
            Some(&rust_i18n::t!("preferences.appearance_desc")),
        ))
        .child(
            grouped_card(p)
                .child(setting_row(
                    p,
                    &rust_i18n::t!("preferences.theme"),
                    Some(&rust_i18n::t!("preferences.theme_desc")),
                    theme_switcher,
                    false,
                ))
                .child(setting_row(
                    p,
                    &rust_i18n::t!("preferences.language"),
                    Some(&rust_i18n::t!("preferences.language_desc")),
                    Select::new(language_select).w(px(240.)),
                    true,
                ))
                .child(setting_row(
                    p,
                    &rust_i18n::t!("preferences.close_to_tray"),
                    Some(&rust_i18n::t!("preferences.close_to_tray_desc")),
                    Switch::new("close-to-tray-switch")
                        .checked(cfg.close_to_tray)
                        .on_change(move |checked, _, cx| {
                            let val = *checked;
                            app_tray.update(cx, |s, cx| {
                                s.controller.toggle_close_to_tray(val);
                                cx.notify();
                            });
                        }),
                    true,
                ))
                .child(setting_row(
                    p,
                    &rust_i18n::t!("preferences.case_popup"),
                    Some(&rust_i18n::t!("preferences.case_popup_desc")),
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap(px(10.))
                        .child(
                            Button::new("preview-case-popup-btn")
                                .ghost()
                                .compact()
                                .rounded(ButtonRounded::Size(px(999.)))
                                .label(rust_i18n::t!("preferences.preview_popup"))
                                .on_click(move |_, _, cx| {
                                    app_preview.read(cx).controller.preview_case_popup();
                                }),
                        )
                        .child(
                            Switch::new("case-popup-switch")
                                .checked(cfg.case_popup_enabled)
                                .on_change(move |checked, _, cx| {
                                    let val = *checked;
                                    app_popup.update(cx, |s, cx| {
                                        s.controller.toggle_case_popup(val);
                                        cx.notify();
                                    });
                                }),
                        ),
                    true,
                ))
                .child(setting_row(
                    p,
                    &rust_i18n::t!("preferences.tray_feature"),
                    Some(&rust_i18n::t!("preferences.tray_feature_desc")),
                    Icon::new(gpui_kit::component::IconName::Check)
                        .size(px(16.))
                        .text_color(p.accent_green),
                    true,
                )),
        )
        .child(section_header(
            p,
            &rust_i18n::t!("preferences.hardware_title"),
            Some(&rust_i18n::t!("preferences.hardware_desc")),
        ))
        .child(
            grouped_card(p)
                .child(setting_row(
                    p,
                    &rust_i18n::t!("preferences.model_code"),
                    Some(&format!("{} (Apple Inc.)", dev.capabilities.name)),
                    div(),
                    false,
                ))
                .child(setting_row(
                    p,
                    &rust_i18n::t!("preferences.mac"),
                    Some(&dev.mac_address),
                    div(),
                    true,
                ))
                .child(setting_row(
                    p,
                    &rust_i18n::t!("preferences.firmware"),
                    Some("A2564 • 65.1868046005000000.52 (官方 6A321 固件)"),
                    div(),
                    true,
                ))
                .child(setting_row(
                    p,
                    &rust_i18n::t!("preferences.serial"),
                    Some("主盒: CDGJFXHF63 | 左: GX6MGE30035Y | 右: GX6MGMWS0360"),
                    div(),
                    true,
                )),
        )
        .child(section_header(p, &rust_i18n::t!("controls.mpris_integration"), None))
        .child(
            grouped_card(p)
                .child(setting_row(
                    p,
                    "D-Bus MPRIS 媒体联动",
                    Some(&rust_i18n::t!("controls.mpris_desc")),
                    Tag::success().outline().child(rust_i18n::t!("preferences.running")),
                    false,
                ))
                .child(setting_row(
                    p,
                    "PipeWire / PulseAudio 原生音频",
                    Some("支持 A2DP 高清立体声音频流与无缝切换"),
                    Tag::success().outline().child(rust_i18n::t!("preferences.normal")),
                    true,
                )),
        )
        .child(section_header(p, &rust_i18n::t!("preferences.about_title"), None))
        .child({
            let update_control = match &state.update_state {
                UpdateState::Idle => {
                    let app_check = app_state.clone();
                    div()
                        .flex()
                        .flex_row()
                        .flex_wrap()
                        .items_center()
                        .gap(px(8.))
                        .child(
                            Button::new("check-update-btn")
                                .ghost()
                                .compact()
                                .rounded(ButtonRounded::Size(px(999.)))
                                .label(rust_i18n::t!("preferences.check_update"))
                                .on_click(move |_, _, cx| {
                                    app_check.read(cx).controller.check_for_updates();
                                }),
                        )
                        .into_any_element()
                }
                UpdateState::Checking => div()
                    .text_size(px(12.))
                    .text_color(p.text_secondary)
                    .child(rust_i18n::t!("preferences.checking_update"))
                    .into_any_element(),
                UpdateState::UpToDate { .. } => {
                    let app_check = app_state.clone();
                    div()
                        .flex()
                        .flex_row()
                        .flex_wrap()
                        .items_center()
                        .gap(px(8.))
                        .child(
                            Tag::success()
                                .outline()
                                .child(rust_i18n::t!("preferences.up_to_date")),
                        )
                        .child(
                            Button::new("recheck-update-btn")
                                .ghost()
                                .compact()
                                .rounded(ButtonRounded::Size(px(999.)))
                                .label(rust_i18n::t!("preferences.check_update"))
                                .on_click(move |_, _, cx| {
                                    app_check.read(cx).controller.check_for_updates();
                                }),
                        )
                        .into_any_element()
                }
                UpdateState::UpdateAvailable {
                    latest_version,
                    release_url,
                    is_package_managed,
                    can_self_update,
                    ..
                } => {
                    let rel_url = release_url.clone();
                    let latest = latest_version.clone();
                    let is_pkg = *is_package_managed;
                    let can_self = *can_self_update;
                    let app_install = app_state.clone();
                    let hint = if is_pkg {
                        rust_i18n::t!("preferences.update_via_package_manager")
                    } else if can_self {
                        rust_i18n::t!("preferences.update_ready_hint")
                    } else {
                        rust_i18n::t!("preferences.update_via_script")
                    };

                    let mut actions = div()
                        .flex()
                        .flex_row()
                        .flex_wrap()
                        .items_center()
                        .gap(px(8.))
                        .child(
                            Tag::warning().outline().child(format!(
                                "{}: v{}",
                                rust_i18n::t!("preferences.update_available"),
                                latest
                            )),
                        );
                    if can_self {
                        actions = actions.child(
                            Button::new("install-update-btn")
                                .primary()
                                .compact()
                                .rounded(ButtonRounded::Size(px(999.)))
                                .label(rust_i18n::t!("preferences.update_now"))
                                .on_click(move |_, _, cx| {
                                    app_install
                                        .read(cx)
                                        .controller
                                        .install_update(latest.clone());
                                }),
                        );
                    }
                    actions = actions.child(
                        Button::new("view-release-btn")
                            .ghost()
                            .compact()
                            .rounded(ButtonRounded::Size(px(999.)))
                            .label(rust_i18n::t!("preferences.view_release_notes"))
                            .on_click(move |_, _, _| {
                                let _ = open_browser(&rel_url);
                            }),
                    );

                    div()
                        .w_full()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .gap(px(6.))
                        .child(actions)
                        .child(
                            div()
                                .w_full()
                                .min_w_0()
                                .text_size(px(11.))
                                .text_color(p.text_secondary)
                                .child(hint),
                        )
                        .into_any_element()
                }
                UpdateState::Installing { latest_version } => div()
                    .w_full()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap(px(4.))
                    .child(
                        Tag::warning().outline().child(format!(
                            "{}: v{}",
                            rust_i18n::t!("preferences.updating"),
                            latest_version
                        )),
                    )
                    .child(
                        div()
                            .w_full()
                            .min_w_0()
                            .text_size(px(11.))
                            .text_color(p.text_secondary)
                            .child(rust_i18n::t!("preferences.updating_hint")),
                    )
                    .into_any_element(),
                UpdateState::Updated { version } => {
                    let app_restart = app_state.clone();
                    div()
                        .w_full()
                        .min_w_0()
                        .flex()
                        .flex_row()
                        .flex_wrap()
                        .items_center()
                        .gap(px(8.))
                        .child(Tag::success().outline().child(
                            rust_i18n::t!("preferences.update_complete", version = version),
                        ))
                        .child(
                            Button::new("restart-app-btn")
                                .primary()
                                .compact()
                                .rounded(ButtonRounded::Size(px(999.)))
                                .label(rust_i18n::t!("preferences.restart_now"))
                                .on_click(move |_, _, cx| {
                                    app_restart.read(cx).controller.restart_app();
                                }),
                        )
                        .into_any_element()
                }
                UpdateState::Error(err) => {
                    let app_check = app_state.clone();
                    let err_text = err.clone();
                    div()
                        .w_full()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .gap(px(6.))
                        .child(
                            div()
                                .flex()
                                .flex_row()
                                .flex_wrap()
                                .items_center()
                                .gap(px(8.))
                                .child(
                                    Tag::danger()
                                        .outline()
                                        .child(rust_i18n::t!("preferences.update_failed")),
                                )
                                .child(
                                    Button::new("retry-update-btn")
                                        .ghost()
                                        .compact()
                                        .rounded(ButtonRounded::Size(px(999.)))
                                        .label(rust_i18n::t!("preferences.check_update"))
                                        .on_click(move |_, _, cx| {
                                            app_check.read(cx).controller.check_for_updates();
                                        }),
                                ),
                        )
                        .child(
                            div()
                                .w_full()
                                .min_w_0()
                                .text_size(px(11.))
                                .text_color(p.accent_red)
                                .child(err_text),
                        )
                        .into_any_element()
                }
            };

            grouped_card(p).child(
                div()
                    .w_full()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap(px(12.))
                    .px(px(18.))
                    .py(px(16.))
                    .child(
                        div()
                            .w_full()
                            .min_w_0()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap(px(16.))
                            .child(
                                gpui_kit::gpui::img("icons/rustpods.png")
                                    .size(px(48.))
                                    .rounded(px(10.)),
                            )
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .gap(px(4.))
                                    .flex_1()
                                    .min_w_0()
                                    .child(
                                        div()
                                            .flex()
                                            .flex_row()
                                            .flex_wrap()
                                            .items_center()
                                            .gap(px(8.))
                                            .child(
                                                div()
                                                    .text_size(px(15.))
                                                    .font_weight(FontWeight::BOLD)
                                                    .text_color(p.text_primary)
                                                    .child("RustPods"),
                                            )
                                            .child(
                                                Tag::success().outline().child(format!(
                                                    "v{}",
                                                    env!("CARGO_PKG_VERSION")
                                                )),
                                            ),
                                    )
                                    .child(
                                        div()
                                            .w_full()
                                            .min_w_0()
                                            .text_size(px(12.))
                                            .text_color(p.text_secondary)
                                            .child(rust_i18n::t!("preferences.about_summary")),
                                    ),
                            ),
                    )
                    .child(div().w_full().min_w_0().child(update_control)),
            )
        })
}

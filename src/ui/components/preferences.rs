use crate::system::config::get_config_dir;
use crate::system::updater::{open_browser, UpdateState};
use crate::ui::components::setting_rows::{
    action_row, collapsible_section, danger_action_row, feedback_banner, info_row, page_header,
    segmented_control, select_row, toggle_row, SettingGroup,
};
use crate::ui::state::{PreferencesTab, RustPodsState};
use crate::ui::theme::{
    apply_apple_theme, grouped_card, section_header, ApplePalette, ROW_DESC_SIZE,
    SECTION_TITLE_SIZE, SELECT_W_MD,
};
use gpui_kit::component::button::{Button, ButtonRounded, ButtonVariants as _};
use gpui_kit::component::select::SelectState;
use gpui_kit::component::switch::Switch;
use gpui_kit::component::tag::Tag;
use gpui_kit::component::{Icon, IconName, ThemeMode};
use gpui_kit::{
    div, px, AnyElement, Entity, FontWeight, IntoElement, ParentElement as _, Styled as _,
};

pub fn render_preferences_page(
    state: &RustPodsState,
    app_state: &Entity<RustPodsState>,
    language_select: &Entity<SelectState<Vec<String>>>,
    p: &ApplePalette,
) -> impl IntoElement {
    let feedback = state
        .settings_feedback
        .as_ref()
        .map(|fb| feedback_banner(p, &fb.message, fb.is_error).into_any_element());

    let tabs: Vec<(&'static str, String, bool)> = PreferencesTab::ALL
        .iter()
        .map(|&tab| (tab.id(), tab.label(), state.preferences_tab == tab))
        .collect();

    let app_tab = app_state.clone();
    let tab_bar = segmented_control(p, "pref-tab", tabs, move |idx, _, cx| {
        if let Some(&tab) = PreferencesTab::ALL.get(idx) {
            app_tab.update(cx, |s, cx| {
                s.set_preferences_tab(tab);
                cx.notify();
            });
        }
    });

    let body: AnyElement = match state.preferences_tab {
        PreferencesTab::General => {
            render_general_tab(state, app_state, language_select, p).into_any_element()
        }
        PreferencesTab::System => render_system_tab(state, app_state, p).into_any_element(),
        PreferencesTab::About => render_about_tab(state, app_state, p).into_any_element(),
    };

    div()
        .w_full()
        .flex()
        .flex_col()
        .gap(px(16.))
        .child(page_header(
            p,
            &rust_i18n::t!("preferences.title"),
            Some(&rust_i18n::t!("preferences.subtitle")),
            feedback,
        ))
        .child(div().flex().flex_row().child(tab_bar))
        .child(body)
}

fn render_general_tab(
    state: &RustPodsState,
    app_state: &Entity<RustPodsState>,
    language_select: &Entity<SelectState<Vec<String>>>,
    p: &ApplePalette,
) -> impl IntoElement {
    let cfg = state.config_snapshot();
    let is_dark = state.theme_mode.is_dark();

    let app_theme = app_state.clone();
    let theme_items = vec![
        ("light", rust_i18n::t!("shell.theme_light").to_string(), !is_dark),
        ("dark", rust_i18n::t!("shell.theme_dark").to_string(), is_dark),
    ];
    let theme_switcher = segmented_control(p, "pref-theme", theme_items, move |idx, _, cx| {
        let next = if idx == 0 {
            ThemeMode::Light
        } else {
            ThemeMode::Dark
        };
        apply_apple_theme(next, cx);
        app_theme.update(cx, |s, cx| {
            s.apply_theme_mode(next);
            s.notify_saved();
            cx.notify();
        });
    });

    let app_tray = app_state.clone();
    let app_min = app_state.clone();
    let app_popup = app_state.clone();
    let app_preview = app_state.clone();

    div()
        .w_full()
        .flex()
        .flex_col()
        .gap(px(14.))
        .child(section_header(
            p,
            &rust_i18n::t!("preferences.appearance_title"),
            Some(&rust_i18n::t!("preferences.appearance_desc")),
        ))
        .child(
            SettingGroup::new(p)
                .row(action_row(
                    p,
                    &rust_i18n::t!("preferences.theme"),
                    Some(&rust_i18n::t!("preferences.theme_desc")),
                    theme_switcher,
                ))
                .row(select_row(
                    p,
                    &rust_i18n::t!("preferences.language"),
                    Some(&rust_i18n::t!("preferences.language_desc")),
                    language_select,
                    SELECT_W_MD,
                    false,
                ))
                .build(),
        )
        .child(section_header(
            p,
            &rust_i18n::t!("preferences.startup_section"),
            Some(&rust_i18n::t!("preferences.startup_section_desc")),
        ))
        .child(
            SettingGroup::new(p)
                .row(toggle_row(
                    p,
                    "close-to-tray-switch",
                    &rust_i18n::t!("preferences.close_to_tray"),
                    Some(&rust_i18n::t!("preferences.close_to_tray_desc")),
                    cfg.close_to_tray,
                    false,
                    move |checked, _, cx| {
                        let val = *checked;
                        app_tray.update(cx, |s, cx| {
                            s.controller.toggle_close_to_tray(val);
                            s.notify_saved();
                            cx.notify();
                        });
                    },
                ))
                .row(toggle_row(
                    p,
                    "start-minimized-switch",
                    &rust_i18n::t!("preferences.start_minimized"),
                    Some(&rust_i18n::t!("preferences.start_minimized_desc")),
                    cfg.start_minimized,
                    false,
                    move |checked, _, cx| {
                        let val = *checked;
                        app_min.update(cx, |s, cx| {
                            s.controller.toggle_start_minimized(val);
                            s.notify_saved();
                            cx.notify();
                        });
                    },
                ))
                .row(action_row(
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
                                        s.notify_saved();
                                        cx.notify();
                                    });
                                }),
                        ),
                ))
                .build(),
        )
}

fn render_system_tab(
    state: &RustPodsState,
    app_state: &Entity<RustPodsState>,
    p: &ApplePalette,
) -> impl IntoElement {
    let dev = &state.device;
    let connected = dev.is_connected;

    let model_value = if connected {
        format!("{} (Apple Inc.)", dev.capabilities.name)
    } else {
        rust_i18n::t!("preferences.device_disconnected_placeholder").to_string()
    };

    let mac_value = if connected && dev.mac_address != "00:00:00:00:00:00" {
        dev.mac_address.clone()
    } else {
        rust_i18n::t!("preferences.device_disconnected_placeholder").to_string()
    };

    let diag_summary = if connected {
        rust_i18n::t!(
            "preferences.diagnostics_summary_connected",
            model = dev.capabilities.name
        )
        .to_string()
    } else {
        rust_i18n::t!("preferences.diagnostics_summary_disconnected").to_string()
    };

    let diag_content = div()
        .w_full()
        .flex()
        .flex_col()
        .child(action_row(
            p,
            &rust_i18n::t!("preferences.tray_feature"),
            Some(&rust_i18n::t!("preferences.tray_feature_desc")),
            Tag::success().outline().child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(4.))
                    .child(Icon::new(IconName::Check).size(px(12.)))
                    .child(rust_i18n::t!("preferences.running")),
            ),
        ))
        .child(
            div()
                .w_full()
                .border_t_1()
                .border_color(p.separator)
                .child(action_row(
                    p,
                    &rust_i18n::t!("preferences.mpris_row_title"),
                    Some(&rust_i18n::t!("controls.mpris_desc")),
                    Tag::success()
                        .outline()
                        .child(rust_i18n::t!("preferences.running")),
                )),
        )
        .child(
            div()
                .w_full()
                .border_t_1()
                .border_color(p.separator)
                .child(action_row(
                    p,
                    &rust_i18n::t!("preferences.linux_audio_title"),
                    Some(&rust_i18n::t!("preferences.linux_audio_desc")),
                    Tag::success()
                        .outline()
                        .child(rust_i18n::t!("preferences.normal")),
                )),
        );

    let app_diag = app_state.clone();

    div()
        .w_full()
        .flex()
        .flex_col()
        .gap(px(14.))
        .child(section_header(
            p,
            &rust_i18n::t!("preferences.hardware_title"),
            Some(&rust_i18n::t!("preferences.hardware_desc")),
        ))
        .child(
            SettingGroup::new(p)
                .row(info_row(
                    p,
                    &rust_i18n::t!("preferences.model_code"),
                    &model_value,
                    !connected,
                ))
                .row(info_row(
                    p,
                    &rust_i18n::t!("preferences.mac"),
                    &mac_value,
                    !connected,
                ))
                .row(info_row(
                    p,
                    &rust_i18n::t!("preferences.firmware"),
                    &rust_i18n::t!("preferences.unavailable_value"),
                    true,
                ))
                .build(),
        )
        .child(collapsible_section(
            p,
            "pref-diagnostics-toggle",
            &rust_i18n::t!("preferences.diagnostics_title"),
            &diag_summary,
            state.diagnostics_expanded,
            move |_, cx| {
                app_diag.update(cx, |s, cx| {
                    s.toggle_diagnostics_expanded();
                    cx.notify();
                });
            },
            diag_content,
        ))
}

fn render_about_tab(
    state: &RustPodsState,
    app_state: &Entity<RustPodsState>,
    p: &ApplePalette,
) -> impl IntoElement {
    let config_dir = get_config_dir();
    let config_dir_display = config_dir.to_string_lossy().into_owned();
    let app_open = app_state.clone();
    let app_req_reset = app_state.clone();
    let app_confirm_reset = app_state.clone();
    let app_cancel_reset = app_state.clone();

    let reset_row = if state.confirm_reset {
        danger_action_row(
            p,
            &rust_i18n::t!("preferences.reset_confirm_prompt"),
            Some(&rust_i18n::t!("preferences.reset_defaults_desc")),
            div()
                .flex()
                .flex_row()
                .flex_wrap()
                .items_center()
                .gap(px(8.))
                .child(
                    Button::new("cancel-reset-btn")
                        .ghost()
                        .compact()
                        .rounded(ButtonRounded::Size(px(999.)))
                        .label(rust_i18n::t!("preferences.reset_cancel_btn"))
                        .on_click(move |_, _, cx| {
                            app_cancel_reset.update(cx, |s, cx| {
                                s.cancel_reset_confirm();
                                cx.notify();
                            });
                        }),
                )
                .child(
                    Button::new("confirm-reset-btn")
                        .danger()
                        .compact()
                        .rounded(ButtonRounded::Size(px(999.)))
                        .label(rust_i18n::t!("preferences.reset_confirm_btn"))
                        .on_click(move |_, _, cx| {
                            app_confirm_reset.update(cx, |s, cx| {
                                s.reset_config_to_defaults();
                                cx.notify();
                            });
                        }),
                ),
        )
    } else {
        action_row(
            p,
            &rust_i18n::t!("preferences.reset_defaults"),
            Some(&rust_i18n::t!("preferences.reset_defaults_desc")),
            Button::new("request-reset-btn")
                .secondary()
                .compact()
                .rounded(ButtonRounded::Size(px(999.)))
                .icon(Icon::new(IconName::RotateCw))
                .label(rust_i18n::t!("preferences.reset_defaults"))
                .on_click(move |_, _, cx| {
                    app_req_reset.update(cx, |s, cx| {
                        s.request_reset_confirm();
                        cx.notify();
                    });
                }),
        )
    };

    div()
        .w_full()
        .flex()
        .flex_col()
        .gap(px(14.))
        .child(render_about_update_card(state, app_state, p))
        .child(section_header(
            p,
            &rust_i18n::t!("preferences.maintenance_title"),
            Some(&rust_i18n::t!("preferences.maintenance_desc")),
        ))
        .child(
            SettingGroup::new(p)
                .row(action_row(
                    p,
                    &rust_i18n::t!("preferences.open_config_dir"),
                    Some(&config_dir_display),
                    Button::new("open-config-dir-btn")
                        .secondary()
                        .compact()
                        .rounded(ButtonRounded::Size(px(999.)))
                        .icon(Icon::new(IconName::FolderOpen))
                        .label(rust_i18n::t!("preferences.open_folder"))
                        .on_click(move |_, _, cx| {
                            if let Err(err) = open::that(get_config_dir()) {
                                app_open.update(cx, |s, cx| {
                                    s.set_settings_feedback(format!("{}", err), true);
                                    cx.notify();
                                });
                            }
                        }),
                ))
                .row(reset_row)
                .build(),
        )
}

fn render_about_update_card(
    state: &RustPodsState,
    app_state: &Entity<RustPodsState>,
    p: &ApplePalette,
) -> impl IntoElement {
    let update_control = render_update_control(&state.update_state, app_state, p);

    grouped_card(p).child(
        div()
            .w_full()
            .min_w_0()
            .flex()
            .flex_col()
            .gap(px(14.))
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
                                            .text_size(px(SECTION_TITLE_SIZE + 2.))
                                            .font_weight(FontWeight::BOLD)
                                            .text_color(p.text_primary)
                                            .child("RustPods"),
                                    )
                                    .child(Tag::success().outline().child(format!(
                                        "v{}",
                                        env!("CARGO_PKG_VERSION")
                                    ))),
                            )
                            .child(
                                div()
                                    .w_full()
                                    .min_w_0()
                                    .text_size(px(ROW_DESC_SIZE))
                                    .text_color(p.text_secondary)
                                    .child(rust_i18n::t!("preferences.about_summary")),
                            ),
                    ),
            )
            .child(div().w_full().min_w_0().child(update_control)),
    )
}

fn render_update_control(
    update_state: &UpdateState,
    app_state: &Entity<RustPodsState>,
    p: &ApplePalette,
) -> AnyElement {
    match update_state {
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
                        .secondary()
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
            .text_size(px(ROW_DESC_SIZE))
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
            let app_install = app_state.clone();
            let hint = if *is_package_managed {
                rust_i18n::t!("preferences.update_via_package_manager")
            } else if *can_self_update {
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
                .child(Tag::warning().outline().child(format!(
                    "{}: v{}",
                    rust_i18n::t!("preferences.update_available"),
                    latest
                )));
            if *can_self_update {
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
            .child(Tag::warning().outline().child(format!(
                "{}: v{}",
                rust_i18n::t!("preferences.updating"),
                latest_version
            )))
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
                .child(
                    Tag::success().outline().child(rust_i18n::t!(
                        "preferences.update_complete",
                        version = version
                    )),
                )
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
    }
}

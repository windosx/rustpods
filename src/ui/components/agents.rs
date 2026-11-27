use crate::system::agent_hooks::{registered_agents, HookInstallStatus};
use crate::system::zcode_ask::AskReply;
use crate::ui::components::setting_rows::{
    action_row, feedback_banner, page_header, select_row, toggle_row, SettingGroup,
};
use crate::ui::state::RustPodsState;
use crate::ui::theme::{section_header, ApplePalette, SELECT_W_MD};
use gpui_kit::component::button::{Button, ButtonRounded, ButtonVariant, ButtonVariants as _};
use gpui_kit::component::select::SelectState;
use gpui_kit::component::tag::Tag;
use gpui_kit::{div, px, Entity, IntoElement, ParentElement as _, Styled as _};

pub fn render_agents_page(
    state: &RustPodsState,
    app_state: &Entity<RustPodsState>,
    sensitivity_select: &Entity<SelectState<Vec<String>>>,
    p: &ApplePalette,
) -> impl IntoElement {
    let cfg = state.config_snapshot();
    let app_toggle = app_state.clone();

    let mut agents_group = SettingGroup::new(p);
    for (idx, agent) in registered_agents().iter().enumerate() {
        if !agent.available {
            agents_group = agents_group.row(action_row(
                p,
                agent.display_name,
                Some(agent.description),
                Tag::secondary().child(rust_i18n::t!("agents.coming_soon")),
            ));
            continue;
        }

        let status = agent.status();
        let is_installed = matches!(status, HookInstallStatus::Installed(_));
        let is_testing = state.agent_testing.get(agent.id).copied().unwrap_or(false);

        let subtitle = if let Some(note) = state.agent_notes.get(agent.id) {
            note.clone()
        } else {
            match &status {
                HookInstallStatus::Installed(cmd) => {
                    format!("{} · {}", rust_i18n::t!("agents.installed"), cmd)
                }
                HookInstallStatus::NotInstalled => rust_i18n::t!("agents.not_installed").to_string(),
                HookInstallStatus::ConfigUnreadable(e) => format!("Config error: {}", e),
            }
        };

        let app_test_perm = app_state.clone();
        let app_install = app_state.clone();
        let agent_id = agent.id;

        let controls = div()
            .flex()
            .flex_row()
            .flex_wrap()
            .items_center()
            .gap(px(8.))
            .child(
                Button::new(("agent-test-perm", idx))
                    .secondary()
                    .compact()
                    .rounded(ButtonRounded::Size(px(999.)))
                    .loading(is_testing)
                    .label(if is_testing {
                        rust_i18n::t!("agents.testing")
                    } else {
                        rust_i18n::t!("agents.test")
                    })
                    .on_click(move |_, _, cx| {
                        app_test_perm.update(cx, |s, cx| {
                            s.agent_testing.insert(agent_id, true);
                            s.agent_notes.insert(
                                agent_id,
                                "Permission test: nod to allow, shake to deny...".to_string(),
                            );
                            let rx = s.controller.start_agent_permission_test(agent.display_name);
                            cx.notify();

                            cx.spawn(async move |this, cx| {
                                let Ok(result) = rx.await else {
                                    return;
                                };
                                let msg = match result {
                                    Ok(AskReply::Allow) => {
                                        "Test passed: nodded to allow permission".to_string()
                                    }
                                    Ok(AskReply::Deny) => {
                                        "Test passed: shook head to deny permission".to_string()
                                    }
                                    Ok(AskReply::Pass) => {
                                        "Test completed: passed through to terminal".to_string()
                                    }
                                    Ok(other) => format!("Test result: {:?}", other),
                                    Err(e) => format!("Test failed: {}", e),
                                };
                                let _ = this.update(cx, |s, cx| {
                                    s.agent_testing.insert(agent_id, false);
                                    s.agent_notes.insert(agent_id, msg);
                                    cx.notify();
                                });
                            })
                            .detach();
                        });
                    }),
            )
            .child(
                Button::new(("agent-install", idx))
                    .with_variant(if is_installed {
                        ButtonVariant::Secondary
                    } else {
                        ButtonVariant::Primary
                    })
                    .compact()
                    .rounded(ButtonRounded::Size(px(999.)))
                    .label(if is_installed {
                        rust_i18n::t!("agents.uninstall")
                    } else {
                        rust_i18n::t!("agents.install")
                    })
                    .on_click(move |_, _, cx| {
                        let installed_now =
                            matches!(agent.status(), HookInstallStatus::Installed(_));
                        app_install.update(cx, |s, cx| {
                            let outcome = if installed_now {
                                agent.uninstall()
                            } else {
                                agent.install()
                            };
                            match outcome {
                                Ok(()) => {
                                    s.agent_notes.insert(
                                        agent_id,
                                        if installed_now {
                                            "Uninstalled".to_string()
                                        } else {
                                            "Installed".to_string()
                                        },
                                    );
                                }
                                Err(e) => {
                                    s.agent_notes
                                        .insert(agent_id, format!("Failed: {}", e));
                                }
                            }
                            cx.notify();
                        });
                    }),
            );

        agents_group = agents_group.row(action_row(
            p,
            agent.display_name,
            Some(&subtitle),
            controls,
        ));
    }

    let feedback = state
        .settings_feedback
        .as_ref()
        .map(|fb| feedback_banner(p, &fb.message, fb.is_error).into_any_element());

    let sens_desc = if cfg.gesture_permission_answers_enabled {
        rust_i18n::t!("agents.sensitivity_desc")
    } else {
        rust_i18n::t!("settings.disabled_requires_permission_answers")
    };

    div()
        .w_full()
        .flex()
        .flex_col()
        .gap(px(16.))
        .child(page_header(
            p,
            &rust_i18n::t!("agents.title"),
            Some(&rust_i18n::t!("agents.desc")),
            feedback,
        ))
        .child(section_header(
            p,
            &rust_i18n::t!("agents.permission_answers"),
            None,
        ))
        .child(
            SettingGroup::new(p)
                .row(toggle_row(
                    p,
                    "gesture-permission-answers-switch",
                    &rust_i18n::t!("agents.permission_answers"),
                    Some(&rust_i18n::t!("agents.permission_answers_desc")),
                    cfg.gesture_permission_answers_enabled,
                    false,
                    move |checked, _, cx| {
                        let val = *checked;
                        app_toggle.update(cx, |s, cx| {
                            s.controller.toggle_gesture_permission_answers(val);
                            s.notify_saved();
                            cx.notify();
                        });
                    },
                ))
                .row(select_row(
                    p,
                    &rust_i18n::t!("agents.sensitivity"),
                    Some(&sens_desc),
                    sensitivity_select,
                    SELECT_W_MD,
                    !cfg.gesture_permission_answers_enabled,
                ))
                .build(),
        )
        .child(section_header(
            p,
            &rust_i18n::t!("agents.hook_title"),
            None,
        ))
        .child(agents_group.build())
}

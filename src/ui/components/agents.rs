use crate::system::agent_hooks::{registered_agents, HookInstallStatus};
use crate::system::zcode_ask::AskReply;
use crate::ui::state::RustPodsState;
use crate::ui::theme::{grouped_card, section_header, setting_row, ApplePalette};
use gpui_kit::component::button::{Button, ButtonRounded, ButtonVariants as _};
use gpui_kit::component::select::{Select, SelectState};
use gpui_kit::component::switch::Switch;
use gpui_kit::component::tag::Tag;
use gpui_kit::{
    div, px, Entity, FontWeight, IntoElement, ParentElement as _, Styled as _,
};

pub fn render_agents_page(
    state: &RustPodsState,
    app_state: &Entity<RustPodsState>,
    sensitivity_select: &Entity<SelectState<Vec<String>>>,
    p: &ApplePalette,
) -> impl IntoElement {
    let cfg = state.config_snapshot();
    let app_toggle = app_state.clone();

    let mut agents_group = grouped_card(p);
    for (idx, agent) in registered_agents().iter().enumerate() {
        let with_divider = idx > 0;
        if !agent.available {
            agents_group = agents_group.child(setting_row(
                p,
                agent.display_name,
                Some(agent.description),
                Tag::secondary().child(rust_i18n::t!("agents.coming_soon")),
                with_divider,
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
                HookInstallStatus::Installed(cmd) => format!("{} · {}", rust_i18n::t!("agents.installed"), cmd),
                HookInstallStatus::NotInstalled => rust_i18n::t!("agents.not_installed").to_string(),
                HookInstallStatus::ConfigUnreadable(e) => format!("Config error: {}", e),
            }
        };

        let app_test = app_state.clone();
        let app_install = app_state.clone();
        let agent_id = agent.id;

        let controls = div()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(8.))
            .child(
                Button::new(("agent-test", idx))
                    .secondary()
                    .compact()
                    .rounded(ButtonRounded::Size(px(999.)))
                    .loading(is_testing)
                    .label(if is_testing { rust_i18n::t!("agents.testing") } else { rust_i18n::t!("agents.test") })
                    .on_click(move |_, _, cx| {
                        app_test.update(cx, |s, cx| {
                            s.agent_testing.insert(agent_id, true);
                            s.agent_notes.insert(
                                agent_id,
                                "Test in progress: nod to answer, or wait...".to_string(),
                            );
                            let rx = s.controller.start_agent_test(agent.display_name);
                            cx.notify();

                            cx.spawn(async move |this, cx| {
                                let Ok(result) = rx.await else {
                                    return;
                                };
                                let msg = match result {
                                    Ok(AskReply::Select { index, label }) => {
                                        format!("Test passed: selected #{} \"{}\"", index + 1, label)
                                    }
                                    Ok(AskReply::BatchSelect { selections }) => {
                                        format!("Test passed: completed {} questions", selections.len())
                                    }
                                    Ok(AskReply::Pass) => {
                                        "Test completed: passed through".to_string()
                                    }
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
                        gpui_kit::component::button::ButtonVariant::Secondary
                    } else {
                        gpui_kit::component::button::ButtonVariant::Primary
                    })
                    .compact()
                    .rounded(ButtonRounded::Size(px(999.)))
                    .label(if is_installed { rust_i18n::t!("agents.uninstall") } else { rust_i18n::t!("agents.install") })
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

        agents_group = agents_group.child(setting_row(
            p,
            agent.display_name,
            Some(&subtitle),
            controls,
            with_divider,
        ));
    }

    div()
        .w_full()
        .flex()
        .flex_col()
        .gap(px(16.))
        .child(
            div()
                .flex()
                .flex_col()
                .gap(px(4.))
                .px(px(4.))
                .child(
                    div()
                        .text_size(px(20.))
                        .font_weight(FontWeight::BOLD)
                        .text_color(p.text_primary)
                        .child(rust_i18n::t!("agents.title")),
                )
                .child(
                    div()
                        .text_size(px(12.5))
                        .text_color(p.text_secondary)
                        .child(rust_i18n::t!("agents.desc")),
                ),
        )
        .child(section_header(p, &rust_i18n::t!("agents.nod_answers"), None))
        .child(
            grouped_card(p)
                .child(setting_row(
                    p,
                    &rust_i18n::t!("agents.nod_answers"),
                    Some(&rust_i18n::t!("agents.nod_answers_desc")),
                    Switch::new("head-gesture-answers-switch")
                        .checked(cfg.head_gesture_answers_enabled)
                        .on_change(move |checked, _, cx| {
                            let val = *checked;
                            app_toggle.update(cx, |s, cx| {
                                s.controller.toggle_head_gesture_answers(val);
                                cx.notify();
                            });
                        }),
                    false,
                ))
                .child(setting_row(
                    p,
                    &rust_i18n::t!("agents.sensitivity"),
                    Some(&rust_i18n::t!("agents.sensitivity_desc")),
                    Select::new(sensitivity_select).w(px(170.)),
                    true,
                )),
        )
        .child(section_header(
            p,
            &rust_i18n::t!("agents.title"),
            None,
        ))
        .child(agents_group)
}

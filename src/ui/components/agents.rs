use crate::system::agent_hooks::{registered_agents, HookInstallStatus};
use crate::system::zcode_ask::AskReply;
use crate::ui::state::RustPodsState;
use crate::ui::theme::{grouped_card, section_header, setting_row, ApplePalette};
use gpui_kit::component::button::{Button, ButtonRounded, ButtonVariants as _};
use gpui_kit::component::select::{Select, SelectState};
use gpui_kit::component::switch::Switch;
use gpui_kit::component::tag::Tag;
use gpui_kit::{div, px, Entity, FontWeight, IntoElement, ParentElement as _, Styled as _};

pub fn render_agents_page(
    state: &RustPodsState,
    app_state: &Entity<RustPodsState>,
    sensitivity_select: &Entity<SelectState<Vec<&'static str>>>,
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
                Tag::secondary().child("即将支持"),
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
                HookInstallStatus::Installed(cmd) => format!("已接入 · {}", cmd),
                HookInstallStatus::NotInstalled => "未接入".to_string(),
                HookInstallStatus::ConfigUnreadable(e) => format!("配置读取失败: {}", e),
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
                    .label(if is_testing { "测试中" } else { "测试" })
                    .on_click(move |_, _, cx| {
                        app_test.update(cx, |s, cx| {
                            s.agent_testing.insert(agent_id, true);
                            s.agent_notes.insert(
                                agent_id,
                                "测试进行中:点头作答,或等待自动放行…".to_string(),
                            );
                            let rx = s.controller.start_zcode_test();
                            cx.notify();

                            cx.spawn(async move |this, cx| {
                                let Ok(result) = rx.await else {
                                    return;
                                };
                                let msg = match result {
                                    Ok(AskReply::Select { index, label }) => {
                                        format!("测试通过:已选择第 {} 项「{}」", index + 1, label)
                                    }
                                    Ok(AskReply::Pass) => {
                                        "测试完成:已放行(未佩戴 / 未连接 / 摇头或超时)".to_string()
                                    }
                                    Err(e) => format!("测试失败: {}", e),
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
                    .label(if is_installed { "卸载" } else { "安装" })
                    .on_click(move |_, _, cx| {
                        let installed_now =
                            matches!(agent.status(), HookInstallStatus::Installed(_));
                        let res = if installed_now {
                            agent.uninstall()
                        } else {
                            agent.install()
                        };
                        app_install.update(cx, |s, cx| {
                            match res {
                                Ok(()) => {
                                    s.agent_notes.remove(agent_id);
                                }
                                Err(e) => {
                                    s.agent_notes
                                        .insert(agent_id, format!("操作失败: {}", e));
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
                        .child("智能体接入"),
                )
                .child(
                    div()
                        .text_size(px(12.5))
                        .text_color(p.text_secondary)
                        .child("智能体弹出选择题时,点头 N 次选择第 N 项,摇头取消;未佩戴或超时自动回退到智能体自己的界面。"),
                ),
        )
        .child(section_header(p, "点头应答", None))
        .child(
            grouped_card(p)
                .child(setting_row(
                    p,
                    "启用点头应答",
                    Some("关闭后,智能体的选择题不再由点头代答,直接走各自的界面"),
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
                    "手势灵敏度",
                    Some("点头计数与摇头取消的判定幅度:1 = 需要明显的动作,5 = 轻微动作即可"),
                    Select::new(sensitivity_select).w(px(170.)),
                    true,
                )),
        )
        .child(section_header(
            p,
            "智能体",
            Some("安装后,该智能体的选择题会先送到 RustPods 由点头作答"),
        ))
        .child(agents_group)
}

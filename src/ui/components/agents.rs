//! The dedicated "智能体接入" page.
//!
//! Everything about answering coding agents' multiple-choice questions with
//! head gestures lives here, as a first-class sidebar entry: the master
//! switch, the gesture sensitivity, and one row per supported agent with
//! install / uninstall / test controls. The install/uninstall buttons drive
//! the same entry points the CLI flags use, and the test button pushes a
//! sample question through the real answering pipeline, so what it shows is
//! what a live question does. The live question card itself stays on the
//! 空间音频 page, next to the compass that doubles as gesture feedback.

use crate::system::agent_hooks::{registered_agents, AgentHook, HookInstallStatus};
use crate::system::config::{save_config, AppConfig};
use crate::system::zcode_ask::{self, AskReply};
use gtk4::prelude::*;
use gtk4::{Align, Box, Button, Label, Orientation};
use libadwaita::prelude::*;
use libadwaita::{ActionRow, ComboRow, PreferencesGroup, SwitchRow};
use std::sync::Arc;

/// Builds the "智能体接入" page.
pub fn build_agents_page(config: &Arc<std::sync::RwLock<AppConfig>>) -> Box {
    let container = Box::new(Orientation::Vertical, 12);
    container.set_margin_top(16);
    container.set_margin_bottom(24);
    container.set_margin_start(20);
    container.set_margin_end(20);

    // Page header
    let header = Box::new(Orientation::Vertical, 4);
    let title = Label::new(Some("智能体接入"));
    title.add_css_class("title-1");
    title.set_halign(Align::Start);
    header.append(&title);
    let description = Label::new(Some(
        "智能体弹出选择题时,点头 N 次选择第 N 项,摇头取消;未佩戴或超时自动回退到智能体自己的界面。",
    ));
    description.set_wrap(true);
    description.set_xalign(0.0);
    description.add_css_class("dim-label");
    description.set_halign(Align::Start);
    header.append(&description);
    container.append(&header);

    // Group 1: the gesture answer itself
    let gesture_group = PreferencesGroup::new();
    gesture_group.set_title("点头应答");

    let answer_switch = SwitchRow::new();
    answer_switch.set_title("启用点头应答");
    answer_switch.set_subtitle("关闭后,智能体的选择题不再由点头代答,直接走各自的界面");
    let init_answer = config.read().unwrap().head_gesture_answers_enabled;
    answer_switch.set_active(init_answer);
    {
        let cfg = config.clone();
        answer_switch.connect_active_notify(move |sw| {
            if let Ok(mut c) = cfg.write() {
                c.head_gesture_answers_enabled = sw.is_active();
                save_config(&c);
            }
        });
    }
    gesture_group.add(&answer_switch);

    // Gesture sensitivity, 1-5. Stored in the config, applied process-wide
    // immediately, and picked up by the answering task at the next question.
    let sensitivity_row = ComboRow::new();
    sensitivity_row.set_title("手势灵敏度");
    sensitivity_row.set_subtitle("点头计数与摇头取消的判定幅度:1 = 需要明显的动作,5 = 轻微动作即可");
    sensitivity_row.set_model(Some(&gtk4::StringList::new(&[
        "1 · 很不灵敏",
        "2 · 较不灵敏",
        "3 · 标准",
        "4 · 较灵敏",
        "5 · 很灵敏",
    ])));
    let current_sensitivity = config.read().unwrap().gesture_sensitivity.clamp(1, 5);
    sensitivity_row.set_selected((current_sensitivity - 1) as u32);
    {
        let cfg = config.clone();
        sensitivity_row.connect_selected_notify(move |row| {
            let value = crate::core::nod_counter::set_sensitivity(row.selected() as u8 + 1);
            if let Ok(mut c) = cfg.write() {
                c.gesture_sensitivity = value;
                save_config(&c);
            }
        });
    }
    gesture_group.add(&sensitivity_row);

    container.append(&gesture_group);

    // Group 2: one row per supported agent
    let agents_group = PreferencesGroup::new();
    agents_group.set_title("智能体");
    agents_group.set_description(Some("安装后,该智能体的选择题会先送到 RustPods 由点头作答"));

    for agent in registered_agents() {
        if !agent.available {
            let row = ActionRow::new();
            row.set_title(agent.display_name);
            row.set_subtitle(agent.description);
            let badge = Label::new(Some("即将支持"));
            badge.add_css_class("dim-label");
            badge.add_css_class("caption");
            row.add_suffix(&badge);
            agents_group.add(&row);
            continue;
        }

        let row = ActionRow::new();
        row.set_title(agent.display_name);

        let test_btn = Button::with_label("测试");
        test_btn.add_css_class("pill");
        let install_btn = Button::with_label("");
        install_btn.add_css_class("pill");
        row.add_suffix(&test_btn);
        row.add_suffix(&install_btn);
        agents_group.add(&row);

        refresh_agent_row(agent, &row, &install_btn);

        // Install / uninstall, then re-read the agent's real config so the
        // row always reflects what is actually on disk, not what we hoped.
        let row_action = row.clone();
        install_btn.connect_clicked(move |btn| {
            let installed = matches!(agent.status(), HookInstallStatus::Installed(_));
            let result = if installed {
                agent.uninstall()
            } else {
                agent.install()
            };
            if let Err(e) = result {
                row_action.set_subtitle(&format!("操作失败: {}", e));
            }
            refresh_agent_row(agent, &row_action, btn);
        });

        // Push a sample question through the live pipeline. While head
        // tracking is active this waits for nods, exactly like a real
        // question would; the spatial page's card mirrors the progress.
        let row_test = row.clone();
        test_btn.connect_clicked(move |_| {
            row_test.set_subtitle("测试进行中:点头作答,或等待自动放行…");
            let (tx, rx) = tokio::sync::oneshot::channel::<Result<AskReply, String>>();
            tokio::spawn(async move {
                let _ = tx.send(zcode_ask::run_test_question().await);
            });
            // A fresh handle per click: the row itself stays with the closure.
            let row_result = row_test.clone();
            glib::MainContext::default().spawn_local(async move {
                let Ok(result) = rx.await else {
                    return;
                };
                match result {
                    Ok(AskReply::Select { index, label }) => {
                        row_result.set_subtitle(&format!(
                            "测试通过:已选择第 {} 项「{}」",
                            index + 1,
                            label
                        ));
                    }
                    Ok(AskReply::Pass) => {
                        row_result
                            .set_subtitle("测试完成:已放行(未佩戴 / 未连接 / 摇头或超时)");
                    }
                    Err(e) => row_result.set_subtitle(&format!("测试失败: {}", e)),
                }
            });
        });
    }

    container.append(&agents_group);

    container
}

/// Re-renders one agent row from that agent's on-disk install status.
fn refresh_agent_row(agent: &AgentHook, row: &ActionRow, install_btn: &Button) {
    match agent.status() {
        HookInstallStatus::Installed(cmd) => {
            row.set_subtitle(&format!("已接入 · {}", cmd));
            install_btn.set_label("卸载");
        }
        HookInstallStatus::NotInstalled => {
            row.set_subtitle("未接入");
            install_btn.set_label("安装");
        }
        HookInstallStatus::ConfigUnreadable(e) => {
            row.set_subtitle(&format!("配置读取失败: {}", e));
            install_btn.set_label("安装");
        }
    }
}

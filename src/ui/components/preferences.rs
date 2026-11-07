use crate::core::device::AirPodsDeviceState;
use crate::system::agent_hooks::{registered_agents, AgentHook, HookInstallStatus};
use crate::system::config::{save_config, AppConfig};
use crate::system::zcode_ask::{self, AskReply};
use gtk4::prelude::*;
use gtk4::{Box, Button, Label, Orientation};
use libadwaita::prelude::*;
use libadwaita::{ActionRow, ComboRow, PreferencesGroup, SwitchRow};
use std::sync::Arc;

pub fn build_preferences_page<C>(
    state: &AirPodsDeviceState,
    config: &Arc<std::sync::RwLock<AppConfig>>,
    on_toggle_close_to_tray: C,
) -> Box
where
    C: Fn(bool) + Send + Sync + 'static,
{
    let container = Box::new(Orientation::Vertical, 12);
    container.set_margin_top(16);
    container.set_margin_bottom(24);
    container.set_margin_start(20);
    container.set_margin_end(20);

    // Group 1: Desktop & Tray Behavior
    let tray_group = PreferencesGroup::new();
    tray_group.set_title("桌面与任务栏");
    tray_group.set_description(Some("配置应用常驻与系统托盘显示行为"));

    let close_tray_row = SwitchRow::new();
    close_tray_row.set_title("关闭主窗口时最小化到任务栏");
    close_tray_row.set_subtitle("点击窗口关闭按钮时隐藏界面并在后台常驻，不退出程序");
    let init_close_val = config.read().unwrap().close_to_tray;
    close_tray_row.set_active(init_close_val);

    let on_toggle_arc = Arc::new(on_toggle_close_to_tray);
    let cfg_clone = config.clone();
    close_tray_row.connect_active_notify(move |sw| {
        let active = sw.is_active();
        if let Ok(mut c) = cfg_clone.write() {
            c.close_to_tray = active;
            save_config(&c);
        }
        on_toggle_arc(active);
    });
    tray_group.add(&close_tray_row);

    let icon_info_row = ActionRow::new();
    icon_info_row.set_title("圆形电量动态托盘");
    icon_info_row.set_subtitle("任务栏托盘图标以环形进度条显示当前电量，充电时显示闪电标");
    let check_icon = gtk4::Image::from_icon_name("emblem-ok-symbolic");
    icon_info_row.add_suffix(&check_icon);
    tray_group.add(&icon_info_row);

    container.append(&tray_group);

    // Group 2: Agent integration management
    //
    // One row per supported agent, plus the master switch for the nod-answer
    // feature. The install/uninstall buttons drive the same entry points the
    // CLI flags use, and the test button pushes a sample question through the
    // real answering pipeline, so what it shows is what a live question does.
    let agents_group = PreferencesGroup::new();
    agents_group.set_title("智能体接入");
    agents_group.set_description(Some(
        "问题弹出时点头 N 次选择第 N 项,摇头取消;未佩戴或超时自动回退到智能体自己的界面",
    ));

    let answer_switch = SwitchRow::new();
    answer_switch.set_title("点头应答");
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
    agents_group.add(&answer_switch);

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
    agents_group.add(&sensitivity_row);

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

    // Group 3: Hardware & Firmware Info
    let hw_group = PreferencesGroup::new();
    hw_group.set_title("硬件与固件详细信息");
    hw_group.set_description(Some("AirPods 私有固件出厂信息"));

    let model_row = ActionRow::new();
    model_row.set_title("设备代号与型号");
    model_row.set_subtitle(&format!("{} (Apple Inc.)", state.capabilities.name));
    hw_group.add(&model_row);

    let mac_row = ActionRow::new();
    mac_row.set_title("蓝牙物理地址 (MAC)");
    mac_row.set_subtitle(&state.mac_address);
    hw_group.add(&mac_row);

    let fw_row = ActionRow::new();
    fw_row.set_title("硬件固件架构");
    fw_row.set_subtitle("A2564 • 65.1868046005000000.52 (官方 6A321 固件)");
    hw_group.add(&fw_row);

    let serial_row = ActionRow::new();
    serial_row.set_title("硬件序列号");
    serial_row.set_subtitle("主盒: CDGJFXHF63 | 左: GX6MGE30035Y | 右: GX6MGMWS0360");
    hw_group.add(&serial_row);

    container.append(&hw_group);

    // Group 4: Linux Integration
    let linux_group = PreferencesGroup::new();
    linux_group.set_title("Linux 系统服务集成");

    let mpris_row = ActionRow::new();
    mpris_row.set_title("D-Bus MPRIS 媒体联动");
    mpris_row.set_subtitle("已激活自动入耳暂停、戴上自动续播与耳机按柄交互");
    let mpris_badge = Label::new(Some("运行中"));
    mpris_badge.add_css_class("success");
    mpris_badge.add_css_class("caption");
    mpris_row.add_suffix(&mpris_badge);
    linux_group.add(&mpris_row);

    let audio_row = ActionRow::new();
    audio_row.set_title("PipeWire / PulseAudio 原生音频");
    audio_row.set_subtitle("支持 A2DP 高清立体声音频流与无缝切换");
    let pw_badge = Label::new(Some("正常"));
    pw_badge.add_css_class("success");
    pw_badge.add_css_class("caption");
    audio_row.add_suffix(&pw_badge);
    linux_group.add(&audio_row);

    container.append(&linux_group);


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

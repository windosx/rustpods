use crate::core::aacp::commands::MicrophoneMode;
use crate::system::config::PauseBehavior;
use gtk4::StringList;
use libadwaita::prelude::*;
use libadwaita::{ActionRow, ComboRow, PreferencesGroup, SwitchRow};
use std::sync::Arc;

pub fn build_smart_settings_group<E, P, C, M, K>(
    ear_detection_enabled: bool,
    pause_behavior: PauseBehavior,
    conv_awareness_enabled: bool,
    mic_mode: MicrophoneMode,
    case_chime_enabled: bool,
    has_case_speaker: bool,
    on_toggle_ear_detection: E,
    on_change_pause_behavior: P,
    on_toggle_conv_awareness: C,
    on_change_mic_mode: M,
    on_toggle_case_chime: K,
) -> PreferencesGroup
where
    E: Fn(bool) + Send + Sync + 'static,
    P: Fn(PauseBehavior) + Send + Sync + 'static,
    C: Fn(bool) + Send + Sync + 'static,
    M: Fn(MicrophoneMode) + Send + Sync + 'static,
    K: Fn(bool) + Send + Sync + 'static,
{
    let group = PreferencesGroup::new();
    group.set_title("智能交互与音频控制");
    group.set_description(Some("联动 Linux MPRIS 播放器与 PipeWire 音频系统"));

    // Ear detection switch
    let ear_row = SwitchRow::new();
    ear_row.set_title("自动入耳检测");
    ear_row.set_subtitle("摘下耳机自动暂停媒体播放，重新戴上后自动恢复");
    ear_row.set_active(ear_detection_enabled);
    let on_ear_arc = Arc::new(on_toggle_ear_detection);
    ear_row.connect_active_notify(move |sw| {
        on_ear_arc(sw.is_active());
    });
    group.add(&ear_row);

    // Pause behavior combo row
    let pause_items = StringList::new(&["单耳摘下即暂停 (推荐)", "双耳均摘下才暂停", "从不暂停"]);
    let pause_combo = ComboRow::new();
    pause_combo.set_title("暂停媒体行为");
    pause_combo.set_subtitle("检测到摘下耳塞时的响应模式");
    pause_combo.set_model(Some(&pause_items));
    let initial_selected = match pause_behavior {
        PauseBehavior::OneRemoved => 0,
        PauseBehavior::BothRemoved => 1,
        PauseBehavior::Never => 2,
    };
    pause_combo.set_selected(initial_selected);
    let on_pause_arc = Arc::new(on_change_pause_behavior);
    pause_combo.connect_selected_notify(move |combo| {
        let behavior = match combo.selected() {
            1 => PauseBehavior::BothRemoved,
            2 => PauseBehavior::Never,
            _ => PauseBehavior::OneRemoved,
        };
        on_pause_arc(behavior);
    });
    group.add(&pause_combo);

    // Conversational awareness
    let conv_row = SwitchRow::new();
    conv_row.set_title("对话感知 (Conversational Awareness)");
    conv_row.set_subtitle("开口说话时自动压低媒体播放音量，并降低背景环境噪声");
    conv_row.set_active(conv_awareness_enabled);
    let on_conv_arc = Arc::new(on_toggle_conv_awareness);
    conv_row.connect_active_notify(move |sw| {
        on_conv_arc(sw.is_active());
    });
    group.add(&conv_row);

    // Microphone Mode
    let mic_items = StringList::new(&["自动切换 (默认)", "始终左耳", "始终右耳"]);
    let mic_combo = ComboRow::new();
    mic_combo.set_title("麦克风选择");
    mic_combo.set_subtitle("指定通话及录音优先使用的耳塞麦克风");
    mic_combo.set_model(Some(&mic_items));
    let initial_mic_idx = match mic_mode {
        MicrophoneMode::Automatic => 0,
        MicrophoneMode::AlwaysLeft => 1,
        MicrophoneMode::AlwaysRight => 2,
    };
    mic_combo.set_selected(initial_mic_idx);
    let on_mic_arc = Arc::new(on_change_mic_mode);
    mic_combo.connect_selected_notify(move |combo| {
        let mode = match combo.selected() {
            1 => MicrophoneMode::AlwaysLeft,
            2 => MicrophoneMode::AlwaysRight,
            _ => MicrophoneMode::Automatic,
        };
        on_mic_arc(mode);
    });
    group.add(&mic_combo);

    // Case Tone
    if has_case_speaker {
        let chime_row = SwitchRow::new();
        chime_row.set_title("充电盒提示音");
        chime_row.set_subtitle("充电开始与寻找定位时通过充电盒外放提示音");
        chime_row.set_active(case_chime_enabled);
        let on_chime_arc = Arc::new(on_toggle_case_chime);
        chime_row.connect_active_notify(move |sw| {
            on_chime_arc(sw.is_active());
        });
        group.add(&chime_row);
    }

    // MPRIS integration info row
    let info_row = ActionRow::new();
    info_row.set_title("Linux MPRIS 播放器集成");
    info_row.set_subtitle("入耳自动暂停/续播适用于任何实现 MPRIS 接口的正在播放的播放器");
    let check_icon = gtk4::Image::from_icon_name("emblem-ok-symbolic");
    info_row.add_suffix(&check_icon);
    group.add(&info_row);

    group
}

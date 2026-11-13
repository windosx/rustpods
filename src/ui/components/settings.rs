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
    pause_select: &Entity<SelectState<Vec<&'static str>>>,
    mic_select: &Entity<SelectState<Vec<&'static str>>>,
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
            "智能交互与音频控制",
            Some("联动 Linux MPRIS 播放器与 PipeWire 音频系统"),
        ))
        .child(
            grouped_card(p)
                .child(setting_row(
                    p,
                    "自动入耳检测",
                    Some("摘下耳机自动暂停媒体播放，重新戴上后自动恢复"),
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
                    "暂停媒体行为",
                    Some("检测到摘下耳塞时的响应模式"),
                    Select::new(pause_select).w(px(210.)),
                    true,
                ))
                .child(setting_row(
                    p,
                    "对话感知 (Conversational Awareness)",
                    Some("开口说话时自动压低媒体播放音量，并降低背景环境噪声"),
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
                    "麦克风选择",
                    Some("指定通话及录音优先使用的耳塞麦克风"),
                    Select::new(mic_select).w(px(170.)),
                    true,
                ))
                .when(dev.capabilities.has_case_speaker, |this| {
                    this.child(setting_row(
                        p,
                        "充电盒提示音",
                        Some("充电开始与寻找定位时通过充电盒外放提示音"),
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
                    "Linux MPRIS 播放器集成",
                    Some("入耳自动暂停/续播适用于任何实现 MPRIS 接口的正在播放的播放器"),
                    Tag::success().outline().child(
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap(px(4.))
                            .child(Icon::new(gpui_kit::component::IconName::Check).size(px(12.)))
                            .child("已启用"),
                    ),
                    true,
                )),
        )
}

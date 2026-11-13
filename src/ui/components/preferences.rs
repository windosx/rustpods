use crate::ui::state::RustPodsState;
use crate::ui::theme::{apply_apple_theme, grouped_card, section_header, setting_row, ApplePalette};
use gpui_kit::component::button::{Button, ButtonRounded, ButtonVariants as _};
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
                .child("浅色"),
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
                .child("深色"),
        );

    div()
        .w_full()
        .flex()
        .flex_col()
        .gap(px(16.))
        .child(section_header(
            p,
            "桌面与任务栏",
            Some("配置应用常驻、外观主题与系统托盘显示行为"),
        ))
        .child(
            grouped_card(p)
                .child(setting_row(
                    p,
                    "界面外观主题",
                    Some("在 Apple 控制中心浅色与深色模式间切换"),
                    theme_switcher,
                    false,
                ))
                .child(setting_row(
                    p,
                    "关闭主窗口时最小化到任务栏",
                    Some("点击窗口关闭按钮时隐藏界面并在后台常驻，不退出程序"),
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
                    "开盖时显示 iOS 风格弹窗",
                    Some("在附近打开 AirPods 充电盒盖时弹出带 3D 旋转动画与电量的卡片"),
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
                                .label("预览弹窗")
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
                    "圆形电量动态托盘",
                    Some("任务栏托盘图标以环形进度条显示当前电量，充电时显示闪电标"),
                    Icon::new(gpui_kit::component::IconName::Check)
                        .size(px(16.))
                        .text_color(p.accent_green),
                    true,
                )),
        )
        .child(section_header(
            p,
            "硬件与固件详细信息",
            Some("AirPods 私有固件出厂信息"),
        ))
        .child(
            grouped_card(p)
                .child(setting_row(
                    p,
                    "设备代号与型号",
                    Some(&format!("{} (Apple Inc.)", dev.capabilities.name)),
                    div(),
                    false,
                ))
                .child(setting_row(
                    p,
                    "蓝牙物理地址 (MAC)",
                    Some(&dev.mac_address),
                    div(),
                    true,
                ))
                .child(setting_row(
                    p,
                    "硬件固件架构",
                    Some("A2564 • 65.1868046005000000.52 (官方 6A321 固件)"),
                    div(),
                    true,
                ))
                .child(setting_row(
                    p,
                    "硬件序列号",
                    Some("主盒: CDGJFXHF63 | 左: GX6MGE30035Y | 右: GX6MGMWS0360"),
                    div(),
                    true,
                )),
        )
        .child(section_header(p, "Linux 系统服务集成", None))
        .child(
            grouped_card(p)
                .child(setting_row(
                    p,
                    "D-Bus MPRIS 媒体联动",
                    Some("已激活自动入耳暂停、戴上自动续播与耳机按柄交互"),
                    Tag::success().outline().child("运行中"),
                    false,
                ))
                .child(setting_row(
                    p,
                    "PipeWire / PulseAudio 原生音频",
                    Some("支持 A2DP 高清立体声音频流与无缝切换"),
                    Tag::success().outline().child("正常"),
                    true,
                )),
        )
}

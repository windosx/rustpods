use crate::system::head_mouse::{HeadMouseCommand, HeadMouseState, UinputStatus, HEAD_MOUSE};
use crate::ui::state::RustPodsState;
use crate::ui::theme::{grouped_card, section_header, setting_row, ApplePalette};
use gpui_kit::component::button::{Button, ButtonRounded, ButtonVariant, ButtonVariants as _};
use gpui_kit::component::select::Select;
use gpui_kit::component::slider::Slider;
use gpui_kit::component::switch::Switch;
use gpui_kit::component::tag::Tag;
use gpui_kit::{div, px, App, Entity, IntoElement, ParentElement as _, Styled as _, Window};
use std::rc::Rc;

pub fn render_head_mouse_page(
    state: &RustPodsState,
    app_state: &Entity<RustPodsState>,
    mode_select: &Entity<gpui_kit::component::select::SelectState<Vec<String>>>,
    speed_slider: &Entity<gpui_kit::component::slider::SliderState>,
    accel_slider: &Entity<gpui_kit::component::slider::SliderState>,
    smooth_slider: &Entity<gpui_kit::component::slider::SliderState>,
    deadzone_slider: &Entity<gpui_kit::component::slider::SliderState>,
    on_enable_request: Rc<dyn Fn(&mut Window, &mut App)>,
    p: &ApplePalette,
) -> impl IntoElement {
    let cfg = state.config_snapshot();
    let runtime_state = HEAD_MOUSE.state();

    let app_toggle = app_state.clone();
    let app_invert_x = app_state.clone();
    let app_invert_y = app_state.clone();
    let app_roll = app_state.clone();
    let app_dwell = app_state.clone();
    let app_recenter = app_state.clone();
    let app_pause = app_state.clone();
    let app_scroll = app_state.clone();

    let status_tag = match runtime_state {
        HeadMouseState::Active => Tag::success().outline().child(rust_i18n::t!("head_mouse.status_running")),
        HeadMouseState::Paused => Tag::warning().outline().child(rust_i18n::t!("head_mouse.status_paused")),
        HeadMouseState::ScrollMode => Tag::primary().outline().child(rust_i18n::t!("head_mouse.status_scrolling")),
        HeadMouseState::PermissionDenied => Tag::danger().outline().child(rust_i18n::t!("head_mouse.status_unauthorized")),
        HeadMouseState::Disconnected => Tag::secondary().outline().child(rust_i18n::t!("head_mouse.status_disconnected")),
        HeadMouseState::Disabled => Tag::secondary().child(rust_i18n::t!("head_mouse.status_disabled")),
    };

    let is_paused = runtime_state == HeadMouseState::Paused;
    let is_scrolling = runtime_state == HeadMouseState::ScrollMode;
    let enable_fn = on_enable_request.clone();

    div()
        .w_full()
        .flex()
        .flex_col()
        .gap(px(14.))
        .child(section_header(
            p,
            &rust_i18n::t!("head_mouse.title"),
            Some(&rust_i18n::t!("head_mouse.desc")),
        ))
        // 卡片 1: 核心开关与运行状态
        .child(
            grouped_card(p)
                .child(setting_row(
                    p,
                    &rust_i18n::t!("head_mouse.enable"),
                    Some(&rust_i18n::t!("head_mouse.enable_desc")),
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap(px(10.))
                        .child(status_tag)
                        .child(
                            Switch::new("head-mouse-enabled-switch")
                                .checked(cfg.head_mouse_enabled)
                                .on_change(move |checked, window, cx| {
                                    let val = *checked;
                                    if val {
                                        if UinputStatus::probe().is_available() {
                                            // 已就绪，静默开启
                                            app_toggle.update(cx, |s, cx| {
                                                s.controller.toggle_head_mouse(true);
                                                cx.notify();
                                            });
                                        } else {
                                            // 未授权 / 未安装驱动，弹出 Polkit (pkexec) 授权窗口
                                            enable_fn(window, cx);
                                        }
                                    } else {
                                        app_toggle.update(cx, |s, cx| {
                                            s.controller.toggle_head_mouse(false);
                                            cx.notify();
                                        });
                                    }
                                }),
                        ),
                    false,
                ))
                .child(setting_row(
                    p,
                    &rust_i18n::t!("head_mouse.realtime_ctrl"),
                    Some(&rust_i18n::t!("head_mouse.realtime_desc")),
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap(px(8.))
                        .child(
                            Button::new("mouse-recenter-btn")
                                .secondary()
                                .compact()
                                .rounded(ButtonRounded::Size(px(999.)))
                                .label(rust_i18n::t!("head_mouse.recenter"))
                                .on_click(move |_, _, cx| {
                                    app_recenter.update(cx, |s, _| {
                                        s.controller.head_mouse_recenter();
                                    });
                                }),
                        )
                        .child(
                            Button::new("mouse-scroll-toggle-btn")
                                .with_variant(if is_scrolling {
                                    ButtonVariant::Primary
                                } else {
                                    ButtonVariant::Secondary
                                })
                                .compact()
                                .rounded(ButtonRounded::Size(px(999.)))
                                .label(if is_scrolling { rust_i18n::t!("head_mouse.exit_scroll") } else { rust_i18n::t!("head_mouse.scroll_mode") })
                                .on_click(move |_, _, cx| {
                                    app_scroll.update(cx, |_, _| {
                                        HEAD_MOUSE.send_command(HeadMouseCommand::ToggleScroll);
                                    });
                                }),
                        )
                        .child(
                            Button::new("mouse-pause-btn")
                                .with_variant(if is_paused {
                                    ButtonVariant::Primary
                                } else {
                                    ButtonVariant::Secondary
                                })
                                .compact()
                                .rounded(ButtonRounded::Size(px(999.)))
                                .label(if is_paused { rust_i18n::t!("head_mouse.resume") } else { rust_i18n::t!("head_mouse.pause") })
                                .on_click(move |_, _, cx| {
                                    app_pause.update(cx, |s, _| {
                                        s.controller.head_mouse_toggle_pause();
                                    });
                                }),
                        ),
                    true,
                )),
        )
        // 卡片 2: 模式与滚动交互
        .child(section_header(
            p,
            &rust_i18n::t!("head_mouse.mode_section"),
            Some(&rust_i18n::t!("head_mouse.mode_section_desc")),
        ))
        .child(
            grouped_card(p)
                .child(setting_row(
                    p,
                    &rust_i18n::t!("head_mouse.tracking_mode"),
                    Some(&rust_i18n::t!("head_mouse.tracking_mode_desc")),
                    Select::new(mode_select).w(px(220.)),
                    false,
                ))
                .child(setting_row(
                    p,
                    &rust_i18n::t!("head_mouse.invert_x"),
                    Some(&rust_i18n::t!("head_mouse.invert_x_desc")),
                    Switch::new("mouse-invert-x-switch")
                        .checked(cfg.head_mouse_invert_x)
                        .on_change(move |checked, _, cx| {
                            let val = *checked;
                            app_invert_x.update(cx, |s, cx| {
                                s.controller.toggle_head_mouse_invert_x(val);
                                cx.notify();
                            });
                        }),
                    true,
                ))
                .child(setting_row(
                    p,
                    &rust_i18n::t!("head_mouse.invert_y"),
                    Some(&rust_i18n::t!("head_mouse.invert_y_desc")),
                    Switch::new("mouse-invert-y-switch")
                        .checked(cfg.head_mouse_invert_y)
                        .on_change(move |checked, _, cx| {
                            let val = *checked;
                            app_invert_y.update(cx, |s, cx| {
                                s.controller.toggle_head_mouse_invert_y(val);
                                cx.notify();
                            });
                        }),
                    true,
                )),
        )
        // 卡片 3: 速度与动力学调节
        .child(section_header(
            p,
            &rust_i18n::t!("head_mouse.tuning_section"),
            Some(&rust_i18n::t!("head_mouse.tuning_desc")),
        ))
        .child(
            grouped_card(p)
                .child(setting_row(
                    p,
                    &rust_i18n::t!("head_mouse.speed"),
                    Some(&format!("{}: {:.0} px/°", rust_i18n::t!("head_mouse.speed"), cfg.head_mouse_speed)),
                    div()
                        .w(px(200.))
                        .child(Slider::new(speed_slider).horizontal()),
                    false,
                ))
                .child(setting_row(
                    p,
                    &rust_i18n::t!("head_mouse.accel"),
                    Some(&format!("{}: {:.1}x", rust_i18n::t!("head_mouse.accel"), cfg.head_mouse_accel)),
                    div()
                        .w(px(200.))
                        .child(Slider::new(accel_slider).horizontal()),
                    true,
                ))
                .child(setting_row(
                    p,
                    &rust_i18n::t!("head_mouse.smooth"),
                    Some(&format!("{}: {:.2}", rust_i18n::t!("head_mouse.smooth"), cfg.head_mouse_smoothing)),
                    div()
                        .w(px(200.))
                        .child(Slider::new(smooth_slider).horizontal()),
                    true,
                ))
                .child(setting_row(
                    p,
                    &rust_i18n::t!("head_mouse.deadzone"),
                    Some(&format!("{}: {:.1}°/s", rust_i18n::t!("head_mouse.deadzone"), cfg.head_mouse_deadzone)),
                    div()
                        .w(px(200.))
                        .child(Slider::new(deadzone_slider).horizontal()),
                    true,
                )),
        )
        // 卡片 4: 辅助手势与点击交互
        .child(section_header(
            p,
            &rust_i18n::t!("head_mouse.gestures_section"),
            Some(&rust_i18n::t!("head_mouse.gestures_desc")),
        ))
        .child(
            grouped_card(p)
                .child(setting_row(
                    p,
                    &rust_i18n::t!("head_mouse.stem_shortcuts"),
                    Some(&rust_i18n::t!("head_mouse.stem_desc")),
                    Tag::primary().outline().child(rust_i18n::t!("head_mouse.ready")),
                    false,
                ))
                .child(setting_row(
                    p,
                    &rust_i18n::t!("head_mouse.roll_click"),
                    Some(&rust_i18n::t!("head_mouse.roll_click_desc")),
                    Switch::new("mouse-roll-click-switch")
                        .checked(cfg.head_mouse_roll_click)
                        .on_change(move |checked, _, cx| {
                            let val = *checked;
                            app_roll.update(cx, |s, cx| {
                                s.controller.toggle_head_mouse_roll_click(val);
                                cx.notify();
                            });
                        }),
                    true,
                ))
                .child(setting_row(
                    p,
                    &rust_i18n::t!("head_mouse.dwell_click"),
                    Some(&rust_i18n::t!("head_mouse.dwell_click_desc")),
                    Switch::new("mouse-dwell-click-switch")
                        .checked(cfg.head_mouse_dwell_click)
                        .on_change(move |checked, _, cx| {
                            let val = *checked;
                            app_dwell.update(cx, |s, cx| {
                                s.controller.toggle_head_mouse_dwell_click(val);
                                cx.notify();
                            });
                        }),
                    true,
                )),
        )
}

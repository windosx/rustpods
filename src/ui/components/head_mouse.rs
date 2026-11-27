use crate::system::head_mouse::{HeadMouseCommand, HeadMouseState, UinputStatus, HEAD_MOUSE};
use crate::ui::components::setting_rows::{
    action_row, feedback_banner, page_header, select_row, slider_row, toggle_row, SettingGroup,
};
use crate::ui::state::RustPodsState;
use crate::ui::theme::{section_header, ApplePalette, SELECT_W_MD};
use gpui_kit::component::button::{Button, ButtonRounded, ButtonVariant, ButtonVariants as _};
use gpui_kit::component::tag::Tag;
use gpui_kit::{div, px, App, Entity, IntoElement, ParentElement as _, Styled as _, Window};
use std::rc::Rc;

/// Authorization callback that may pop the Polkit dialog.
pub type EnableRequest = Rc<dyn Fn(&mut Window, &mut App)>;

/// The gpui-kit select holding a localized option list.
pub type MouseSelect = Entity<gpui_kit::component::select::SelectState<Vec<String>>>;
/// The gpui-kit slider used by the tuning rows.
pub type MouseSlider = Entity<gpui_kit::component::slider::SliderState>;

/// The five tuning widgets the head-mouse page binds to, grouped so the
/// render function's signature stays readable.
pub struct HeadMouseWidgets<'a> {
    pub mode_select: &'a MouseSelect,
    pub speed: &'a MouseSlider,
    pub accel: &'a MouseSlider,
    pub smoothing: &'a MouseSlider,
    pub deadzone: &'a MouseSlider,
}

pub fn render_head_mouse_page(
    state: &RustPodsState,
    app_state: &Entity<RustPodsState>,
    widgets: &HeadMouseWidgets<'_>,
    on_enable_request: EnableRequest,
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

    let status_tag = if cfg.head_mouse_enabled && runtime_state == HeadMouseState::Disabled {
        Tag::secondary()
            .outline()
            .child(rust_i18n::t!("head_mouse.status_disconnected"))
    } else {
        match runtime_state {
            HeadMouseState::Active => {
                Tag::success().outline().child(rust_i18n::t!("head_mouse.status_running"))
            }
            HeadMouseState::Paused => {
                Tag::warning().outline().child(rust_i18n::t!("head_mouse.status_paused"))
            }
            HeadMouseState::ScrollMode => {
                Tag::primary().outline().child(rust_i18n::t!("head_mouse.status_scrolling"))
            }
            HeadMouseState::PermissionDenied => Tag::danger()
                .outline()
                .child(rust_i18n::t!("head_mouse.status_unauthorized")),
            HeadMouseState::Disconnected => Tag::secondary()
                .outline()
                .child(rust_i18n::t!("head_mouse.status_disconnected")),
            HeadMouseState::Disabled => {
                Tag::secondary().child(rust_i18n::t!("head_mouse.status_disabled"))
            }
        }
    };

    let header_trailing = state
        .settings_feedback
        .as_ref()
        .map(|fb| feedback_banner(p, &fb.message, fb.is_error).into_any_element())
        .or_else(|| Some(status_tag.into_any_element()));

    let is_paused = runtime_state == HeadMouseState::Paused;
    let is_scrolling = runtime_state == HeadMouseState::ScrollMode;
    let enable_fn = on_enable_request.clone();

    div()
        .w_full()
        .flex()
        .flex_col()
        .gap(px(16.))
        .child(page_header(
            p,
            &rust_i18n::t!("head_mouse.title"),
            Some(&rust_i18n::t!("head_mouse.desc")),
            header_trailing,
        ))
        .child(
            SettingGroup::new(p)
                .row(toggle_row(
                    p,
                    "head-mouse-enabled-switch",
                    &rust_i18n::t!("head_mouse.enable"),
                    Some(&rust_i18n::t!("head_mouse.enable_desc")),
                    cfg.head_mouse_enabled,
                    false,
                    move |checked, window, cx| {
                        let val = *checked;
                        if val && !UinputStatus::probe().is_available() {
                            enable_fn(window, cx);
                        } else {
                            app_toggle.update(cx, |s, cx| {
                                s.controller.toggle_head_mouse(val);
                                s.notify_saved();
                                cx.notify();
                            });
                        }
                    },
                ))
                .row(action_row(
                    p,
                    &rust_i18n::t!("head_mouse.realtime_ctrl"),
                    Some(&if cfg.head_mouse_enabled {
                        rust_i18n::t!("head_mouse.realtime_desc")
                    } else {
                        rust_i18n::t!("settings.disabled_requires_head_mouse")
                    }),
                    div()
                        .flex()
                        .flex_row()
                        .flex_wrap()
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
                                .label(if is_scrolling {
                                    rust_i18n::t!("head_mouse.exit_scroll")
                                } else {
                                    rust_i18n::t!("head_mouse.scroll_mode")
                                })
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
                                .label(if is_paused {
                                    rust_i18n::t!("head_mouse.resume")
                                } else {
                                    rust_i18n::t!("head_mouse.pause")
                                })
                                .on_click(move |_, _, cx| {
                                    app_pause.update(cx, |s, _| {
                                        s.controller.head_mouse_toggle_pause();
                                    });
                                }),
                        ),
                ))
                .build(),
        )
        .child(section_header(
            p,
            &rust_i18n::t!("head_mouse.mode_section"),
            Some(&rust_i18n::t!("head_mouse.mode_section_desc")),
        ))
        .child(
            SettingGroup::new(p)
                .row(select_row(
                    p,
                    &rust_i18n::t!("head_mouse.tracking_mode"),
                    Some(&rust_i18n::t!("head_mouse.tracking_mode_desc")),
                    widgets.mode_select,
                    SELECT_W_MD,
                    false,
                ))
                .row(toggle_row(
                    p,
                    "mouse-invert-x-switch",
                    &rust_i18n::t!("head_mouse.invert_x"),
                    Some(&rust_i18n::t!("head_mouse.invert_x_desc")),
                    cfg.head_mouse_invert_x,
                    false,
                    move |checked, _, cx| {
                        let val = *checked;
                        app_invert_x.update(cx, |s, cx| {
                            s.controller.toggle_head_mouse_invert_x(val);
                            s.notify_saved();
                            cx.notify();
                        });
                    },
                ))
                .row(toggle_row(
                    p,
                    "mouse-invert-y-switch",
                    &rust_i18n::t!("head_mouse.invert_y"),
                    Some(&rust_i18n::t!("head_mouse.invert_y_desc")),
                    cfg.head_mouse_invert_y,
                    false,
                    move |checked, _, cx| {
                        let val = *checked;
                        app_invert_y.update(cx, |s, cx| {
                            s.controller.toggle_head_mouse_invert_y(val);
                            s.notify_saved();
                            cx.notify();
                        });
                    },
                ))
                .build(),
        )
        .child(section_header(
            p,
            &rust_i18n::t!("head_mouse.tuning_section"),
            Some(&rust_i18n::t!("head_mouse.tuning_desc")),
        ))
        .child(
            SettingGroup::new(p)
                .row(slider_row(
                    p,
                    &rust_i18n::t!("head_mouse.speed"),
                    Some(&rust_i18n::t!("head_mouse.speed_desc")),
                    format!("{:.0} px/°", cfg.head_mouse_speed),
                    widgets.speed,
                ))
                .row(slider_row(
                    p,
                    &rust_i18n::t!("head_mouse.accel"),
                    Some(&rust_i18n::t!("head_mouse.accel_desc")),
                    format!("{:.1}x", cfg.head_mouse_accel),
                    widgets.accel,
                ))
                .row(slider_row(
                    p,
                    &rust_i18n::t!("head_mouse.smooth"),
                    Some(&rust_i18n::t!("head_mouse.smooth_desc")),
                    format!("{:.2}", cfg.head_mouse_smoothing),
                    widgets.smoothing,
                ))
                .row(slider_row(
                    p,
                    &rust_i18n::t!("head_mouse.deadzone"),
                    Some(&rust_i18n::t!("head_mouse.deadzone_desc")),
                    format!("{:.1}°/s", cfg.head_mouse_deadzone),
                    widgets.deadzone,
                ))
                .build(),
        )
        .child(section_header(
            p,
            &rust_i18n::t!("head_mouse.gestures_section"),
            Some(&rust_i18n::t!("head_mouse.gestures_desc")),
        ))
        .child(
            SettingGroup::new(p)
                .row(action_row(
                    p,
                    &rust_i18n::t!("head_mouse.stem_shortcuts"),
                    Some(&rust_i18n::t!("head_mouse.stem_desc")),
                    Tag::primary()
                        .outline()
                        .child(rust_i18n::t!("head_mouse.ready")),
                ))
                .row(toggle_row(
                    p,
                    "mouse-roll-click-switch",
                    &rust_i18n::t!("head_mouse.roll_click"),
                    Some(&rust_i18n::t!("head_mouse.roll_click_desc")),
                    cfg.head_mouse_roll_click,
                    false,
                    move |checked, _, cx| {
                        let val = *checked;
                        app_roll.update(cx, |s, cx| {
                            s.controller.toggle_head_mouse_roll_click(val);
                            s.notify_saved();
                            cx.notify();
                        });
                    },
                ))
                .row(toggle_row(
                    p,
                    "mouse-dwell-click-switch",
                    &rust_i18n::t!("head_mouse.dwell_click"),
                    Some(&rust_i18n::t!("head_mouse.dwell_click_desc")),
                    cfg.head_mouse_dwell_click,
                    false,
                    move |checked, _, cx| {
                        let val = *checked;
                        app_dwell.update(cx, |s, cx| {
                            s.controller.toggle_head_mouse_dwell_click(val);
                            s.notify_saved();
                            cx.notify();
                        });
                    },
                ))
                .build(),
        )
}

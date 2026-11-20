use crate::system::zcode_ask::ASK_HUB;
use crate::ui::state::RustPodsState;
use crate::ui::theme::palette;
use gpui_kit::component::Icon;
#[cfg(target_os = "linux")]
use gpui_kit::layer_shell::{
    Anchor as LayerAnchor, KeyboardInteractivity, Layer, LayerShellOptions,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    div, ease_in_out, ease_out_quint, px, size, Animation, AnimationExt as _,
    AnyWindowHandle, App, AppContext as _, Bounds, Context, Entity, FontWeight,
    InteractiveElement as _, IntoElement, ParentElement as _, Render,
    StatefulInteractiveElement as _, Styled as _, Subscription, Window,
    WindowBackgroundAppearance, WindowBounds, WindowDecorations, WindowHandle,
    WindowKind, WindowOptions,
};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tracing::{info, warn};

const ASK_POPUP_CLOSE_ANIM_MS: u64 = 220;

pub struct AskPopupWindow {
    app_state: Entity<RustPodsState>,
    closing: bool,
    anim_gen: u64,
    auto_close_scheduled: bool,
    slot: Arc<Mutex<Option<WindowHandle<Self>>>>,
    _obs: Subscription,
}

impl AskPopupWindow {
    pub fn new(
        app_state: Entity<RustPodsState>,
        _window: &mut Window,
        slot: Arc<Mutex<Option<WindowHandle<Self>>>>,
        cx: &mut Context<Self>,
    ) -> Self {
        let obs = cx.observe(&app_state, |_, _, cx| {
            cx.notify();
        });

        Self {
            app_state,
            closing: false,
            anim_gen: 0,
            auto_close_scheduled: false,
            slot,
            _obs: obs,
        }
    }

    pub fn start_close(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.closing {
            return;
        }
        self.closing = true;
        self.anim_gen = self.anim_gen.wrapping_add(1);
        cx.notify();

        let slot = self.slot.clone();
        cx.spawn_in(window, async move |_this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(ASK_POPUP_CLOSE_ANIM_MS))
                .await;
            let _ = cx.update(|window, _| {
                window.remove_window();
            });
            if let Ok(mut guard) = slot.lock() {
                *guard = None;
            }
        })
        .detach();
    }
}

impl Render for AskPopupWindow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = palette(cx);
        let state = self.app_state.read(cx);
        let progress = state.zcode_progress.clone();
        let resolved = state.zcode_resolved.clone();
        let closing = self.closing;
        let anim_gen = self.anim_gen;

        let card_bg = if p.is_dark {
            gpui_kit::rgba(0x1C1C1EF4)
        } else {
            gpui_kit::rgba(0xFFFFFFF7)
        };
        let card_border = if p.is_dark {
            gpui_kit::rgba(0xFFFFFF22)
        } else {
            gpui_kit::rgba(0x00000014)
        };
        let close_btn_bg = if p.is_dark {
            gpui_kit::rgba(0xFFFFFF18)
        } else {
            gpui_kit::rgba(0x0000000D)
        };
        let close_btn_hover = if p.is_dark {
            gpui_kit::rgba(0xFFFFFF2A)
        } else {
            gpui_kit::rgba(0x00000018)
        };

        let anim = if closing {
            Animation::new(Duration::from_millis(ASK_POPUP_CLOSE_ANIM_MS)).with_easing(ease_in_out)
        } else {
            Animation::new(Duration::from_millis(320)).with_easing(ease_out_quint())
        };

        // Auto-close transitions are scheduled from here but executed later:
        // closing inside render would mutate window state mid-render. The
        // empty case needs no handling at all - the UI loop in mod.rs closes
        // the popup once neither progress nor a fresh resolution exists.
        let is_resolved_only = progress.is_none() && resolved.is_some();
        let is_denied = resolved
            .as_ref()
            .map(|r| r.text.starts_with('✗') || r.text.contains("拒绝"))
            .unwrap_or(false);
        if is_resolved_only && !self.closing && !self.auto_close_scheduled {
            self.auto_close_scheduled = true;
            cx.spawn_in(window, async move |this, cx| {
                cx.background_executor()
                    .timer(Duration::from_millis(1200))
                    .await;
                let _ = this.update_in(cx, |view, window, cx| {
                    view.start_close(window, cx);
                });
            })
            .detach();
        }

        div()
            .size_full()
            .flex()
            .flex_col()
            .justify_end()
            .items_center()
            .when(!cfg!(target_os = "windows"), |el| el.p(px(12.)))
            .child(
                div()
                    .when(!cfg!(target_os = "windows"), |el| {
                        el.w(px(436.)).rounded(px(24.)).shadow_lg()
                    })
                    .when(cfg!(target_os = "windows"), |el| {
                        el.size_full().rounded(px(20.))
                    })
                    .bg(card_bg)
                    .border_1()
                    .border_color(if is_resolved_only {
                        if is_denied {
                            p.accent_red.opacity(0.35)
                        } else {
                            p.accent_green.opacity(0.35)
                        }
                    } else {
                        p.accent_blue.opacity(0.35)
                    })
                    .shadow_lg()
                    .px(px(18.))
                    .pt(px(14.))
                    .pb(px(16.))
                    .flex()
                    .flex_col()
                    .gap(px(10.))
                    .child(
                        div()
                            .relative()
                            .w_full()
                            .flex()
                            .flex_row()
                            .items_center()
                            .justify_between()
                            .child(
                                div()
                                    .flex()
                                    .flex_row()
                                    .items_center()
                                    .gap(px(8.))
                                    .child(
                                        Icon::new(if is_resolved_only {
                                            if is_denied {
                                                gpui_kit::assets::IconName::X
                                            } else {
                                                gpui_kit::assets::IconName::Check
                                            }
                                        } else {
                                            gpui_kit::assets::IconName::MessageCircleQuestionMark
                                        })
                                        .size(px(18.))
                                        .text_color(if is_resolved_only {
                                            if is_denied {
                                                p.accent_red
                                            } else {
                                                p.accent_green
                                            }
                                        } else {
                                            p.accent_blue
                                        }),
                                    )
                                    .child(
                                        div()
                                            .text_size(px(14.5))
                                            .font_weight(FontWeight::BOLD)
                                            .text_color(p.text_primary)
                                            .child({
                                                let agent_name = progress
                                                    .as_ref()
                                                    .map(|p| p.agent.as_str())
                                                    .or_else(|| resolved.as_ref().map(|r| r.agent.as_str()))
                                                    .filter(|s| !s.is_empty())
                                                    .unwrap_or("Agent");
                                                if is_resolved_only {
                                                    rust_i18n::t!("agents.ask_resolved", agent = agent_name)
                                                } else {
                                                    rust_i18n::t!("agents.permission_title", agent = agent_name)
                                                }
                                            }),
                                    )
                                    .when_some(progress.as_ref(), |this, ask| {
                                        let (status_text, is_active, active_color) = if ask.decision_preview == Some(true) {
                                            (rust_i18n::t!("agents.nod_allowed").to_string(), true, p.accent_green)
                                        } else if ask.decision_preview == Some(false) {
                                            (rust_i18n::t!("agents.shake_denied").to_string(), true, p.accent_red)
                                        } else {
                                            (rust_i18n::t!("agents.waiting_gesture").to_string(), false, p.accent_blue)
                                        };
                                        this.child(
                                            div()
                                                .px(px(9.))
                                                .py(px(3.))
                                                .rounded(px(999.))
                                                .bg(if is_active {
                                                    active_color.opacity(0.22)
                                                } else {
                                                    p.pill_bg
                                                })
                                                .text_size(px(12.))
                                                .font_weight(if is_active {
                                                    FontWeight::BOLD
                                                } else {
                                                    FontWeight::MEDIUM
                                                })
                                                .text_color(if is_active {
                                                    active_color
                                                } else {
                                                    p.text_secondary
                                                })
                                                .child(status_text),
                                        )
                                    }),
                            )
                            .child({
                                let view_c = cx.entity().clone();
                                div()
                                    .id("ask-popup-close-btn")
                                    .w(px(26.))
                                    .h(px(26.))
                                    .rounded(px(999.))
                                    .bg(close_btn_bg)
                                    .hover(|s| s.bg(close_btn_hover))
                                    .cursor_pointer()
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .on_click(move |_, window, cx| {
                                        ASK_HUB.cancel();
                                        view_c.update(cx, |v, cx| v.start_close(window, cx));
                                    })
                                    .child(
                                        Icon::new(gpui_kit::component::IconName::Close)
                                            .size(px(13.))
                                            .text_color(p.text_secondary),
                                    )
                            }),
                    )
                    .when_some(progress.as_ref(), |this, ask| {
                        let view_opt = cx.entity().clone();
                        if let Some(perm) = &ask.permission {
                            let view_allow = view_opt.clone();
                            let view_deny = view_opt.clone();
                            let preview_allow = ask.decision_preview == Some(true);
                            let preview_deny = ask.decision_preview == Some(false);
                            this.child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .gap(px(8.))
                                    .child(
                                        div()
                                            .flex()
                                            .flex_row()
                                            .items_center()
                                            .gap(px(8.))
                                            .child(
                                                div()
                                                    .px(px(7.))
                                                    .py(px(2.))
                                                    .rounded(px(6.))
                                                    .bg(p.accent_blue.opacity(0.18))
                                                    .text_size(px(12.))
                                                    .font_weight(FontWeight::BOLD)
                                                    .text_color(p.accent_blue)
                                                    .child(perm.tool_name.clone()),
                                            )
                                            .child(
                                                div()
                                                    .text_size(px(13.5))
                                                    .font_weight(FontWeight::MEDIUM)
                                                    .text_color(p.text_primary)
                                                    .child(ask.question.clone()),
                                            ),
                                    )
                                    .when(!perm.command_or_arg.is_empty(), |el| {
                                        el.child(
                                            div()
                                                .px(px(10.))
                                                .py(px(8.))
                                                .rounded(px(8.))
                                                .bg(if p.is_dark {
                                                    gpui_kit::rgba(0xFFFFFF0A)
                                                } else {
                                                    gpui_kit::rgba(0x00000006)
                                                })
                                                .border_1()
                                                .border_color(card_border)
                                                .text_size(px(12.))
                                                .text_color(p.text_secondary)
                                                .child(perm.command_or_arg.clone()),
                                        )
                                    })
                                    .child(
                                        div()
                                            .flex()
                                            .flex_row()
                                            .items_center()
                                            .gap(px(10.))
                                            .pt(px(4.))
                                            .child(
                                                // Allow button / gesture indicator
                                                div()
                                                    .id("permission-allow-btn")
                                                    .flex_1()
                                                    .cursor_pointer()
                                                    .px(px(12.))
                                                    .py(px(10.))
                                                    .rounded(px(12.))
                                                    .bg(if preview_allow {
                                                        p.accent_green.opacity(0.24)
                                                    } else if p.is_dark {
                                                        gpui_kit::rgba(0xFFFFFF0D).into()
                                                    } else {
                                                        gpui_kit::rgba(0x00000008).into()
                                                    })
                                                    .border_1()
                                                    .border_color(if preview_allow {
                                                        p.accent_green.opacity(0.8)
                                                    } else {
                                                        card_border.into()
                                                    })
                                                    .hover(|s| s.bg(p.accent_green.opacity(0.18)))
                                                    .flex()
                                                    .flex_row()
                                                    .items_center()
                                                    .justify_center()
                                                    .gap(px(6.))
                                                    .on_click(move |_, window, cx| {
                                                        ASK_HUB.resolve_permission(true);
                                                        view_allow.update(cx, |v, cx| {
                                                            v.start_close(window, cx);
                                                        });
                                                    })
                                                    .child(
                                                        Icon::new(gpui_kit::assets::IconName::Check)
                                                            .size(px(15.))
                                                            .text_color(p.accent_green),
                                                    )
                                                    .child(
                                                        div()
                                                            .text_size(px(13.))
                                                            .font_weight(FontWeight::SEMIBOLD)
                                                            .text_color(p.accent_green)
                                                            .child(rust_i18n::t!("agents.permission_nod_allow")),
                                                    ),
                                            )
                                            .child(
                                                // Deny button / gesture indicator
                                                div()
                                                    .id("permission-deny-btn")
                                                    .flex_1()
                                                    .cursor_pointer()
                                                    .px(px(12.))
                                                    .py(px(10.))
                                                    .rounded(px(12.))
                                                    .bg(if preview_deny {
                                                        p.accent_red.opacity(0.24)
                                                    } else if p.is_dark {
                                                        gpui_kit::rgba(0xFFFFFF0D).into()
                                                    } else {
                                                        gpui_kit::rgba(0x00000008).into()
                                                    })
                                                    .border_1()
                                                    .border_color(if preview_deny {
                                                        p.accent_red.opacity(0.8)
                                                    } else {
                                                        card_border.into()
                                                    })
                                                    .hover(|s| s.bg(p.accent_red.opacity(0.18)))
                                                    .flex()
                                                    .flex_row()
                                                    .items_center()
                                                    .justify_center()
                                                    .gap(px(6.))
                                                    .on_click(move |_, window, cx| {
                                                        ASK_HUB.resolve_permission(false);
                                                        view_deny.update(cx, |v, cx| {
                                                            v.start_close(window, cx);
                                                        });
                                                    })
                                                    .child(
                                                        Icon::new(gpui_kit::assets::IconName::X)
                                                            .size(px(15.))
                                                            .text_color(p.accent_red),
                                                    )
                                                    .child(
                                                        div()
                                                            .text_size(px(13.))
                                                            .font_weight(FontWeight::SEMIBOLD)
                                                            .text_color(p.accent_red)
                                                            .child(rust_i18n::t!("agents.permission_shake_deny")),
                                                    ),
                                            ),
                                    )
                                    .child(
                                        div()
                                            .text_size(px(11.5))
                                            .text_color(p.text_secondary)
                                            .child(rust_i18n::t!("agents.permission_footer_hint")),
                                    ),
                            )
                        } else {
                            this
                        }
                    })
                    .when_some(
                        if progress.is_none() {
                            resolved.as_ref()
                        } else {
                            None
                        },
                        |this, note| {
                            let is_denied = note.text.starts_with('✗') || note.text.contains("拒绝");
                            let color = if is_denied {
                                p.accent_red
                            } else {
                                p.accent_green
                            };
                            this.child(
                                div()
                                    .py(px(8.))
                                    .px(px(12.))
                                    .rounded(px(10.))
                                    .bg(color.opacity(0.12))
                                    .border_1()
                                    .border_color(color.opacity(0.3))
                                    .text_size(px(13.5))
                                    .font_weight(FontWeight::MEDIUM)
                                    .text_color(color)
                                    .child(note.text.clone()),
                            )
                            .child(
                                div()
                                    .text_size(px(11.5))
                                    .text_color(p.text_secondary)
                                    .child("已提交作答 · 稍后自动关闭"),
                            )
                        },
                    )
                    .relative()
                    .with_animation(("ask-popup-sheet", anim_gen), anim, move |el, t| {
                        let (dy, alpha) = if closing {
                            (t * 80.0, (1.0 - t).max(0.0))
                        } else {
                            ((1.0 - t) * 80.0, (t * 1.35).min(1.0))
                        };
                        el.top(px(dy)).opacity(alpha)
                    }),
            )
    }
}

pub fn open_or_refresh_ask_popup(
    app_state: &Entity<RustPodsState>,
    slot: &Arc<Mutex<Option<WindowHandle<AskPopupWindow>>>>,
    cx: &mut App,
) {
    if let Ok(mut guard) = slot.lock()
        && let Some(handle) = guard.as_ref() {
            let mut is_closing = false;
            let ok = handle
                .update(cx, |view, _window, cx| {
                    is_closing = view.closing;
                    if !is_closing {
                        cx.notify();
                    }
                })
                .is_ok();
            if ok && !is_closing {
                return;
            }
            *guard = None;
        }

    let popup_size = if cfg!(target_os = "windows") {
        size(px(436.), px(296.))
    } else {
        size(px(460.), px(320.))
    };
    let popup_bounds = WindowBounds::Windowed(Bounds::centered(None, popup_size, cx));
    let make_opts = |kind: WindowKind| WindowOptions {
        window_bounds: Some(popup_bounds),
        titlebar: Some(gpui_kit::TitlebarOptions {
            title: Some("RustPodsAskPopup".into()),
            appears_transparent: true,
            traffic_light_position: None,
        }),
        focus: false,
        show: true,
        kind,
        is_movable: false,
        is_resizable: false,
        is_minimizable: false,
        window_background: WindowBackgroundAppearance::Transparent,
        app_id: Some("io.github.rustpods.RustPods".to_string()),
        window_decorations: Some(WindowDecorations::Client),
        ..Default::default()
    };

    #[cfg(target_os = "linux")]
    let layer_opts = make_opts(WindowKind::LayerShell(LayerShellOptions {
        namespace: "rustpods-zcode-ask-popup".to_string(),
        layer: Layer::Overlay,
        anchor: LayerAnchor::BOTTOM,
        exclusive_zone: None,
        exclusive_edge: None,
        margin: Some((px(0.), px(0.), px(28.), px(0.))),
        keyboard_interactivity: KeyboardInteractivity::None,
    }));

    #[cfg(target_os = "linux")]
    let state_for_popup = app_state.clone();
    #[cfg(target_os = "linux")]
    let slot_for_popup = slot.clone();
    #[cfg(target_os = "linux")]
    let result = cx.open_window(layer_opts, move |window, cx| {
        cx.new(|cx| AskPopupWindow::new(state_for_popup, window, slot_for_popup, cx))
    });

    #[cfg(target_os = "linux")]
    let handle: Option<WindowHandle<AskPopupWindow>> = match result {
        Ok(h) => {
            info!("Opened ZCode Ask popup via Wayland LayerShell");
            Some(h)
        }
        Err(e) => {
            warn!("LayerShell popup unavailable ({}), falling back to PopUp window", e);
            let state_fallback = app_state.clone();
            let slot_fallback = slot.clone();
            match cx.open_window(make_opts(WindowKind::PopUp), move |window, cx| {
                cx.new(|cx| AskPopupWindow::new(state_fallback, window, slot_fallback, cx))
            }) {
                Ok(h) => Some(h),
                Err(e2) => {
                    warn!("Failed to open fallback PopUp window for ZCode Ask: {}", e2);
                    None
                }
            }
        }
    };

    #[cfg(not(target_os = "linux"))]
    let handle: Option<WindowHandle<AskPopupWindow>> = {
        let state_fallback = app_state.clone();
        let slot_fallback = slot.clone();
        match cx.open_window(make_opts(WindowKind::PopUp), move |window, cx| {
            cx.new(|cx| AskPopupWindow::new(state_fallback, window, slot_fallback, cx))
        }) {
            Ok(h) => {
                info!("Opened ZCode Ask popup via PopUp window");
                #[cfg(target_os = "windows")]
                {
                    crate::ui::popup::configure_windows_popup_dwm("RustPodsAskPopup");
                    tokio::spawn(async move {
                        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                        crate::ui::popup::configure_windows_popup_dwm("RustPodsAskPopup");
                    });
                }
                Some(h)
            }
            Err(e) => {
                warn!("Failed to open PopUp window for ZCode Ask: {}", e);
                None
            }
        }
    };

    if let (Some(h), Ok(mut guard)) = (handle, slot.lock()) {
        *guard = Some(h);
    }
}

pub fn close_ask_popup(
    slot: &Arc<Mutex<Option<WindowHandle<AskPopupWindow>>>>,
    cx: &mut App,
) {
    let handle = slot.lock().ok().and_then(|g| *g);
    if let Some(handle) = handle {
        let _ = handle.update(cx, |view, window, cx| {
            view.start_close(window, cx);
        });
    }
}

#[allow(dead_code)]
fn _assert_handle(_: AnyWindowHandle) {}

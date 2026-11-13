use crate::system::zcode_ask::ASK_HUB;
use crate::ui::state::RustPodsState;
use crate::ui::theme::palette;
use gpui_kit::component::Icon;
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

        let is_resolved_only = progress.is_none() && resolved.is_some();
        let is_empty = progress.is_none() && resolved.is_none();

        if is_empty && !self.closing {
            self.start_close(window, cx);
        } else if is_resolved_only && !self.closing && !self.auto_close_scheduled {
            self.auto_close_scheduled = true;
            let view_c = cx.entity().clone();
            cx.spawn_in(window, async move |_this, cx| {
                cx.background_executor()
                    .timer(Duration::from_millis(1200))
                    .await;
                let _ = cx.update(|window, cx| {
                    view_c.update(cx, |v, cx| {
                        v.start_close(window, cx);
                    });
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
            .p(px(12.))
            .child(
                div()
                    .w(px(436.))
                    .rounded(px(24.))
                    .bg(card_bg)
                    .border_1()
                    .border_color(if is_resolved_only {
                        p.accent_green.opacity(0.35)
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
                                            gpui_kit::assets::IconName::Check
                                        } else {
                                            gpui_kit::assets::IconName::MessageCircleQuestionMark
                                        })
                                        .size(px(18.))
                                        .text_color(if is_resolved_only {
                                            p.accent_green
                                        } else {
                                            p.accent_blue
                                        }),
                                    )
                                    .child(
                                        div()
                                            .text_size(px(14.5))
                                            .font_weight(FontWeight::BOLD)
                                            .text_color(p.text_primary)
                                            .child(if is_resolved_only {
                                                "ZCode 应答完成"
                                            } else {
                                                "ZCode 提问"
                                            }),
                                    )
                                    .when_some(progress.as_ref(), |this, ask| {
                                        let count_text = if ask.count == 0 {
                                            "等待点头…".to_string()
                                        } else {
                                            format!("已识别 {} 次点头", ask.count)
                                        };
                                        this.child(
                                            div()
                                                .px(px(9.))
                                                .py(px(3.))
                                                .rounded(px(999.))
                                                .bg(if ask.count > 0 {
                                                    p.accent_blue.opacity(0.22)
                                                } else {
                                                    p.pill_bg
                                                })
                                                .text_size(px(12.))
                                                .font_weight(if ask.count > 0 {
                                                    FontWeight::BOLD
                                                } else {
                                                    FontWeight::MEDIUM
                                                })
                                                .text_color(if ask.count > 0 {
                                                    p.accent_blue
                                                } else {
                                                    p.text_secondary
                                                })
                                                .child(count_text),
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
                        this.child(
                            div()
                                .text_size(px(13.5))
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(p.text_primary)
                                .child(ask.question.clone()),
                        )
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .gap(px(6.))
                                .children(ask.options.iter().enumerate().map(|(i, label)| {
                                    let is_selected = ask.count > 0 && i + 1 == ask.count;
                                    let is_passed = ask.count > 0 && i + 1 < ask.count;
                                    let view_opt_c = view_opt.clone();
                                    div()
                                        .id(("ask-option", i))
                                        .cursor_pointer()
                                        .px(px(12.))
                                        .py(px(8.))
                                        .rounded(px(10.))
                                        .bg(if is_selected {
                                            p.accent_blue.opacity(0.18)
                                        } else if is_passed {
                                            if p.is_dark {
                                                gpui_kit::rgba(0xFFFFFF08).into()
                                            } else {
                                                gpui_kit::rgba(0x00000005).into()
                                            }
                                        } else if p.is_dark {
                                            gpui_kit::rgba(0xFFFFFF0D).into()
                                        } else {
                                            gpui_kit::rgba(0x00000008).into()
                                        })
                                        .border_1()
                                        .border_color(if is_selected {
                                            p.accent_blue.opacity(0.65)
                                        } else {
                                            card_border.into()
                                        })
                                        .hover(|s| s.bg(p.accent_blue.opacity(0.14)))
                                        .flex()
                                        .flex_row()
                                        .items_center()
                                        .justify_between()
                                        .on_click(move |_, window, cx| {
                                            ASK_HUB.resolve_by_index(i);
                                            view_opt_c.update(cx, |v, cx| {
                                                v.start_close(window, cx);
                                            });
                                        })
                                        .child(
                                            div()
                                                .text_size(px(13.))
                                                .font_weight(if is_selected {
                                                    FontWeight::BOLD
                                                } else {
                                                    FontWeight::NORMAL
                                                })
                                                .text_color(if is_selected {
                                                    p.accent_blue
                                                } else if is_passed {
                                                    p.text_secondary
                                                } else {
                                                    p.text_primary
                                                })
                                                .child(format!("{}. {}", i + 1, label)),
                                        )
                                        .when(is_selected, |this| {
                                            this.child(
                                                div()
                                                    .flex()
                                                    .flex_row()
                                                    .items_center()
                                                    .gap(px(3.))
                                                    .px(px(7.))
                                                    .py(px(1.5))
                                                    .rounded(px(999.))
                                                    .bg(p.accent_blue.opacity(0.2))
                                                    .text_size(px(11.))
                                                    .font_weight(FontWeight::SEMIBOLD)
                                                    .text_color(p.accent_blue)
                                                    .child(
                                                        Icon::new(gpui_kit::assets::IconName::Check)
                                                            .size(px(11.))
                                                            .text_color(p.accent_blue),
                                                    )
                                                    .child("待确认"),
                                            )
                                        })
                                })),
                        )
                        .child(
                            div()
                                .flex()
                                .flex_row()
                                .items_center()
                                .justify_between()
                                .pt(px(2.))
                                .child(
                                    div()
                                        .text_size(px(11.5))
                                        .text_color(if ask.count > 0 {
                                            p.accent_blue
                                        } else {
                                            p.text_secondary
                                        })
                                        .child(if ask.count > 0 {
                                            format!(
                                                "已选第 {} 项 · 保持头部静止将自动提交 · 再次点头切换",
                                                ask.count
                                            )
                                        } else {
                                            "点头 1 次选择第 1 项 · 再次点头换下一项 · 也可点击选择".to_string()
                                        }),
                                )
                                .when(ask.count > 0 && ask.count <= ask.options.len(), |this| {
                                    let idx = ask.count - 1;
                                    let view_c = view_opt.clone();
                                    this.child(
                                        div()
                                            .id("ask-popup-confirm-now-btn")
                                            .px(px(10.))
                                            .py(px(3.))
                                            .rounded(px(999.))
                                            .bg(p.accent_blue)
                                            .hover(|s| s.opacity(0.9))
                                            .cursor_pointer()
                                            .text_size(px(11.5))
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .text_color(gpui_kit::white())
                                            .on_click(move |_, window, cx| {
                                                ASK_HUB.resolve_by_index(idx);
                                                view_c.update(cx, |v, cx| {
                                                    v.start_close(window, cx);
                                                });
                                            })
                                            .child("立即确认"),
                                    )
                                }),
                        )
                    })
                    .when_some(
                        if progress.is_none() {
                            resolved.as_ref()
                        } else {
                            None
                        },
                        |this, note| {
                            this.child(
                                div()
                                    .py(px(8.))
                                    .px(px(12.))
                                    .rounded(px(10.))
                                    .bg(p.accent_green.opacity(0.12))
                                    .border_1()
                                    .border_color(p.accent_green.opacity(0.3))
                                    .text_size(px(13.5))
                                    .font_weight(FontWeight::MEDIUM)
                                    .text_color(p.accent_green)
                                    .child(note.clone()),
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
    if let Ok(mut guard) = slot.lock() {
        if let Some(handle) = guard.as_ref() {
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
    }

    let popup_size = size(px(460.), px(320.));
    let popup_bounds = WindowBounds::Windowed(Bounds::centered(None, popup_size, cx));
    let make_opts = |kind: WindowKind| WindowOptions {
        window_bounds: Some(popup_bounds),
        titlebar: None,
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

    let layer_opts = make_opts(WindowKind::LayerShell(LayerShellOptions {
        namespace: "rustpods-zcode-ask-popup".to_string(),
        layer: Layer::Overlay,
        anchor: LayerAnchor::BOTTOM,
        exclusive_zone: None,
        exclusive_edge: None,
        margin: Some((px(0.), px(0.), px(28.), px(0.))),
        keyboard_interactivity: KeyboardInteractivity::None,
    }));

    let state_for_popup = app_state.clone();
    let slot_for_popup = slot.clone();
    let result = cx.open_window(layer_opts, move |window, cx| {
        cx.new(|cx| AskPopupWindow::new(state_for_popup, window, slot_for_popup, cx))
    });

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

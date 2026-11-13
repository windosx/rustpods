use crate::core::models::AirPodsModel;
use crate::ui::state::RustPodsState;
use crate::ui::theme::{palette, ApplePalette};
use gpui_kit::component::Icon;
use gpui_kit::layer_shell::{
    Anchor as LayerAnchor, KeyboardInteractivity, Layer, LayerShellOptions,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    canvas, div, ease_in_out, ease_out_quint, px, size, Animation, AnimationExt as _,
    AnyWindowHandle, App, AppContext as _, Bounds, Context, Corners, Entity, FontWeight,
    ImgResourceLoader, InteractiveElement as _, IntoElement, ObjectFit, ParentElement as _,
    Render, Resource, StatefulInteractiveElement as _, Styled as _, Subscription, Window,
    WindowBackgroundAppearance, WindowBounds, WindowDecorations, WindowHandle, WindowKind,
    WindowOptions,
};
use std::time::{Duration, Instant};
use tracing::{info, warn};

const POPUP_AUTO_DISMISS: Duration = Duration::from_secs(10);
const POPUP_CLOSE_ANIM_MS: u64 = 240;

pub fn model_animation_asset(model: AirPodsModel) -> (&'static str, bool) {
    match model {
        AirPodsModel::AirPods1 => ("animations/airpods_1.webp", false),
        AirPodsModel::AirPods2 => ("animations/airpods_2.webp", false),
        AirPodsModel::AirPods3 => ("animations/airpods_3.webp", false),
        AirPodsModel::AirPods4 | AirPodsModel::AirPods4Anc => ("animations/airpods_4.webp", false),
        AirPodsModel::AirPodsPro1 => ("animations/airpods_pro.webp", false),
        AirPodsModel::AirPodsPro2Lightning | AirPodsModel::AirPodsPro2UsbC => {
            ("animations/airpods_pro_2.webp", false)
        }
        AirPodsModel::AirPodsMax | AirPodsModel::AirPodsMaxUsbC => {
            ("animations/airpods_max.webp", true)
        }
        AirPodsModel::Unknown(_) => ("animations/airpods_3.webp", false),
    }
}

pub struct CasePopupWindow {
    app_state: Entity<RustPodsState>,
    started_at: Instant,
    closing: bool,
    anim_gen: u64,
    auto_close_gen: u64,
    _obs: Subscription,
}

impl CasePopupWindow {
    pub fn new(
        app_state: Entity<RustPodsState>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let obs = cx.observe(&app_state, |_, _, cx| {
            cx.notify();
        });

        let mut this = Self {
            app_state,
            started_at: Instant::now(),
            closing: false,
            anim_gen: 0,
            auto_close_gen: 0,
            _obs: obs,
        };
        this.schedule_auto_close(window, cx);
        this.start_frame_ticker(window, cx);
        this
    }

    /// Drives the 25 fps WebP rotation explicitly because GPUI's `img()` pauses
    /// animated images whenever `window.is_window_active()` is false (which is
    /// always true for a non-focus-stealing `LayerShell` overlay).
    fn start_frame_ticker(&self, window: &mut Window, cx: &mut Context<Self>) {
        cx.spawn_in(window, async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(40))
                    .await;
                let keep_going = this
                    .update(cx, |view, cx| {
                        cx.notify();
                        !view.closing
                    })
                    .unwrap_or(false);
                if !keep_going {
                    break;
                }
            }
        })
        .detach();
    }

    pub fn schedule_auto_close(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.auto_close_gen = self.auto_close_gen.wrapping_add(1);
        let expected = self.auto_close_gen;
        cx.spawn_in(window, async move |this, cx| {
            cx.background_executor().timer(POPUP_AUTO_DISMISS).await;
            let _ = this.update_in(cx, |view, window, cx| {
                if view.auto_close_gen == expected && !view.closing {
                    view.start_close(window, cx);
                }
            });
        })
        .detach();
    }

    pub fn start_close(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.closing {
            return;
        }
        self.closing = true;
        self.anim_gen = self.anim_gen.wrapping_add(1);
        cx.notify();

        cx.spawn_in(window, async move |_this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(POPUP_CLOSE_ANIM_MS))
                .await;
            let _ = cx.update(|window, _| {
                window.remove_window();
            });
        })
        .detach();
    }
}

impl Render for CasePopupWindow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = palette(cx);
        let state = self.app_state.read(cx);
        let dev = state.device.clone();
        let (asset_path, is_max) = model_animation_asset(dev.model);
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

        let (img_w, img_h) = if is_max { (186., 202.) } else { (336., 168.) };
        let elapsed_ms = self.started_at.elapsed().as_millis() as usize;

        let _ = window;
        let anim = if closing {
            Animation::new(Duration::from_millis(POPUP_CLOSE_ANIM_MS)).with_easing(ease_in_out)
        } else {
            Animation::new(Duration::from_millis(380)).with_easing(ease_out_quint())
        };

        div()
            .size_full()
            .flex()
            .flex_col()
            .justify_center()
            .items_center()
            .p(px(12.))
            .child(
                div()
                    .w(px(388.))
                    .rounded(px(28.))
                    .bg(card_bg)
                    .border_1()
                    .border_color(card_border)
                    .shadow_lg()
                    .px(px(20.))
                    .pt(px(16.))
                    .pb(px(20.))
                    .flex()
                    .flex_col()
                    .items_center()
                    .gap(px(10.))
                    .child(
                        div()
                            .relative()
                            .w_full()
                            .flex()
                            .flex_col()
                            .items_center()
                            .justify_center()
                            .pt(px(2.))
                            .child(
                                div()
                                    .max_w(px(290.))
                                    .text_size(px(20.))
                                    .font_weight(FontWeight::BOLD)
                                    .text_color(p.text_primary)
                                    .child(dev.name.clone()),
                            )
                            .child(
                                div()
                                    .text_size(px(11.5))
                                    .text_color(p.text_secondary)
                                    .mt(px(2.))
                                    .child(if dev.is_connected {
                                        format!("{} · 已连接", dev.capabilities.name)
                                    } else {
                                        format!("{} · 已就绪", dev.capabilities.name)
                                    }),
                            )
                            .child({
                                let view_c = cx.entity().clone();
                                div()
                                    .id("case-popup-close-btn")
                                    .absolute()
                                    .top(px(0.))
                                    .right(px(0.))
                                    .w(px(28.))
                                    .h(px(28.))
                                    .rounded(px(999.))
                                    .bg(close_btn_bg)
                                    .hover(|s| s.bg(close_btn_hover))
                                    .cursor_pointer()
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .on_click(move |_, window, cx| {
                                        view_c.update(cx, |v, cx| v.start_close(window, cx));
                                    })
                                    .child(
                                        Icon::new(gpui_kit::component::IconName::Close)
                                            .size(px(14.))
                                            .text_color(p.text_secondary),
                                    )
                            }),
                    )
                    .child(
                        div()
                            .w_full()
                            .h(px(if is_max { 206. } else { 174. }))
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(
                                canvas(
                                    move |bounds: Bounds<gpui_kit::Pixels>,
                                          window: &mut Window,
                                          cx: &mut App| {
                                        let loaded = window
                                            .use_asset::<ImgResourceLoader>(
                                                &Resource::Embedded(asset_path.into()),
                                                cx,
                                            )
                                            .and_then(|res| res.ok());
                                        (bounds, loaded)
                                    },
                                    move |_bounds,
                                          (bounds, loaded): (
                                        Bounds<gpui_kit::Pixels>,
                                        Option<std::sync::Arc<gpui_kit::RenderImage>>,
                                    ),
                                          window: &mut Window,
                                          _cx: &mut App| {
                                        let Some(data) = loaded else {
                                            return;
                                        };
                                        let count = data.frame_count();
                                        if count == 0 {
                                            return;
                                        }
                                        // 40 ms per frame (25 fps WebP)
                                        let frame_idx = (elapsed_ms / 40) % count;
                                        let new_bounds = ObjectFit::Contain
                                            .get_bounds(bounds, data.size(frame_idx));
                                        let _ = window.paint_image(
                                            bounds,
                                            new_bounds,
                                            Corners::default(),
                                            data,
                                            frame_idx,
                                            false,
                                        );
                                    },
                                )
                                .w(px(img_w))
                                .h(px(img_h)),
                            ),
                    )
                    .child(if is_max {
                        let level = dev.battery_left.or(dev.battery_right);
                        let charging = dev.battery_left_charging || dev.battery_right_charging;
                        div()
                            .w_full()
                            .flex()
                            .justify_center()
                            .child(render_popup_battery_cell(&rust_i18n::t!("popup.headphones"), level, charging, &p))
                    } else {
                        div()
                            .w_full()
                            .px(px(8.))
                            .flex()
                            .flex_row()
                            .items_center()
                            .justify_around()
                            .gap(px(8.))
                            .child(render_popup_battery_cell(
                                &rust_i18n::t!("popup.left_ear"),
                                dev.battery_left,
                                dev.battery_left_charging,
                                &p,
                            ))
                            .child(render_popup_battery_cell(
                                &rust_i18n::t!("popup.right_ear"),
                                dev.battery_right,
                                dev.battery_right_charging,
                                &p,
                            ))
                            .child(render_popup_battery_cell(
                                &rust_i18n::t!("popup.case"),
                                dev.battery_case,
                                dev.battery_case_charging,
                                &p,
                            ))
                    })
                    .relative()
                    .with_animation(("case-popup-sheet", anim_gen), anim, move |el, t| {
                        let (dy, alpha) = if closing {
                            (t * 18.0, (1.0 - t).max(0.0))
                        } else {
                            ((1.0 - t) * 18.0, (t * 1.35).min(1.0))
                        };
                        el.top(px(dy)).opacity(alpha)
                    }),
            )
    }
}

fn render_popup_battery_cell(
    label: &str,
    level: Option<u8>,
    charging: bool,
    p: &ApplePalette,
) -> impl IntoElement {
    let pct_text = level
        .map(|v| format!("{}%", v))
        .unwrap_or_else(|| "--".to_string());
    let fill_color = if charging {
        p.accent_green
    } else {
        match level {
            Some(v) if v > 40 => p.accent_green,
            Some(v) if v > 20 => p.accent_amber,
            Some(_) => p.accent_red,
            None => p.text_muted,
        }
    };
    let fill_ratio = level.map(|v| (v as f32 / 100.0).clamp(0.08, 1.0)).unwrap_or(0.0);
    let inner_w = 22.0 * fill_ratio;

    div()
        .flex_1()
        .min_w(px(82.))
        .py(px(8.))
        .px(px(6.))
        .rounded(px(14.))
        .bg(if p.is_dark {
            gpui_kit::rgba(0xFFFFFF0D)
        } else {
            gpui_kit::rgba(0x00000008)
        })
        .flex()
        .flex_col()
        .items_center()
        .gap(px(5.))
        .child(
            div()
                .text_size(px(11.5))
                .font_weight(FontWeight::MEDIUM)
                .text_color(p.text_secondary)
                .child(label.to_string()),
        )
        // Mini iOS battery glyph
        .child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap(px(1.5))
                .child(
                    div()
                        .w(px(26.))
                        .h(px(13.))
                        .p(px(2.))
                        .rounded(px(4.))
                        .border_1()
                        .border_color(if p.is_dark {
                            gpui_kit::rgba(0xFFFFFF44)
                        } else {
                            gpui_kit::rgba(0x00000033)
                        })
                        .flex()
                        .items_center()
                        .when(level.is_some(), |this| {
                            this.child(
                                div()
                                    .w(px(inner_w))
                                    .h(px(7.))
                                    .rounded(px(2.))
                                    .bg(fill_color),
                            )
                        }),
                )
                .child(
                    div()
                        .w(px(2.))
                        .h(px(5.))
                        .rounded(px(1.))
                        .bg(if p.is_dark {
                            gpui_kit::rgba(0xFFFFFF44)
                        } else {
                            gpui_kit::rgba(0x00000033)
                        }),
                ),
        )
        .child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap(px(3.))
                .child(
                    div()
                        .text_size(px(15.))
                        .font_weight(FontWeight::BOLD)
                        .text_color(p.text_primary)
                        .child(pct_text),
                )
                .when(charging, |this| {
                    this.child(
                        Icon::new(gpui_kit::assets::IconName::Zap)
                            .size(px(12.))
                            .text_color(p.accent_green),
                    )
                }),
        )
}

pub fn open_or_refresh_case_popup(
    app_state: &Entity<RustPodsState>,
    slot: &std::sync::Arc<std::sync::Mutex<Option<WindowHandle<CasePopupWindow>>>>,
    cx: &mut App,
) {
    if let Ok(mut guard) = slot.lock() {
        if let Some(handle) = guard.as_ref() {
            if handle
                .update(cx, |view, window, cx| {
                    let was_closing = view.closing;
                    view.closing = false;
                    view.schedule_auto_close(window, cx);
                    if was_closing {
                        view.anim_gen = view.anim_gen.wrapping_add(1);
                        view.start_frame_ticker(window, cx);
                    }
                    cx.notify();
                })
                .is_ok()
            {
                return;
            }
            *guard = None;
        }
    }

    let popup_size = size(px(416.), px(370.));
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
        namespace: "rustpods-popup".to_string(),
        layer: Layer::Overlay,
        anchor: LayerAnchor::empty(),
        exclusive_zone: None,
        exclusive_edge: None,
        margin: None,
        keyboard_interactivity: KeyboardInteractivity::None,
    }));

    let state_for_popup = app_state.clone();
    let result = cx.open_window(layer_opts, move |window, cx| {
        cx.new(|cx| CasePopupWindow::new(state_for_popup, window, cx))
    });

    let handle: Option<WindowHandle<CasePopupWindow>> = match result {
        Ok(h) => {
            info!("Opened iOS case popup via Wayland LayerShell");
            Some(h)
        }
        Err(e) => {
            warn!("LayerShell popup unavailable ({}), falling back to PopUp window", e);
            let state_fallback = app_state.clone();
            match cx.open_window(make_opts(WindowKind::PopUp), move |window, cx| {
                cx.new(|cx| CasePopupWindow::new(state_fallback, window, cx))
            }) {
                Ok(h) => Some(h),
                Err(e2) => {
                    warn!("Failed to open fallback PopUp window: {}", e2);
                    None
                }
            }
        }
    };

    if let (Some(h), Ok(mut guard)) = (handle, slot.lock()) {
        *guard = Some(h);
    }
}

pub fn close_case_popup(
    slot: &std::sync::Arc<std::sync::Mutex<Option<WindowHandle<CasePopupWindow>>>>,
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

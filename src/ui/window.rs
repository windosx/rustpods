use crate::core::aacp::commands::MicrophoneMode;
use crate::system::config::PauseBehavior;
use crate::ui::components::{
    agents::render_agents_page, battery::render_battery_card, headtracking::render_head_tracking_card,
    hero::render_hero_card, noise::render_noise_control_card,
    preferences::render_preferences_page, settings::render_settings_page,
    zcode_ask::render_zcode_ask_card,
};
use crate::ui::state::{NavPage, RustPodsState};
use crate::ui::theme::{apply_apple_theme, palette};
use gpui_kit::component::button::{Button, ButtonRounded, ButtonVariants as _};
use gpui_kit::component::input::{InputEvent, InputState};
use gpui_kit::component::scroll::ScrollableElement as _;
use gpui_kit::component::select::{SelectEvent, SelectState};
use gpui_kit::component::slider::{SliderEvent, SliderState, SliderValue};
use gpui_kit::component::{Icon, IndexPath, ThemeMode, TitleBar};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    div, px, size, App, AppContext as _, Context, Entity, FontWeight, InteractiveElement as _,
    IntoElement, ParentElement as _, Render, StatefulInteractiveElement as _, Styled as _,
    Subscription, TitlebarOptions, Window, WindowBounds, WindowOptions,
};
use std::time::Duration;

pub fn window_options(cx: &App) -> WindowOptions {
    let mut opts = WindowOptions {
        window_bounds: Some(WindowBounds::centered(size(px(820.), px(730.)), cx)),
        window_min_size: Some(size(px(680.), px(560.))),
        app_id: Some("io.github.rustpods.RustPods".to_string()),
        ..TitleBar::window_options()
    };
    opts.titlebar = Some(TitlebarOptions {
        title: Some("RustPods".into()),
        ..TitleBar::title_bar_options()
    });
    opts
}

pub struct MainWindow {
    app_state: Entity<RustPodsState>,
    rename_input: Entity<InputState>,
    adaptive_slider: Entity<SliderState>,
    pause_select: Entity<SelectState<Vec<&'static str>>>,
    mic_select: Entity<SelectState<Vec<&'static str>>>,
    sensitivity_select: Entity<SelectState<Vec<&'static str>>>,
    last_synced_adaptive: u8,
    adaptive_debounce_gen: u64,
    _subscriptions: Vec<Subscription>,
}

impl MainWindow {
    pub fn new(
        app_state: Entity<RustPodsState>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let (initial_name, initial_adaptive, initial_mic, cfg) = {
            let s = app_state.read(cx);
            (
                s.device.name.clone(),
                s.device.adaptive_strength,
                s.device.mic_mode,
                s.config_snapshot(),
            )
        };

        // Intercept compositor close (Mod+Q / Alt+F4): quit only when close_to_tray is off.
        {
            let state_close = app_state.clone();
            window.on_window_should_close(cx, move |_window, cx| {
                let close_to_tray = state_close.read(cx).config_snapshot().close_to_tray;
                if !close_to_tray {
                    cx.quit();
                }
                true
            });
        }

        let rename_input = cx.new(|cx| {
            let mut input = InputState::new(window, cx).placeholder("输入 AirPods 名称");
            input.set_value(&initial_name, window, cx);
            input
        });

        let adaptive_slider = cx.new(|_| {
            SliderState::new()
                .min(0.)
                .max(100.)
                .step(5.)
                .default_value(initial_adaptive as f32)
        });

        let pause_items = vec![
            "单耳摘下即暂停 (推荐)",
            "双耳均摘下才暂停",
            "从不暂停",
        ];
        let pause_idx = match cfg.pause_behavior {
            PauseBehavior::OneRemoved => 0,
            PauseBehavior::BothRemoved => 1,
            PauseBehavior::Never => 2,
        };
        let pause_select = cx.new(|cx| {
            SelectState::new(pause_items, Some(IndexPath::default().row(pause_idx)), window, cx)
        });

        let mic_items = vec!["自动切换 (默认)", "始终左耳", "始终右耳"];
        let mic_idx = match initial_mic {
            MicrophoneMode::Automatic => 0,
            MicrophoneMode::AlwaysLeft => 1,
            MicrophoneMode::AlwaysRight => 2,
        };
        let mic_select = cx.new(|cx| {
            SelectState::new(mic_items, Some(IndexPath::default().row(mic_idx)), window, cx)
        });

        let sens_items = vec![
            "1 · 很不灵敏",
            "2 · 较不灵敏",
            "3 · 标准",
            "4 · 较灵敏",
            "5 · 很灵敏",
        ];
        let sens_idx = (cfg.gesture_sensitivity.clamp(1, 5) - 1) as usize;
        let sensitivity_select = cx.new(|cx| {
            SelectState::new(sens_items, Some(IndexPath::default().row(sens_idx)), window, cx)
        });

        let mut subscriptions = Vec::new();

        // Re-render window when background state changes, and keep slider synced.
        subscriptions.push(cx.observe_in(&app_state, window, |this, state, window, cx| {
            let ext_adaptive = state.read(cx).device.adaptive_strength;
            if ext_adaptive != this.last_synced_adaptive {
                this.last_synced_adaptive = ext_adaptive;
                this.adaptive_slider.update(cx, |s, cx| {
                    s.set_value(ext_adaptive as f32, window, cx);
                });
            }
            cx.notify();
        }));

        // Rename submit on Enter key.
        {
            let app_enter = app_state.clone();
            subscriptions.push(cx.subscribe(
                &rename_input,
                move |_this, input, event: &InputEvent, cx| {
                    if let InputEvent::PressEnter { .. } = event {
                        let new_name = input.read(cx).value().trim().to_string();
                        if !new_name.is_empty() {
                            app_enter.update(cx, |s, cx| {
                                s.device.name = new_name.clone();
                                s.controller.rename(new_name);
                                cx.notify();
                            });
                        }
                    }
                },
            ));
        }

        // Adaptive noise slider: update display immediately, coalesce AACP writes by 150 ms.
        {
            let app_slider = app_state.clone();
            subscriptions.push(cx.subscribe(
                &adaptive_slider,
                move |this, _slider, event: &SliderEvent, cx| {
                    let raw = match event {
                        SliderEvent::Change(SliderValue::Single(v))
                        | SliderEvent::Release(SliderValue::Single(v)) => *v,
                        _ => return,
                    };
                    let snapped = ((raw / 5.0).round() as u8).min(20) * 5;
                    this.last_synced_adaptive = snapped;
                    app_slider.update(cx, |s, cx| {
                        s.device.adaptive_strength = snapped;
                        cx.notify();
                    });

                    this.adaptive_debounce_gen = this.adaptive_debounce_gen.wrapping_add(1);
                    let expected_gen = this.adaptive_debounce_gen;
                    let app_send = app_slider.clone();
                    cx.spawn(async move |this, cx| {
                        cx.background_executor()
                            .timer(Duration::from_millis(150))
                            .await;
                        let should_send = this
                            .read_with(cx, |view, _| view.adaptive_debounce_gen == expected_gen)
                            .unwrap_or(false);
                        if should_send {
                            let _ = app_send.read_with(cx, |s, _| {
                                s.controller.set_adaptive_strength(snapped);
                            });
                        }
                    })
                    .detach();
                },
            ));
        }

        // Pause behavior selector.
        {
            let app_pause = app_state.clone();
            subscriptions.push(cx.subscribe(
                &pause_select,
                move |_this, select, event: &SelectEvent<Vec<&'static str>>, cx| {
                    let SelectEvent::Confirm(_) = event;
                    let row = select.read(cx).selected_index(cx).map(|p| p.row).unwrap_or(0);
                    let behavior = match row {
                        1 => PauseBehavior::BothRemoved,
                        2 => PauseBehavior::Never,
                        _ => PauseBehavior::OneRemoved,
                    };
                    app_pause.update(cx, |s, cx| {
                        s.controller.change_pause_behavior(behavior);
                        cx.notify();
                    });
                },
            ));
        }

        // Microphone mode selector.
        {
            let app_mic = app_state.clone();
            subscriptions.push(cx.subscribe(
                &mic_select,
                move |_this, select, event: &SelectEvent<Vec<&'static str>>, cx| {
                    let SelectEvent::Confirm(_) = event;
                    let row = select.read(cx).selected_index(cx).map(|p| p.row).unwrap_or(0);
                    let mode = match row {
                        1 => MicrophoneMode::AlwaysLeft,
                        2 => MicrophoneMode::AlwaysRight,
                        _ => MicrophoneMode::Automatic,
                    };
                    app_mic.update(cx, |s, cx| {
                        s.device.mic_mode = mode;
                        s.controller.change_mic_mode(mode);
                        cx.notify();
                    });
                },
            ));
        }

        // Gesture sensitivity selector.
        {
            let app_sens = app_state.clone();
            subscriptions.push(cx.subscribe(
                &sensitivity_select,
                move |_this, select, event: &SelectEvent<Vec<&'static str>>, cx| {
                    let SelectEvent::Confirm(_) = event;
                    let row = select.read(cx).selected_index(cx).map(|p| p.row).unwrap_or(2);
                    let level = (row as u8) + 1;
                    app_sens.update(cx, |s, cx| {
                        s.controller.set_gesture_sensitivity(level);
                        cx.notify();
                    });
                },
            ));
        }

        Self {
            app_state,
            rename_input,
            adaptive_slider,
            pause_select,
            mic_select,
            sensitivity_select,
            last_synced_adaptive: initial_adaptive,
            adaptive_debounce_gen: 0,
            _subscriptions: subscriptions,
        }
    }

    fn nav_icon(page: NavPage) -> gpui_kit::assets::IconName {
        match page {
            NavPage::Overview => gpui_kit::assets::IconName::Headphones,
            NavPage::Spatial => gpui_kit::assets::IconName::Compass,
            NavPage::Controls => gpui_kit::assets::IconName::SlidersHorizontal,
            NavPage::Agents => gpui_kit::assets::IconName::Bot,
            NavPage::Preferences => gpui_kit::assets::IconName::Settings,
        }
    }
}

impl Render for MainWindow {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = palette(cx);
        let state = self.app_state.read(cx);
        let active_page = state.active_page;
        let is_connected = state.device.is_connected;
        let is_dark = state.theme_mode.is_dark();

        let app_for_close = self.app_state.clone();
        let app_for_theme = self.app_state.clone();

        let sidebar = div()
            .w(px(210.))
            .h_full()
            .flex_shrink_0()
            .bg(p.sidebar_bg)
            .border_r_1()
            .border_color(p.sidebar_border)
            .px(px(10.))
            .py(px(14.))
            .flex()
            .flex_col()
            .justify_between()
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(4.))
                    .children(NavPage::ALL.into_iter().map(|page| {
                        let active = active_page == page;
                        let app_nav = self.app_state.clone();
                        div()
                            .id(("nav-item", page as usize))
                            .cursor_pointer()
                            .px(px(12.))
                            .py(px(9.))
                            .rounded(px(10.))
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap(px(10.))
                            .bg(if active {
                                p.sidebar_item_active
                            } else {
                                gpui_kit::transparent_black()
                            })
                            .text_color(if active {
                                p.sidebar_item_active_fg
                            } else {
                                p.text_primary
                            })
                            .hover(|this| {
                                if active {
                                    this
                                } else {
                                    this.bg(p.sidebar_item_hover)
                                }
                            })
                            .on_click(move |_, _, cx| {
                                app_nav.update(cx, |s, cx| {
                                    s.active_page = page;
                                    cx.notify();
                                });
                            })
                            .child(
                                Icon::new(Self::nav_icon(page))
                                    .size(px(17.))
                                    .text_color(if active {
                                        p.sidebar_item_active_fg
                                    } else {
                                        p.text_secondary
                                    }),
                            )
                            .child(
                                div()
                                    .text_size(px(13.5))
                                    .font_weight(if active {
                                        FontWeight::SEMIBOLD
                                    } else {
                                        FontWeight::MEDIUM
                                    })
                                    .child(page.label()),
                            )
                    })),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(8.))
                    .pt(px(10.))
                    .border_t_1()
                    .border_color(p.separator)
                    .child(
                        div()
                            .px(px(8.))
                            .flex()
                            .flex_row()
                            .items_center()
                            .justify_between()
                            .child(
                                div()
                                    .flex()
                                    .flex_row()
                                    .items_center()
                                    .gap(px(6.))
                                    .child(
                                        div()
                                            .w(px(7.))
                                            .h(px(7.))
                                            .rounded(px(999.))
                                            .bg(if is_connected {
                                                p.accent_green
                                            } else {
                                                p.text_muted
                                            }),
                                    )
                                    .child(
                                        div()
                                            .text_size(px(11.5))
                                            .text_color(p.text_secondary)
                                            .child(if is_connected {
                                                "AACP 已连接"
                                            } else {
                                                "等待连接"
                                            }),
                                    ),
                            )
                            .child(
                                Button::new("sidebar-theme-toggle")
                                    .ghost()
                                    .compact()
                                    .rounded(ButtonRounded::Size(px(999.)))
                                    .tooltip(if is_dark {
                                        "切换到浅色模式"
                                    } else {
                                        "切换到深色模式"
                                    })
                                    .icon(Icon::new(if is_dark {
                                        gpui_kit::component::IconName::Sun
                                    } else {
                                        gpui_kit::component::IconName::Moon
                                    }))
                                    .on_click(move |_, _, cx| {
                                        let next = if is_dark {
                                            ThemeMode::Light
                                        } else {
                                            ThemeMode::Dark
                                        };
                                        apply_apple_theme(next, cx);
                                        app_for_theme.update(cx, |s, cx| {
                                            s.theme_mode = next;
                                            cx.notify();
                                        });
                                    }),
                            ),
                    ),
            );

        let page_content = match active_page {
            NavPage::Overview => div()
                .w_full()
                .flex()
                .flex_col()
                .gap(px(14.))
                .child(render_hero_card(
                    state,
                    &self.app_state,
                    &self.rename_input,
                    &p,
                ))
                .child(render_battery_card(&state.device, &p))
                .child(render_noise_control_card(
                    state,
                    &self.app_state,
                    &self.adaptive_slider,
                    &p,
                    cx,
                ))
                .into_any_element(),

            NavPage::Spatial => div()
                .w_full()
                .flex()
                .flex_col()
                .gap(px(14.))
                .child(render_head_tracking_card(&state.head_tracking, &p))
                .when_some(
                    render_zcode_ask_card(
                        state.zcode_progress.as_ref(),
                        state.zcode_resolved.as_deref(),
                        &p,
                    ),
                    |this, card| this.child(card),
                )
                .into_any_element(),

            NavPage::Controls => render_settings_page(
                state,
                &self.app_state,
                &self.pause_select,
                &self.mic_select,
                &p,
            )
            .into_any_element(),

            NavPage::Agents => render_agents_page(
                state,
                &self.app_state,
                &self.sensitivity_select,
                &p,
            )
            .into_any_element(),

            NavPage::Preferences => render_preferences_page(state, &self.app_state, &p)
                .into_any_element(),
        };

        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(p.window_bg)
            .text_color(p.text_primary)
            .child(
                TitleBar::new()
                    .on_close_window(move |_, window, cx| {
                        let close_to_tray =
                            app_for_close.read(cx).config_snapshot().close_to_tray;
                        if close_to_tray {
                            window.remove_window();
                        } else {
                            cx.quit();
                        }
                    })
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap(px(8.))
                            .child(
                                Icon::new(gpui_kit::assets::IconName::Headphones)
                                    .size(px(14.))
                                    .text_color(p.accent_blue),
                            )
                            .child(
                                div()
                                    .text_size(px(13.))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(p.text_primary)
                                    .child("RustPods"),
                            ),
                    ),
            )
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .w_full()
                    .flex()
                    .flex_row()
                    .child(sidebar)
                    .child(
                        div()
                            .flex_1()
                            .h_full()
                            .min_w_0()
                            .overflow_y_scrollbar()
                            .child(
                                div()
                                    .w_full()
                                    .max_w(px(680.))
                                    .mx_auto()
                                    .p(px(20.))
                                    .pb(px(28.))
                                    .child(page_content),
                            ),
                    ),
            )
    }
}

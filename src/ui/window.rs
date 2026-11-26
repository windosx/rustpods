use crate::core::aacp::commands::MicrophoneMode;
use crate::system::config::PauseBehavior;
use crate::ui::state::RustPodsController;
use crate::ui::components::{
    agents::render_agents_page, battery::render_battery_card, head_mouse::render_head_mouse_page,
    headtracking::render_head_tracking_card, hero::render_hero_card, noise::render_noise_control_card,
    preferences::render_preferences_page, settings::render_settings_page,
};
use crate::ui::state::{NavPage, RustPodsState};
use crate::ui::theme::{apply_apple_theme, palette};
use gpui_kit::component::button::{Button, ButtonRounded, ButtonVariants as _};
use gpui_kit::component::input::{InputEvent, InputState};
use gpui_kit::component::scroll::ScrollableElement as _;
use gpui_kit::component::select::{SelectEvent, SelectState};
use gpui_kit::component::slider::{SliderEvent, SliderState, SliderValue};
use gpui_kit::component::{Icon, IndexPath, ThemeMode, TitleBar};
use gpui_kit::{
    div, px, size, App, AppContext as _, Context, Entity, FontWeight, InteractiveElement as _,
    IntoElement, ParentElement as _, Render, StatefulInteractiveElement as _, Styled as _,
    Subscription, TitlebarOptions, Window, WindowBounds, WindowOptions,
};
use std::time::Duration;

/// Coalesces slider drags into at most one config write per 150 ms, the same
/// debounce the adaptive-strength slider uses.
const MOUSE_SLIDER_DEBOUNCE_MS: u64 = 150;

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

fn get_pause_items() -> Vec<String> {
    vec![
        rust_i18n::t!("controls.pause_one").to_string(),
        rust_i18n::t!("controls.pause_both").to_string(),
        rust_i18n::t!("controls.pause_never").to_string(),
    ]
}

fn get_mic_items() -> Vec<String> {
    vec![
        rust_i18n::t!("controls.mic_auto").to_string(),
        rust_i18n::t!("controls.mic_left").to_string(),
        rust_i18n::t!("controls.mic_right").to_string(),
    ]
}

fn get_sens_items() -> Vec<String> {
    vec![
        rust_i18n::t!("agents.sens_1").to_string(),
        rust_i18n::t!("agents.sens_2").to_string(),
        rust_i18n::t!("agents.sens_3").to_string(),
        rust_i18n::t!("agents.sens_4").to_string(),
        rust_i18n::t!("agents.sens_5").to_string(),
    ]
}

fn get_mode_items() -> Vec<String> {
    vec![
        rust_i18n::t!("head_mouse.mode_relative").to_string(),
        rust_i18n::t!("head_mouse.mode_absolute").to_string(),
    ]
}

fn get_language_items() -> Vec<String> {
    crate::system::config::AppLanguage::ALL
        .iter()
        .map(|lang| lang.localized_label())
        .collect()
}

pub struct MainWindow {
    app_state: Entity<RustPodsState>,
    rename_input: Entity<InputState>,
    adaptive_slider: Entity<SliderState>,
    pause_select: Entity<SelectState<Vec<String>>>,
    mic_select: Entity<SelectState<Vec<String>>>,
    sensitivity_select: Entity<SelectState<Vec<String>>>,
    mouse_mode_select: Entity<SelectState<Vec<String>>>,
    language_select: Entity<SelectState<Vec<String>>>,
    mouse_speed_slider: Entity<SliderState>,
    mouse_accel_slider: Entity<SliderState>,
    mouse_smooth_slider: Entity<SliderState>,
    mouse_deadzone_slider: Entity<SliderState>,
    last_synced_adaptive: u8,
    last_synced_language: crate::system::config::AppLanguage,
    adaptive_debounce_gen: u64,
    /// Debounce generations for the four head-mouse sliders (speed, accel,
    /// smoothing, deadzone). These sliders had no event subscription at all
    /// once, so dragging them moved the knob without ever touching the config.
    mouse_slider_gens: [u64; 4],
    _subscriptions: Vec<Subscription>,
    _select_subscriptions: Vec<Subscription>,
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
                    #[cfg(target_os = "windows")]
                    crate::system::tray::close_windows_tray();
                    cx.quit();
                }
                true
            });
        }

        let rename_input = cx.new(|cx| {
            let mut input = InputState::new(window, cx).placeholder(rust_i18n::t!("shell.rename_placeholder").as_ref());
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

        let pause_items = get_pause_items();
        let pause_idx = match cfg.pause_behavior {
            PauseBehavior::OneRemoved => 0,
            PauseBehavior::BothRemoved => 1,
            PauseBehavior::Never => 2,
        };
        let pause_select = cx.new(|cx| {
            SelectState::new(pause_items, Some(IndexPath::default().row(pause_idx)), window, cx)
        });

        let mic_items = get_mic_items();
        let mic_idx = match initial_mic {
            MicrophoneMode::Automatic => 0,
            MicrophoneMode::AlwaysLeft => 1,
            MicrophoneMode::AlwaysRight => 2,
        };
        let mic_select = cx.new(|cx| {
            SelectState::new(mic_items, Some(IndexPath::default().row(mic_idx)), window, cx)
        });

        let sens_items = get_sens_items();
        let sens_idx = (cfg.gesture_sensitivity.clamp(1, 5) - 1) as usize;
        let sensitivity_select = cx.new(|cx| {
            SelectState::new(sens_items, Some(IndexPath::default().row(sens_idx)), window, cx)
        });

        let mode_items = get_mode_items();
        let mode_idx = match cfg.head_mouse_mode {
            crate::system::config::HeadMouseMode::Relative => 0,
            crate::system::config::HeadMouseMode::Absolute => 1,
        };
        let mouse_mode_select = cx.new(|cx| {
            SelectState::new(mode_items, Some(IndexPath::default().row(mode_idx)), window, cx)
        });

        let lang_items = get_language_items();
        let lang_idx = crate::system::config::AppLanguage::ALL
            .iter()
            .position(|&l| l == cfg.language)
            .unwrap_or(0);
        let language_select = cx.new(|cx| {
            SelectState::new(lang_items, Some(IndexPath::default().row(lang_idx)), window, cx)
        });

        let mouse_speed_slider = cx.new(|_| {
            SliderState::new()
                .min(5.)
                .max(60.)
                .step(1.)
                .default_value(cfg.head_mouse_speed)
        });

        let mouse_accel_slider = cx.new(|_| {
            SliderState::new()
                .min(0.)
                .max(3.)
                .step(0.1)
                .default_value(cfg.head_mouse_accel)
        });

        let mouse_smooth_slider = cx.new(|_| {
            SliderState::new()
                .min(0.)
                .max(1.)
                .step(0.05)
                .default_value(cfg.head_mouse_smoothing)
        });

        let mouse_deadzone_slider = cx.new(|_| {
            SliderState::new()
                .min(0.5)
                .max(5.)
                .step(0.25)
                .default_value(cfg.head_mouse_deadzone)
        });

        let mut subscriptions = Vec::new();

        // Re-render window when background state changes, and keep slider/language synced.
        subscriptions.push(cx.observe_in(&app_state, window, |this, state, window, cx| {
            let (ext_adaptive, ext_lang) = {
                let s = state.read(cx);
                (s.device.adaptive_strength, s.config_snapshot().language)
            };
            if ext_adaptive != this.last_synced_adaptive {
                this.last_synced_adaptive = ext_adaptive;
                this.adaptive_slider.update(cx, |s, cx| {
                    s.set_value(ext_adaptive as f32, window, cx);
                });
            }
            if ext_lang != this.last_synced_language {
                this.last_synced_language = ext_lang;
                this.rebuild_selects(window, cx);
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
                            app_send.read_with(cx, |s, _| {
                                s.controller.set_adaptive_strength(snapped);
                            });
                        }
                    })
                    .detach();
                },
            ));
        }

        // The slider entities move into `Self` below; the subscriptions need
        // their own clones.
        let speed_slider_sub = mouse_speed_slider.clone();
        let accel_slider_sub = mouse_accel_slider.clone();
        let smooth_slider_sub = mouse_smooth_slider.clone();
        let deadzone_slider_sub = mouse_deadzone_slider.clone();
        let app_state_sub = app_state.clone();

        let mut this = Self {
            app_state,
            rename_input,
            adaptive_slider,
            pause_select,
            mic_select,
            sensitivity_select,
            mouse_mode_select,
            language_select,
            mouse_speed_slider,
            mouse_accel_slider,
            mouse_smooth_slider,
            mouse_deadzone_slider,
            last_synced_adaptive: initial_adaptive,
            last_synced_language: cfg.language,
            adaptive_debounce_gen: 0,
            mouse_slider_gens: [0; 4],
            _subscriptions: subscriptions,
            _select_subscriptions: Vec::new(),
        };
        this.bind_mouse_slider(
            &speed_slider_sub,
            0,
            &app_state_sub,
            cx,
            RustPodsController::set_head_mouse_speed,
        );
        this.bind_mouse_slider(
            &accel_slider_sub,
            1,
            &app_state_sub,
            cx,
            RustPodsController::set_head_mouse_accel,
        );
        this.bind_mouse_slider(
            &smooth_slider_sub,
            2,
            &app_state_sub,
            cx,
            RustPodsController::set_head_mouse_smoothing,
        );
        this.bind_mouse_slider(
            &deadzone_slider_sub,
            3,
            &app_state_sub,
            cx,
            RustPodsController::set_head_mouse_deadzone,
        );
        this.subscribe_selects(cx);
        this
    }

    /// Wires one head-mouse slider to its config setter with the shared
    /// debounce: only the latest drag position within the window is written.
    fn bind_mouse_slider(
        &mut self,
        slider: &Entity<SliderState>,
        idx: usize,
        app_state: &Entity<RustPodsState>,
        cx: &mut Context<Self>,
        apply: fn(&RustPodsController, f32),
    ) {
        let app = app_state.clone();
        self._subscriptions.push(cx.subscribe(
            slider,
            move |this, _slider, event: &SliderEvent, cx| {
                let raw = match event {
                    SliderEvent::Change(SliderValue::Single(v))
                    | SliderEvent::Release(SliderValue::Single(v)) => *v,
                    _ => return,
                };
                this.mouse_slider_gens[idx] = this.mouse_slider_gens[idx].wrapping_add(1);
                let expected_gen = this.mouse_slider_gens[idx];
                let app = app.clone();
                cx.spawn(async move |this, cx| {
                    cx.background_executor()
                        .timer(Duration::from_millis(MOUSE_SLIDER_DEBOUNCE_MS))
                        .await;
                    let due = this
                        .read_with(cx, |view, _| view.mouse_slider_gens[idx] == expected_gen)
                        .unwrap_or(false);
                    if due {
                        app.read_with(cx, |s, _| apply(&s.controller, raw));
                    }
                })
                .detach();
            },
        ));
    }

    fn subscribe_selects(&mut self, cx: &mut Context<Self>) {
        self._select_subscriptions.clear();

        // Pause behavior selector.
        {
            let app_pause = self.app_state.clone();
            self._select_subscriptions.push(cx.subscribe(
                &self.pause_select,
                move |_this, select, event: &SelectEvent<Vec<String>>, cx| {
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
            let app_mic = self.app_state.clone();
            self._select_subscriptions.push(cx.subscribe(
                &self.mic_select,
                move |_this, select, event: &SelectEvent<Vec<String>>, cx| {
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
            let app_sens = self.app_state.clone();
            self._select_subscriptions.push(cx.subscribe(
                &self.sensitivity_select,
                move |_this, select, event: &SelectEvent<Vec<String>>, cx| {
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

        // Mouse mode selector.
        {
            let app_mode = self.app_state.clone();
            self._select_subscriptions.push(cx.subscribe(
                &self.mouse_mode_select,
                move |_this, select, event: &SelectEvent<Vec<String>>, cx| {
                    let SelectEvent::Confirm(_) = event;
                    let row = select.read(cx).selected_index(cx).map(|p| p.row).unwrap_or(0);
                    let mode = match row {
                        1 => crate::system::config::HeadMouseMode::Absolute,
                        _ => crate::system::config::HeadMouseMode::Relative,
                    };
                    app_mode.update(cx, |s, cx| {
                        s.controller.set_head_mouse_mode(mode);
                        cx.notify();
                    });
                },
            ));
        }

        // Language selector.
        {
            let app_lang = self.app_state.clone();
            self._select_subscriptions.push(cx.subscribe(
                &self.language_select,
                move |_this, select, event: &SelectEvent<Vec<String>>, cx| {
                    let SelectEvent::Confirm(_) = event;
                    let row = select.read(cx).selected_index(cx).map(|p| p.row).unwrap_or(0);
                    let lang = crate::system::config::AppLanguage::ALL
                        .get(row)
                        .copied()
                        .unwrap_or(crate::system::config::AppLanguage::Auto);
                    app_lang.update(cx, |s, cx| {
                        s.controller.set_language(lang);
                        cx.notify();
                    });
                },
            ));
        }
    }

    fn rebuild_selects(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (cfg, mic) = {
            let s = self.app_state.read(cx);
            (s.config_snapshot(), s.device.mic_mode)
        };

        let pause_row = match cfg.pause_behavior {
            PauseBehavior::OneRemoved => 0,
            PauseBehavior::BothRemoved => 1,
            PauseBehavior::Never => 2,
        };
        self.pause_select = cx.new(|cx| {
            SelectState::new(get_pause_items(), Some(IndexPath::default().row(pause_row)), window, cx)
        });

        let mic_row = match mic {
            MicrophoneMode::Automatic => 0,
            MicrophoneMode::AlwaysLeft => 1,
            MicrophoneMode::AlwaysRight => 2,
        };
        self.mic_select = cx.new(|cx| {
            SelectState::new(get_mic_items(), Some(IndexPath::default().row(mic_row)), window, cx)
        });

        let sens_row = (cfg.gesture_sensitivity.clamp(1, 5) - 1) as usize;
        self.sensitivity_select = cx.new(|cx| {
            SelectState::new(get_sens_items(), Some(IndexPath::default().row(sens_row)), window, cx)
        });

        let mode_row = match cfg.head_mouse_mode {
            crate::system::config::HeadMouseMode::Relative => 0,
            crate::system::config::HeadMouseMode::Absolute => 1,
        };
        self.mouse_mode_select = cx.new(|cx| {
            SelectState::new(get_mode_items(), Some(IndexPath::default().row(mode_row)), window, cx)
        });

        let lang_row = crate::system::config::AppLanguage::ALL
            .iter()
            .position(|&l| l == cfg.language)
            .unwrap_or(0);
        self.language_select = cx.new(|cx| {
            SelectState::new(get_language_items(), Some(IndexPath::default().row(lang_row)), window, cx)
        });

        self.rename_input.update(cx, |input, cx| {
            input.set_placeholder(rust_i18n::t!("shell.rename_placeholder").as_ref(), window, cx);
        });

        self.subscribe_selects(cx);
    }

    fn nav_icon(page: NavPage) -> gpui_kit::assets::IconName {
        match page {
            NavPage::Overview => gpui_kit::assets::IconName::Headphones,
            NavPage::Spatial => gpui_kit::assets::IconName::Compass,
            NavPage::HeadMouse => gpui_kit::assets::IconName::MousePointer,
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
                                                rust_i18n::t!("shell.connected")
                                            } else {
                                                rust_i18n::t!("shell.waiting")
                                            }),
                                    ),
                            )
                            .child(
                                Button::new("sidebar-theme-toggle")
                                    .ghost()
                                    .compact()
                                    .rounded(ButtonRounded::Size(px(999.)))
                                    .tooltip(if is_dark {
                                        rust_i18n::t!("shell.toggle_light")
                                    } else {
                                        rust_i18n::t!("shell.toggle_dark")
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

        let view_handle = cx.entity().clone();
        let on_enable_request = {
            let view = view_handle.clone();
            std::rc::Rc::new(move |_window: &mut Window, cx: &mut App| {
                view.update(cx, |_this, cx| {
                    cx.spawn(async move |this, cx| {
                        let res = tokio::task::spawn_blocking(crate::system::head_mouse::grant_uinput_permission_pkexec)
                            .await
                            .unwrap_or_else(|e| Err(format!("任务执行异常: {}", e)));

                        let _ = this.update(cx, |this, cx| {
                            if res.is_ok() && crate::system::head_mouse::UinputStatus::probe().is_available() {
                                this.app_state.update(cx, |s, cx| {
                                    s.controller.toggle_head_mouse(true);
                                    cx.notify();
                                });
                            } else {
                                this.app_state.update(cx, |s, cx| {
                                    s.controller.toggle_head_mouse(false);
                                    cx.notify();
                                });
                            }
                            cx.notify();
                        });
                    })
                    .detach();
                });
            })
        };

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
                .into_any_element(),

            NavPage::HeadMouse => render_head_mouse_page(
                state,
                &self.app_state,
                &crate::ui::components::head_mouse::HeadMouseWidgets {
                    mode_select: &self.mouse_mode_select,
                    speed: &self.mouse_speed_slider,
                    accel: &self.mouse_accel_slider,
                    smoothing: &self.mouse_smooth_slider,
                    deadzone: &self.mouse_deadzone_slider,
                },
                on_enable_request,
                &p,
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

            NavPage::Preferences => render_preferences_page(
                state,
                &self.app_state,
                &self.language_select,
                &p,
            )
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
                            #[cfg(target_os = "windows")]
                            crate::system::tray::close_windows_tray();
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
                                gpui_kit::gpui::img("icons/rustpods_32.png")
                                    .size(px(16.))
                                    .rounded(px(3.)),
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

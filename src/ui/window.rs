use super::components::agents::build_agents_page;
use super::components::battery::{build_battery_card, update_battery_widgets, BatteryViewWidgets};
use super::components::headtracking::{build_head_tracking_page, HeadTrackingWidgets};
use super::components::hero::build_hero_card;
use super::components::noise::build_noise_control_card;
use super::components::preferences::build_preferences_page;
use super::components::settings::build_smart_settings_group;
use super::components::zcode_ask::{build_zcode_ask_card, ZcodeAskWidgets};
use crate::core::aacp::commands::{MicrophoneMode, NoiseControlMode};
use crate::core::device::AirPodsDeviceState;
use crate::core::models::AirPodsModel;
use crate::system::config::{AppConfig, PauseBehavior};
use gtk4::prelude::*;
use gtk4::{Align, Box, Button, Image, Label, Orientation, ScrolledWindow, Stack};
use libadwaita::prelude::*;
use libadwaita::{Application, ApplicationWindow, HeaderBar, ToolbarView};
use std::cell::Cell;
use std::sync::Arc;

/// A card that is rebuilt wholesale when the connected device's model changes.
///
/// The noise card and the settings group branch on the device model at build
/// time (ANC segmented control vs the half-in-ear banner, case-chime row vs
/// nothing), and those branches cannot be flipped on an existing widget tree.
/// The slot holds exactly one child; [`sync`](Self::sync) swaps in a fresh
/// card whenever the model differs from the one the current child was built
/// for. A real device replacing the build-time placeholder is the case that
/// matters: without this, a Pro/Max unit inherits whatever the placeholder
/// was built with, and the noise controls never appear.
pub struct RebuildableCard {
    slot: Box,
    model: Cell<Option<AirPodsModel>>,
    rebuild: std::boxed::Box<dyn Fn(&AirPodsDeviceState)>,
}

impl RebuildableCard {
    fn new(
        slot: Box,
        initial: &AirPodsDeviceState,
        build: impl Fn(&AirPodsDeviceState) -> gtk4::Widget + 'static,
    ) -> Self {
        slot.append(&build(initial));
        let rebuild_slot = slot.clone();
        Self {
            slot,
            model: Cell::new(Some(initial.capabilities.model)),
            rebuild: std::boxed::Box::new(move |state| {
                while let Some(child) = rebuild_slot.first_child() {
                    rebuild_slot.remove(&child);
                }
                rebuild_slot.append(&build(state));
            }),
        }
    }

    /// Rebuilds the card if `state`'s model differs from the current one.
    pub fn sync(&self, state: &AirPodsDeviceState) {
        let model = state.capabilities.model;
        if self.model.get() != Some(model) {
            self.model.set(Some(model));
            (self.rebuild)(state);
        }
    }
}

pub struct MainWindowHandles {
    pub window: ApplicationWindow,
    pub hero_title: Label,
    pub hero_subtitle: Label,
    pub hero_status: Label,
    pub connect_btn: Button,
    pub battery_widgets: BatteryViewWidgets,
    pub noise_card: RebuildableCard,
    pub controls_card: RebuildableCard,
    pub head_tracking: HeadTrackingWidgets,
    pub zcode_ask: ZcodeAskWidgets,
}

pub fn create_main_window<R, W, N, A, E, P, C, M, K, Y>(
    app: &Application,
    initial_state: &AirPodsDeviceState,
    config: &Arc<std::sync::RwLock<AppConfig>>,
    on_rename: R,
    on_connect: W,
    on_set_noise_mode: N,
    on_set_adaptive: A,
    on_toggle_ear: E,
    on_change_pause: P,
    on_toggle_conv: C,
    on_change_mic: M,
    on_toggle_case_chime: K,
    on_toggle_close_to_tray: Y,
) -> MainWindowHandles
where
    R: Fn(String) + Send + Sync + 'static,
    W: Fn() + Send + Sync + 'static,
    N: Fn(NoiseControlMode) + Send + Sync + 'static,
    A: Fn(u8) + Send + Sync + 'static,
    E: Fn(bool) + Send + Sync + 'static,
    P: Fn(PauseBehavior) + Send + Sync + 'static,
    C: Fn(bool) + Send + Sync + 'static,
    M: Fn(MicrophoneMode) + Send + Sync + 'static,
    K: Fn(bool) + Send + Sync + 'static,
    Y: Fn(bool) + Send + Sync + 'static,
{
    let window = ApplicationWindow::new(app);
    window.set_title(Some("RustPods"));
    window.set_default_size(780, 720);
    window.set_resizable(true);

    let toolbar_view = ToolbarView::new();
    let header_bar = HeaderBar::new();
    toolbar_view.add_top_bar(&header_bar);

    // Root horizontal split: Sidebar (Left) + Stack Pages (Right)
    let root_box = Box::new(Orientation::Horizontal, 0);
    root_box.set_vexpand(true);
    root_box.set_hexpand(true);

    // Left Navigation Sidebar
    let sidebar_box = Box::new(Orientation::Vertical, 4);
    sidebar_box.set_width_request(210);
    sidebar_box.add_css_class("sidebar");
    sidebar_box.set_margin_top(16);
    sidebar_box.set_margin_bottom(16);
    sidebar_box.set_margin_start(12);
    sidebar_box.set_margin_end(8);

    // Right Content Stack
    let stack = Stack::new();
    stack.set_transition_type(gtk4::StackTransitionType::Crossfade);
    stack.set_transition_duration(160);
    stack.set_hexpand(true);
    stack.set_vexpand(true);

    // -------------------------------------------------------------
    // Page 1: 概览首页 (Overview)
    // -------------------------------------------------------------
    let (hero_container, hero_title, hero_subtitle, hero_status, connect_btn) =
        build_hero_card(initial_state, on_rename, on_connect);

    let battery_widgets = build_battery_card(initial_state);

    // The noise card and the settings group depend on the device model, and
    // the build-time state is a placeholder. Keep them in rebuildable slots so
    // a real device with different capabilities gets its own card instead of
    // inheriting the placeholder's.
    let on_noise_mode: Arc<dyn Fn(NoiseControlMode) + Send + Sync> = Arc::new(on_set_noise_mode);
    let on_adaptive: Arc<dyn Fn(u8) + Send + Sync> = Arc::new(on_set_adaptive);

    let noise_slot = Box::new(Orientation::Vertical, 0);
    let noise_card = RebuildableCard::new(noise_slot.clone(), initial_state, {
        let n = on_noise_mode.clone();
        let a = on_adaptive.clone();
        move |state| {
            let n = n.clone();
            let a = a.clone();
            build_noise_control_card(state, move |m| n(m), move |v| a(v)).upcast()
        }
    });

    let overview_box = Box::new(Orientation::Vertical, 14);
    overview_box.set_margin_top(14);
    overview_box.set_margin_bottom(24);
    overview_box.set_margin_start(20);
    overview_box.set_margin_end(20);

    overview_box.append(&hero_container);
    overview_box.append(&battery_widgets.container);
    overview_box.append(&noise_slot);

    let overview_scrolled = ScrolledWindow::new();
    overview_scrolled.set_vexpand(true);
    overview_scrolled.set_hexpand(true);
    overview_scrolled.set_hscrollbar_policy(gtk4::PolicyType::Never);
    overview_scrolled.set_vscrollbar_policy(gtk4::PolicyType::Automatic);
    overview_scrolled.set_child(Some(&overview_box));

    stack.add_named(&overview_scrolled, Some("overview"));

    // -------------------------------------------------------------
    // Page 2: 空间音频与头部追踪 (Spatial Audio & Head Tracking)
    // -------------------------------------------------------------
    let head_tracking = build_head_tracking_page();

    // The ZCode question card lives on this page too: answering by nodding is
    // a head-tracking activity, and the compass beside it doubles as live
    // feedback for the gesture.
    let zcode_ask = build_zcode_ask_card();
    head_tracking.container.append(&zcode_ask.container);

    let tracking_scrolled = ScrolledWindow::new();
    tracking_scrolled.set_vexpand(true);
    tracking_scrolled.set_hexpand(true);
    tracking_scrolled.set_hscrollbar_policy(gtk4::PolicyType::Never);
    tracking_scrolled.set_vscrollbar_policy(gtk4::PolicyType::Automatic);
    tracking_scrolled.set_child(Some(&head_tracking.container));

    stack.add_named(&tracking_scrolled, Some("spatial"));

    // -------------------------------------------------------------
    // Page 3: 耳机控制与交互 (Controls & Gestures)
    // -------------------------------------------------------------
    let on_toggle_ear: Arc<dyn Fn(bool) + Send + Sync> = Arc::new(on_toggle_ear);
    let on_change_pause: Arc<dyn Fn(PauseBehavior) + Send + Sync> = Arc::new(on_change_pause);
    let on_toggle_conv: Arc<dyn Fn(bool) + Send + Sync> = Arc::new(on_toggle_conv);
    let on_change_mic: Arc<dyn Fn(MicrophoneMode) + Send + Sync> = Arc::new(on_change_mic);
    let on_toggle_chime: Arc<dyn Fn(bool) + Send + Sync> = Arc::new(on_toggle_case_chime);

    let controls_slot = Box::new(Orientation::Vertical, 0);
    let controls_card = RebuildableCard::new(controls_slot.clone(), initial_state, {
        let config = config.clone();
        let e = on_toggle_ear.clone();
        let p = on_change_pause.clone();
        let c = on_toggle_conv.clone();
        let m = on_change_mic.clone();
        let k = on_toggle_chime.clone();
        move |state| {
            // Re-read the config at rebuild time: the switches must show the
            // values that are actually in force, not the ones from launch.
            let cfg = config.read().unwrap().clone();
            let (e, p, c, m, k) =
                (e.clone(), p.clone(), c.clone(), m.clone(), k.clone());
            build_smart_settings_group(
                cfg.ear_detection_enabled,
                cfg.pause_behavior,
                cfg.conversational_awareness_enabled,
                state.mic_mode,
                state.case_chime_enabled,
                state.capabilities.has_case_speaker,
                move |b| e(b),
                move |b| p(b),
                move |b| c(b),
                move |b| m(b),
                move |b| k(b),
            )
            .upcast()
        }
    });

    let controls_box = Box::new(Orientation::Vertical, 14);
    controls_box.set_margin_top(16);
    controls_box.set_margin_bottom(24);
    controls_box.set_margin_start(20);
    controls_box.set_margin_end(20);
    controls_box.append(&controls_slot);

    let controls_scrolled = ScrolledWindow::new();
    controls_scrolled.set_vexpand(true);
    controls_scrolled.set_hexpand(true);
    controls_scrolled.set_hscrollbar_policy(gtk4::PolicyType::Never);
    controls_scrolled.set_vscrollbar_policy(gtk4::PolicyType::Automatic);
    controls_scrolled.set_child(Some(&controls_box));

    stack.add_named(&controls_scrolled, Some("controls"));

    // -------------------------------------------------------------
    // Page 4: 智能体接入 (Agent hooks) — its own page, per the management
    // surface the nod-answer feature needs: master switch, sensitivity, and
    // per-agent install/uninstall/test in one place.
    // -------------------------------------------------------------
    let agents_page = build_agents_page(config);

    let agents_scrolled = ScrolledWindow::new();
    agents_scrolled.set_vexpand(true);
    agents_scrolled.set_hexpand(true);
    agents_scrolled.set_hscrollbar_policy(gtk4::PolicyType::Never);
    agents_scrolled.set_vscrollbar_policy(gtk4::PolicyType::Automatic);
    agents_scrolled.set_child(Some(&agents_page));

    stack.add_named(&agents_scrolled, Some("agents"));

    // -------------------------------------------------------------
    // Page 5: 偏好设置与硬件 (Preferences)
    // -------------------------------------------------------------
    let preferences_page = build_preferences_page(initial_state, config, on_toggle_close_to_tray);

    let pref_scrolled = ScrolledWindow::new();
    pref_scrolled.set_vexpand(true);
    pref_scrolled.set_hexpand(true);
    pref_scrolled.set_hscrollbar_policy(gtk4::PolicyType::Never);
    pref_scrolled.set_vscrollbar_policy(gtk4::PolicyType::Automatic);
    pref_scrolled.set_child(Some(&preferences_page));

    stack.add_named(&pref_scrolled, Some("preferences"));

    // -------------------------------------------------------------
    // Sidebar Navigation Buttons
    // -------------------------------------------------------------
    let nav_items = [
        ("overview", "audio-headphones-symbolic", "概览首页"),
        ("spatial", "emblem-music-symbolic", "空间音频"),
        ("controls", "preferences-other-symbolic", "控制与交互"),
        ("agents", "applications-engineering-symbolic", "智能体接入"),
        ("preferences", "emblem-system-symbolic", "偏好设置"),
    ];

    // Apply custom styling for pixel-perfect macOS sidebar
    let provider = gtk4::CssProvider::new();
    provider.load_from_string(
        r#"
        .macos-sidebar {
            background-color: alpha(@window_bg_color, 0.6);
        }
        .sidebar-nav-btn {
            border-radius: 9px;
            padding: 8px 12px;
            margin: 2px 6px;
            background-color: transparent;
            border: none;
            box-shadow: none;
            transition: background-color 150ms ease, color 150ms ease;
        }
        .sidebar-nav-btn:hover {
            background-color: alpha(@theme_fg_color, 0.08);
        }
        .sidebar-nav-btn.active {
            background-color: @accent_bg_color;
            color: @accent_fg_color;
            box-shadow: 0 1px 3px rgba(0, 0, 0, 0.2);
        }
        .sidebar-nav-btn.active label {
            font-weight: 700;
            color: @accent_fg_color;
        }
        .sidebar-nav-btn.active image {
            color: @accent_fg_color;
        }
        .sidebar-nav-btn label {
            font-weight: 500;
        }
        "#,
    );
    if let Some(display) = gtk4::gdk::Display::default() {
        gtk4::style_context_add_provider_for_display(
            &display,
            &provider,
            gtk4::STYLE_PROVIDER_PRIORITY_APPLICATION,
        );
    }

    let mut buttons = Vec::new();

    for (page_id, icon_name, label_text) in nav_items {
        let btn = Button::new();
        btn.add_css_class("sidebar-nav-btn");
        btn.set_halign(Align::Fill);

        let row = Box::new(Orientation::Horizontal, 10);
        row.set_margin_top(6);
        row.set_margin_bottom(6);
        row.set_margin_start(8);
        row.set_margin_end(8);

        let icon = Image::from_icon_name(icon_name);
        icon.set_pixel_size(18);
        let label = Label::new(Some(label_text));
        label.set_halign(Align::Start);
        label.add_css_class("heading");

        row.append(&icon);
        row.append(&label);
        btn.set_child(Some(&row));

        if page_id == "overview" {
            btn.add_css_class("active");
        }

        buttons.push((page_id, btn));
    }

    let buttons_rc = Arc::new(buttons);
    for (page_id, btn) in buttons_rc.iter() {
        let stack_clone = stack.clone();
        let target_page = page_id.to_string();
        let buttons_list = buttons_rc.clone();

        btn.connect_clicked(move |_| {
            stack_clone.set_visible_child_name(&target_page);
            for (p, b) in buttons_list.iter() {
                if *p == target_page {
                    b.add_css_class("active");
                } else {
                    b.remove_css_class("active");
                }
            }
        });
        sidebar_box.append(btn);
    }

    root_box.append(&sidebar_box);
    root_box.append(&stack);

    toolbar_view.set_content(Some(&root_box));
    window.set_content(Some(&toolbar_view));

    // Handle Window Close Request (Intercept and minimize to tray if configured)
    let cfg_close = config.clone();
    window.connect_close_request(move |win| {
        let close_to_tray = {
            let guard = cfg_close.read().unwrap();
            guard.close_to_tray
        };
        if close_to_tray {
            win.set_visible(false);
            glib::Propagation::Stop
        } else {
            glib::Propagation::Proceed
        }
    });

    MainWindowHandles {
        window,
        hero_title,
        hero_subtitle,
        hero_status,
        connect_btn,
        battery_widgets,
        noise_card,
        controls_card,
        head_tracking,
        zcode_ask,
    }
}

pub fn update_ui_state(handles: &MainWindowHandles, state: &AirPodsDeviceState) {
    handles.hero_title.set_text(&state.name);
    let subtitle_text = format!("{} • {}", state.capabilities.name, state.mac_address);
    handles.hero_subtitle.set_text(&subtitle_text);

    let both_in_case = !state.is_wearing_any()
        && (state.left_in_ear == crate::core::aacp::events::EarStatus::InCase
            || state.right_in_ear == crate::core::aacp::events::EarStatus::InCase);

    if !state.is_connected {
        handles.hero_status.set_text("○ 未连接");
        handles.hero_status.remove_css_class("success");
        handles.hero_status.remove_css_class("accent");
        handles.hero_status.add_css_class("dim-label");
        handles.connect_btn.set_visible(true);
    } else if both_in_case {
        handles.hero_status.set_text("● 充电盒中");
        handles.hero_status.remove_css_class("success");
        handles.hero_status.remove_css_class("dim-label");
        handles.hero_status.add_css_class("accent");
        handles.connect_btn.set_visible(false);
    } else {
        handles.hero_status.set_text("● 已连接");
        handles.hero_status.remove_css_class("dim-label");
        handles.hero_status.remove_css_class("accent");
        handles.hero_status.add_css_class("success");
        handles.connect_btn.set_visible(false);
    }

    update_battery_widgets(&handles.battery_widgets, state);
    handles.noise_card.sync(state);
    handles.controls_card.sync(state);
}

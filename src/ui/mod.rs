pub mod components;
pub mod window;

use crate::core::aacp::commands::{MicrophoneMode, NoiseControlMode};
use crate::core::aacp::events::AacpEvent;
use crate::core::aacp::AacpManager;
use crate::core::device::AirPodsDeviceState;
use crate::core::models::AirPodsModel;
use crate::system::bluetooth::BluetoothManager;
use crate::system::config::{load_config, save_config, AppConfig, PauseBehavior, StemAction};
use crate::system::mpris::MprisController;
use crate::system::tray::RustPodsTray;
use gtk4::prelude::*;
use libadwaita::Application;
use std::rc::Rc;
use std::sync::Arc;
use tokio::sync::{broadcast, mpsc, RwLock};
use tracing::{debug, error, info, warn};

pub enum UiEvent {
    DeviceStateUpdated(AirPodsDeviceState),
    ConnectedChanged(bool),
    OpenWindow,
    HeadGesture(crate::core::head_tracking::HeadGesture),
}

pub fn run_app() {
    let app = Application::builder()
        .application_id("io.github.rustpods.RustPods")
        .build();

    app.connect_activate(build_ui);
    // Pass only program name to GTK so it doesn't fail on custom CLI flags parsed by Clap
    let prog_name = std::env::args().next().unwrap_or_else(|| "rustpods".to_string());
    app.run_with_args(&[prog_name]);
}

fn build_ui(app: &Application) {
    let config = Arc::new(std::sync::RwLock::new(load_config()));

    // Shared state between Tokio background engine and UI
    let device_state = Arc::new(RwLock::new(None::<AirPodsDeviceState>));
    let aacp_manager = Arc::new(AacpManager::new());
    let mpris_controller = Arc::new(MprisController::new());

    // Channels for UI updates: Tokio -> GLib Main Context
    let (ui_event_tx, mut ui_event_rx) = mpsc::unbounded_channel::<UiEvent>();

    // Initial dummy state for UI creation
    let init_state = AirPodsDeviceState::new(
        "CC:68:E0:38:9C:E9".to_string(),
        "浩的AirPods".to_string(),
        AirPodsModel::AirPods3,
    );

    // Callbacks for UI user interactions
    let aacp_rename = aacp_manager.clone();
    let on_rename = move |new_name: String| {
        let aacp = aacp_rename.clone();
        tokio::spawn(async move {
            let _ = aacp.rename(&new_name).await;
        });
    };

    let dev_state_conn = device_state.clone();
    let on_connect = move || {
        let dev_state = dev_state_conn.clone();
        tokio::spawn(async move {
            let mac = {
                let guard = dev_state.read().await;
                guard.as_ref().map(|s| s.mac_address.clone())
            };
            if let Some(mac_addr) = mac {
                info!("Triggering connect to {}", mac_addr);
                let _ = tokio::process::Command::new("bluetoothctl")
                    .arg("connect")
                    .arg(&mac_addr)
                    .output()
                    .await;
            }
        });
    };

    let aacp_noise = aacp_manager.clone();
    let on_set_noise_mode = move |mode: NoiseControlMode| {
        let aacp = aacp_noise.clone();
        tokio::spawn(async move {
            let _ = aacp.set_noise_mode(mode).await;
        });
    };

    let aacp_adapt = aacp_manager.clone();
    let on_set_adaptive = move |val: u8| {
        let aacp = aacp_adapt.clone();
        tokio::spawn(async move {
            let _ = aacp.set_adaptive_strength(val).await;
        });
    };

    let aacp_ear = aacp_manager.clone();
    let cfg_ear = config.clone();
    let on_toggle_ear = move |enabled: bool| {
        let aacp = aacp_ear.clone();
        if let Ok(mut c) = cfg_ear.write() {
            c.ear_detection_enabled = enabled;
            save_config(&c);
        }
        tokio::spawn(async move {
            let _ = aacp.set_ear_detection(enabled).await;
        });
    };

    let cfg_pause = config.clone();
    let on_change_pause = move |behavior: PauseBehavior| {
        if let Ok(mut c) = cfg_pause.write() {
            c.pause_behavior = behavior;
            save_config(&c);
        }
    };

    let aacp_conv = aacp_manager.clone();
    let cfg_conv = config.clone();
    let on_toggle_conv = move |enabled: bool| {
        let aacp = aacp_conv.clone();
        if let Ok(mut c) = cfg_conv.write() {
            c.conversational_awareness_enabled = enabled;
            save_config(&c);
        }
        tokio::spawn(async move {
            let _ = aacp.set_conversational_awareness(enabled).await;
        });
    };

    let aacp_mic = aacp_manager.clone();
    let on_change_mic = move |mode: MicrophoneMode| {
        let aacp = aacp_mic.clone();
        tokio::spawn(async move {
            let _ = aacp.set_microphone_mode(mode).await;
        });
    };

    let aacp_chime = aacp_manager.clone();
    let on_toggle_case_chime = move |enabled: bool| {
        let aacp = aacp_chime.clone();
        tokio::spawn(async move {
            let _ = aacp.set_case_chime(enabled).await;
        });
    };

    let cfg_close = config.clone();
    let on_toggle_close_to_tray = move |enabled: bool| {
        if let Ok(mut c) = cfg_close.write() {
            c.close_to_tray = enabled;
            save_config(&c);
        }
    };

    // Create Main Window
    let handles = Rc::new(window::create_main_window(
        app,
        &init_state,
        &config,
        on_rename,
        on_connect,
        on_set_noise_mode,
        on_set_adaptive,
        on_toggle_ear,
        on_change_pause,
        on_toggle_conv,
        on_change_mic,
        on_toggle_case_chime,
        on_toggle_close_to_tray,
    ));

    handles.window.present();

    // Spawn System Tray
    let tray_dev_state = device_state.clone();
    let tx_tray_open = ui_event_tx.clone();
    let on_open_win = Arc::new(move || {
        let _ = tx_tray_open.send(UiEvent::OpenWindow);
    });
    let aacp_tray_noise = aacp_manager.clone();
    let on_tray_noise = Arc::new(move |mode: NoiseControlMode| {
        let aacp = aacp_tray_noise.clone();
        tokio::spawn(async move {
            let _ = aacp.set_noise_mode(mode).await;
        });
    });
    let aacp_tray_ear = aacp_manager.clone();
    let on_tray_ear = Arc::new(move |enabled: bool| {
        let aacp = aacp_tray_ear.clone();
        tokio::spawn(async move {
            let _ = aacp.set_ear_detection(enabled).await;
        });
    });

    let tray = RustPodsTray {
        device_state: tray_dev_state,
        on_open_window: on_open_win,
        on_set_noise_mode: on_tray_noise,
        on_toggle_ear_detection: on_tray_ear,
    };

    let tray_handle_holder: Arc<tokio::sync::Mutex<Option<ksni::Handle<RustPodsTray>>>> =
        Arc::new(tokio::sync::Mutex::new(None));
    let tray_holder_clone = tray_handle_holder.clone();

    tokio::spawn(async move {
        use ksni::TrayMethods;
        match tray.spawn().await {
            Ok(handle) => {
                let mut guard = tray_holder_clone.lock().await;
                *guard = Some(handle);
            }
            Err(e) => warn!("Failed to spawn system tray: {}", e),
        }
    });

    // Live head-tracking feed: pull the latest pose on every frame rather than
    // forwarding all ~50 samples/s through the GTK main loop.
    {
        let aacp_feed = aacp_manager.clone();
        let handles_feed = handles.clone();
        let tx_gesture = ui_event_tx.clone();
        let mut pose_rx = aacp_manager.head_pose();
        let mut gesture_rx = aacp_manager.subscribe_gestures();

        glib::MainContext::default().spawn_local(async move {
            // Gestures are rare, so they are pushed as they happen.
            glib::MainContext::default().spawn_local(async move {
                while let Ok(gesture) = gesture_rx.recv().await {
                    if tx_gesture.send(UiEvent::HeadGesture(gesture)).is_err() {
                        break;
                    }
                }
            });

            loop {
                let pose = (*pose_rx.borrow_and_update()).clone();
                let calibrated = aacp_feed.is_head_tracking_active().await;
                let rate = aacp_feed.head_tracking_rate_hz().await;

                {
                    let mut state = handles_feed.head_tracking.state.borrow_mut();
                    state.pose = pose;
                    state.calibrated = calibrated;
                    state.rate_hz = rate;
                    state.status = match (pose, calibrated) {
                        (None, _) => "等待数据流…（请确认 AirPods 已连接）".to_string(),
                        (Some(_), false) => "正在校准中立姿态…（保持头部静止）".to_string(),
                        (Some(_), true) => match rate {
                            Some(hz) => format!("实时姿态流 · {:.0} Hz", hz),
                            None => "实时姿态流".to_string(),
                        },
                    };
                }
                components::headtracking::update_head_tracking_widgets(&handles_feed.head_tracking);

                if pose_rx.changed().await.is_err() {
                    break;
                }
            }
        });
    }

    // GLib Main Loop Listener for UI Updates
    let handles_listener = handles.clone();
    let tray_holder_events = tray_handle_holder.clone();
    glib::MainContext::default().spawn_local(async move {
        while let Some(evt) = ui_event_rx.recv().await {
            match evt {
                UiEvent::DeviceStateUpdated(state) => {
                    window::update_ui_state(&handles_listener, &state);

                    // Refresh tray icon with new battery level
                    let holder = tray_holder_events.clone();
                    tokio::spawn(async move {
                        let guard = holder.lock().await;
                        if let Some(h) = guard.as_ref() {
                            h.update(|_| {}).await;
                        }
                    });
                }
                UiEvent::OpenWindow => {
                    handles_listener.window.set_visible(true);
                    handles_listener.window.present();
                }
                UiEvent::ConnectedChanged(connected) => {
                    if connected {
                        handles_listener.hero_status.set_text("● 已连接");
                        handles_listener.hero_status.remove_css_class("dim-label");
                        handles_listener.hero_status.add_css_class("success");
                    } else {
                        handles_listener.hero_status.set_text("○ 未连接");
                        handles_listener.hero_status.remove_css_class("success");
                        handles_listener.hero_status.add_css_class("dim-label");

                        // Drop the last pose so the compass does not keep showing
                        // a stale orientation after the stream stops.
                        {
                            let mut state = handles_listener.head_tracking.state.borrow_mut();
                            state.pose = None;
                            state.calibrated = false;
                            state.rate_hz = None;
                            state.status = "等待数据流…（请确认 AirPods 已连接）".to_string();
                        }
                        components::headtracking::update_head_tracking_widgets(
                            &handles_listener.head_tracking,
                        );
                    }

                    let holder = tray_holder_events.clone();
                    tokio::spawn(async move {
                        let guard = holder.lock().await;
                        if let Some(h) = guard.as_ref() {
                            h.update(|_| {}).await;
                        }
                    });
                }

                UiEvent::HeadGesture(gesture) => {
                    info!("Head gesture: {:?}", gesture);
                    {
                        let mut state = handles_listener.head_tracking.state.borrow_mut();
                        state.gesture = Some(gesture);
                    }
                    components::headtracking::update_head_tracking_widgets(
                        &handles_listener.head_tracking,
                    );

                    // Clear the badge so a stale gesture is not shown forever.
                    let handles_clear = handles_listener.clone();
                    glib::timeout_add_local_once(
                        std::time::Duration::from_millis(1800),
                        move || {
                            {
                                let mut state =
                                    handles_clear.head_tracking.state.borrow_mut();
                                state.gesture = None;
                            }
                            components::headtracking::update_head_tracking_widgets(
                                &handles_clear.head_tracking,
                            );
                        },
                    );
                }
            }
        }
    });

    // Spawn Tokio Background Services
    let aacp_bg = aacp_manager.clone();
    let dev_state_bg = device_state.clone();
    let mpris_bg = mpris_controller.clone();
    let ui_tx_bg = ui_event_tx.clone();
    let cfg_bg = config.clone();

    tokio::spawn(async move {
        start_bluetooth_daemon(aacp_bg, dev_state_bg, mpris_bg, ui_tx_bg, cfg_bg).await;
    });
}

pub async fn start_bluetooth_daemon(
    aacp: Arc<AacpManager>,
    dev_state: Arc<RwLock<Option<AirPodsDeviceState>>>,
    mpris: Arc<MprisController>,
    ui_tx: mpsc::UnboundedSender<UiEvent>,
    config: Arc<std::sync::RwLock<AppConfig>>,
) {
    info!("Starting RustPods Bluetooth Background Daemon...");

    // Start BLE Proximity Monitor for real-time background battery updates
    let dev_state_le = dev_state.clone();
    let ui_tx_le = ui_tx.clone();
    tokio::spawn(async move {
        if let Err(e) = crate::system::le_monitor::start_le_monitor(dev_state_le, ui_tx_le).await {
            warn!("BLE Proximity Monitor error: {}", e);
        }
    });

    // Start periodic battery status refresh task
    let aacp_poll = aacp.clone();
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(tokio::time::Duration::from_secs(8)).await;
            if aacp_poll.is_connected().await {
                let _ = aacp_poll.request_battery_and_status().await;
            }
        }
    });

    let bt_manager = match BluetoothManager::new().await {
        Ok(m) => m,
        Err(e) => {
            error!("Failed to initialize BlueZ: {}", e);
            return;
        }
    };

    if let Ok(ad) = bt_manager.get_default_adapter().await {
        if let Ok(addr) = ad.address().await {
            info!("Local Bluetooth Adapter MAC: {}", addr);
            aacp.set_local_address(addr.to_string()).await;
        }
    }

    loop {
        info!("Searching for connected AirPods...");
        match bt_manager.find_connected_airpods().await {
            Ok(pods_list) if !pods_list.is_empty() => {
                let pod = pods_list[0].clone();
                info!(
                    "Found connected AirPods: {} ({}) - Model: {:?}",
                    pod.name, pod.address, pod.model
                );

                let mut current_state =
                    AirPodsDeviceState::new(pod.address.to_string(), pod.name.clone(), pod.model);
                current_state.is_connected = false;

                // Subscribe to AACP events BEFORE connect so no initial packets (battery, ear status) are missed
                let mut event_rx = aacp.subscribe();

                // Connect AACP L2CAP socket
                if let Err(e) = aacp.connect(pod.address).await {
                    error!("AACP connection failed: {}", e);
                    current_state.is_connected = false;
                    current_state.battery_left = None;
                    current_state.battery_right = None;
                    current_state.battery_case = None;
                    {
                        let mut lock = dev_state.write().await;
                        *lock = Some(current_state.clone());
                    }
                    let _ = ui_tx.send(UiEvent::DeviceStateUpdated(current_state.clone()));
                    tokio::time::sleep(tokio::time::Duration::from_secs(4)).await;
                    continue;
                }

                info!("AACP event stream active and connected");
                current_state.is_connected = true;
                {
                    let mut lock = dev_state.write().await;
                    *lock = Some(current_state.clone());
                }
                let _ = ui_tx.send(UiEvent::DeviceStateUpdated(current_state.clone()));

                info!("Requesting initial battery and status...");
                let _ = aacp.request_battery_and_status().await;

                // Head tracking needs its own negotiation: the accessory only
                // announces its sensor services after the RemoteHID prelude, and
                // the stream stays off until Feature report 1 is written on
                // devmotion6. Without this the spatial-audio page has nothing to
                // draw even though battery and ear detection work fine.
                //
                // A spatial-audio claim is required first, matching macOS, which
                // will not enable tracking until an application asks for it.
                {
                    let camera = aacp.clone();
                    let target = current_state.mac_address.clone();
                    tokio::spawn(async move {
                        camera
                            .claim_spatial_audio(
                                "io.github.rustpods.RustPods",
                                crate::core::spatial::SpatialMode::HeadTracked,
                                true,
                            )
                            .await;

                        // The accessory announces its sensor services
                        // unreliably: a freshly connected unit often reports
                        // none, and only a link cycle clears that. Verified on
                        // hardware - the same unit needed two cycles before it
                        // listed all eight services. So cycle a few times around
                        // the in-place retries that start_head_tracking already
                        // does.
                        for attempt in 1..=3 {
                            match camera.start_head_tracking().await {
                                Ok(()) => {
                                    info!("Head tracking started (attempt {})", attempt);
                                    for _ in 0..10 {
                                        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                                        if camera.head_samples_seen().await > 0 {
                                            info!("Head tracking is producing samples");
                                            return;
                                        }
                                    }
                                    warn!("No samples after attempt {}; retrying", attempt);
                                }
                                Err(e) => {
                                    warn!("Attempt {} failed: {}", attempt, e);
                                    // The driver cycled the Bluetooth link, which
                                    // kills the socket this manager holds. Rebuild
                                    // it before trying again, otherwise every
                                    // later attempt talks to a dead session.
                                    if e.contains("link recycled") {
                                        if let Ok(addr) = target.parse() {
                                            if let Err(e) =
                                                camera.connect_without_handshake(addr).await
                                            {
                                                warn!("Reconnect failed: {}", e);
                                                return;
                                            }
                                            let _ = camera
                                                .send_raw(vec![
                                                    0x00, 0x00, 0x04, 0x00, 0x01, 0x00, 0x02,
                                                    0x00, 0, 0, 0, 0, 0, 0, 0, 0,
                                                ])
                                                .await;
                                            tokio::time::sleep(
                                                std::time::Duration::from_millis(300),
                                            )
                                            .await;
                                            let _ = camera
                                                .send_raw(vec![
                                                    0x04, 0x00, 0x04, 0x00, 0x4D, 0x00, 0xD7,
                                                    0x00, 0, 0, 0, 0, 0, 0,
                                                ])
                                                .await;
                                            tokio::time::sleep(
                                                std::time::Duration::from_millis(150),
                                            )
                                            .await;
                                            let _ = camera
                                                .send_raw(vec![
                                                    0x04, 0x00, 0x04, 0x00, 0x0F, 0x00, 0xFF,
                                                    0xFF, 0xFF, 0xFF,
                                                ])
                                                .await;
                                        }
                                    }
                                }
                            }
                        }
                        warn!("Head tracking could not be started automatically");
                    });
                }

                let mut last_both_in_ear = false;

                // The AACP socket can also die silently (device out of range,
                // grabbed by another host) without sending Disconnected. Poll
                // the manager alongside the event stream so the UI cannot get
                // stuck on a stale "connected" state.
                let mut health = tokio::time::interval(tokio::time::Duration::from_secs(3));
                health.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

                loop {
                    let event = tokio::select! {
                        event = event_rx.recv() => event,
                        _ = health.tick() => {
                            if !aacp.is_connected().await {
                                warn!("AACP socket is gone; leaving the event loop");
                                break;
                            }
                            continue;
                        }
                    };

                    let event = match event {
                        Ok(event) => event,
                        Err(broadcast::error::RecvError::Closed) => break,
                        // Lagging only means we skipped samples; keep going.
                        Err(broadcast::error::RecvError::Lagged(n)) => {
                            warn!("AACP event stream lagged, skipped {} events", n);
                            continue;
                        }
                    };

                    match event {
                        AacpEvent::Battery(battery_list) => {
                            info!("Received Battery Info over AACP: {:?}", battery_list);
                            for b in battery_list {
                                current_state.update_battery(b.component, b.level, b.status);
                            }
                            {
                                let mut lock = dev_state.write().await;
                                *lock = Some(current_state.clone());
                            }
                            let _ = ui_tx.send(UiEvent::DeviceStateUpdated(current_state.clone()));
                        }

                        AacpEvent::EarDetection(ear_evt) => {
                            info!("Received Ear Detection over AACP: {:?}", ear_evt);
                            let old_wearing = current_state.is_wearing_any();
                            current_state.left_in_ear = ear_evt.left;
                            current_state.right_in_ear = ear_evt.right;
                            let new_wearing = current_state.is_wearing_any();

                            let cfg = config.read().unwrap().clone();
                            if cfg.ear_detection_enabled {
                                match cfg.pause_behavior {
                                    PauseBehavior::OneRemoved => {
                                        if !current_state.is_wearing_both() && last_both_in_ear {
                                            mpris.handle_ear_detection_pause().await;
                                        } else if current_state.is_wearing_both()
                                            && !last_both_in_ear
                                        {
                                            mpris.handle_ear_detection_resume().await;
                                        }
                                    }
                                    PauseBehavior::BothRemoved => {
                                        if old_wearing && !new_wearing {
                                            mpris.handle_ear_detection_pause().await;
                                        } else if !old_wearing && new_wearing {
                                            mpris.handle_ear_detection_resume().await;
                                        }
                                    }
                                    PauseBehavior::Never => {}
                                }
                            }

                            last_both_in_ear = current_state.is_wearing_both();

                            {
                                let mut lock = dev_state.write().await;
                                *lock = Some(current_state.clone());
                            }
                            let _ = ui_tx.send(UiEvent::DeviceStateUpdated(current_state.clone()));
                        }

                        AacpEvent::StemPress { press_type, bud } => {
                            info!("Stem press: {:?} on {:?}", press_type, bud);
                            let cfg = config.read().unwrap().clone();
                            let action = match press_type {
                                crate::core::aacp::events::StemPressType::Single => {
                                    cfg.stem_single_click
                                }
                                crate::core::aacp::events::StemPressType::Double => {
                                    cfg.stem_double_click
                                }
                                crate::core::aacp::events::StemPressType::Triple => {
                                    cfg.stem_triple_click
                                }
                                crate::core::aacp::events::StemPressType::Long => {
                                    cfg.stem_long_press
                                }
                            };
                            match action {
                                StemAction::PlayPause => mpris.play_pause().await,
                                StemAction::NextTrack => mpris.next().await,
                                StemAction::PreviousTrack => mpris.previous().await,
                                _ => {}
                            }
                        }

                        AacpEvent::ConversationalAwareness(active) => {
                            info!("Conversational awareness: user speaking = {}", active);
                            current_state.conversational_awareness = active;
                            let _ = ui_tx.send(UiEvent::DeviceStateUpdated(current_state.clone()));
                        }

                        AacpEvent::Renamed(new_name) => {
                            info!("AirPods renamed to: {}", new_name);
                            current_state.name = new_name;
                            let _ = ui_tx.send(UiEvent::DeviceStateUpdated(current_state.clone()));
                        }

                        AacpEvent::Disconnected => {
                            // The broadcast sender lives as long as the manager, so
                            // recv() never errors on its own. Leaving the loop here
                            // is what lets the outer loop notice the drop and retry.
                            warn!("AACP reported disconnect; leaving the event loop");
                            break;
                        }

                        _ => {}
                    }
                }

                warn!("AirPods AACP disconnected, will retry...");
                current_state.is_connected = false;
                current_state.battery_left = None;
                current_state.battery_right = None;
                current_state.battery_case = None;
                current_state.left_in_ear = crate::core::aacp::events::EarStatus::Disconnected;
                current_state.right_in_ear = crate::core::aacp::events::EarStatus::Disconnected;
                {
                    let mut lock = dev_state.write().await;
                    *lock = Some(current_state.clone());
                }
                let _ = ui_tx.send(UiEvent::DeviceStateUpdated(current_state.clone()));
                let _ = ui_tx.send(UiEvent::ConnectedChanged(false));
            }

            Ok(_) => {
                debug!("No connected AirPods detected, checking paired devices...");
                if let Ok(paired) = bt_manager.find_paired_airpods().await {
                    if let Some(first_paired) = paired.first() {
                        let mut state = AirPodsDeviceState::new(
                            first_paired.address.to_string(),
                            first_paired.name.clone(),
                            first_paired.model,
                        );
                        state.is_connected = false;
                        {
                            let mut lock = dev_state.write().await;
                            *lock = Some(state.clone());
                        }
                        let _ = ui_tx.send(UiEvent::DeviceStateUpdated(state));
                    }
                }
                // Also refresh the status pill; without this the hero header keeps
                // whatever it last showed even though the device is gone.
                let _ = ui_tx.send(UiEvent::ConnectedChanged(false));
            }

            Err(e) => {
                error!("Error querying Bluetooth devices: {}", e);
            }
        }

        tokio::time::sleep(tokio::time::Duration::from_secs(5)).await;
    }
}

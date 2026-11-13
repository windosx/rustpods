pub mod components;
pub mod popup;
pub mod state;
pub mod theme;
pub mod window;

use crate::core::aacp::commands::NoiseControlMode;
use crate::core::aacp::events::AacpEvent;
use crate::core::aacp::AacpManager;
use crate::core::device::AirPodsDeviceState;
use crate::system::bluetooth::BluetoothManager;
use crate::system::config::{load_config, save_config, AppConfig, PauseBehavior, StemAction};
use crate::system::mpris::MprisController;
use crate::system::tray::RustPodsTray;
use gpui_kit::assets::{icon_assets, Assets as DefaultKitAssets};
use gpui_kit::component::ThemeMode;
use gpui_kit::{
    AnyWindowHandle, App, AppContext as _, AssetSource, Entity, QuitMode, Result, SharedString,
};
use state::{RustPodsController, RustPodsState};
use std::borrow::Cow;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{broadcast, mpsc, RwLock};
use tracing::{debug, error, info, warn};
use window::{window_options, MainWindow};

pub enum UiEvent {
    DeviceStateUpdated(AirPodsDeviceState),
    ConnectedChanged(bool),
    OpenWindow,
    CaseLidOpened,
    CaseLidClosed,
    HeadGesture(crate::core::head_tracking::HeadGesture),
}

// Extra Lucide icons used in RustPods beyond gpui-component's default bundle.
icon_assets!(
    RustPodsExtraIcons,
    [
        Headphones,
        Pencil,
        AudioLines,
        SlidersHorizontal,
        Volume2,
        Bluetooth,
        BluetoothConnected,
        Compass,
        Zap,
        MessageCircleQuestionMark,
    ]
);

struct CompositeAssets;

const ANIM_AIRPODS_1: &[u8] = include_bytes!("../../assets/animations/airpods_1.webp");
const ANIM_AIRPODS_2: &[u8] = include_bytes!("../../assets/animations/airpods_2.webp");
const ANIM_AIRPODS_3: &[u8] = include_bytes!("../../assets/animations/airpods_3.webp");
const ANIM_AIRPODS_4: &[u8] = include_bytes!("../../assets/animations/airpods_4.webp");
const ANIM_AIRPODS_PRO: &[u8] = include_bytes!("../../assets/animations/airpods_pro.webp");
const ANIM_AIRPODS_PRO_2: &[u8] = include_bytes!("../../assets/animations/airpods_pro_2.webp");
const ANIM_AIRPODS_MAX: &[u8] = include_bytes!("../../assets/animations/airpods_max.webp");

impl AssetSource for CompositeAssets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        let embedded = match path {
            "animations/airpods_1.webp" => Some(ANIM_AIRPODS_1),
            "animations/airpods_2.webp" => Some(ANIM_AIRPODS_2),
            "animations/airpods_3.webp" => Some(ANIM_AIRPODS_3),
            "animations/airpods_4.webp" => Some(ANIM_AIRPODS_4),
            "animations/airpods_pro.webp" => Some(ANIM_AIRPODS_PRO),
            "animations/airpods_pro_2.webp" => Some(ANIM_AIRPODS_PRO_2),
            "animations/airpods_max.webp" => Some(ANIM_AIRPODS_MAX),
            _ => None,
        };
        if let Some(bytes) = embedded {
            return Ok(Some(Cow::Borrowed(bytes)));
        }
        if let Some(bytes) = RustPodsExtraIcons.load(path)? {
            return Ok(Some(bytes));
        }
        DefaultKitAssets.load(path)
    }

    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        let mut items = DefaultKitAssets.list(path)?;
        items.extend(RustPodsExtraIcons.list(path)?);
        Ok(items)
    }
}

struct SingleInstanceService {
    ui_tx: mpsc::UnboundedSender<UiEvent>,
}

#[zbus::interface(name = "io.github.rustpods.RustPods")]
impl SingleInstanceService {
    fn activate(&self) {
        let _ = self.ui_tx.send(UiEvent::OpenWindow);
    }
}

async fn serve_single_instance_dbus(ui_tx: mpsc::UnboundedSender<UiEvent>) {
    match zbus::connection::Builder::session() {
        Ok(builder) => {
            match builder
                .name("io.github.rustpods.RustPods")
                .and_then(|b| {
                    b.serve_at(
                        "/io/github/rustpods/RustPods",
                        SingleInstanceService { ui_tx },
                    )
                }) {
                Ok(b) => {
                    if let Err(e) = b.build().await {
                        warn!("Failed to register single-instance D-Bus service: {}", e);
                    } else {
                        std::future::pending::<()>().await;
                    }
                }
                Err(e) => warn!("Failed to configure single-instance D-Bus service: {}", e),
            }
        }
        Err(e) => warn!("D-Bus session connection unavailable for single-instance: {}", e),
    }
}

/// Starts the GPUI desktop application.
pub fn run_app(rt_handle: tokio::runtime::Handle) {
    gpui_kit::application()
        .with_assets(CompositeAssets)
        .with_quit_mode(QuitMode::Explicit)
        .run(move |cx| {
            gpui_kit::init(cx);
            let initial_mode = ThemeMode::Dark;
            theme::apply_apple_theme(initial_mode, cx);
            bootstrap_gui(rt_handle, initial_mode, cx);
        });
}

fn bootstrap_gui(
    rt_handle: tokio::runtime::Handle,
    initial_mode: ThemeMode,
    cx: &mut App,
) {
    let config = Arc::new(std::sync::RwLock::new(load_config()));
    crate::core::nod_counter::set_sensitivity(config.read().unwrap().gesture_sensitivity);

    let device_state = Arc::new(RwLock::new(None::<AirPodsDeviceState>));
    let aacp_manager = Arc::new(AacpManager::new());
    let mpris_controller = Arc::new(MprisController::new());
    let (ui_event_tx, mut ui_event_rx) = mpsc::unbounded_channel::<UiEvent>();

    let controller = RustPodsController {
        rt: rt_handle.clone(),
        aacp: aacp_manager.clone(),
        device_state: device_state.clone(),
        config: config.clone(),
        ui_tx: ui_event_tx.clone(),
    };

    let app_state = cx.new(|_| RustPodsState::new(controller, initial_mode));

    // Open initial main window and track its handle so OpenWindow can either
    // activate the live window or recreate it after close-to-tray.
    let active_window: Arc<std::sync::Mutex<Option<AnyWindowHandle>>> =
        Arc::new(std::sync::Mutex::new(None));
    let popup_window: Arc<std::sync::Mutex<Option<gpui_kit::WindowHandle<popup::CasePopupWindow>>>> =
        Arc::new(std::sync::Mutex::new(None));
    open_or_activate_window(&app_state, &active_window, cx);

    // Register D-Bus single-instance Activate handler.
    rt_handle.spawn(serve_single_instance_dbus(ui_event_tx.clone()));

    // Spawn System Tray (StatusNotifierItem via ksni).
    let tray_handle_holder: Arc<tokio::sync::Mutex<Option<ksni::Handle<RustPodsTray>>>> =
        Arc::new(tokio::sync::Mutex::new(None));
    {
        let tray_dev_state = device_state.clone();
        let tx_tray_open = ui_event_tx.clone();
        let on_open_win = Arc::new(move || {
            let _ = tx_tray_open.send(UiEvent::OpenWindow);
        });
        let aacp_tray_noise = aacp_manager.clone();
        let dev_tray_noise = device_state.clone();
        let tx_tray_noise = ui_event_tx.clone();
        let rt_noise = rt_handle.clone();
        let on_tray_noise = Arc::new(move |mode: NoiseControlMode| {
            let aacp = aacp_tray_noise.clone();
            let dev = dev_tray_noise.clone();
            let tx = tx_tray_noise.clone();
            rt_noise.spawn(async move {
                let _ = aacp.set_noise_mode(mode).await;
                let updated = {
                    let mut guard = dev.write().await;
                    match guard.as_mut() {
                        Some(state) => {
                            state.noise_mode = mode;
                            Some(state.clone())
                        }
                        None => None,
                    }
                };
                if let Some(state) = updated {
                    let _ = tx.send(UiEvent::DeviceStateUpdated(state));
                }
            });
        });

        let aacp_tray_ear = aacp_manager.clone();
        let cfg_tray_ear = config.clone();
        let dev_tray_ear = device_state.clone();
        let tx_tray_ear = ui_event_tx.clone();
        let rt_ear = rt_handle.clone();
        let on_tray_ear = Arc::new(move |enabled: bool| {
            let aacp = aacp_tray_ear.clone();
            if let Ok(mut c) = cfg_tray_ear.write() {
                c.ear_detection_enabled = enabled;
                save_config(&c);
            }
            let dev = dev_tray_ear.clone();
            let tx = tx_tray_ear.clone();
            rt_ear.spawn(async move {
                let _ = aacp.set_ear_detection(enabled).await;
                let updated = {
                    let mut guard = dev.write().await;
                    match guard.as_mut() {
                        Some(state) => {
                            state.ear_detection_enabled = enabled;
                            Some(state.clone())
                        }
                        None => None,
                    }
                };
                if let Some(state) = updated {
                    let _ = tx.send(UiEvent::DeviceStateUpdated(state));
                }
            });
        });

        let tray = RustPodsTray {
            device_state: tray_dev_state,
            on_open_window: on_open_win,
            on_set_noise_mode: on_tray_noise,
            on_toggle_ear_detection: on_tray_ear,
        };

        let tray_holder_clone = tray_handle_holder.clone();
        rt_handle.spawn(async move {
            use ksni::TrayMethods;
            match tray.spawn().await {
                Ok(handle) => {
                    let mut guard = tray_holder_clone.lock().await;
                    *guard = Some(handle);
                }
                Err(e) => warn!("Failed to spawn system tray: {}", e),
            }
        });
    }

    // Spawn Tokio background engines.
    rt_handle.spawn(crate::system::zcode_ask::run_socket_server());
    rt_handle.spawn(run_audio_follow(aacp_manager.head_pose()));
    {
        let aacp_bg = aacp_manager.clone();
        let dev_bg = device_state.clone();
        let mpris_bg = mpris_controller.clone();
        let tx_bg = ui_event_tx.clone();
        let cfg_bg = config.clone();
        rt_handle.spawn(async move {
            start_bluetooth_daemon(aacp_bg, dev_bg, mpris_bg, tx_bg, cfg_bg).await;
        });
    }

    // GPUI foreground listener for UiEvent (device state, connection, window open, gestures).
    {
        let state_ev = app_state.clone();
        let win_ev = active_window.clone();
        let popup_ev = popup_window.clone();
        let tray_ev = tray_handle_holder.clone();
        let rt_ev = rt_handle.clone();
        cx.spawn(async move |cx| {
            let mut gesture_gen: u64 = 0;
            while let Some(evt) = ui_event_rx.recv().await {
                match evt {
                    UiEvent::DeviceStateUpdated(new_dev) => {
                        let _ = state_ev.update(cx, |s, cx| {
                            s.device = new_dev;
                            cx.notify();
                        });
                        let holder = tray_ev.clone();
                        rt_ev.spawn(async move {
                            let guard = holder.lock().await;
                            if let Some(h) = guard.as_ref() {
                                h.update(|_| {}).await;
                            }
                        });
                    }
                    UiEvent::OpenWindow => {
                        let _ = cx.update(|cx| {
                            open_or_activate_window(&state_ev, &win_ev, cx);
                        });
                    }
                    UiEvent::CaseLidOpened => {
                        let _ = cx.update(|cx| {
                            popup::open_or_refresh_case_popup(&state_ev, &popup_ev, cx);
                        });
                    }
                    UiEvent::CaseLidClosed => {
                        let _ = cx.update(|cx| {
                            popup::close_case_popup(&popup_ev, cx);
                        });
                    }
                    UiEvent::ConnectedChanged(connected) => {
                        let _ = state_ev.update(cx, |s, cx| {
                            s.device.is_connected = connected;
                            if !connected {
                                s.head_tracking.pose = None;
                                s.head_tracking.calibrated = false;
                                s.head_tracking.rate_hz = None;
                                s.head_tracking.status =
                                    "等待数据流…（请确认 AirPods 已连接）".to_string();
                            }
                            cx.notify();
                        });
                        let holder = tray_ev.clone();
                        rt_ev.spawn(async move {
                            let guard = holder.lock().await;
                            if let Some(h) = guard.as_ref() {
                                h.update(|_| {}).await;
                            }
                        });
                    }
                    UiEvent::HeadGesture(gesture) => {
                        info!("Head gesture: {:?}", gesture);
                        gesture_gen = gesture_gen.wrapping_add(1);
                        let expected = gesture_gen;
                        let _ = state_ev.update(cx, |s, cx| {
                            s.head_tracking.gesture = Some(gesture);
                            cx.notify();
                        });
                        let state_clear = state_ev.clone();
                        cx.spawn(async move |cx| {
                            cx.background_executor()
                                .timer(Duration::from_millis(1800))
                                .await;
                            let _ = state_clear.update(cx, |s, cx| {
                                if s.head_tracking.gesture == Some(gesture) {
                                    let _ = expected;
                                    s.head_tracking.gesture = None;
                                    cx.notify();
                                }
                            });
                        })
                        .detach();
                    }
                }
            }
        })
        .detach();
    }

    // Live head-tracking feed (throttled to ~30 Hz; None disconnect sentinels pass immediately).
    {
        const RENDER_INTERVAL: Duration = Duration::from_millis(33);
        let aacp_feed = aacp_manager.clone();
        let state_feed = app_state.clone();
        let tx_gesture = ui_event_tx.clone();
        let mut pose_rx = aacp_manager.head_pose();
        let mut gesture_rx = aacp_manager.subscribe_gestures();

        cx.spawn(async move |_cx| {
            while let Ok(gesture) = gesture_rx.recv().await {
                if tx_gesture.send(UiEvent::HeadGesture(gesture)).is_err() {
                    break;
                }
            }
        })
        .detach();

        cx.spawn(async move |cx| {
            let mut last_render = None::<std::time::Instant>;
            loop {
                if pose_rx.changed().await.is_err() {
                    break;
                }
                let pose = *pose_rx.borrow_and_update();
                let now = std::time::Instant::now();
                if pose.is_some()
                    && last_render.is_some_and(|t| now.duration_since(t) < RENDER_INTERVAL)
                {
                    continue;
                }
                last_render = Some(now);

                let calibrated = aacp_feed.is_head_tracking_active().await;
                let rate = aacp_feed.head_tracking_rate_hz().await;
                let _ = state_feed.update(cx, |s, cx| {
                    s.head_tracking.pose = pose;
                    s.head_tracking.calibrated = calibrated;
                    s.head_tracking.rate_hz = rate;
                    if pose.is_some() {
                        s.head_tracking.status = if calibrated {
                            match rate {
                                Some(hz) => format!("实时姿态流 · {:.0} Hz", hz),
                                None => "实时姿态流".to_string(),
                            }
                        } else {
                            "正在校准中立姿态…（保持头部静止）".to_string()
                        };
                    }
                    cx.notify();
                });
            }
        })
        .detach();
    }

    // 5 Hz poll for ZCode question card.
    {
        let state_ask = app_state.clone();
        cx.spawn(async move |cx| loop {
            cx.background_executor()
                .timer(Duration::from_millis(200))
                .await;
            let progress = crate::system::zcode_ask::ASK_HUB.progress_snapshot();
            let resolved = crate::system::zcode_ask::ASK_HUB.resolved_note();
            state_ask.update(cx, |s, cx| {
                if s.zcode_progress != progress || s.zcode_resolved != resolved {
                    s.zcode_progress = progress;
                    s.zcode_resolved = resolved;
                    cx.notify();
                }
            });
        })
        .detach();
    }
}

fn open_or_activate_window(
    app_state: &Entity<RustPodsState>,
    slot: &Arc<std::sync::Mutex<Option<AnyWindowHandle>>>,
    cx: &mut App,
) {
    if let Ok(mut guard) = slot.lock() {
        if let Some(handle) = guard.as_ref() {
            if handle
                .update(cx, |_, window, _| {
                    window.activate_window();
                })
                .is_ok()
            {
                return;
            }
            *guard = None;
        }
    }

    let state_for_win = app_state.clone();
    match gpui_kit::open_window(window_options(cx), cx, move |window, cx| {
        cx.new(|cx| MainWindow::new(state_for_win, window, cx))
    }) {
        Ok((handle, _)) => {
            if let Ok(mut guard) = slot.lock() {
                *guard = Some(handle);
            }
            let _ = handle.update(cx, |_, window, _| {
                window.activate_window();
            });
        }
        Err(e) => {
            error!("Failed to open GPUI main window: {}", e);
        }
    }
}

/// Drives the head-tracked audio stage for as long as poses arrive.
#[cfg(feature = "audio-follow")]
pub async fn run_audio_follow(
    pose_rx: tokio::sync::watch::Receiver<Option<crate::core::head_tracking::HeadPose>>,
) {
    use crate::system::audio_follow::{FilterPlan, HrtfClient, StageSchedule};

    let plan = FilterPlan::default();
    let mut client = match HrtfClient::start(&plan) {
        Ok(client) => client,
        Err(e) => {
            warn!("Head-tracked audio unavailable: {}", e);
            return;
        }
    };

    let mut schedule = StageSchedule::new();
    let mut rx = pose_rx;
    let started = std::time::Instant::now();
    let mut announced = false;

    loop {
        if rx.changed().await.is_err() {
            break;
        }

        if !announced && client.is_running() {
            info!("Head-tracked audio is live on '{}'", plan.node_name);
            announced = true;
        }

        let pose = *rx.borrow_and_update();
        let Some(pose) = pose else {
            schedule.reset();
            let now_ms = started.elapsed().as_millis() as u64;
            client.set_stage(schedule.commit([0.0, 0.0, 0.0], now_ms));
            continue;
        };

        let now_ms = started.elapsed().as_millis() as u64;
        let axes = [pose.yaw, pose.pitch, pose.roll];
        if schedule.should_update(axes, now_ms) {
            let stage = schedule.commit(axes, now_ms);
            client.set_stage(stage);
        }
    }

    client.shutdown();
}

/// Stub for builds without the feature, so the call site stays uniform.
#[cfg(not(feature = "audio-follow"))]
pub async fn run_audio_follow(
    _pose_rx: tokio::sync::watch::Receiver<Option<crate::core::head_tracking::HeadPose>>,
) {
}

pub async fn start_bluetooth_daemon(
    aacp: Arc<AacpManager>,
    dev_state: Arc<RwLock<Option<AirPodsDeviceState>>>,
    mpris: Arc<MprisController>,
    ui_tx: mpsc::UnboundedSender<UiEvent>,
    config: Arc<std::sync::RwLock<AppConfig>>,
) {
    info!("Starting RustPods Bluetooth Background Daemon...");

    crate::core::nod_counter::set_sensitivity(config.read().unwrap().gesture_sensitivity);

    let dev_state_le = dev_state.clone();
    let ui_tx_le = ui_tx.clone();
    let cfg_le = config.clone();
    tokio::spawn(async move {
        if let Err(e) =
            crate::system::le_monitor::start_le_monitor(dev_state_le, ui_tx_le, cfg_le).await
        {
            warn!("BLE Proximity Monitor error: {}", e);
        }
    });

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

    let mut was_disconnected = false;

    loop {
        info!("Searching for connected AirPods...");
        match bt_manager.find_connected_airpods().await {
            Ok(pods_list) if !pods_list.is_empty() => {
                let pod = pods_list[0].clone();
                info!(
                    "Found connected AirPods: {} ({}) - Model: {:?}",
                    pod.name, pod.address, pod.model
                );

                if was_disconnected {
                    // AirPods just woke up from the case and BR/EDR came up; give
                    // link encryption and audio profiles 1.5s to settle before
                    // opening PSM 0x1001 so the accessory answers the handshake.
                    tokio::time::sleep(tokio::time::Duration::from_millis(1500)).await;
                }

                let mut current_state =
                    AirPodsDeviceState::new(pod.address.to_string(), pod.name.clone(), pod.model);
                current_state.is_connected = false;
                current_state.ear_detection_enabled = config.read().unwrap().ear_detection_enabled;

                let mut event_rx = aacp.subscribe();

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

                {
                    let camera = aacp.clone();
                    tokio::spawn(async move {
                        camera
                            .claim_spatial_audio(
                                "io.github.rustpods.RustPods",
                                crate::core::spatial::SpatialMode::HeadTracked,
                                true,
                            )
                            .await;

                        match camera.start_head_tracking().await {
                            Ok(()) => {
                                for _ in 0..15 {
                                    tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                                    if camera.head_samples_seen().await > 0 {
                                        info!("Head tracking is producing samples");
                                        return;
                                    }
                                }
                                warn!(
                                    "No motion samples yet. If the compass is still \
                                     idle, put the buds in the case and take them out \
                                     again and restart RustPods."
                                );
                            }
                            Err(e) => warn!("Head tracking unavailable: {}", e),
                        }
                    });
                }

                let mut last_both_in_ear = false;

                {
                    let pose_rx = aacp.head_pose();
                    tokio::spawn(crate::system::zcode_ask::run_question_answers(pose_rx));
                }

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

                            if !old_wearing && new_wearing {
                                let _ = ui_tx.send(UiEvent::CaseLidClosed);
                            }

                            if !last_both_in_ear && current_state.is_wearing_both() {
                                let camera = aacp.clone();
                                tokio::spawn(async move {
                                    if camera.head_samples_seen().await == 0 {
                                        if !camera.has_motion_device().await {
                                            let _ = camera.subscribe_rtbuddy_motion().await;
                                        }
                                        let _ = camera.start_head_tracking().await;
                                    }
                                });
                            }

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

                        AacpEvent::ProximityKeys { irk, enc_key } => {
                            if let Ok(mut cfg) = config.write() {
                                let mut changed = false;
                                if let Some(k) = irk {
                                    let encoded = hex::encode(k);
                                    if cfg.proximity_irk.as_deref() != Some(&encoded) {
                                        cfg.proximity_irk = Some(encoded);
                                        changed = true;
                                    }
                                }
                                if let Some(k) = enc_key {
                                    let encoded = hex::encode(k);
                                    if cfg.proximity_enc_key.as_deref() != Some(&encoded) {
                                        cfg.proximity_enc_key = Some(encoded);
                                        changed = true;
                                    }
                                }
                                if changed {
                                    info!("Saved Apple Proximity keys (IRK/EncKey) for BLE lid detection");
                                    save_config(&cfg);
                                }
                            }
                        }

                        AacpEvent::Disconnected => {
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
                was_disconnected = true;
            }

            Ok(_) => {
                was_disconnected = true;
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
                let _ = ui_tx.send(UiEvent::ConnectedChanged(false));
            }

            Err(e) => {
                error!("Error querying Bluetooth devices: {}", e);
            }
        }

        tokio::time::sleep(tokio::time::Duration::from_secs(5)).await;
    }
}

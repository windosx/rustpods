use crate::core::aacp::commands::{MicrophoneMode, NoiseControlMode};
use crate::core::aacp::AacpManager;
use crate::core::device::AirPodsDeviceState;
use crate::core::head_tracking::{HeadGesture, HeadPose};
use crate::core::models::AirPodsModel;
use crate::system::config::{save_config, AppConfig, PauseBehavior};
use crate::system::zcode_ask::{self, AskProgress, AskReply};
use crate::ui::UiEvent;
use gpui_kit::component::ThemeMode;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{mpsc, RwLock};
use tracing::info;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum NavPage {
    #[default]
    Overview,
    Spatial,
    Controls,
    Agents,
    Preferences,
}

impl NavPage {
    pub const ALL: [NavPage; 5] = [
        NavPage::Overview,
        NavPage::Spatial,
        NavPage::Controls,
        NavPage::Agents,
        NavPage::Preferences,
    ];

    pub fn label(self) -> &'static str {
        match self {
            NavPage::Overview => "概览首页",
            NavPage::Spatial => "空间音频",
            NavPage::Controls => "控制与交互",
            NavPage::Agents => "智能体接入",
            NavPage::Preferences => "偏好设置",
        }
    }
}

#[derive(Debug, Clone)]
pub struct HeadTrackingViewState {
    pub pose: Option<HeadPose>,
    pub status: String,
    pub gesture: Option<HeadGesture>,
    pub rate_hz: Option<f32>,
    pub calibrated: bool,
}

impl Default for HeadTrackingViewState {
    fn default() -> Self {
        Self {
            pose: None,
            status: "等待数据流…".to_string(),
            gesture: None,
            rate_hz: None,
            calibrated: false,
        }
    }
}

/// Thread-safe handle to backend managers and the Tokio runtime so GPUI event
/// handlers can dispatch Bluetooth/AACP/config commands without depending on a
/// Tokio thread-local guard.
#[derive(Clone)]
pub struct RustPodsController {
    pub rt: tokio::runtime::Handle,
    pub aacp: Arc<AacpManager>,
    pub device_state: Arc<RwLock<Option<AirPodsDeviceState>>>,
    pub config: Arc<std::sync::RwLock<AppConfig>>,
    pub ui_tx: mpsc::UnboundedSender<UiEvent>,
}

impl RustPodsController {
    pub fn rename(&self, new_name: String) {
        let aacp = self.aacp.clone();
        self.rt.spawn(async move {
            let _ = aacp.rename(&new_name).await;
        });
    }

    pub fn connect(&self) {
        let dev_state = self.device_state.clone();
        self.rt.spawn(async move {
            let mac = {
                let guard = dev_state.read().await;
                guard.as_ref().map(|s| s.mac_address.clone())
            };
            let mac_addr = match mac {
                Some(m) => Some(m),
                None => {
                    if let Ok(bm) = crate::system::bluetooth::BluetoothManager::new().await {
                        if let Ok(paired) = bm.find_paired_airpods().await {
                            paired.first().map(|p| p.address.to_string())
                        } else {
                            None
                        }
                    } else {
                        None
                    }
                }
            };
            if let Some(mac_addr) = mac_addr {
                info!("Triggering connect to {}", mac_addr);
                let _ = tokio::process::Command::new("bluetoothctl")
                    .arg("connect")
                    .arg(&mac_addr)
                    .output()
                    .await;
            }
        });
    }

    pub fn set_noise_mode(&self, mode: NoiseControlMode) {
        let aacp = self.aacp.clone();
        let dev = self.device_state.clone();
        let tx = self.ui_tx.clone();
        self.rt.spawn(async move {
            let _ = aacp.set_noise_mode(mode).await;
            // The accessory never echoes the mode back, so without this the
            // window and tray would show the old mode until restart.
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
    }

    pub fn set_adaptive_strength(&self, val: u8) {
        let aacp = self.aacp.clone();
        let dev = self.device_state.clone();
        let tx = self.ui_tx.clone();
        self.rt.spawn(async move {
            let _ = aacp.set_adaptive_strength(val).await;
            let updated = {
                let mut guard = dev.write().await;
                match guard.as_mut() {
                    Some(state) => {
                        state.adaptive_strength = val;
                        Some(state.clone())
                    }
                    None => None,
                }
            };
            if let Some(state) = updated {
                let _ = tx.send(UiEvent::DeviceStateUpdated(state));
            }
        });
    }

    pub fn toggle_ear_detection(&self, enabled: bool) {
        if let Ok(mut c) = self.config.write() {
            c.ear_detection_enabled = enabled;
            save_config(&c);
        }
        let aacp = self.aacp.clone();
        let dev = self.device_state.clone();
        let tx = self.ui_tx.clone();
        self.rt.spawn(async move {
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
    }

    pub fn change_pause_behavior(&self, behavior: PauseBehavior) {
        if let Ok(mut c) = self.config.write() {
            c.pause_behavior = behavior;
            save_config(&c);
        }
    }

    pub fn toggle_conversational_awareness(&self, enabled: bool) {
        if let Ok(mut c) = self.config.write() {
            c.conversational_awareness_enabled = enabled;
            save_config(&c);
        }
        let aacp = self.aacp.clone();
        self.rt.spawn(async move {
            let _ = aacp.set_conversational_awareness(enabled).await;
        });
    }

    pub fn change_mic_mode(&self, mode: MicrophoneMode) {
        let aacp = self.aacp.clone();
        let dev = self.device_state.clone();
        let tx = self.ui_tx.clone();
        self.rt.spawn(async move {
            let _ = aacp.set_microphone_mode(mode).await;
            let updated = {
                let mut guard = dev.write().await;
                match guard.as_mut() {
                    Some(state) => {
                        state.mic_mode = mode;
                        Some(state.clone())
                    }
                    None => None,
                }
            };
            if let Some(state) = updated {
                let _ = tx.send(UiEvent::DeviceStateUpdated(state));
            }
        });
    }

    pub fn toggle_case_chime(&self, enabled: bool) {
        let aacp = self.aacp.clone();
        let dev = self.device_state.clone();
        let tx = self.ui_tx.clone();
        self.rt.spawn(async move {
            let _ = aacp.set_case_chime(enabled).await;
            let updated = {
                let mut guard = dev.write().await;
                match guard.as_mut() {
                    Some(state) => {
                        state.case_chime_enabled = enabled;
                        Some(state.clone())
                    }
                    None => None,
                }
            };
            if let Some(state) = updated {
                let _ = tx.send(UiEvent::DeviceStateUpdated(state));
            }
        });
    }

    pub fn toggle_close_to_tray(&self, enabled: bool) {
        if let Ok(mut c) = self.config.write() {
            c.close_to_tray = enabled;
            save_config(&c);
        }
    }

    pub fn toggle_case_popup(&self, enabled: bool) {
        if let Ok(mut c) = self.config.write() {
            c.case_popup_enabled = enabled;
            save_config(&c);
        }
    }

    pub fn preview_case_popup(&self) {
        let _ = self.ui_tx.send(UiEvent::CaseLidOpened);
    }

    pub fn toggle_head_gesture_answers(&self, enabled: bool) {
        if let Ok(mut c) = self.config.write() {
            c.head_gesture_answers_enabled = enabled;
            save_config(&c);
        }
    }

    pub fn set_gesture_sensitivity(&self, level: u8) -> u8 {
        let value = crate::core::nod_counter::set_sensitivity(level);
        if let Ok(mut c) = self.config.write() {
            c.gesture_sensitivity = value;
            save_config(&c);
        }
        value
    }

    pub fn start_zcode_test(&self) -> tokio::sync::oneshot::Receiver<Result<AskReply, String>> {
        let (tx, rx) = tokio::sync::oneshot::channel();
        self.rt.spawn(async move {
            let _ = tx.send(zcode_ask::run_test_question().await);
        });
        rx
    }
}

/// Reactive application state observed by any open `MainWindow`.
pub struct RustPodsState {
    pub device: AirPodsDeviceState,
    pub head_tracking: HeadTrackingViewState,
    pub zcode_progress: Option<AskProgress>,
    pub zcode_resolved: Option<String>,
    pub active_page: NavPage,
    pub theme_mode: ThemeMode,
    pub agent_notes: HashMap<&'static str, String>,
    pub agent_testing: HashMap<&'static str, bool>,
    pub controller: RustPodsController,
}

impl RustPodsState {
    pub fn new(controller: RustPodsController, theme_mode: ThemeMode) -> Self {
        let device = AirPodsDeviceState::new(
            "CC:68:E0:38:9C:E9".to_string(),
            "浩的AirPods".to_string(),
            AirPodsModel::AirPods3,
        );
        Self {
            device,
            head_tracking: HeadTrackingViewState::default(),
            zcode_progress: None,
            zcode_resolved: None,
            active_page: NavPage::Overview,
            theme_mode,
            agent_notes: HashMap::new(),
            agent_testing: HashMap::new(),
            controller,
        }
    }

    pub fn config_snapshot(&self) -> AppConfig {
        self.controller.config.read().unwrap().clone()
    }
}

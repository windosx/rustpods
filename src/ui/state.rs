use crate::core::aacp::commands::{MicrophoneMode, NoiseControlMode};
use crate::core::aacp::AacpManager;
use crate::core::device::AirPodsDeviceState;
use crate::core::head_tracking::{HeadGesture, HeadPose};
use crate::core::models::AirPodsModel;
use crate::system::config::{
    save_config, AppConfig, AppLanguage, AppThemePreference, HeadMouseMode, PauseBehavior,
    StemAction,
};
use crate::system::head_mouse::{HeadMouseCommand, HEAD_MOUSE};
use crate::system::zcode_ask::{self, AskProgress, AskReply, ResolvedNote};
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
    HeadMouse,
    Controls,
    Agents,
    Preferences,
}

impl NavPage {
    pub const ALL: [NavPage; 6] = [
        NavPage::Overview,
        NavPage::Spatial,
        NavPage::HeadMouse,
        NavPage::Controls,
        NavPage::Agents,
        NavPage::Preferences,
    ];

    pub fn label(self) -> String {
        match self {
            NavPage::Overview => rust_i18n::t!("nav.overview").to_string(),
            NavPage::Spatial => rust_i18n::t!("nav.spatial").to_string(),
            NavPage::HeadMouse => rust_i18n::t!("nav.head_mouse").to_string(),
            NavPage::Controls => rust_i18n::t!("nav.controls").to_string(),
            NavPage::Agents => rust_i18n::t!("nav.agents").to_string(),
            NavPage::Preferences => rust_i18n::t!("nav.preferences").to_string(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PreferencesTab {
    #[default]
    General,
    System,
    About,
}

impl PreferencesTab {
    pub const ALL: [PreferencesTab; 3] = [
        PreferencesTab::General,
        PreferencesTab::System,
        PreferencesTab::About,
    ];

    pub fn id(self) -> &'static str {
        match self {
            PreferencesTab::General => "general",
            PreferencesTab::System => "system",
            PreferencesTab::About => "about",
        }
    }

    pub fn label(self) -> String {
        match self {
            PreferencesTab::General => rust_i18n::t!("preferences.tab_general").to_string(),
            PreferencesTab::System => rust_i18n::t!("preferences.tab_system").to_string(),
            PreferencesTab::About => rust_i18n::t!("preferences.tab_about").to_string(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SettingsFeedback {
    pub message: String,
    pub is_error: bool,
    pub seq: u64,
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
            status: rust_i18n::t!("spatial.waiting_stream").to_string(),
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
        let tx = self.ui_tx.clone();
        self.rt.spawn(async move {
            let mac = {
                let guard = dev_state.read().await;
                guard
                    .as_ref()
                    .map(|s| s.mac_address.clone())
                    .filter(|m| m != "00:00:00:00:00:00")
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
            let Some(mac_addr) = mac_addr else {
                let _ = tx.send(UiEvent::ConnectAttemptFinished);
                return;
            };
            info!("Triggering connect to {}", mac_addr);
            let ok = if let Ok(parsed) = mac_addr.parse::<crate::core::models::MacAddress>() {
                match crate::system::bluetooth::BluetoothManager::new().await {
                    Ok(bm) => match bm.ensure_connected(parsed).await {
                        Ok(()) => true,
                        Err(e) => {
                            tracing::warn!("Bluetooth connect failed ({}): {}", mac_addr, e);
                            false
                        }
                    },
                    Err(e) => {
                        tracing::warn!("BluetoothManager unavailable: {}", e);
                        false
                    }
                }
            } else {
                false
            };
            if ok {
                // Give the Bluetooth daemon poll cycle time to observe the connection
                // and emit ConnectedChanged(true) before clearing the loading state.
                tokio::time::sleep(std::time::Duration::from_secs(7)).await;
            }
            let _ = tx.send(UiEvent::ConnectAttemptFinished);
        });
    }

    pub fn set_noise_mode(&self, mode: NoiseControlMode) {
        let aacp = self.aacp.clone();
        let dev = self.device_state.clone();
        let tx = self.ui_tx.clone();
        self.rt.spawn(async move {
            crate::system::tray::apply_noise_mode(&aacp, &dev, &tx, mode).await;
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
        let aacp = self.aacp.clone();
        let cfg = self.config.clone();
        let dev = self.device_state.clone();
        let tx = self.ui_tx.clone();
        self.rt.spawn(async move {
            crate::system::tray::apply_ear_detection_toggle(&aacp, &cfg, &dev, &tx, enabled).await;
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

    pub fn toggle_start_minimized(&self, enabled: bool) {
        if let Ok(mut c) = self.config.write() {
            c.start_minimized = enabled;
            save_config(&c);
        }
    }

    pub fn set_theme_mode(&self, mode: AppThemePreference) {
        if let Ok(mut c) = self.config.write() {
            c.theme_mode = mode;
            save_config(&c);
        }
    }

    pub fn reset_config_to_defaults(&self) -> AppConfig {
        let defaults = AppConfig::default();
        if let Ok(mut c) = self.config.write() {
            *c = defaults.clone();
            save_config(&c);
        }
        crate::core::nod_counter::set_sensitivity(defaults.gesture_sensitivity);
        let loc = defaults.language.resolved_locale();
        rust_i18n::set_locale(loc);
        gpui_kit::component::set_locale(loc);
        HEAD_MOUSE.send_command(HeadMouseCommand::SyncConfig);
        defaults
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
            c.gesture_permission_answers_enabled = enabled;
            save_config(&c);
        }
    }

    pub fn toggle_gesture_permission_answers(&self, enabled: bool) {
        if let Ok(mut c) = self.config.write() {
            c.gesture_permission_answers_enabled = enabled;
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

    pub fn start_agent_test(&self, agent_name: &str) -> tokio::sync::oneshot::Receiver<Result<AskReply, String>> {
        self.start_agent_permission_test(agent_name)
    }

    pub fn start_agent_permission_test(&self, agent_name: &str) -> tokio::sync::oneshot::Receiver<Result<AskReply, String>> {
        let (tx, rx) = tokio::sync::oneshot::channel();
        let agent = agent_name.to_string();
        self.rt.spawn(async move {
            let _ = tx.send(zcode_ask::run_test_permission(&agent).await);
        });
        rx
    }

    pub fn start_zcode_test(&self) -> tokio::sync::oneshot::Receiver<Result<AskReply, String>> {
        self.start_agent_test("ZCode")
    }

    pub fn toggle_head_mouse(&self, enabled: bool) {
        if let Ok(mut c) = self.config.write() {
            c.head_mouse_enabled = enabled;
            save_config(&c);
        }
        HEAD_MOUSE.send_command(HeadMouseCommand::SyncConfig);
    }

    pub fn set_head_mouse_mode(&self, mode: HeadMouseMode) {
        if let Ok(mut c) = self.config.write() {
            c.head_mouse_mode = mode;
            save_config(&c);
        }
    }

    pub fn set_head_mouse_speed(&self, speed: f32) {
        if let Ok(mut c) = self.config.write() {
            c.head_mouse_speed = speed;
            save_config(&c);
        }
    }

    pub fn set_head_mouse_accel(&self, accel: f32) {
        if let Ok(mut c) = self.config.write() {
            c.head_mouse_accel = accel;
            save_config(&c);
        }
    }

    pub fn set_head_mouse_smoothing(&self, smoothing: f32) {
        if let Ok(mut c) = self.config.write() {
            c.head_mouse_smoothing = smoothing;
            save_config(&c);
        }
    }

    pub fn set_head_mouse_deadzone(&self, deadzone: f32) {
        if let Ok(mut c) = self.config.write() {
            c.head_mouse_deadzone = deadzone;
            save_config(&c);
        }
    }

    pub fn toggle_head_mouse_invert_x(&self, invert: bool) {
        if let Ok(mut c) = self.config.write() {
            c.head_mouse_invert_x = invert;
            save_config(&c);
        }
    }

    pub fn toggle_head_mouse_invert_y(&self, invert: bool) {
        if let Ok(mut c) = self.config.write() {
            c.head_mouse_invert_y = invert;
            save_config(&c);
        }
    }

    pub fn toggle_head_mouse_roll_click(&self, enabled: bool) {
        if let Ok(mut c) = self.config.write() {
            c.head_mouse_roll_click = enabled;
            save_config(&c);
        }
    }

    pub fn toggle_head_mouse_dwell_click(&self, enabled: bool) {
        if let Ok(mut c) = self.config.write() {
            c.head_mouse_dwell_click = enabled;
            save_config(&c);
        }
    }

    pub fn set_stem_action(&self, press_type: crate::core::aacp::events::StemPressType, action: StemAction) {
        if let Ok(mut c) = self.config.write() {
            match press_type {
                crate::core::aacp::events::StemPressType::Single => c.stem_single_click = action,
                crate::core::aacp::events::StemPressType::Double => c.stem_double_click = action,
                crate::core::aacp::events::StemPressType::Triple => c.stem_triple_click = action,
                crate::core::aacp::events::StemPressType::Long => c.stem_long_press = action,
            }
            save_config(&c);
        }
    }

    pub fn head_mouse_recenter(&self) {
        HEAD_MOUSE.send_command(HeadMouseCommand::Recenter);
    }

    pub fn head_mouse_toggle_pause(&self) {
        HEAD_MOUSE.send_command(HeadMouseCommand::TogglePause);
    }

    pub fn set_language(&self, lang: AppLanguage) {
        if let Ok(mut c) = self.config.write() {
            c.language = lang;
            save_config(&c);
        }
        let loc = lang.resolved_locale();
        rust_i18n::set_locale(loc);
        gpui_kit::component::set_locale(loc);
    }

    pub fn check_for_updates(&self) {
        let tx = self.ui_tx.clone();
        let _ = tx.send(UiEvent::UpdateStateChanged(
            crate::system::updater::UpdateState::Checking,
        ));
        self.rt.spawn(async move {
            match crate::system::updater::check_for_update().await {
                Ok(Some((latest, url, changelog))) => {
                    let is_pkg = crate::system::updater::is_package_managed();
                    let _ = tx.send(UiEvent::UpdateStateChanged(
                        crate::system::updater::UpdateState::UpdateAvailable {
                            current_version: env!("CARGO_PKG_VERSION").to_string(),
                            latest_version: latest,
                            release_url: url,
                            changelog,
                            is_package_managed: is_pkg,
                            can_self_update: crate::system::updater::can_self_update(),
                        },
                    ));
                }
                Ok(None) => {
                    let _ = tx.send(UiEvent::UpdateStateChanged(
                        crate::system::updater::UpdateState::UpToDate {
                            version: env!("CARGO_PKG_VERSION").to_string(),
                        },
                    ));
                }
                Err(e) => {
                    let _ = tx.send(UiEvent::UpdateStateChanged(
                        crate::system::updater::UpdateState::Error(e),
                    ));
                }
            }
        });
    }

    /// Downloads and installs the pending release in the background. The
    /// installer is the same `rustpods --update` path the CLI uses, so the
    /// package-manager guard and checksum handling stay in one place.
    pub fn install_update(&self, latest_version: String) {
        let tx = self.ui_tx.clone();
        let _ = tx.send(UiEvent::UpdateStateChanged(
            crate::system::updater::UpdateState::Installing {
                latest_version: latest_version.clone(),
            },
        ));
        self.rt.spawn(async move {
            match crate::system::updater::install_update().await {
                Ok(()) => {
                    let _ = tx.send(UiEvent::UpdateStateChanged(
                        crate::system::updater::UpdateState::Updated {
                            version: latest_version,
                        },
                    ));
                }
                Err(e) => {
                    let _ = tx.send(UiEvent::UpdateStateChanged(
                        crate::system::updater::UpdateState::Error(e),
                    ));
                }
            }
        });
    }

    /// Relaunches the freshly installed binary after this process releases the
    /// single-instance lock. A detached shell waits for the current PID to
    /// disappear and then execs the installed path, so the new process starts
    /// with the lock already free.
    pub fn restart_app(&self) {
        let exe = crate::system::updater::restart_binary_path();
        let exe = exe.to_string_lossy().into_owned();
        let pid = std::process::id().to_string();
        let spawned = std::process::Command::new("sh")
            .args([
                "-c",
                "while kill -0 \"$1\" 2>/dev/null; do sleep 0.2; done; exec \"$2\"",
                "rustpods-restart",
                &pid,
                &exe,
            ])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .is_ok();
        if spawned {
            let _ = self.ui_tx.send(UiEvent::Quit);
        } else {
            tracing::warn!("Failed to spawn the RustPods restart helper");
        }
    }
}

/// Reactive application state observed by any open `MainWindow`.
pub struct RustPodsState {
    pub device: AirPodsDeviceState,
    pub is_connecting: bool,
    pub head_tracking: HeadTrackingViewState,
    pub zcode_progress: Option<AskProgress>,
    pub zcode_resolved: Option<ResolvedNote>,
    pub active_page: NavPage,
    pub preferences_tab: PreferencesTab,
    pub diagnostics_expanded: bool,
    pub confirm_reset: bool,
    pub reset_confirm_seq: u64,
    pub settings_feedback: Option<SettingsFeedback>,
    pub feedback_seq: u64,
    pub config_revision: u64,
    pub theme_mode: ThemeMode,
    pub update_state: crate::system::updater::UpdateState,
    pub agent_notes: HashMap<&'static str, String>,
    pub agent_testing: HashMap<&'static str, bool>,
    pub controller: RustPodsController,
}

impl RustPodsState {
    pub fn new(controller: RustPodsController, theme_mode: ThemeMode) -> Self {
        // Neutral placeholder: BlueZ populates the real device within seconds
        // of startup. (This used to carry a developer's personal MAC and
        // device name, which every other user saw as their first frame.)
        let device = AirPodsDeviceState::new(
            "00:00:00:00:00:00".to_string(),
            "AirPods".to_string(),
            AirPodsModel::AirPods3,
        );
        Self {
            device,
            is_connecting: false,
            head_tracking: HeadTrackingViewState::default(),
            zcode_progress: None,
            zcode_resolved: None,
            active_page: NavPage::Overview,
            preferences_tab: PreferencesTab::General,
            diagnostics_expanded: false,
            confirm_reset: false,
            reset_confirm_seq: 0,
            settings_feedback: None,
            feedback_seq: 0,
            config_revision: 0,
            theme_mode,
            update_state: crate::system::updater::UpdateState::default(),
            agent_notes: HashMap::new(),
            agent_testing: HashMap::new(),
            controller,
        }
    }

    pub fn config_snapshot(&self) -> AppConfig {
        self.controller.config.read().unwrap().clone()
    }

    pub fn set_preferences_tab(&mut self, tab: PreferencesTab) {
        self.preferences_tab = tab;
        if tab != PreferencesTab::About {
            self.confirm_reset = false;
        }
    }

    pub fn toggle_diagnostics_expanded(&mut self) {
        self.diagnostics_expanded = !self.diagnostics_expanded;
    }

    pub fn request_reset_confirm(&mut self) -> u64 {
        self.confirm_reset = true;
        self.reset_confirm_seq = self.reset_confirm_seq.wrapping_add(1);
        self.reset_confirm_seq
    }

    pub fn cancel_reset_confirm(&mut self) {
        self.confirm_reset = false;
    }

    pub fn expire_reset_confirm(&mut self, seq: u64) {
        if self.reset_confirm_seq == seq {
            self.confirm_reset = false;
        }
    }

    pub fn set_settings_feedback(&mut self, message: impl Into<String>, is_error: bool) -> u64 {
        self.feedback_seq = self.feedback_seq.wrapping_add(1);
        let seq = self.feedback_seq;
        self.settings_feedback = Some(SettingsFeedback {
            message: message.into(),
            is_error,
            seq,
        });
        seq
    }

    pub fn notify_saved(&mut self) -> u64 {
        let msg = rust_i18n::t!("settings.saved").to_string();
        self.set_settings_feedback(msg, false)
    }

    pub fn clear_settings_feedback(&mut self, expected_seq: Option<u64>) -> bool {
        match expected_seq {
            Some(seq) => {
                if self
                    .settings_feedback
                    .as_ref()
                    .is_some_and(|f| f.seq == seq)
                {
                    self.settings_feedback = None;
                    true
                } else {
                    false
                }
            }
            None => {
                let had = self.settings_feedback.is_some();
                self.settings_feedback = None;
                had
            }
        }
    }

    pub fn apply_theme_mode(&mut self, mode: ThemeMode) {
        self.theme_mode = mode;
        let pref = if mode.is_dark() {
            AppThemePreference::Dark
        } else {
            AppThemePreference::Light
        };
        self.controller.set_theme_mode(pref);
    }

    pub fn reset_config_to_defaults(&mut self) -> AppConfig {
        let defaults = self.controller.reset_config_to_defaults();
        self.theme_mode = if defaults.theme_mode.is_dark() {
            ThemeMode::Dark
        } else {
            ThemeMode::Light
        };
        self.confirm_reset = false;
        self.config_revision = self.config_revision.wrapping_add(1);
        let msg = rust_i18n::t!("settings.reset_done").to_string();
        self.set_settings_feedback(msg, false);
        defaults
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_state() -> RustPodsState {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let (ui_tx, _ui_rx) = mpsc::unbounded_channel();
        let controller = RustPodsController {
            rt: rt.handle().clone(),
            aacp: Arc::new(AacpManager::new()),
            device_state: Arc::new(RwLock::new(None)),
            config: Arc::new(std::sync::RwLock::new(AppConfig::default())),
            ui_tx,
        };
        // Leak runtime handle only in test helper? Better: keep runtime alive via Box::leak is unnecessary if we don't spawn.
        std::mem::forget(rt);
        RustPodsState::new(controller, ThemeMode::Dark)
    }

    #[test]
    fn preferences_tab_switch_resets_confirm_flag() {
        let mut state = test_state();
        assert_eq!(state.preferences_tab, PreferencesTab::General);
        assert!(!state.confirm_reset);

        state.set_preferences_tab(PreferencesTab::About);
        let seq = state.request_reset_confirm();
        assert!(state.confirm_reset);
        assert_eq!(seq, 1);

        // Switching away from About clears pending reset confirmation.
        state.set_preferences_tab(PreferencesTab::System);
        assert!(!state.confirm_reset);
    }

    #[test]
    fn reset_confirm_expiry_respects_sequence() {
        let mut state = test_state();
        let seq1 = state.request_reset_confirm();
        let seq2 = state.request_reset_confirm();
        assert!(state.confirm_reset);

        // Stale timer must not clear a newer confirmation request.
        state.expire_reset_confirm(seq1);
        assert!(state.confirm_reset);

        state.expire_reset_confirm(seq2);
        assert!(!state.confirm_reset);
    }

    #[test]
    fn settings_feedback_clear_respects_sequence() {
        let mut state = test_state();
        let seq1 = state.set_settings_feedback("Saved", false);
        let seq2 = state.set_settings_feedback("Reset", false);

        state.clear_settings_feedback(Some(seq1));
        assert!(state.settings_feedback.is_some());

        state.clear_settings_feedback(Some(seq2));
        assert!(state.settings_feedback.is_none());
    }

    #[test]
    fn diagnostics_toggle_flips_state() {
        let mut state = test_state();
        assert!(!state.diagnostics_expanded);
        state.toggle_diagnostics_expanded();
        assert!(state.diagnostics_expanded);
        state.toggle_diagnostics_expanded();
        assert!(!state.diagnostics_expanded);
    }
}

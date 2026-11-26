use super::aacp::commands::{MicrophoneMode, NoiseControlMode};
use super::aacp::events::{BatteryComponent, BatteryStatus, EarStatus};
use super::models::{AirPodsModel, ModelCapabilities};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AirPodsDeviceState {
    pub mac_address: String,
    pub name: String,
    pub model: AirPodsModel,
    pub capabilities: ModelCapabilities,
    /// Whether `model` was guessed from the Bluetooth device name rather than
    /// confirmed from a hardware PID / modalias / BLE proximity packet.
    #[serde(default)]
    pub model_inferred: bool,
    pub is_connected: bool,

    // Battery
    pub battery_left: Option<u8>,
    pub battery_left_charging: bool,
    pub battery_right: Option<u8>,
    pub battery_right_charging: bool,
    pub battery_case: Option<u8>,
    pub battery_case_charging: bool,

    // In-Ear
    pub left_in_ear: EarStatus,
    pub right_in_ear: EarStatus,

    // Noise Control
    pub noise_mode: NoiseControlMode,
    pub adaptive_strength: u8,
    pub conversational_awareness: bool,

    // Settings
    pub ear_detection_enabled: bool,
    pub mic_mode: MicrophoneMode,
    pub case_chime_enabled: bool,
}

impl AirPodsDeviceState {
    pub fn new(mac_address: String, name: String, model: AirPodsModel) -> Self {
        let capabilities = model.capabilities();
        Self {
            mac_address,
            name,
            model,
            capabilities,
            model_inferred: false,
            is_connected: false,

            battery_left: None,
            battery_left_charging: false,
            battery_right: None,
            battery_right_charging: false,
            battery_case: None,
            battery_case_charging: false,

            left_in_ear: EarStatus::Disconnected,
            right_in_ear: EarStatus::Disconnected,

            noise_mode: NoiseControlMode::Off,
            adaptive_strength: 50,
            conversational_awareness: false,

            ear_detection_enabled: true,
            mic_mode: MicrophoneMode::Automatic,
            case_chime_enabled: true,
        }
    }

    pub fn is_wearing_any(&self) -> bool {
        self.left_in_ear.is_in_ear() || self.right_in_ear.is_in_ear()
    }

    pub fn is_wearing_both(&self) -> bool {
        self.left_in_ear.is_in_ear() && self.right_in_ear.is_in_ear()
    }

    pub fn clear_battery(&mut self) {
        self.battery_left = None;
        self.battery_left_charging = false;
        self.battery_right = None;
        self.battery_right_charging = false;
        self.battery_case = None;
        self.battery_case_charging = false;
    }

    pub fn mark_disconnected(&mut self) {
        self.is_connected = false;
        self.clear_battery();
        self.left_in_ear = EarStatus::Disconnected;
        self.right_in_ear = EarStatus::Disconnected;
    }

    pub fn update_battery(
        &mut self,
        comp: BatteryComponent,
        level: u8,
        status: BatteryStatus,
    ) {
        let charging = matches!(status, BatteryStatus::Charging);
        match comp {
            BatteryComponent::Left => {
                if status == BatteryStatus::Disconnected {
                    if self.left_in_ear == EarStatus::Disconnected {
                        self.battery_left = None;
                        self.battery_left_charging = false;
                    }
                } else {
                    self.battery_left = Some(level);
                    self.battery_left_charging = charging;
                }
            }
            BatteryComponent::Right => {
                if status == BatteryStatus::Disconnected {
                    if self.right_in_ear == EarStatus::Disconnected {
                        self.battery_right = None;
                        self.battery_right_charging = false;
                    }
                } else {
                    self.battery_right = Some(level);
                    self.battery_right_charging = charging;
                }
            }
            BatteryComponent::Case => {
                if status != BatteryStatus::Disconnected {
                    self.battery_case = Some(level);
                    self.battery_case_charging = charging;
                }
            }
            BatteryComponent::Headphone => {
                // For AirPods Max or combined
                if status == BatteryStatus::Disconnected {
                    self.battery_left = None;
                    self.battery_left_charging = false;
                } else {
                    self.battery_left = Some(level);
                    self.battery_left_charging = charging;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mark_disconnected_clears_connection_battery_and_ear_status() {
        let mut dev = AirPodsDeviceState::new(
            "AA:BB:CC:DD:EE:FF".to_string(),
            "AirPods".to_string(),
            AirPodsModel::AirPodsPro2UsbC,
        );
        dev.is_connected = true;
        dev.battery_left = Some(85);
        dev.battery_left_charging = true;
        dev.battery_right = Some(90);
        dev.battery_right_charging = false;
        dev.battery_case = Some(70);
        dev.battery_case_charging = true;
        dev.left_in_ear = EarStatus::InEar;
        dev.right_in_ear = EarStatus::InEar;

        dev.mark_disconnected();

        assert!(!dev.is_connected);
        assert_eq!(dev.battery_left, None);
        assert!(!dev.battery_left_charging);
        assert_eq!(dev.battery_right, None);
        assert!(!dev.battery_right_charging);
        assert_eq!(dev.battery_case, None);
        assert!(!dev.battery_case_charging);
        assert_eq!(dev.left_in_ear, EarStatus::Disconnected);
        assert_eq!(dev.right_in_ear, EarStatus::Disconnected);
    }

    #[test]
    fn update_battery_ignores_transient_disconnect_when_in_case_and_clears_when_ear_disconnected() {
        let mut dev = AirPodsDeviceState::new(
            "AA:BB:CC:DD:EE:FF".to_string(),
            "AirPods".to_string(),
            AirPodsModel::AirPodsPro2UsbC,
        );
        dev.left_in_ear = EarStatus::InCase;
        dev.right_in_ear = EarStatus::InCase;
        dev.update_battery(BatteryComponent::Left, 80, BatteryStatus::Charging);
        dev.update_battery(BatteryComponent::Right, 70, BatteryStatus::Charging);
        dev.update_battery(BatteryComponent::Case, 60, BatteryStatus::NotCharging);

        // Transient Disconnected/0xFF while still in case must keep previous reading.
        dev.update_battery(BatteryComponent::Right, 100, BatteryStatus::Disconnected);
        dev.update_battery(BatteryComponent::Case, 100, BatteryStatus::Disconnected);
        assert_eq!(dev.battery_right, Some(70));
        assert_eq!(dev.battery_case, Some(60));

        // When ear status is also Disconnected, clearing battery is allowed.
        dev.left_in_ear = EarStatus::Disconnected;
        dev.update_battery(BatteryComponent::Left, 100, BatteryStatus::Disconnected);
        assert_eq!(dev.battery_left, None);
        assert!(!dev.battery_left_charging);
    }
}

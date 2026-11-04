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

    pub fn update_battery(
        &mut self,
        comp: BatteryComponent,
        level: u8,
        status: BatteryStatus,
    ) {
        let charging = matches!(status, BatteryStatus::Charging);
        match comp {
            BatteryComponent::Left => {
                if status != BatteryStatus::Disconnected {
                    self.battery_left = Some(level);
                    self.battery_left_charging = charging;
                }
            }
            BatteryComponent::Right => {
                if status != BatteryStatus::Disconnected {
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
                if status != BatteryStatus::Disconnected {
                    self.battery_left = Some(level);
                    self.battery_left_charging = charging;
                }
            }
        }
    }
}

use serde::{Deserialize, Serialize};

#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ControlCommandId {
    MicMode = 0x01,
    ButtonSendMode = 0x05,
    OwnsConnection = 0x06,
    EarDetection = 0x0A,
    ListeningMode = 0x0D,
    SingleClickMode = 0x14,
    DoubleClickMode = 0x15,
    ClickHoldMode = 0x16,
    DoubleClickInterval = 0x17,
    ClickHoldInterval = 0x18,
    ListeningModeConfigs = 0x1A,
    OneBudAncMode = 0x1B,
    ChimeVolume = 0x1F,
    CallManagementConfig = 0x24,
    AdaptiveVolumeConfig = 0x26,
    SoftwareMuteConfig = 0x27,
    ConversationDetectConfig = 0x28,
    HearingAid = 0x2C,
    AutoAncStrength = 0x2E,
    InCaseToneConfig = 0x31,
    AllowOffOption = 0x34,
    StemConfig = 0x39,
    InCaseToneVolume = 0x40,
}

impl ControlCommandId {
    pub fn from_u8(val: u8) -> Option<Self> {
        match val {
            0x01 => Some(Self::MicMode),
            0x05 => Some(Self::ButtonSendMode),
            0x06 => Some(Self::OwnsConnection),
            0x0A => Some(Self::EarDetection),
            0x0D => Some(Self::ListeningMode),
            0x14 => Some(Self::SingleClickMode),
            0x15 => Some(Self::DoubleClickMode),
            0x16 => Some(Self::ClickHoldMode),
            0x17 => Some(Self::DoubleClickInterval),
            0x18 => Some(Self::ClickHoldInterval),
            0x1A => Some(Self::ListeningModeConfigs),
            0x1B => Some(Self::OneBudAncMode),
            0x1F => Some(Self::ChimeVolume),
            0x24 => Some(Self::CallManagementConfig),
            0x26 => Some(Self::AdaptiveVolumeConfig),
            0x27 => Some(Self::SoftwareMuteConfig),
            0x28 => Some(Self::ConversationDetectConfig),
            0x2C => Some(Self::HearingAid),
            0x2E => Some(Self::AutoAncStrength),
            0x31 => Some(Self::InCaseToneConfig),
            0x34 => Some(Self::AllowOffOption),
            0x39 => Some(Self::StemConfig),
            0x40 => Some(Self::InCaseToneVolume),
            _ => None,
        }
    }
}

#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum NoiseControlMode {
    Off = 0x01,
    NoiseCancellation = 0x02,
    Transparency = 0x03,
    Adaptive = 0x04,
}

impl NoiseControlMode {
    pub fn from_u8(val: u8) -> Self {
        match val {
            0x01 => Self::Off,
            0x02 => Self::NoiseCancellation,
            0x03 => Self::Transparency,
            0x04 => Self::Adaptive,
            _ => Self::Off,
        }
    }

    pub fn to_u8(self) -> u8 {
        self as u8
    }
}

#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum MicrophoneMode {
    Automatic = 0x00,
    AlwaysRight = 0x01,
    AlwaysLeft = 0x02,
}

impl MicrophoneMode {
    pub fn from_u8(val: u8) -> Self {
        match val {
            0x01 => Self::AlwaysRight,
            0x02 => Self::AlwaysLeft,
            _ => Self::Automatic,
        }
    }
}

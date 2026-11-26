use super::commands::ControlCommandId;
use super::packets::VersionNegotiation;
use serde::{Deserialize, Serialize};

#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum BatteryComponent {
    Headphone = 0x01,
    Right = 0x02,
    Left = 0x04,
    Case = 0x08,
}

impl BatteryComponent {
    pub fn from_u8(val: u8) -> Option<Self> {
        match val {
            0x01 => Some(Self::Headphone),
            0x02 => Some(Self::Right),
            0x04 => Some(Self::Left),
            0x08 => Some(Self::Case),
            _ => None,
        }
    }
}

#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum BatteryStatus {
    Charging = 0x01,
    NotCharging = 0x02,
    Disconnected = 0x04,
}

impl BatteryStatus {
    pub fn from_u8(val: u8) -> Self {
        match val {
            0x01 => Self::Charging,
            0x02 => Self::NotCharging,
            0x04 => Self::Disconnected,
            // Unknown/unset status bytes (e.g. 0x00 in transient frames) must not
            // be treated as a valid NotCharging reading.
            _ => Self::Disconnected,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SingleBatteryInfo {
    pub component: BatteryComponent,
    pub level: u8,
    pub status: BatteryStatus,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum EarStatus {
    InEar = 0x00,
    OutOfEar = 0x01,
    InCase = 0x02,
    Disconnected = 0x03,
}

impl EarStatus {
    pub fn from_u8(val: u8) -> Self {
        match val {
            0x00 => Self::InEar,
            0x01 => Self::OutOfEar,
            0x02 => Self::InCase,
            _ => Self::Disconnected,
        }
    }

    pub fn is_in_ear(&self) -> bool {
        matches!(self, Self::InEar)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct EarDetectionEvent {
    pub left: EarStatus,
    pub right: EarStatus,
}

#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum StemPressType {
    Single = 0x05,
    Double = 0x06,
    Triple = 0x07,
    Long = 0x08,
}

impl StemPressType {
    pub fn from_u8(val: u8) -> Option<Self> {
        match val {
            0x05 => Some(Self::Single),
            0x06 => Some(Self::Double),
            0x07 => Some(Self::Triple),
            0x08 => Some(Self::Long),
            _ => None,
        }
    }
}

#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum StemBud {
    Left = 0x01,
    Right = 0x02,
}

impl StemBud {
    pub fn from_u8(val: u8) -> Option<Self> {
        match val {
            0x01 => Some(Self::Left),
            0x02 => Some(Self::Right),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum AacpEvent {
    Battery(Vec<SingleBatteryInfo>),
    EarDetection(EarDetectionEvent),
    StemPress {
        press_type: StemPressType,
        bud: StemBud,
    },
    ControlCommand {
        id: ControlCommandId,
        data: Vec<u8>,
    },
    ConversationalAwareness(bool),
    Renamed(String),
    /// The accessory announced its AACP version and capability flags. The host
    /// must echo them back before the optional sensor services become usable.
    VersionNegotiation(VersionNegotiation),
    /// A raw `SensorV2` head-tracking sample from the RTBuddy channel.
    ///
    /// Deliberately unparsed: field offsets are absolute within the received
    /// frame, so whoever owns the [`HeadTracker`](crate::core::head_tracking::HeadTracker)
    /// needs the bytes exactly as they arrived.
    HeadSensor(Vec<u8>),
    /// Proximity pairing keys (IRK + encryption key) returned by `PROXIMITY_KEYS_RSP`.
    ProximityKeys {
        irk: Option<[u8; 16]>,
        enc_key: Option<[u8; 16]>,
    },
    Disconnected,
}

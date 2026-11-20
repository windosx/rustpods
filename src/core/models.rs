use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AirPodsModel {
    AirPods1,
    AirPods2,
    AirPods3,
    AirPods4,
    AirPods4Anc,
    AirPodsPro1,
    AirPodsPro2Lightning,
    AirPodsPro2UsbC,
    AirPodsMax,
    AirPodsMaxUsbC,
    Unknown(u16),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelCapabilities {
    pub model: AirPodsModel,
    pub name: String,
    pub has_anc: bool,
    pub has_adaptive_audio: bool,
    pub has_transparency: bool,
    pub has_conversational_awareness: bool,
    pub has_case_speaker: bool,
    pub has_volume_swipe: bool,
    pub has_in_ear_detection: bool,
    pub icon_name: String,
}

impl AirPodsModel {
    pub fn from_product_id(pid: u16) -> Self {
        match pid {
            0x2002 => Self::AirPods1,
            0x200F => Self::AirPods2,
            0x2013 => Self::AirPods3,
            // 0x2024 (8228) is AirPods Pro 2nd gen (USB-C), per the published
            // Bluetooth PID tables. Mapping it to AirPods 4 hid every noise
            // control from Pro 2 owners. The AirPods 4 (non-ANC) product id is
            // deliberately unmapped - the id that used to sit here belongs to
            // the Pro 2, and 0x201B below is the AirPods 4 with ANC.
            0x2024 => Self::AirPodsPro2UsbC,
            0x201B => Self::AirPods4Anc,
            0x200E => Self::AirPodsPro1,
            0x2014 => Self::AirPodsPro2Lightning,
            0x2029 | 0x2030 => Self::AirPodsPro2UsbC,
            0x200A => Self::AirPodsMax,
            0x201F => Self::AirPodsMaxUsbC,
            other => Self::Unknown(other),
        }
    }

    pub fn infer_from_name(name: &str) -> Option<Self> {
        let lower = name.to_lowercase();
        if lower.contains("max") {
            Some(Self::AirPodsMax)
        } else if lower.contains("pro 2") || lower.contains("pro (2") {
            Some(Self::AirPodsPro2UsbC)
        } else if lower.contains("pro") {
            Some(Self::AirPodsPro1)
        } else if lower.contains("airpods 4") || lower.contains("airpods (4") {
            Some(Self::AirPods4Anc)
        } else if lower.contains("airpods 3") || lower.contains("airpods (3") {
            Some(Self::AirPods3)
        } else if lower.contains("airpods 2") || lower.contains("airpods (2") {
            Some(Self::AirPods2)
        } else if lower.contains("airpods 1") || lower.contains("airpods (1") {
            Some(Self::AirPods1)
        } else if lower.contains("airpods") {
            Some(Self::AirPods3)
        } else {
            None
        }
    }

    pub fn capabilities(&self) -> ModelCapabilities {
        match self {
            Self::AirPods1 => ModelCapabilities {
                model: *self,
                name: "AirPods (第 1 代)".to_string(),
                has_anc: false,
                has_adaptive_audio: false,
                has_transparency: false,
                has_conversational_awareness: false,
                has_case_speaker: false,
                has_volume_swipe: false,
                has_in_ear_detection: true,
                icon_name: "airpods-gen1".to_string(),
            },
            Self::AirPods2 => ModelCapabilities {
                model: *self,
                name: "AirPods (第 2 代)".to_string(),
                has_anc: false,
                has_adaptive_audio: false,
                has_transparency: false,
                has_conversational_awareness: false,
                has_case_speaker: false,
                has_volume_swipe: false,
                has_in_ear_detection: true,
                icon_name: "airpods-gen2".to_string(),
            },
            Self::AirPods3 => ModelCapabilities {
                model: *self,
                name: "AirPods (第 3 代)".to_string(),
                has_anc: false,
                has_adaptive_audio: false,
                has_transparency: false,
                has_conversational_awareness: false,
                has_case_speaker: false,
                has_volume_swipe: false,
                has_in_ear_detection: true,
                icon_name: "airpods-gen3".to_string(),
            },
            Self::AirPods4 => ModelCapabilities {
                model: *self,
                name: "AirPods 4".to_string(),
                has_anc: false,
                has_adaptive_audio: false,
                has_transparency: false,
                has_conversational_awareness: false,
                has_case_speaker: false,
                has_volume_swipe: false,
                has_in_ear_detection: true,
                icon_name: "airpods-gen4".to_string(),
            },
            Self::AirPods4Anc => ModelCapabilities {
                model: *self,
                name: "AirPods 4 (支持主动降噪)".to_string(),
                has_anc: true,
                has_adaptive_audio: true,
                has_transparency: true,
                has_conversational_awareness: true,
                has_case_speaker: true,
                has_volume_swipe: false,
                has_in_ear_detection: true,
                icon_name: "airpods-gen4-anc".to_string(),
            },
            Self::AirPodsPro1 => ModelCapabilities {
                model: *self,
                name: "AirPods Pro (第 1 代)".to_string(),
                has_anc: true,
                has_adaptive_audio: false,
                has_transparency: true,
                has_conversational_awareness: false,
                has_case_speaker: false,
                has_volume_swipe: false,
                has_in_ear_detection: true,
                icon_name: "airpods-pro".to_string(),
            },
            Self::AirPodsPro2Lightning | Self::AirPodsPro2UsbC => ModelCapabilities {
                model: *self,
                name: "AirPods Pro (第 2 代)".to_string(),
                has_anc: true,
                has_adaptive_audio: true,
                has_transparency: true,
                has_conversational_awareness: true,
                has_case_speaker: true,
                has_volume_swipe: true,
                has_in_ear_detection: true,
                icon_name: "airpods-pro2".to_string(),
            },
            Self::AirPodsMax | Self::AirPodsMaxUsbC => ModelCapabilities {
                model: *self,
                name: "AirPods Max".to_string(),
                has_anc: true,
                has_adaptive_audio: false,
                has_transparency: true,
                has_conversational_awareness: false,
                has_case_speaker: false,
                has_volume_swipe: false,
                has_in_ear_detection: true,
                icon_name: "airpods-max".to_string(),
            },
            Self::Unknown(id) => ModelCapabilities {
                model: *self,
                name: format!("AirPods (ID: 0x{:04X})", id),
                has_anc: false,
                has_adaptive_audio: false,
                has_transparency: false,
                has_conversational_awareness: false,
                has_case_speaker: false,
                has_volume_swipe: false,
                has_in_ear_detection: true,
                icon_name: "airpods-generic".to_string(),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_airpods3_capabilities() {
        let model = AirPodsModel::from_product_id(0x2013);
        assert_eq!(model, AirPodsModel::AirPods3);
        let caps = model.capabilities();
        assert!(!caps.has_anc);
        assert!(caps.has_in_ear_detection);
        assert_eq!(caps.name, "AirPods (第 3 代)");
    }

    #[test]
    fn test_airpods_pro2_capabilities() {
        let model = AirPodsModel::from_product_id(0x2014);
        assert_eq!(model, AirPodsModel::AirPodsPro2Lightning);
        let caps = model.capabilities();
        assert!(caps.has_anc);
        assert!(caps.has_adaptive_audio);
        assert!(caps.has_transparency);
        assert!(caps.has_conversational_awareness);
        assert!(caps.has_case_speaker);
        assert!(caps.has_volume_swipe);
    }

    #[test]
    fn test_infer_from_name() {
        assert_eq!(AirPodsModel::infer_from_name("Alice's AirPods Pro"), Some(AirPodsModel::AirPodsPro1));
        assert_eq!(AirPodsModel::infer_from_name("Bob's AirPods Pro 2"), Some(AirPodsModel::AirPodsPro2UsbC));
        assert_eq!(AirPodsModel::infer_from_name("AirPods Max - Silver"), Some(AirPodsModel::AirPodsMax));
        assert_eq!(AirPodsModel::infer_from_name("AirPods 3"), Some(AirPodsModel::AirPods3));
        assert_eq!(AirPodsModel::infer_from_name("AirPods 4 (ANC)"), Some(AirPodsModel::AirPods4Anc));
        assert_eq!(AirPodsModel::infer_from_name("Sony WH-1000XM4"), None);
    }

    #[test]
    fn test_product_0x2024_is_pro2_usbc_not_airpods4() {
        // 0x2024 (8228) is AirPods Pro 2nd gen (USB-C) in the published PID
        // tables. It used to map to AirPods 4, whose capabilities hide every
        // noise control - the most common modern model lost ANC in the UI.
        let model = AirPodsModel::from_product_id(0x2024);
        assert_eq!(model, AirPodsModel::AirPodsPro2UsbC);
        let caps = model.capabilities();
        assert!(caps.has_anc, "Pro 2 USB-C has ANC");
        assert!(caps.has_adaptive_audio);
        assert!(caps.has_volume_swipe);
    }

    #[test]
    fn test_mac_address_parse_and_display() {
        let addr: MacAddress = "00:11:22:33:44:55".parse().unwrap();
        assert_eq!(addr.0, [0x00, 0x11, 0x22, 0x33, 0x44, 0x55]);
        assert_eq!(addr.to_string(), "00:11:22:33:44:55");

        let addr_dash: MacAddress = "AA-BB-CC-DD-EE-FF".parse().unwrap();
        assert_eq!(addr_dash.0, [0xAA, 0xBB, 0xCC, 0xDD, 0xEE, 0xFF]);
        assert_eq!(addr_dash.to_string(), "AA:BB:CC:DD:EE:FF");
    }
}

/// A 6-byte Bluetooth device MAC address.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, PartialOrd, Ord, Serialize, Deserialize)]
pub struct MacAddress(pub [u8; 6]);

impl MacAddress {
    pub const ZERO: Self = Self([0; 6]);

    pub fn new(bytes: [u8; 6]) -> Self {
        Self(bytes)
    }

    pub fn as_bytes(&self) -> &[u8; 6] {
        &self.0
    }
}

impl std::fmt::Display for MacAddress {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{:02X}:{:02X}:{:02X}:{:02X}:{:02X}:{:02X}",
            self.0[0], self.0[1], self.0[2], self.0[3], self.0[4], self.0[5]
        )
    }
}

impl std::str::FromStr for MacAddress {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let clean = s.trim();
        let parts: Vec<&str> = if clean.contains(':') {
            clean.split(':').collect()
        } else if clean.contains('-') {
            clean.split('-').collect()
        } else {
            return Err(format!("Invalid MAC address format: {}", s));
        };

        if parts.len() != 6 {
            return Err(format!("Expected 6 octets in MAC address, got {}", parts.len()));
        }

        let mut bytes = [0u8; 6];
        for (i, part) in parts.iter().enumerate() {
            bytes[i] = u8::from_str_radix(part, 16)
                .map_err(|e| format!("Invalid hex octet '{}': {}", part, e))?;
        }
        Ok(Self(bytes))
    }
}

impl From<[u8; 6]> for MacAddress {
    fn from(bytes: [u8; 6]) -> Self {
        Self(bytes)
    }
}

#[cfg(target_os = "linux")]
impl From<bluer::Address> for MacAddress {
    fn from(addr: bluer::Address) -> Self {
        Self(addr.0)
    }
}

#[cfg(target_os = "linux")]
impl From<MacAddress> for bluer::Address {
    fn from(addr: MacAddress) -> Self {
        bluer::Address(addr.0)
    }
}

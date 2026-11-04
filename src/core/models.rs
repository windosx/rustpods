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
            0x2024 => Self::AirPods4,
            0x201B => Self::AirPods4Anc,
            0x200E => Self::AirPodsPro1,
            0x2014 => Self::AirPodsPro2Lightning,
            0x2029 | 0x2030 => Self::AirPodsPro2UsbC,
            0x200A => Self::AirPodsMax,
            0x201F => Self::AirPodsMaxUsbC,
            other => Self::Unknown(other),
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
}

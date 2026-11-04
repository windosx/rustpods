use super::commands::ControlCommandId;
use super::events::{
    AacpEvent, BatteryComponent, BatteryStatus, EarDetectionEvent, EarStatus, SingleBatteryInfo,
    StemBud, StemPressType,
};
use tracing::debug;

pub const PSM_AACP: u16 = 0x1001;
pub const HEADER: [u8; 4] = [0x04, 0x00, 0x04, 0x00];

pub mod opcodes {
    pub const HANDSHAKE: u8 = 0x01;
    /// AACP version / capability negotiation. The accessory sends this right
    /// after the handshake; the sensor services stay unavailable until it is
    /// acknowledged.
    pub const VERSION_NEGOTIATION: u8 = 0x02;
    pub const BATTERY_INFO: u8 = 0x04;
    pub const EAR_DETECTION: u8 = 0x06;
    pub const CONTROL_COMMAND: u8 = 0x09;
    pub const REQUEST_NOTIFICATIONS: u8 = 0x0F;
    pub const HEAD_TRACKING: u8 = 0x17;
    pub const STEM_PRESS: u8 = 0x19;
    pub const RENAME: u8 = 0x1A;
    pub const INFORMATION: u8 = 0x1D;
    pub const SET_FEATURE_FLAGS: u8 = 0x4D;
    pub const CONVERSATIONAL_AWARENESS: u8 = 0x4B;
}

/// AACP 1.3 service connection frame.
///
/// `service` is the 2-byte selector at offset 2 (`0x0000` for service 0,
/// `0x0400` for service 4, little-endian). Byte 6 carries the protocol version
/// `0x03`; the earlier single-service handshake sent `0x02` here, which only
/// completes the legacy handshake and leaves the sensor services unavailable.
pub fn make_service_connect(service: u16) -> Vec<u8> {
    let mut packet = vec![0x00, 0x00, 0x00, 0x00, 0x01, 0x00, 0x03, 0x00];
    packet[2..4].copy_from_slice(&service.to_le_bytes());
    packet.extend_from_slice(&[0x00; 8]);
    packet
}

/// Capability request for one service, sent after [`make_service_connect`].
pub fn make_service_caps_request(service: u16) -> Vec<u8> {
    let mut packet = vec![0x04, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00];
    packet[2..4].copy_from_slice(&service.to_le_bytes());
    packet
}

/// Parsed AACP version/capability negotiation frame (opcode `0x02`).
///
/// The accessory sends `04 00 04 00 02 00 <a> <b> <c> <d> ...` right after the
/// handshake. The bytes after the opcode encode its protocol version and
/// capability flags; we keep them verbatim so they can be echoed back.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct VersionNegotiation {
    pub body: Vec<u8>,
}

impl VersionNegotiation {
    /// Builds the acknowledgement, which echoes the accessory's own bytes back.
    ///
    /// AACP treats this exchange as a two-way announcement: the host states the
    /// same version/capabilities it was told, and only then are the optional
    /// services (including the sensor channel) considered negotiated.
    pub fn acknowledgement(&self) -> Vec<u8> {
        let mut packet = vec![0x04, 0x00, 0x04, 0x00, opcodes::VERSION_NEGOTIATION, 0x00];
        packet.extend_from_slice(&self.body);
        packet
    }
}

pub fn make_handshake_packet() -> Vec<u8> {
    vec![
        0x00, 0x00, 0x04, 0x00, 0x01, 0x00, 0x02, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00,
    ]
}

pub fn make_set_feature_flags_packet() -> Vec<u8> {
    vec![
        0x04, 0x00, 0x04, 0x00, 0x4D, 0x00, 0xD7, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    ]
}

pub fn make_notification_request_packet() -> Vec<u8> {
    vec![0x04, 0x00, 0x04, 0x00, 0x0F, 0x00, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF]
}

pub fn make_proximity_keys_request_packet() -> Vec<u8> {
    vec![0x04, 0x00, 0x04, 0x00, 0x30, 0x00, 0x05, 0x00]
}

pub fn make_control_command_packet(cmd: ControlCommandId, data: &[u8]) -> Vec<u8> {
    let mut packet = vec![0x04, 0x00, 0x04, 0x00, 0x09, 0x00, cmd as u8];
    packet.extend_from_slice(data);
    while packet.len() < 11 {
        packet.push(0x00);
    }
    packet
}

pub fn make_rename_packet(name: &str) -> Vec<u8> {
    let bytes = name.as_bytes();
    let size = bytes.len() as u8;
    let mut packet = vec![0x04, 0x00, 0x04, 0x00, 0x1A, 0x00, 0x01, size, 0x00];
    packet.extend_from_slice(bytes);
    packet
}

pub fn make_smart_routing_packet(
    self_mac: &str,
    target_mac: &str,
    streaming_state: bool,
) -> Vec<u8> {
    let mut buffer = Vec::with_capacity(138);

    // Target MAC in reverse byte order
    let target_mac_bytes: Vec<u8> = target_mac
        .split(':')
        .filter_map(|s| u8::from_str_radix(s, 16).ok())
        .collect();
    buffer.extend(target_mac_bytes.iter().rev());

    buffer.extend_from_slice(&[0x82, 0x00]);
    buffer.extend_from_slice(&[0x01, 0xE5, 0x4A]);
    buffer.extend_from_slice(b"PlayingApp");
    buffer.push(0x56);
    buffer.extend_from_slice(b"com.apple.Music");
    buffer.push(0x52);
    buffer.extend_from_slice(b"HostStreamingState");
    buffer.push(0x42);
    buffer.extend_from_slice(if streaming_state { b"YES" } else { b"NO" });
    buffer.push(0x49);
    buffer.extend_from_slice(b"btAddress");
    buffer.push(0x51);
    buffer.extend_from_slice(self_mac.as_bytes());
    buffer.extend_from_slice(b"btName");
    buffer.push(0x43);
    buffer.extend_from_slice(b"Mac");
    buffer.push(0x58);
    buffer.extend_from_slice(b"otherDevice");
    buffer.extend_from_slice(b"AudioCategory");
    buffer.extend_from_slice(&[0x31, 0x2D, 0x01]);

    while buffer.len() < 138 {
        buffer.push(0x00);
    }

    let mut packet = vec![0x04, 0x00, 0x04, 0x00, 0x10, 0x00];
    packet.extend(buffer);
    packet
}

pub fn parse_aacp_packet(packet: &[u8]) -> Option<AacpEvent> {
    if packet.len() < 6 {
        return None;
    }

    if !packet.starts_with(&HEADER) {
        // Special case: handshake response starting with 01 00 04 00
        if packet.starts_with(&[0x01, 0x00, 0x04, 0x00]) {
            debug!("Received AACP handshake acknowledgment");
            return None;
        }
        debug!("Ignoring packet with unknown header: {:02X?}", &packet[..packet.len().min(8)]);
        return None;
    }

    let opcode = packet[4];
    let payload = &packet[6..];

    match opcode {
        opcodes::VERSION_NEGOTIATION => {
            if payload.is_empty() {
                return None;
            }
            Some(AacpEvent::VersionNegotiation(VersionNegotiation {
                body: payload.to_vec(),
            }))
        }

        opcodes::BATTERY_INFO => {
            if payload.is_empty() {
                return None;
            }
            let count = payload[0] as usize;
            let mut list = Vec::new();
            let mut offset = 1;
            for _ in 0..count {
                if offset + 4 > payload.len() {
                    break;
                }
                let comp_byte = payload[offset];
                let level = payload[offset + 2];
                let status_byte = payload[offset + 3];
                offset += 5; // standard format: [comp] 01 [level] [status] 01

                if let Some(comp) = BatteryComponent::from_u8(comp_byte) {
                    list.push(SingleBatteryInfo {
                        component: comp,
                        level: level.min(100),
                        status: BatteryStatus::from_u8(status_byte),
                    });
                }
            }
            Some(AacpEvent::Battery(list))
        }

        opcodes::EAR_DETECTION => {
            if payload.len() >= 2 {
                let left = EarStatus::from_u8(payload[0]);
                let right = EarStatus::from_u8(payload[1]);
                Some(AacpEvent::EarDetection(EarDetectionEvent { left, right }))
            } else {
                None
            }
        }

        opcodes::CONTROL_COMMAND => {
            if payload.is_empty() {
                return None;
            }
            let cmd_id_byte = payload[0];
            if let Some(id) = ControlCommandId::from_u8(cmd_id_byte) {
                let data = payload[1..].to_vec();
                Some(AacpEvent::ControlCommand { id, data })
            } else {
                debug!("Received unknown control command id: 0x{:02X}", cmd_id_byte);
                None
            }
        }

        opcodes::STEM_PRESS => {
            if payload.len() >= 2 {
                let press_byte = payload[0];
                let bud_byte = payload[1];
                if let (Some(press_type), Some(bud)) = (
                    StemPressType::from_u8(press_byte),
                    StemBud::from_u8(bud_byte),
                ) {
                    Some(AacpEvent::StemPress { press_type, bud })
                } else {
                    None
                }
            } else {
                None
            }
        }

        opcodes::CONVERSATIONAL_AWARENESS => {
            if !payload.is_empty() {
                let active = payload[0] == 0x01;
                Some(AacpEvent::ConversationalAwareness(active))
            } else {
                None
            }
        }

        opcodes::RENAME => {
            if payload.len() >= 4 && payload[0] == 0x01 {
                let len = payload[2] as usize;
                if payload.len() >= 4 + len {
                    if let Ok(name) = std::str::from_utf8(&payload[4..4 + len]) {
                        return Some(AacpEvent::Renamed(name.to_string()));
                    }
                }
            } else if payload.len() > 1 {
                let len = payload[0] as usize;
                if payload.len() >= 1 + len {
                    if let Ok(name) = std::str::from_utf8(&payload[1..1 + len]) {
                        return Some(AacpEvent::Renamed(name.to_string()));
                    }
                }
            }
            None
        }

        opcodes::INFORMATION => {
            if payload.len() >= 6 {
                let data = &payload[4..];
                let mut index = 0;
                while index < data.len() && data[index] != 0x00 {
                    index += 1;
                }
                let mut strings = Vec::new();
                while index < data.len() {
                    while index < data.len() && data[index] == 0x00 {
                        index += 1;
                    }
                    if index >= data.len() {
                        break;
                    }
                    let start = index;
                    while index < data.len() && data[index] != 0x00 {
                        index += 1;
                    }
                    let str_bytes = &data[start..index];
                    if let Ok(s) = std::str::from_utf8(str_bytes) {
                        strings.push(s.to_string());
                    }
                }
                if strings.len() > 1 {
                    strings.remove(0);
                    debug!("Parsed AirPods Hardware Information: {:?}", strings);
                }
            }
            None
        }

        opcodes::HEAD_TRACKING => {
            // RTBuddy SensorV2 samples and their descriptor/control frames share
            // this opcode. A sample is a fixed 65-byte payload; control frames
            // are short and carry a descriptor instead of sensor fields, so they
            // are left for the rtbuddy layer to interpret.
            if packet.len() >= crate::core::head_tracking::SENSOR_V2_LEN {
                Some(AacpEvent::HeadSensor(packet.to_vec()))
            } else {
                debug!(
                    "RTBuddy control frame on opcode 0x17 ({} bytes)",
                    packet.len()
                );
                None
            }
        }

        _ => {
            debug!("Unhandled opcode 0x{:02X}, len: {}", opcode, packet.len());
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_make_packets() {
        let hs = make_handshake_packet();
        assert_eq!(hs.len(), 16);
        assert_eq!(&hs[..4], &[0x00, 0x00, 0x04, 0x00]);

        let ff = make_set_feature_flags_packet();
        assert_eq!(&ff[..6], &[0x04, 0x00, 0x04, 0x00, 0x4D, 0x00]);

        let notif = make_notification_request_packet();
        assert_eq!(&notif[..6], &[0x04, 0x00, 0x04, 0x00, 0x0F, 0x00]);

        let cmd = make_control_command_packet(ControlCommandId::ListeningMode, &[0x02]);
        assert_eq!(&cmd[..7], &[0x04, 0x00, 0x04, 0x00, 0x09, 0x00, 0x0D]);
        assert_eq!(cmd[7], 0x02);
    }

    #[test]
    fn test_parse_version_negotiation() {
        // Captured from an AirPods 3: opcode 0x02 with its version/capability body.
        let packet = [
            0x04, 0x00, 0x04, 0x00, 0x02, 0x00, 0x0D, 0x01, 0x0E, 0x02, 0x02, 0x04, 0x01, 0x00,
            0x00, 0x00,
        ];
        match parse_aacp_packet(&packet) {
            Some(AacpEvent::VersionNegotiation(neg)) => {
                assert_eq!(neg.body, vec![0x0D, 0x01, 0x0E, 0x02, 0x02, 0x04, 0x01, 0x00, 0x00, 0x00]);
                // The ack echoes the body back under the same opcode.
                let ack = neg.acknowledgement();
                assert_eq!(ack[..6], [0x04, 0x00, 0x04, 0x00, 0x02, 0x00]);
                assert_eq!(&ack[6..], &neg.body[..]);
            }
            other => panic!("Unexpected event: {:?}", other),
        }
    }

    #[test]
    fn test_parse_ear_detection() {
        // Opcode 0x06 with Left=InEar (0), Right=OutOfEar (1)
        let packet = [0x04, 0x00, 0x04, 0x00, 0x06, 0x00, 0x00, 0x01];
        let evt = parse_aacp_packet(&packet);
        match evt {
            Some(AacpEvent::EarDetection(ear)) => {
                assert_eq!(ear.left, EarStatus::InEar);
                assert_eq!(ear.right, EarStatus::OutOfEar);
            }
            other => panic!("Unexpected event: {:?}", other),
        }
    }

    #[test]
    fn test_parse_battery_info() {
        // Opcode 0x04: count=2, Left(0x04) level=90 status=Charging(1), Right(0x02) level=85 status=NotCharging(2)
        let packet = [
            0x04, 0x00, 0x04, 0x00, 0x04, 0x00, 0x02, // count = 2
            0x04, 0x01, 90, 0x01, 0x01,                // Left: 90%, charging
            0x02, 0x01, 85, 0x02, 0x01,                // Right: 85%, not charging
        ];
        let evt = parse_aacp_packet(&packet);
        match evt {
            Some(AacpEvent::Battery(list)) => {
                assert_eq!(list.len(), 2);
                assert_eq!(list[0].component, BatteryComponent::Left);
                assert_eq!(list[0].level, 90);
                assert_eq!(list[0].status, BatteryStatus::Charging);
                assert_eq!(list[1].component, BatteryComponent::Right);
                assert_eq!(list[1].level, 85);
                assert_eq!(list[1].status, BatteryStatus::NotCharging);
            }
            other => panic!("Unexpected event: {:?}", other),
        }
    }

    #[test]
    fn test_parse_rename() {
        let name = "浩的AirPods";
        let bytes = name.as_bytes();
        let mut packet = vec![0x04, 0x00, 0x04, 0x00, 0x1A, 0x00, bytes.len() as u8];
        packet.extend_from_slice(bytes);

        let evt = parse_aacp_packet(&packet);
        match evt {
            Some(AacpEvent::Renamed(parsed_name)) => {
                assert_eq!(parsed_name, name);
            }
            other => panic!("Unexpected event: {:?}", other),
        }
    }
}

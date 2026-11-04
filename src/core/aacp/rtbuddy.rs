//! RTBuddy "SensorDataWX" transport used by AirPods for head tracking.
//!
//! Every byte pattern here is taken from a known-working implementation rather
//! than inferred: LibrePods' `head-tracking/` scripts drive real AirPods with
//! exactly these frames, and the same frames are documented in
//! `AAP Definitions.md` under "Head Tracking".
//!
//! Wire envelope (all lengths little-endian):
//!
//! ```text
//! [04 00 04 00 17 00] [descriptor: 4] [body_len: 2] [body: body_len]
//!  \____ AACP ____/    \_____ rtbuddy framing ____/
//! ```
//!
//! `body` is a protobuf-like `SensorDataWX` message:
//! field 1 = sequence number, optional field 2 = mode, field 8 = `ServiceSetting`
//! (`{service, two = 2, configuration}`), where `configuration` is
//! `[0x01][interval_us: 4]` — a zero interval asks the accessory to stop.

/// Parsed header of a sensor service advertisement (the XML descriptor the
/// accessory sends on descriptor `0x800` in reply to a service subscription).
///
/// The advertisement is not just XML: the 6 bytes preceding the XML carry the
/// identifiers the HID enable needs.
///
/// ```text
/// offset 0  4  AACP header            04 00 04 00 17 00
/// offset 6  4  descriptor             00 08 00 00  (0x800)
/// offset 10 2  body length            e.g. 6D 02
/// offset 12 4  assigned service id    00 16 00 00  (0x00001600)
/// offset 16 4  cookie                 83 34 01 00  (0x00013483)
/// offset 20 .. XML plist begins
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ServiceAdvertisement {
    /// Id the accessory assigned to this service, e.g. `0x1600` for `neorelay`.
    pub service_id: u32,
    /// Per-service cookie to carry in the HID enable.
    pub cookie: u32,
}

/// Name of the service that streams 6-DoF head pose (`SensorV2`).
pub const SERVICE_NAME_NEORELAY: &str = "neorelay";

/// Bytes before the XML plist start: header 6 + descriptor 4 + length 2 + ids 8.
const ADVERTISEMENT_PREFIX_LEN: usize = 20;

impl ServiceAdvertisement {
    /// Parses an advertisement frame, returning `None` when the frame is not a
    /// sensor service advertisement (wrong opcode/descriptor, or too short).
    pub fn parse(frame: &[u8]) -> Option<Self> {
        if frame.len() < ADVERTISEMENT_PREFIX_LEN || frame[4] != 0x17 {
            return None;
        }
        let descriptor = u32::from_le_bytes([frame[6], frame[7], frame[8], frame[9]]);
        if descriptor != DESCRIPTOR_SENSOR {
            return None;
        }
        // Byte 20 onward must be the plist; this keeps the XML capability
        // reports apart from other traffic on the same descriptor.
        if frame[ADVERTISEMENT_PREFIX_LEN] != b'<' {
            return None;
        }
        Some(Self {
            service_id: u32::from_le_bytes([frame[12], frame[13], frame[14], frame[15]]),
            cookie: u32::from_le_bytes([frame[16], frame[17], frame[18], frame[19]]),
        })
    }

    /// Which service this advertisement describes, by its `AccessoryService`
    /// key. `None` when the XML does not name one.
    pub fn service_name(frame: &[u8]) -> Option<&str> {
        let text = std::str::from_utf8(&frame[ADVERTISEMENT_PREFIX_LEN..]).ok()?;
        let start = text.find("<key>AccessoryService</key><string>")?;
        let rest = &text[start + "<key>AccessoryService</key><string>".len()..];
        let end = rest.find('<')?;
        Some(&rest[..end])
    }
}

pub const DESCRIPTOR_SENSOR_DATA_WX: u32 = 0x0010_0000;

/// Descriptor used for the HID-level sensor service handshake. The macOS
/// capture shows `Sent custom message ... type 0x800, len 14` right before the
/// stream begins, and `Received ... type 0x800, len 9` immediately after.
pub const DESCRIPTOR_SENSOR: u32 = 0x0000_0800;

/// Service id that streams `SensorV2` head-tracking samples. LibrePods' start
/// frame carries `08 0E`, i.e. service 14.
pub const SERVICE_HEAD_TRACKING: u8 = 14;

/// Sensor services the accessory advertises, in the order it reports them.
/// These are the names that arrive in its XML descriptors.
pub const SERVICE_MANDO: u8 = 6;
pub const SERVICE_IED: u8 = 7;
pub const SERVICE_NEORELAY: u8 = 9;

/// macOS was observed emitting `SensorV2` frames at a 20 ms median (≈50 Hz),
/// so it is presumably asking for a 20 ms cadence.
pub const INTERVAL_50HZ_US: u32 = 20_000;

/// LibrePods hard-codes 40 000 µs (25 Hz). Kept as a conservative fallback.
pub const INTERVAL_25HZ_US: u32 = 40_000;

/// Sequence numbers from the captured start/stop frames. The accessory does not
/// appear to care about their value, but staying byte-identical to the capture
/// removes one variable when debugging a live device.
const SEQ_START: u64 = 289;
const SEQ_STOP: u64 = 126;

/// Service id used only in the captured stop frame (never seen enabled).
const SERVICE_STOP: u8 = 78;
/// Field 2 value present only in the stop frame.
const STOP_MODE: u8 = 2;

pub fn encode_varint(mut val: u64, buf: &mut Vec<u8>) {
    loop {
        let low = (val & 0x7F) as u8;
        val >>= 7;
        if val > 0 {
            buf.push(low | 0x80);
        } else {
            buf.push(low);
            break;
        }
    }
}

pub fn make_rtbuddy_envelope(descriptor: u32, body: &[u8]) -> Vec<u8> {
    let mut packet = vec![0x04, 0x00, 0x04, 0x00, 0x17, 0x00];
    packet.extend_from_slice(&descriptor.to_le_bytes());
    packet.extend_from_slice(&(body.len() as u16).to_le_bytes());
    packet.extend_from_slice(body);
    packet
}

/// `ServiceSetting { service, two = 2, configuration = [0x01, interval_us] }`.
fn service_setting(service: u8, interval_us: u32) -> Vec<u8> {
    let mut config = vec![0x01];
    config.extend_from_slice(&interval_us.to_le_bytes());

    let mut setting = Vec::new();
    setting.push(0x08); // field 1: service
    encode_varint(service as u64, &mut setting);
    setting.push(0x10); // field 2: constant 2
    setting.push(0x02);
    setting.push(0x1A); // field 3: configuration
    encode_varint(config.len() as u64, &mut setting);
    setting.extend_from_slice(&config);
    setting
}

fn sensor_data_wx(seq: u64, mode: Option<u8>, setting: &[u8]) -> Vec<u8> {
    let mut body = Vec::new();
    body.push(0x08); // field 1: seq
    encode_varint(seq, &mut body);
    if let Some(mode) = mode {
        body.push(0x10); // field 2
        body.push(mode);
    }
    body.push(0x42); // field 8: service_settings
    encode_varint(setting.len() as u64, &mut body);
    body.extend_from_slice(setting);

    make_rtbuddy_envelope(DESCRIPTOR_SENSOR_DATA_WX, &body)
}

/// Asks the accessory to begin streaming head-tracking samples at
/// `interval_us`. The accessory only starts streaming once the AACP 1.3
/// handshake has been completed on the same L2CAP channel.
pub fn make_head_tracking_start(interval_us: u32) -> Vec<u8> {
    make_head_tracking_start_for(SERVICE_HEAD_TRACKING, interval_us)
}

/// Same frame but with an explicit service id, for probing which service
/// actually streams on a given firmware.
pub fn make_head_tracking_start_for(service: u8, interval_us: u32) -> Vec<u8> {
    sensor_data_wx(SEQ_START, None, &service_setting(service, interval_us))
}

/// Asks the accessory to stop streaming. Byte-for-byte the captured frame: it
/// differs from the start frame by more than just a zero interval, so it is
/// built separately rather than derived.
pub fn make_head_tracking_stop() -> Vec<u8> {
    sensor_data_wx(
        SEQ_STOP,
        Some(STOP_MODE),
        &service_setting(SERVICE_STOP, 0),
    )
}

/// The `SensorV2` trigger frame. Sent on descriptor `0x40000`; the accessory
/// answers with the cookie that the 14-byte HID enable has to carry.
pub fn make_sensor_v2_trigger() -> Vec<u8> {
    vec![
        0x04, 0x00, 0x04, 0x00, 0x17, 0x00, 0x00, 0x00, 0x04, 0x00, 0x04, 0x00, 0x01, 0x00, 0x00,
        0x00,
    ]
}

/// Builds the 14-byte HID enable that starts the `SensorV2` stream.
///
/// Layout: `[service_id:4][cookie:4][0xD2][0x01][interval_us:4]`.
pub fn make_sensor_v2_enable(service_id: u32, cookie: u32, interval_us: u32) -> Vec<u8> {
    let mut body = Vec::with_capacity(14);
    body.extend_from_slice(&service_id.to_le_bytes());
    body.extend_from_slice(&cookie.to_le_bytes());
    body.push(REPORT_ID_SENSOR_V2);
    body.push(0x01);
    body.extend_from_slice(&interval_us.to_le_bytes());
    make_rtbuddy_envelope(DESCRIPTOR_SENSOR, &body)
}

/// Alternative 14-byte HID enable layout.
///
/// The advertisement's id field reads `00 16 00 00` while the activation
/// payload recorded from a Mac session reads `01 16 00 00` — the same middle
/// `u16`, differing only in the first byte. That suggests the field is
/// `[enable:1][service_id:u16][pad:1]` rather than a flat `u32` id, with
/// `0x00` meaning "advertised" and `0x01` meaning "enabled".
///
/// Both readings produce 14 bytes, so which one the accessory accepts can only
/// be settled by trying them.
pub fn make_sensor_v2_enable_v2(service_id: u16, cookie: u32, interval_us: u32) -> Vec<u8> {
    let mut body = Vec::with_capacity(14);
    body.push(0x01);
    body.extend_from_slice(&service_id.to_le_bytes());
    body.push(0x00);
    body.extend_from_slice(&cookie.to_le_bytes());
    body.push(REPORT_ID_SENSOR_V2);
    body.push(0x01);
    body.extend_from_slice(&interval_us.to_le_bytes());
    make_rtbuddy_envelope(DESCRIPTOR_SENSOR, &body)
}

/// Service id the accessory expects in the enable frame for `SensorV2`.
pub const SENSOR_V2_SERVICE_ID: u32 = 0x0000_1601;

/// Cookie observed in the reference session (`83 34 01 00`). Stable per
/// accessory; used only when the trigger yields no reply.
pub const FALLBACK_COOKIE: u32 = 0x0001_3483;

/// Report id carried in the enable frame for the `SensorV2` report.
pub const REPORT_ID_SENSOR_V2: u8 = 0xD2;

/// Length of the accessory's reply to [`make_sensor_v2_enable`].
pub const HID_ENABLE_ACK_LEN: usize = 9;

/// Alternate START frame.
///
/// `anilsrml/airpods-posture` drives AirPods head tracking from Linux over the
/// same PSM and sends exactly this frame after only the plain handshake. It is
/// byte-identical to LibrePods' `createAlternateStartHeadTrackingPacket`.
///
/// Differences from [`make_head_tracking_start`]: sequence `0x73` instead of
/// `0xA1 0x02` (= 289), body length `0x0F` instead of `0x10`, and service **16**
/// instead of 14.
///
/// This is the variant that matters on Linux, so it is tried first.
pub fn make_alternate_start_head_tracking(interval_us: u32) -> Vec<u8> {
    let mut body = Vec::new();
    body.push(0x08); // field 1: seq
    encode_varint(0x73, &mut body);
    body.push(0x42); // field 8: service setting
    let setting = service_setting(0x10, interval_us);
    encode_varint(setting.len() as u64, &mut body);
    body.extend_from_slice(&setting);
    make_rtbuddy_envelope(DESCRIPTOR_SENSOR_DATA_WX, &body)
}

/// Builds a `SensorDataWX` subscription for an arbitrary service id.
///
/// This is the original multi-service subscribe frame: the accessory only
/// reports its sensor descriptors after being subscribed on each service, and
/// the descriptor replies are what told us the real service names
/// (`iedrelay` / `neorelay` / `mando`).
pub fn make_service_subscription(service: u8, interval_us: u32, seq: u64) -> Vec<u8> {
    sensor_data_wx(seq, None, &service_setting(service, interval_us))
}

/// Builds the acknowledgement the accessory expects after a service reply.
///
/// Field 9 (tag `0x4A`) is `TypeAck { service }`. Dropping this packet is what
/// made the stream stop appearing.
pub fn make_service_ack(service: u8, seq: u64) -> Vec<u8> {
    let mut body = Vec::new();
    body.push(0x08);
    encode_varint(seq, &mut body);

    body.push(0x4A);
    body.push(0x02);
    body.push(0x08);
    body.push(service);

    make_rtbuddy_envelope(DESCRIPTOR_SENSOR_DATA_WX, &body)
}

#[cfg(test)]
mod tests {
    use super::*;

    // Captured from LibrePods (`connection_manager.py` / `AAP Definitions.md`).
    const START_25HZ: [u8; 28] = [
        0x04, 0x00, 0x04, 0x00, 0x17, 0x00, 0x00, 0x00, 0x10, 0x00, 0x10, 0x00, 0x08, 0xA1, 0x02,
        0x42, 0x0B, 0x08, 0x0E, 0x10, 0x02, 0x1A, 0x05, 0x01, 0x40, 0x9C, 0x00, 0x00,
    ];
    const STOP: [u8; 29] = [
        0x04, 0x00, 0x04, 0x00, 0x17, 0x00, 0x00, 0x00, 0x10, 0x00, 0x11, 0x00, 0x08, 0x7E, 0x10,
        0x02, 0x42, 0x0B, 0x08, 0x4E, 0x10, 0x02, 0x1A, 0x05, 0x01, 0x00, 0x00, 0x00, 0x00,
    ];

    #[test]
    fn parses_service_advertisement_identifiers() {
        // Captured from a real AirPods 3: header of the neorelay advertisement.
        // The ids are re-assigned on every connection, so they must be read from
        // the frame rather than hard-coded.
        let mut frame = vec![
            0x04, 0x00, 0x04, 0x00, 0x17, 0x00, // AACP header
            0x00, 0x08, 0x00, 0x00, // descriptor 0x800
            0x6D, 0x02, // body length 621
            0x00, 0x1E, 0x00, 0x00, // service id 0x00001E00
            0x83, 0x34, 0x01, 0x00, // cookie 0x00013483
        ];
        frame.extend_from_slice(
            b"<?xml version=\"1.0\"?><dict><key>AccessoryService</key><string>neorelay</string></dict>",
        );

        let ad = ServiceAdvertisement::parse(&frame).expect("should parse");
        assert_eq!(ad.service_id, 0x0000_1E00);
        assert_eq!(ad.cookie, 0x0001_3483);
        assert_eq!(
            ServiceAdvertisement::service_name(&frame),
            Some("neorelay")
        );
    }

    #[test]
    fn advertisement_rejects_non_xml_and_wrong_descriptor() {
        // Right shape, but the body is not a plist.
        let mut not_xml = vec![
            0x04, 0x00, 0x04, 0x00, 0x17, 0x00, 0x00, 0x08, 0x00, 0x00, 0x6D, 0x02, 0x00, 0x1E,
            0x00, 0x00, 0x83, 0x34, 0x01, 0x00,
        ];
        not_xml.extend_from_slice(&[0xFF; 8]);
        assert!(ServiceAdvertisement::parse(&not_xml).is_none());

        // XML, but on the SensorDataWX descriptor instead of 0x800.
        let mut wrong_desc = vec![
            0x04, 0x00, 0x04, 0x00, 0x17, 0x00, 0x00, 0x00, 0x10, 0x00, 0x6D, 0x02, 0x00, 0x1E,
            0x00, 0x00, 0x83, 0x34, 0x01, 0x00,
        ];
        wrong_desc.extend_from_slice(b"<?xml version=\"1.0\"?>");
        assert!(ServiceAdvertisement::parse(&wrong_desc).is_none());
    }

    #[test]
    fn advertisement_rejects_short_frames() {
        assert!(ServiceAdvertisement::parse(&[]).is_none());
        assert!(ServiceAdvertisement::parse(&[0x04, 0x00, 0x04, 0x00, 0x17, 0x00]).is_none());
    }

    #[test]
    fn alternate_start_matches_linux_implementation() {
        // From anilsrml/airpods-posture (Linux, same PSM as us) and LibrePods'
        // createAlternateStartHeadTrackingPacket.
        //
        // Decoded: seq=115 (0x73), service=16, interval=40000us.
        const LINUX_START: [u8; 27] = [
            0x04, 0x00, 0x04, 0x00, 0x17, 0x00, 0x00, 0x00, 0x10, 0x00, 0x0F, 0x00, 0x08, 0x73,
            0x42, 0x0B, 0x08, 0x10, 0x10, 0x02, 0x1A, 0x05, 0x01, 0x40, 0x9C, 0x00, 0x00,
        ];
        assert_eq!(
            make_alternate_start_head_tracking(INTERVAL_25HZ_US),
            LINUX_START
        );
    }

    #[test]
    fn start_frame_matches_capture_at_25hz() {
        assert_eq!(make_head_tracking_start(INTERVAL_25HZ_US), START_25HZ);
    }

    #[test]
    fn stop_frame_matches_capture() {
        assert_eq!(make_head_tracking_stop(), STOP);
    }

    #[test]
    fn start_frame_only_differs_in_interval_at_50hz() {
        let fifty = make_head_tracking_start(INTERVAL_50HZ_US);
        assert_eq!(fifty.len(), START_25HZ.len());
        // 20 000 µs = 0x00004E20
        assert_eq!(&fifty[24..28], &[0x20, 0x4E, 0x00, 0x00]);
        // Everything before the interval is unchanged.
        assert_eq!(&fifty[..24], &START_25HZ[..24]);
    }

    #[test]
    fn envelope_length_matches_body_length() {
        let frame = make_head_tracking_start(INTERVAL_50HZ_US);
        let body_len = u16::from_le_bytes([frame[10], frame[11]]) as usize;
        assert_eq!(frame.len(), 12 + body_len);
    }

    #[test]
    fn varint_roundtrip() {
        for value in [0u64, 1, 126, 127, 128, 289, 16_383, 16_384] {
            let mut buf = Vec::new();
            encode_varint(value, &mut buf);
            // Decode back.
            let mut out = 0u64;
            let mut shift = 0;
            for byte in &buf {
                out |= ((byte & 0x7F) as u64) << shift;
                shift += 7;
            }
            assert_eq!(out, value, "varint mismatch for {}", value);
        }
    }
}

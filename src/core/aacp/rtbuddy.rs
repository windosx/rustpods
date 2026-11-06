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
/// The advertisement is not just XML: the 8 bytes preceding the plist carry the
/// identifiers the HID enable needs.
///
/// ```text
/// offset 0  6  AACP header + opcode   04 00 04 00 17 00
/// offset 6  4  descriptor             00 08 00 00   (0x800)
/// offset 10 2  body length            e.g. 6D 02
/// offset 12 1  enable flag            00             (00 = advertised)
/// offset 13 2  assigned service id    16 00          (22 for neorelay)
/// offset 15 1  reserved               00
/// offset 16 4  cookie                 83 34 01 00
/// offset 20 .. XML plist begins
/// ```
///
/// The service ids read out here match the accessory's own numbering
/// (`iedrelay` 19, `neorelay` 22, `mando` 23). Reading them as a `u32` from
/// offset 12 yields `0x1600` instead of `22`, which shifts every later field in
/// the enable frame — that mistake is what made the accessory ignore it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ServiceAdvertisement {
    /// Flag byte: `0x00` while advertised, `0x01` when enabled.
    pub flag: u8,
    /// Id the accessory assigned to this service, e.g. `22` for `neorelay`.
    pub service_id: u16,
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
            flag: frame[12],
            service_id: u16::from_le_bytes([frame[13], frame[14]]),
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

/// Spatial audio mode the host declares for head tracking.
///
/// macOS sets `mode: 3, head Tracking 1` before it sends the enable:
///
/// ```text
/// DeviceManager Set Spatial Mode: bundleID: com.apple.WebKit.GPU, mode: 3 head Tracking 1
/// AudioLinkManager setSpatialModeWithBundleID: ..., mode:3, headtrack:1
/// ```
///
/// The byte at offset 9 of the enable is the only candidate field that could
/// carry this value, but every payload captured so far holds `0x01` there, so
/// whether the accessory reads the mode from this frame at all is unconfirmed.
/// `tools/ht_probe.py` sweeps it.
pub const SPATIAL_MODE_HEAD_TRACKED: u8 = 3;

/// Builds the 14-byte HID enable that starts the `SensorV2` stream.
///
/// Layout, matched against the payload recorded from a macOS session
/// (`01 16 00 00 83 34 01 00 D2 01 20 4E 00 00`):
///
/// ```text
/// [0]      enable flag         0x01  (0x00 in the advertisement)
/// [1..3]   service id          u16   (22 = neorelay)
/// [3]      reserved            0x00
/// [4..8]   cookie              u32   (copied from the advertisement)
/// [8]      report id           0xD2
/// [9]      enabled / mode      0x01 observed; macOS runs in mode 3
/// [10..14] interval            u32 microseconds
/// ```
///
/// The service id must come from the advertisement of this connection, because
/// the accessory reassigns it on every connect.
pub fn make_sensor_v2_enable(service_id: u16, cookie: u32, interval_us: u32) -> Vec<u8> {
    make_sensor_v2_enable_with_mode(service_id, cookie, 0x01, interval_us)
}

/// Same frame with the offset-9 byte set explicitly, so the probe tool and the
/// runtime can test the spatial-mode hypothesis without duplicating the layout.
pub fn make_sensor_v2_enable_with_mode(
    service_id: u16,
    cookie: u32,
    mode: u8,
    interval_us: u32,
) -> Vec<u8> {
    let mut body = Vec::with_capacity(14);
    body.push(0x01); // enable flag
    body.extend_from_slice(&service_id.to_le_bytes());
    body.push(0x00); // reserved
    body.extend_from_slice(&cookie.to_le_bytes());
    body.push(REPORT_ID_SENSOR_V2);
    body.push(mode);
    body.extend_from_slice(&interval_us.to_le_bytes());
    make_rtbuddy_envelope(DESCRIPTOR_SENSOR, &body)
}

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
    fn enable_frame_mode_byte_is_overridable() {
        // The offset-9 byte is the only field that could carry the spatial mode
        // macOS sets to 3 before enabling head tracking.
        let frame = make_sensor_v2_enable_with_mode(22, 0x0001_3483, SPATIAL_MODE_HEAD_TRACKED, INTERVAL_50HZ_US);
        assert_eq!(frame[20], REPORT_ID_SENSOR_V2);
        assert_eq!(frame[21], 3);
        // Everything else must be unchanged from the default frame.
        let default = make_sensor_v2_enable(22, 0x0001_3483, INTERVAL_50HZ_US);
        assert_eq!(frame.len(), default.len());
        assert_eq!(&frame[..21], &default[..21]);
        assert_eq!(&frame[22..], &default[22..]);
    }

    #[test]
    fn enable_frame_matches_recorded_macos_payload() {
        // `01 16 00 00 83 34 01 00 D2 01 20 4E 00 00` recorded from a working
        // macOS session: neorelay (service 22) enabled at 20 ms.
        const RECORDED: [u8; 26] = [
            0x04, 0x00, 0x04, 0x00, 0x17, 0x00, 0x00, 0x08, 0x00, 0x00, 0x0E, 0x00, 0x01, 0x16,
            0x00, 0x00, 0x83, 0x34, 0x01, 0x00, 0xD2, 0x01, 0x20, 0x4E, 0x00, 0x00,
        ];
        assert_eq!(
            make_sensor_v2_enable(22, 0x0001_3483, INTERVAL_50HZ_US),
            RECORDED
        );
    }

    #[test]
    fn enable_frame_uses_u16_service_id_not_a_flat_u32() {
        // Regression guard: reading the advertisement id as a u32 yields 0x1600
        // and shifts every following field, which the accessory ignores.
        let frame = make_sensor_v2_enable(22, 0x0001_3483, INTERVAL_50HZ_US);
        assert_eq!(&frame[12..16], &[0x01, 0x16, 0x00, 0x00]);
        assert_eq!(&frame[16..20], &[0x83, 0x34, 0x01, 0x00]);
    }

    #[test]
    fn parses_service_advertisement_identifiers() {
        // Captured verbatim from a real AirPods 3: header of the neorelay
        // advertisement. The ids are re-assigned on every connection, so they
        // must be read from the frame rather than hard-coded.
        let mut frame = vec![
            0x04, 0x00, 0x04, 0x00, 0x17, 0x00, // AACP header
            0x00, 0x08, 0x00, 0x00, // descriptor 0x800
            0x6D, 0x02, // body length 621
            0x00, 0x16, 0x00, 0x00, // flag 0x00, service id 22, reserved
            0x83, 0x34, 0x01, 0x00, // cookie 0x00013483
        ];
        frame.extend_from_slice(
            b"<?xml version=\"1.0\"?><dict><key>AccessoryService</key><string>neorelay</string></dict>",
        );

        let ad = ServiceAdvertisement::parse(&frame).expect("should parse");
        assert_eq!(ad.flag, 0x00, "advertised, not yet enabled");
        assert_eq!(ad.service_id, 22, "neorelay is service 22");
        assert_eq!(ad.cookie, 0x0001_3483);
        assert_eq!(
            ServiceAdvertisement::service_name(&frame),
            Some("neorelay")
        );
    }

    #[test]
    fn advertisement_service_ids_match_accessory_numbering() {
        // Captured headers: the u16 at offset 13 must read 19 / 22 / 23, which is
        // the accessory's own numbering for iedrelay / neorelay / mando.
        let cases = [
            ("iedrelay", 19u16, [0x00, 0x13, 0x00, 0x00]),
            ("neorelay", 22, [0x00, 0x16, 0x00, 0x00]),
            ("mando", 23, [0x00, 0x17, 0x00, 0x00]),
        ];
        for (name, expected, id_field) in cases {
            let mut frame = vec![
                0x04, 0x00, 0x04, 0x00, 0x17, 0x00, 0x00, 0x08, 0x00, 0x00, 0x6D, 0x02,
            ];
            frame.extend_from_slice(&id_field);
            frame.extend_from_slice(&[0x83, 0x34, 0x01, 0x00]);
            frame.extend_from_slice(
                format!("<?xml?><key>AccessoryService</key><string>{}</string>", name).as_bytes(),
            );
            let ad = ServiceAdvertisement::parse(&frame).expect("should parse");
            assert_eq!(ad.service_id, expected, "wrong id for {}", name);
            assert_eq!(ServiceAdvertisement::service_name(&frame), Some(name));
        }
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

//! Apple RemoteHID over AACP — the `0x800` transport AirPods 3 actually uses.
//!
//! ## Why this exists
//!
//! Everything in [`super::rtbuddy`] targets `SensorDataWX` (descriptor
//! `0x100000`). AirPods 3 (model `0x2013`) does not implement that endpoint at
//! all — it serves motion data through Apple RemoteHID on descriptor `0x800`
//! instead. This module implements that transport, following
//! `starman-tech/mousepods` `docs/REMOTEHID-PROTOCOL.md`, which was verified on
//! the same model.
//!
//! ## Frame layout
//!
//! ```text
//! [04 00 04 00] [17 00] [descriptor: 4] [body_len: 2] [body]
//!    AACP header                                        |
//!                                                       v
//! body := [version: 1][generation: 1][0: 1][side_flags: 1] [sub-packet...]
//!                                  ^ host writes 2 * (side & 1)
//!
//! sub_packet header, u32 LE:
//!   bits  0..6   device id   (receiver ORs the bud side into bit 6)
//!   bits  7..16  length in bytes, including this 4-byte header
//!   bits 17..19  kind
//!   bit  20      input report carries a u64 timestamp at payload +5
//!   bit  21      this is a response
//!   bit  22      a status byte is present
//! ```
//!
//! `generation` increments per message on each side. It is not a handle.
//!
//! ## Kinds
//!
//! | Kind | Name | Direction |
//! |------|------|-----------|
//! | 0 | HostConnect / DeviceConnect | host → device / device → host |
//! | 1 | DeviceDisconnect | device → host |
//! | 2 | InputReport | device → host |
//! | 3 | SetReport (+ response) | host → device / device → host |
//! | 4 | GetReport (+ response) | host → device / device → host |
//! | 5 | DeviceConfirmConnect | device → host |
//!
//! ## What is verified on our hardware and what is not
//!
//! Verified on AirPods 3 (firmware `65.1868046005000000.52`):
//!
//! * `HostConnect` is accepted and the accessory answers with `DeviceConnect`
//!   frames carrying `AccessoryService` + a base64 HID report descriptor.
//! * The accessory announces **three** services here — `iedrelay` (device 8),
//!   `neorelay` (device 3) and `mando` (device 2). The protocol document reports
//!   `devmotion6` as device 7 on its own hardware.
//! * Every announced service exposes only `Input` reports (0x01-0x08 and
//!   0xD1-0xD3 for `neorelay`). **No Feature report exists**, and the document's
//!   start sequence writes Feature report 1 with the sample interval. On our
//!   unit that write is answered with a non-empty status payload instead of the
//!   `00` the document expects, so the stream never starts.
//!
//! That mismatch is the open problem. Two readings are possible: either the
//! accessory exposes the interval control through a path this firmware does not
//! advertise over RemoteHID, or the document's device numbering does not line up
//! with what our unit announces and device 7 exists only after some further
//! handshake.

use std::collections::HashMap;

/// Descriptor for the RemoteHID ("Sensor") channel.
pub const DESCRIPTOR_REMOTE_HID: u32 = 0x0000_0800;

/// Sub-packet kinds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    HostConnect,
    DeviceDisconnect,
    InputReport,
    SetReport,
    GetReport,
    DeviceConfirmConnect,
}

impl Kind {
    pub fn from_bits(bits: u32) -> Option<Self> {
        Some(match bits {
            0 => Self::HostConnect,
            1 => Self::DeviceDisconnect,
            2 => Self::InputReport,
            3 => Self::SetReport,
            4 => Self::GetReport,
            5 => Self::DeviceConfirmConnect,
            _ => return None,
        })
    }

    pub fn bits(self) -> u32 {
        match self {
            Self::HostConnect => 0,
            Self::DeviceDisconnect => 1,
            Self::InputReport => 2,
            Self::SetReport => 3,
            Self::GetReport => 4,
            Self::DeviceConfirmConnect => 5,
        }
    }
}

/// Report type byte carried by Set/GetReport payloads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReportType {
    Input = 0,
    Output = 1,
    Feature = 2,
}

/// A decoded sub-packet header.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SubHeader {
    pub device: u8,
    pub length: u16,
    pub kind_raw: u32,
    pub has_timestamp: bool,
    pub is_response: bool,
    pub has_status: bool,
}

impl SubHeader {
    pub fn parse(value: u32) -> Self {
        Self {
            device: (value & 0x7F) as u8,
            length: ((value >> 7) & 0x3FF) as u16,
            kind_raw: (value >> 17) & 0x7,
            has_timestamp: (value >> 20) & 1 == 1,
            is_response: (value >> 21) & 1 == 1,
            has_status: (value >> 22) & 1 == 1,
        }
    }

    pub fn kind(&self) -> Option<Kind> {
        Kind::from_bits(self.kind_raw)
    }
}

/// Encodes a sub-packet header. `length` counts the header itself.
pub fn encode_sub_header(device: u8, length: usize, kind: Kind) -> u32 {
    (device as u32 & 0x7F) | ((length as u32 & 0x3FF) << 7) | (kind.bits() << 17)
}

/// Message counter carried in the body prefix.
///
/// Must start at 1: a `HostConnect` with generation 0 is silently ignored by the
/// accessory (verified on AirPods 3 - generation 0 yields no DeviceConnect, 1 and
/// above yield one per service). The counter advances per message and wraps.
#[derive(Debug)]
pub struct Generation(u8);

impl Default for Generation {
    fn default() -> Self {
        Self(1)
    }
}

impl Generation {
    pub fn next(&mut self) -> u8 {
        let g = self.0;
        self.0 = self.0.wrapping_add(1);
        if self.0 == 0 {
            self.0 = 1; // never emit the ignored zero value again
        }
        g
    }

    pub fn current(&self) -> u8 {
        self.0
    }
}

/// Builds one RemoteHID frame.
///
/// `side_flags` is `2 * (side & 1)`; the receiver folds the bud side into device
/// bit 6, so single-side use can pass 0.
pub fn make_frame(generation: u8, device: u8, kind: Kind, payload: &[u8]) -> Vec<u8> {
    let length = 4 + payload.len();
    let header = encode_sub_header(device, length, kind);

    let mut body = Vec::with_capacity(4 + length);
    body.push(0x00); // transport version: 0 for 0x800
    body.push(generation);
    body.push(0x00);
    body.push(0x00); // side flags
    body.extend_from_slice(&header.to_le_bytes());
    body.extend_from_slice(payload);

    let mut frame = vec![0x04, 0x00, 0x04, 0x00, 0x17, 0x00];
    frame.extend_from_slice(&DESCRIPTOR_REMOTE_HID.to_le_bytes());
    frame.extend_from_slice(&(body.len() as u16).to_le_bytes());
    frame.extend_from_slice(&body);
    frame
}

/// The document's HostConnect: empty payload, device 0.
pub fn make_host_connect(generation: u8) -> Vec<u8> {
    make_frame(generation, 0, Kind::HostConnect, &[])
}

/// SetReport starting or stopping a report stream.
///
/// `interval_us` of 0 stops it. Per the document this targets Feature report 1
/// on `devmotion6` (device 7); see the module docs for why that does not line up
/// on our unit.
pub fn make_set_interval(generation: u8, device: u8, interval_us: u32) -> Vec<u8> {
    let mut payload = Vec::with_capacity(6);
    payload.push(ReportType::Feature as u8);
    payload.push(1); // report id: the interval lives in report 1
    payload.extend_from_slice(&interval_us.to_le_bytes());
    make_frame(generation, device, Kind::SetReport, &payload)
}

/// GetReport for Feature report 1, used to read back the configured interval.
pub fn make_get_interval(generation: u8, device: u8) -> Vec<u8> {
    make_frame(generation, device, Kind::GetReport, &[ReportType::Feature as u8, 1])
}

/// One service advertised by a DeviceConnect.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteDevice {
    /// Device id from the sub-packet header.
    pub device: u8,
    /// `AccessoryService` value from the plist, e.g. `neorelay`.
    pub service: String,
    /// `MaxReportSize` from the plist.
    pub max_report_size: Option<u16>,
    /// HID report ids found in the descriptor.
    pub report_ids: Vec<u8>,
    /// Whether the descriptor declares any Feature report.
    pub has_feature_report: bool,
}

/// Splits a received frame into (generation, sub-header, payload).
pub fn parse_frame(frame: &[u8]) -> Option<(u8, SubHeader, &[u8])> {
    if frame.len() < 8 || frame[4] != 0x17 {
        return None;
    }
    let descriptor = u32::from_le_bytes([frame[6], frame[7], frame[8], frame[9]]);
    if descriptor != DESCRIPTOR_REMOTE_HID {
        return None;
    }
    let body_len = u16::from_le_bytes([frame[10], frame[11]]) as usize;
    let body = frame.get(12..12 + body_len)?;
    if body.len() < 8 {
        return None;
    }
    let generation = body[1];
    let header = u32::from_le_bytes([body[4], body[5], body[6], body[7]]);
    Some((generation, SubHeader::parse(header), &body[8..]))
}

/// Extracts the HID report ids declared in a descriptor, and whether any of them
/// is a Feature report.
///
/// The HID item stream is scanned for `0x85` (Report ID) and the surrounding
/// Main item (`0x80`/0x81 Input, 0x90/0x91 Output, 0xB0/0xB1 Feature) that closes
/// the current report.
pub fn scan_report_ids(descriptor: &[u8]) -> (Vec<u8>, bool) {
    let mut ids = Vec::new();
    let mut pending_id: Option<u8> = None;
    let mut has_feature = false;
    let mut i = 0;
    while i < descriptor.len() {
        let prefix = descriptor[i];
        if prefix == 0xFE {
            // Long item: prefix, data size, tag, data.
            if i + 3 > descriptor.len() {
                break;
            }
            let size = descriptor[i + 1] as usize;
            i += 3 + size;
            continue;
        }
        let size = match prefix & 0x03 {
            0 => 4,
            1 => 1,
            2 => 2,
            _ => 4,
        };
        let tag = prefix & 0xFC;
        if tag == 0x84 {
            // Report ID
            if i + 1 < descriptor.len() {
                let id = descriptor[i + 1];
                if !ids.contains(&id) {
                    ids.push(id);
                }
                pending_id = Some(id);
            }
        } else if tag == 0xB0 {
            // Feature
            has_feature = true;
        }
        let _ = pending_id.take();
        i += 1 + size;
    }
    (ids, has_feature)
}

/// Collects the services announced in DeviceConnect frames.
#[derive(Debug, Default, Clone)]
pub struct DeviceRegistry {
    devices: HashMap<u8, RemoteDevice>,
}

impl DeviceRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Records a DeviceConnect. `plist_text` is the decoded plist body.
    pub fn record(&mut self, device: u8, plist_text: &str, descriptor: &[u8]) {
        let service = extract_key(plist_text, "AccessoryService").unwrap_or_default();
        let max_report_size = extract_int(plist_text, "MaxReportSize").map(|v| v as u16);
        let (report_ids, has_feature_report) = scan_report_ids(descriptor);
        self.devices.insert(
            device,
            RemoteDevice {
                device,
                service,
                max_report_size,
                report_ids,
                has_feature_report,
            },
        );
    }

    pub fn by_service(&self, name: &str) -> Option<&RemoteDevice> {
        self.devices.values().find(|d| d.service == name)
    }

    pub fn all(&self) -> impl Iterator<Item = &RemoteDevice> {
        self.devices.values()
    }

    pub fn clear(&mut self) {
        self.devices.clear();
    }

    /// The device that streams motion, chosen by service name because the ids
    /// differ between firmware revisions.
    pub fn motion_device(&self) -> Option<&RemoteDevice> {
        self.by_service("devmotion6").or_else(|| self.by_service("neorelay"))
    }
}

fn extract_key(text: &str, key: &str) -> Option<String> {
    let marker = format!("<key>{}</key><string>", key);
    let start = text.find(&marker)? + marker.len();
    let rest = &text[start..];
    let end = rest.find('<')?;
    Some(rest[..end].to_string())
}

fn extract_int(text: &str, key: &str) -> Option<i64> {
    let marker = format!("<key>{}</key><integer>", key);
    let start = text.find(&marker)? + marker.len();
    let rest = &text[start..];
    let end = rest.find('<')?;
    rest[..end].parse().ok()
}

/// The 58-byte motion block carried by `devmotion6` InputReports.
///
/// The layout matches `SensorDataWX`, so [`crate::core::head_tracking`] can parse
/// either source.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MotionSample {
    pub rotation: [i16; 3],
    pub user_accel: [i16; 3],
    pub gravity: [i16; 3],
    pub counter: u32,
}

impl MotionSample {
    /// Parses the block that follows the report-id byte.
    pub fn parse(block: &[u8]) -> Option<Self> {
        if block.len() < 58 {
            return None;
        }
        let i16_at = |off: usize| i16::from_le_bytes([block[off], block[off + 1]]);
        Some(Self {
            rotation: [i16_at(26), i16_at(28), i16_at(30)],
            user_accel: [i16_at(38), i16_at(40), i16_at(42)],
            gravity: [i16_at(44), i16_at(46), i16_at(48)],
            counter: u32::from_le_bytes([block[54], block[55], block[56], block[57]]),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sub_header_matches_documented_examples() {
        // 07 05 06 00 -> device 7, length 10, kind 3
        let h = SubHeader::parse(0x0006_0507);
        assert_eq!(h.device, 7);
        assert_eq!(h.length, 10);
        assert_eq!(h.kind(), Some(Kind::SetReport));

        // 00 02 00 00 -> device 0, length 4, kind 0
        let h = SubHeader::parse(0x0000_0200);
        assert_eq!(h.device, 0);
        assert_eq!(h.length, 4);
        assert_eq!(h.kind(), Some(Kind::HostConnect));

        // 87 1f 04 00 -> device 7, length 63, kind 2
        let h = SubHeader::parse(0x0004_1F87);
        assert_eq!(h.device, 7);
        assert_eq!(h.length, 63);
        assert_eq!(h.kind(), Some(Kind::InputReport));
    }

    #[test]
    fn encode_sub_header_round_trips() {
        for (dev, len, kind) in [
            (0u8, 4usize, Kind::HostConnect),
            (7, 10, Kind::SetReport),
            (7, 63, Kind::InputReport),
            (8, 633, Kind::HostConnect),
        ] {
            let raw = encode_sub_header(dev, len, kind);
            let parsed = SubHeader::parse(raw);
            assert_eq!(parsed.device, dev);
            assert_eq!(parsed.length as usize, len);
            assert_eq!(parsed.kind(), Some(kind));
        }
    }

    #[test]
    fn host_connect_frame_matches_document() {
        // 04 00 04 00 17 00 00 08 00 00 08 00  00 05 00 00  00 02 00 00
        let frame = make_host_connect(5);
        assert_eq!(
            frame,
            vec![
                0x04, 0x00, 0x04, 0x00, 0x17, 0x00, 0x00, 0x08, 0x00, 0x00, 0x08, 0x00, 0x00, 0x05,
                0x00, 0x00, 0x00, 0x02, 0x00, 0x00
            ]
        );
    }

    #[test]
    fn set_interval_frame_matches_document() {
        // 04 00 04 00 17 00 00 08 00 00 0e 00
        // 00 06 00 00  07 05 06 00  02  01 10 27 00 00
        let frame = make_set_interval(6, 7, 10_000);
        assert_eq!(
            frame,
            vec![
                0x04, 0x00, 0x04, 0x00, 0x17, 0x00, 0x00, 0x08, 0x00, 0x00, 0x0E, 0x00, 0x00, 0x06,
                0x00, 0x00, 0x07, 0x05, 0x06, 0x00, 0x02, 0x01, 0x10, 0x27, 0x00, 0x00
            ]
        );
    }

    #[test]
    fn get_interval_frame_has_kind_4_and_length_6() {
        let frame = make_get_interval(7, 7);
        let raw = u32::from_le_bytes([frame[16], frame[17], frame[18], frame[19]]);
        let h = SubHeader::parse(raw);
        assert_eq!(h.device, 7);
        // The document states GetReport's length is 6: 4-byte header plus the
        // 2-byte (type, report id) payload.
        assert_eq!(h.length, 6);
        assert_eq!(h.kind(), Some(Kind::GetReport));
        assert_eq!(&frame[20..], &[0x02, 0x01]);
    }

    #[test]
    fn parse_frame_reads_generation_and_payload() {
        let frame = make_set_interval(9, 3, 20_000);
        let (generation, header, payload) = parse_frame(&frame).expect("should parse");
        assert_eq!(generation, 9);
        assert_eq!(header.device, 3);
        assert_eq!(header.kind(), Some(Kind::SetReport));
        assert_eq!(payload, &[0x02, 0x01, 0x20, 0x4E, 0x00, 0x00]);
    }

    #[test]
    fn parse_frame_rejects_other_descriptors() {
        let mut frame = make_host_connect(1);
        frame[6] = 0x00;
        frame[7] = 0x00;
        frame[8] = 0x10; // 0x100000, the SensorDataWX channel
        frame[9] = 0x00;
        assert!(parse_frame(&frame).is_none());
    }

    #[test]
    fn scan_report_ids_finds_input_reports() {
        // Captured neorelay descriptor: Input reports 0x01-0x08 and 0xD1-0xD3,
        // with no Feature report anywhere.
        let raw: [u8; 126] = [
            0x06, 0x00, 0xff, 0x95, 0x01, 0x15, 0x00, 0x26, 0xff, 0x00, 0x0a, 0x51, 0x00, 0xa1,
            0x01, 0x09, 0x01, 0x85, 0x01, 0x76, 0x60, 0x00, 0x82, 0x22, 0x01, 0x09, 0x02, 0x85,
            0x02, 0x76, 0x70, 0x00, 0x82, 0x22, 0x01, 0x09, 0x03, 0x85, 0x03, 0x76, 0x80, 0x00,
            0x82, 0x22, 0x01, 0x09, 0x04, 0x85, 0x04, 0x76, 0x90, 0x00, 0x82, 0x22, 0x01, 0x09,
            0x05, 0x85, 0x05, 0x76, 0xa0, 0x00, 0x82, 0x22, 0x01, 0x09, 0x06, 0x85, 0x06, 0x76,
            0xb0, 0x00, 0x82, 0x22, 0x01, 0x09, 0x07, 0x85, 0x07, 0x76, 0xc0, 0x00, 0x82, 0x22,
            0x01, 0x09, 0x08, 0x85, 0x08, 0x76, 0xd0, 0x00, 0x82, 0x22, 0x01, 0x09, 0xd1, 0x85,
            0xd1, 0x76, 0x98, 0x00, 0x82, 0x22, 0x01, 0x09, 0xd2, 0x85, 0xd2, 0x76, 0xd8, 0x00,
            0x82, 0x22, 0x01, 0x09, 0xd3, 0x85, 0xd3, 0x76, 0x50, 0x00, 0x82, 0x22, 0x01, 0xc0,
        ];
        let (ids, has_feature) = scan_report_ids(&raw);
        assert_eq!(
            ids,
            vec![1, 2, 3, 4, 5, 6, 7, 8, 0xD1, 0xD2, 0xD3],
            "report ids from the captured descriptor"
        );
        assert!(
            !has_feature,
            "this descriptor declares Input reports only - which is exactly why \
             the document's Feature-report SetReport is rejected on this unit"
        );
    }

    #[test]
    fn motion_sample_parses_documented_offsets() {
        let mut block = vec![0u8; 58];
        block[0] = 0x01;
        block[26..28].copy_from_slice(&100i16.to_le_bytes());
        block[28..30].copy_from_slice(&(-200i16).to_le_bytes());
        block[44..46].copy_from_slice(&1000i16.to_le_bytes());
        block[54..58].copy_from_slice(&42u32.to_le_bytes());

        let s = MotionSample::parse(&block).expect("58 bytes is enough");
        assert_eq!(s.rotation[0], 100);
        assert_eq!(s.rotation[1], -200);
        assert_eq!(s.gravity[0], 1000);
        assert_eq!(s.counter, 42);
    }

    #[test]
    fn motion_sample_rejects_short_blocks() {
        assert!(MotionSample::parse(&[0u8; 57]).is_none());
    }

    #[test]
    fn registry_reads_service_and_capabilities() {
        let plist = "<plist><dict>\
            <key>AccessoryService</key><string>neorelay</string>\
            <key>MaxReportSize</key><integer>28</integer>\
            </dict></plist>";
        let mut reg = DeviceRegistry::new();
        reg.record(3, plist, &[0x85, 0x01, 0x82, 0x22, 0x01]);
        let d = reg.by_service("neorelay").expect("registered");
        assert_eq!(d.device, 3);
        assert_eq!(d.max_report_size, Some(28));
        assert_eq!(d.report_ids, vec![1]);
    }

    #[test]
    fn motion_device_prefers_devmotion6_then_neorelay() {
        let mut reg = DeviceRegistry::new();
        reg.record(2, "<key>AccessoryService</key><string>mando</string>", &[]);
        reg.record(3, "<key>AccessoryService</key><string>neorelay</string>", &[]);
        assert_eq!(reg.motion_device().map(|d| d.device), Some(3));

        reg.record(7, "<key>AccessoryService</key><string>devmotion6</string>", &[]);
        assert_eq!(
            reg.motion_device().map(|d| d.device),
            Some(7),
            "devmotion6 wins when the accessory exposes it"
        );
    }

    #[test]
    fn generation_starts_at_one_and_advances() {
        // Generation 0 is ignored by the accessory, so the counter must not start
        // there and must never wrap back to it.
        let mut generation = Generation::default();
        assert_eq!(generation.next(), 1);
        assert_eq!(generation.next(), 2);
        assert_eq!(generation.current(), 3);

        let mut wrap = Generation::default();
        wrap.0 = 255;
        assert_eq!(wrap.next(), 255);
        assert_eq!(wrap.current(), 1, "wraps to 1, never 0");
    }
}

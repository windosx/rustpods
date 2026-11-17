use crate::core::aacp::events::EarStatus;
use crate::core::crypto::{decrypt_proximity_payload, verify_rpa, verify_rpa_be};
use crate::core::device::AirPodsDeviceState;
use crate::system::config::AppConfig;
use crate::ui::UiEvent;
use bluer::{AdapterEvent, DiscoveryFilter, DiscoveryTransport, Session};
use futures::StreamExt;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::{mpsc, RwLock};
use tracing::{debug, info, warn};

const POPUP_COOLDOWN: Duration = Duration::from_secs(5);
const LID_SLEEP_RESET: Duration = Duration::from_secs(60);
/// How long each scan window stays active. Kept below BlueZ's 5 s discovery
/// idle timeout: if the controller stops scanning for a connection attempt,
/// BlueZ's restart timer would fire after this window has already ended (we
/// then hold no discovery client), so the scan is never re-armed underneath a
/// connection in progress.
const SCAN_WINDOW_ON: Duration = Duration::from_secs(4);
/// Quiet gap between scan windows. Gives other BLE devices a clear stretch of
/// time to complete their wake-up reconnections.
const SCAN_WINDOW_OFF: Duration = Duration::from_secs(8);
/// Gate check cadence while a scan window is open. AACP connect should stop
/// the scan promptly instead of waiting for the window to expire.
const SCAN_GATE_POLL: Duration = Duration::from_secs(1);
/// Gate check cadence while scanning is paused or disabled; no radio work is
/// happening, so a slower poll is enough for settings/state changes.
const IDLE_GATE_POLL: Duration = Duration::from_secs(5);
/// Delay before retrying when BlueZ ends our discovery stream by itself
/// (adapter reset, bus restart).
const DISCOVERY_RESTART_DELAY: Duration = Duration::from_secs(2);

#[derive(Debug, Default)]
struct LidTracker {
    last_lid_open: Option<bool>,
    last_popped_counter: Option<u8>,
    last_seen_at: Option<Instant>,
    last_popup_at: Option<Instant>,
    popup_active: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LidTransition {
    Opened,
    Closed,
    None,
}

impl LidTracker {
    fn observe(
        &mut self,
        lid_open: bool,
        lid_counter: u8,
        wearing_any: bool,
        now: Instant,
    ) -> LidTransition {
        let slept = self
            .last_seen_at
            .map(|t| now.duration_since(t) >= LID_SLEEP_RESET)
            .unwrap_or(false);
        if slept {
            self.last_lid_open = None;
            self.last_popped_counter = None;
            self.popup_active = false;
        }
        self.last_seen_at = Some(now);

        let prev_open = self.last_lid_open;
        self.last_lid_open = Some(lid_open);

        if !lid_open {
            self.last_popped_counter = None;
            if prev_open == Some(true) || self.popup_active {
                self.popup_active = false;
                return LidTransition::Closed;
            }
            return LidTransition::None;
        }

        // As soon as the user wears an earbud, dismiss any open popup and do not pop up.
        if wearing_any {
            if self.popup_active {
                self.popup_active = false;
                self.last_popped_counter = Some(lid_counter);
                return LidTransition::Closed;
            }
            self.last_popped_counter = Some(lid_counter);
            return LidTransition::None;
        }

        let new_open_event =
            prev_open == Some(false) || self.last_popped_counter != Some(lid_counter);
        if !new_open_event {
            return LidTransition::None;
        }

        let cooled_down = self
            .last_popup_at
            .map(|t| now.duration_since(t) >= POPUP_COOLDOWN)
            .unwrap_or(true);
        if cooled_down {
            self.last_popup_at = Some(now);
            self.last_popped_counter = Some(lid_counter);
            self.popup_active = true;
            LidTransition::Opened
        } else {
            LidTransition::None
        }
    }
}

/// Why the proximity scanner is or is not running. Re-evaluated every few
/// seconds so a settings toggle or an AACP connect takes effect without a
/// daemon restart.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ScanGate {
    /// The tracked AirPods are disconnected: scan for the case advertisement.
    Active,
    /// AACP is connected. It already carries battery and in-ear state, and the
    /// lid popup is only wanted before a connection exists. Scanning now would
    /// just compete with audio and other BLE traffic on a shared controller.
    PausedConnected,
    /// The user turned background BLE scanning off in the settings.
    Disabled,
    /// No AirPods are tracked, so advertisements could not be associated with
    /// a device anyway.
    NoTrackedDevice,
}

async fn scan_gate(
    dev_state: &Arc<RwLock<Option<AirPodsDeviceState>>>,
    config: &Arc<std::sync::RwLock<AppConfig>>,
) -> ScanGate {
    let enabled = config.read().map(|c| c.ble_scan_enabled).unwrap_or(true);
    if !enabled {
        return ScanGate::Disabled;
    }
    match dev_state.read().await.as_ref() {
        None => ScanGate::NoTrackedDevice,
        Some(state) if state.is_connected => ScanGate::PausedConnected,
        Some(_) => ScanGate::Active,
    }
}

/// BlueZ caches the last advertisement per device and replays every known
/// device as `DeviceAdded` when a new discovery stream starts. Those cache hits
/// are not fresh advertisements: after the lid tracker's 60 s reset, an old
/// "lid open" payload would pop the card again. This keeps the last payload
/// parsed per address and accepts only a changed one.
#[derive(Debug, Default)]
struct AdvertFreshness {
    last: HashMap<bluer::Address, Vec<u8>>,
}

impl AdvertFreshness {
    /// Records a payload without treating it as fresh (used to seed the cache
    /// entries that predate a scan window).
    fn remember(&mut self, addr: bluer::Address, data: &[u8]) {
        self.last.insert(addr, data.to_vec());
    }

    /// Returns true only when this payload differs from the last one parsed
    /// for the address, and records it.
    fn accept(&mut self, addr: bluer::Address, data: &[u8]) -> bool {
        if self
            .last
            .get(&addr)
            .is_some_and(|previous| previous.as_slice() == data)
        {
            return false;
        }
        self.remember(addr, data);
        true
    }

    /// Drops addresses BlueZ has already forgotten, so rotating private
    /// addresses do not grow the map without bound.
    fn retain_known(&mut self, known: &[bluer::Address]) {
        self.last.retain(|addr, _| known.contains(addr));
    }
}

pub async fn start_le_monitor(
    dev_state: Arc<RwLock<Option<AirPodsDeviceState>>>,
    ui_tx: mpsc::UnboundedSender<UiEvent>,
    config: Arc<std::sync::RwLock<AppConfig>>,
) -> Result<(), String> {
    let session = Session::new()
        .await
        .map_err(|e| format!("Failed to create BlueZ session for LE monitor: {}", e))?;
    let adapter = session
        .default_adapter()
        .await
        .map_err(|e| format!("Failed to get default adapter for LE monitor: {}", e))?;

    // Keep this a *regular* discovery. BlueZ classifies a LE-only transport
    // (or `duplicate_data = true`, even with transport Auto) as a filtered
    // discovery and restarts the scan immediately every time the controller
    // stops scanning to establish a connection. That immediate restart aborts
    // the pending LE connect, which surfaces as
    // `le-connection-abort-by-local` and breaks automatic reconnection of BLE
    // keyboards/mice sharing the controller. A regular scan is retried only
    // after BlueZ's idle timeout, leaving connection attempts alone.
    //
    // Suppressing duplicate reports matters too: with duplicate data enabled
    // BlueZ emits a ManufacturerData property change for every advertisement
    // from every nearby device, flooding D-Bus and keeping blueman busy. We
    // only need payload changes (lid counter, battery), which still arrive.
    let regular_filter = DiscoveryFilter {
        transport: DiscoveryTransport::Auto,
        duplicate_data: false,
        ..Default::default()
    };

    let lid_tracker = Arc::new(Mutex::new(LidTracker::default()));
    let mut previous_gate: Option<ScanGate> = None;
    let mut freshness = AdvertFreshness::default();

    loop {
        let gate = scan_gate(&dev_state, &config).await;
        if gate != ScanGate::Active {
            if previous_gate != Some(gate) {
                match gate {
                    ScanGate::PausedConnected => {
                        info!("BLE proximity scan paused: AirPods are connected")
                    }
                    ScanGate::Disabled => {
                        info!("BLE proximity scan disabled by settings")
                    }
                    ScanGate::NoTrackedDevice => {
                        info!("BLE proximity scan idle: no tracked AirPods")
                    }
                    ScanGate::Active => {}
                }
            }
            previous_gate = Some(gate);
            tokio::time::sleep(IDLE_GATE_POLL).await;
            continue;
        }

        if previous_gate != Some(ScanGate::Active) {
            info!(
                "BLE proximity monitor active ({}s scan / {}s pause, regular discovery)",
                SCAN_WINDOW_ON.as_secs(),
                SCAN_WINDOW_OFF.as_secs()
            );
        }
        previous_gate = Some(ScanGate::Active);

        // Anything already in BlueZ's cache predates this window; only
        // payload changes observed during the window are fresh. Re-seeding
        // here also covers cache updates made by other discovery clients
        // during the off gap, and drops addresses BlueZ has forgotten so
        // rotating private addresses do not grow the map without bound.
        if let Ok(addrs) = adapter.device_addresses().await {
            freshness.retain_known(&addrs);
            for addr in addrs {
                let Ok(dev) = adapter.device(addr) else {
                    continue;
                };
                let Ok(Some(mfg)) = dev.manufacturer_data().await else {
                    continue;
                };
                if let Some(apple_data) = mfg.get(&76) {
                    freshness.remember(addr, apple_data);
                }
            }
        }

        // Re-apply the filter for every window: BlueZ derives the "auto"
        // transport from the adapter's current BR/EDR/LE settings, which are
        // unknown while the adapter is powered off. A stale filter would be
        // merged as a *filtered* discovery (and re-arm the scan immediately)
        // once the adapter comes up.
        if let Err(e) = adapter.set_discovery_filter(regular_filter.clone()).await {
            // Without a filter StartDiscovery still runs a regular scan, so
            // this is not fatal.
            debug!("Failed to set regular discovery filter ({}); using defaults", e);
        }

        let mut stream = match adapter.discover_devices_with_changes().await {
            Ok(s) => s,
            Err(e) => {
                warn!("BLE discovery failed to start: {}, retrying in 3s", e);
                tokio::time::sleep(Duration::from_secs(3)).await;
                continue;
            }
        };

        let mut gate_tick = tokio::time::interval(SCAN_GATE_POLL);
        gate_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        // Consume the immediate first tick so the first gate check happens one
        // interval from now.
        gate_tick.tick().await;
        let mut interrupted = false;
        let mut stream_ended = false;
        let window_end = tokio::time::sleep(SCAN_WINDOW_ON);
        tokio::pin!(window_end);

        loop {
            tokio::select! {
                evt = stream.next() => {
                    match evt {
                        Some(AdapterEvent::DeviceAdded(addr)) => {
                            let Ok(dev) = adapter.device(addr) else {
                                continue;
                            };
                            let Ok(Some(mfg)) = dev.manufacturer_data().await else {
                                continue;
                            };
                            if let Some(apple_data) = mfg.get(&76) {
                                // Identical payload == cached listing at window
                                // start (or a duplicate re-report); skip it.
                                if !freshness.accept(addr, apple_data) {
                                    continue;
                                }
                                parse_proximity_message(
                                    apple_data,
                                    addr,
                                    &dev_state,
                                    &ui_tx,
                                    &config,
                                    &lid_tracker,
                                )
                                .await;
                            }
                        }
                        Some(_) => {}
                        None => {
                            stream_ended = true;
                            break;
                        }
                    }
                }
                _ = gate_tick.tick() => {
                    if scan_gate(&dev_state, &config).await != ScanGate::Active {
                        interrupted = true;
                        break;
                    }
                }
                _ = &mut window_end => {
                    break;
                }
            }
        }

        // Dropping the stream ends our discovery client. When no other client
        // needs discovery, BlueZ stops the scan, which is what releases the
        // controller while AirPods are connected, scanning is disabled, or we
        // are in the quiet gap between windows.
        drop(stream);

        if interrupted {
            continue;
        }

        if stream_ended {
            warn!(
                "BLE discovery stream ended; restarting in {}s",
                DISCOVERY_RESTART_DELAY.as_secs()
            );
            tokio::time::sleep(DISCOVERY_RESTART_DELAY).await;
        } else {
            tokio::time::sleep(SCAN_WINDOW_OFF).await;
        }
    }
}

fn load_hex_key(hex_opt: Option<&str>) -> Option<[u8; 16]> {
    let bytes = hex::decode(hex_opt?).ok()?;
    if bytes.len() != 16 {
        return None;
    }
    let mut out = [0u8; 16];
    out.copy_from_slice(&bytes);
    Some(out)
}

async fn parse_proximity_message(
    data: &[u8],
    device_addr: bluer::Address,
    dev_state: &Arc<RwLock<Option<AirPodsDeviceState>>>,
    ui_tx: &mpsc::UnboundedSender<UiEvent>,
    config: &Arc<std::sync::RwLock<AppConfig>>,
    lid_tracker: &Arc<Mutex<LidTracker>>,
) {
    // Proximity pairing message starts with 0x07 and is at least 11 bytes long.
    if data.len() < 11 || data[0] != 0x07 {
        return;
    }
    if data[2] != 0x01 {
        debug!(
            "Skipping non-paired 0x07 proximity message from {} (mode=0x{:02X})",
            device_addr, data[2]
        );
        return;
    }

    let (irk_opt, enc_key_opt, popup_enabled) = match config.read() {
        Ok(c) => (
            load_hex_key(c.proximity_irk.as_deref()),
            load_hex_key(c.proximity_enc_key.as_deref()),
            c.case_popup_enabled,
        ),
        Err(_) => (None, None, true),
    };

    let addr_str = device_addr.to_string();
    let rpa_le = irk_opt.is_some_and(|irk| verify_rpa(&addr_str, &irk));
    let rpa_be = !rpa_le && irk_opt.is_some_and(|irk| verify_rpa_be(&addr_str, &irk));
    let rpa_verified = rpa_le || rpa_be;

    let status = data[5] as usize;
    let pods_battery_byte = data[6];
    let flags_and_case = data[7];
    let lid_indicator = data[8];

    let primary_left = (status & 0x20) != 0;
    let are_values_flipped = !primary_left;
    let is_this_pod_in_case = (status & 0x40) != 0;
    let lid_open = ((lid_indicator >> 3) & 0x01) == 0;
    let lid_counter = lid_indicator & 0x07;

    let parse_nibble_battery = |nibble: u8| -> Option<u8> {
        match nibble {
            0x0..=0x9 => Some(nibble * 10),
            0xA..=0xE => Some(100),
            _ => None,
        }
    };

    let mut left_battery = parse_nibble_battery(if are_values_flipped {
        (pods_battery_byte >> 4) & 0x0F
    } else {
        pods_battery_byte & 0x0F
    });
    let mut right_battery = parse_nibble_battery(if are_values_flipped {
        pods_battery_byte & 0x0F
    } else {
        (pods_battery_byte >> 4) & 0x0F
    });
    let mut case_battery = parse_nibble_battery(flags_and_case & 0x0F);

    let flags = (flags_and_case >> 4) & 0x0F;
    let mut right_charging = if are_values_flipped {
        (flags & 0x01) != 0
    } else {
        (flags & 0x02) != 0
    };
    let mut left_charging = if are_values_flipped {
        (flags & 0x02) != 0
    } else {
        (flags & 0x01) != 0
    };
    let mut case_charging = (flags & 0x04) != 0;

    if let Some(enc_key) = enc_key_opt
        && data.len() >= 27 {
            let mut block = [0u8; 16];
            block.copy_from_slice(&data[data.len() - 16..]);
            let dec = decrypt_proximity_payload(&enc_key, &block);
            let left_idx = if are_values_flipped { 2 } else { 1 };
            let right_idx = if are_values_flipped { 1 } else { 2 };

            let parse_exact = |raw: u8| -> Option<(u8, bool)> {
                let charging = (raw & 0x80) != 0;
                let level = raw & 0x7F;
                if level <= 100 {
                    Some((level, charging))
                } else {
                    None
                }
            };

            if let Some((lvl, chg)) = parse_exact(dec[left_idx]) {
                left_battery = Some(lvl);
                left_charging = chg;
            }
            if let Some((lvl, chg)) = parse_exact(dec[right_idx]) {
                right_battery = Some(lvl);
                right_charging = chg;
            }
            if let Some((lvl, chg)) = parse_exact(dec[3]) {
                case_battery = Some(lvl);
                case_charging = chg;
            }
        }

    let xor_factor = are_values_flipped ^ is_this_pod_in_case;
    let left_in_ear = if xor_factor {
        (status & 0x08) != 0
    } else {
        (status & 0x02) != 0
    };
    let right_in_ear = if xor_factor {
        (status & 0x02) != 0
    } else {
        (status & 0x08) != 0
    };

    let mut lock = dev_state.write().await;
    if let Some(ref mut state) = *lock {
        if state.mac_address != addr_str && !rpa_verified {
            debug!(
                "Ignoring proximity message from {} (tracked device is {}, rpa_verified=false)",
                device_addr, state.mac_address
            );
            return;
        }

        // Preserve AACP-authoritative in-ear state when connected, since BLE status
        // bits can lag or reflect only the case-side bud.
        let aacp_wearing = state.is_connected && state.is_wearing_any();

        if let Some(lb) = left_battery {
            state.battery_left = Some(lb);
            state.battery_left_charging = left_charging;
        }
        if let Some(rb) = right_battery {
            state.battery_right = Some(rb);
            state.battery_right_charging = right_charging;
        }
        if let Some(cb) = case_battery {
            state.battery_case = Some(cb);
            state.battery_case_charging = case_charging;
        }

        if !state.is_connected {
            state.left_in_ear = EarStatus::Disconnected;
            state.right_in_ear = EarStatus::Disconnected;
        }

        let wearing_any = aacp_wearing || left_in_ear || right_in_ear;

        let _ = ui_tx.send(UiEvent::DeviceStateUpdated(state.clone()));

        if popup_enabled {
            let transition = lid_tracker
                .lock()
                .map(|mut t| t.observe(lid_open, lid_counter, wearing_any, Instant::now()))
                .unwrap_or(LidTransition::None);
            match transition {
                LidTransition::Opened => {
                    info!(
                        "Case lid opened (counter={}, in_case={}), showing popup",
                        lid_counter, is_this_pod_in_case
                    );
                    let _ = ui_tx.send(UiEvent::CaseLidOpened);
                }
                LidTransition::Closed => {
                    info!("Case lid closed / bud in ear, dismissing popup");
                    let _ = ui_tx.send(UiEvent::CaseLidClosed);
                }
                LidTransition::None => {}
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn scan_gate_tracks_settings_and_connection_state() {
        use crate::core::models::AirPodsModel;

        let config = Arc::new(std::sync::RwLock::new(AppConfig::default()));
        let dev_state: Arc<RwLock<Option<AirPodsDeviceState>>> = Arc::new(RwLock::new(None));

        assert_eq!(
            scan_gate(&dev_state, &config).await,
            ScanGate::NoTrackedDevice
        );

        let mut state = AirPodsDeviceState::new(
            "AA:BB:CC:DD:EE:FF".to_string(),
            "AirPods".to_string(),
            AirPodsModel::AirPods3,
        );
        *dev_state.write().await = Some(state.clone());
        assert_eq!(scan_gate(&dev_state, &config).await, ScanGate::Active);

        state.is_connected = true;
        *dev_state.write().await = Some(state.clone());
        assert_eq!(
            scan_gate(&dev_state, &config).await,
            ScanGate::PausedConnected
        );

        config.write().unwrap().ble_scan_enabled = false;
        assert_eq!(scan_gate(&dev_state, &config).await, ScanGate::Disabled);
    }

    #[test]
    fn advert_freshness_ignores_cached_replays_until_payload_changes() {
        let mut freshness = AdvertFreshness::default();
        let addr: bluer::Address = "AA:BB:CC:DD:EE:FF".parse().unwrap();

        // Seeded from the BlueZ cache at startup: replaying it is not fresh.
        freshness.remember(addr, &[0x07, 0x19]);
        assert!(!freshness.accept(addr, &[0x07, 0x19]));
        // A changed payload is a real advertisement.
        assert!(freshness.accept(addr, &[0x07, 0x19, 0x01]));
        assert!(!freshness.accept(addr, &[0x07, 0x19, 0x01]));
        // Once BlueZ forgets the address, the next payload counts as new again.
        freshness.retain_known(&[]);
        assert!(freshness.accept(addr, &[0x07, 0x19, 0x01]));
    }

    /// Live BlueZ check for the scan windowing and the connected-pause gate.
    /// Ignored by default because it starts and stops discovery on the real
    /// adapter; run with:
    /// `cargo test -- --ignored scan_windows_toggle_discovery`.
    #[tokio::test]
    #[ignore = "live BlueZ: drives discovery on the system adapter"]
    async fn scan_windows_toggle_discovery_and_pause_when_connected() {
        use crate::core::models::AirPodsModel;

        async fn wait_for(adapter: &bluer::Adapter, want: bool, within: Duration) -> bool {
            let deadline = tokio::time::Instant::now() + within;
            loop {
                if adapter.is_discovering().await.unwrap_or(!want) == want {
                    return true;
                }
                if tokio::time::Instant::now() >= deadline {
                    return false;
                }
                tokio::time::sleep(Duration::from_millis(200)).await;
            }
        }

        let session = Session::new().await.unwrap();
        let adapter = session.default_adapter().await.unwrap();

        let config = Arc::new(std::sync::RwLock::new(AppConfig::default()));
        let dev_state: Arc<RwLock<Option<AirPodsDeviceState>>> = Arc::new(RwLock::new(Some(
            AirPodsDeviceState::new(
                "AA:BB:CC:DD:EE:FF".to_string(),
                "AirPods".to_string(),
                AirPodsModel::AirPods3,
            ),
        )));
        let (ui_tx, _ui_rx) = mpsc::unbounded_channel();

        let monitor = tokio::spawn(start_le_monitor(dev_state.clone(), ui_tx, config));

        assert!(
            wait_for(&adapter, true, Duration::from_secs(3)).await,
            "first scan window should open"
        );
        assert!(
            wait_for(&adapter, false, SCAN_WINDOW_ON + Duration::from_secs(3)).await,
            "scan should stop between windows"
        );
        assert!(
            wait_for(&adapter, true, SCAN_WINDOW_OFF + Duration::from_secs(3)).await,
            "next scan window should open"
        );

        dev_state.write().await.as_mut().unwrap().is_connected = true;
        assert!(
            wait_for(&adapter, false, SCAN_GATE_POLL + Duration::from_secs(2)).await,
            "connected AirPods must pause the scan before the window ends"
        );

        monitor.abort();
    }

    #[test]
    fn lid_tracker_does_not_repeat_for_same_open_session_even_after_gap() {
        let mut tracker = LidTracker::default();
        let t0 = Instant::now();

        // First open triggers once.
        assert_eq!(tracker.observe(true, 1, false, t0), LidTransition::Opened);
        // Same open session after 1s, 10s, 30s (e.g. user clicked Done) must NOT re-trigger.
        assert_eq!(
            tracker.observe(true, 1, false, t0 + Duration::from_secs(1)),
            LidTransition::None
        );
        assert_eq!(
            tracker.observe(true, 1, false, t0 + Duration::from_secs(12)),
            LidTransition::None
        );
        assert_eq!(
            tracker.observe(true, 1, false, t0 + Duration::from_secs(30)),
            LidTransition::None
        );

        // Putting a bud in ear dismisses the active popup and blocks reopening.
        assert_eq!(
            tracker.observe(true, 1, true, t0 + Duration::from_secs(32)),
            LidTransition::Closed
        );
        assert_eq!(
            tracker.observe(true, 2, true, t0 + Duration::from_secs(35)),
            LidTransition::None
        );
    }

    #[test]
    fn lid_tracker_reopens_on_close_then_open_or_counter_increment() {
        let mut tracker = LidTracker::default();
        let t0 = Instant::now();

        assert_eq!(tracker.observe(true, 1, false, t0), LidTransition::Opened);
        assert_eq!(
            tracker.observe(false, 1, false, t0 + Duration::from_secs(2)),
            LidTransition::Closed
        );
        // Within 5s cooldown -> suppressed.
        assert_eq!(
            tracker.observe(true, 2, false, t0 + Duration::from_secs(3)),
            LidTransition::None
        );
        // After closing and reopening past 5s cooldown -> Opened again.
        assert_eq!(
            tracker.observe(false, 2, false, t0 + Duration::from_secs(6)),
            LidTransition::Closed
        );
        assert_eq!(
            tracker.observe(true, 3, false, t0 + Duration::from_secs(7)),
            LidTransition::Opened
        );
        // Long sleep (> 60s silence) resets session so a fresh open triggers even if counter wrapped.
        assert_eq!(
            tracker.observe(true, 3, false, t0 + Duration::from_secs(70)),
            LidTransition::Opened
        );
    }
}

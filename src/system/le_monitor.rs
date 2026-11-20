use crate::core::aacp::events::EarStatus;
use crate::core::crypto::{decrypt_proximity_payload, verify_rpa, verify_rpa_be};
use crate::core::device::AirPodsDeviceState;
use crate::system::config::AppConfig;
use crate::ui::UiEvent;
use bluer::monitor::{data_type, Monitor, MonitorEvent, Pattern, Type};
use bluer::{AdapterEvent, DiscoveryFilter, DiscoveryTransport, Session};
use futures::StreamExt;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::{mpsc, RwLock};
use tracing::{debug, info, warn};

const POPUP_COOLDOWN: Duration = Duration::from_secs(5);
const LID_SLEEP_RESET: Duration = Duration::from_secs(60);

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

/// Why the proximity scanner is or is not running.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScanGate {
    /// Actively monitoring for AirPods proximity beacons.
    Active,
    /// Case popup is disabled in settings.
    Disabled,
    /// No AirPods are tracked, so beacons cannot be associated with any device.
    NoTrackedDevice,
}

pub async fn scan_gate(
    dev_state: &Arc<RwLock<Option<AirPodsDeviceState>>>,
    config: &Arc<std::sync::RwLock<AppConfig>>,
) -> ScanGate {
    let enabled = config.read().map(|c| c.case_popup_enabled).unwrap_or(true);
    if !enabled {
        return ScanGate::Disabled;
    }
    match dev_state.read().await.as_ref() {
        None => ScanGate::NoTrackedDevice,
        Some(_) => ScanGate::Active,
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

    let lid_tracker = Arc::new(Mutex::new(LidTracker::default()));
    let mut prev_gate: Option<ScanGate> = None;

    info!("Starting BLE proximity monitor (AdvertisementMonitor with discovery fallback)...");

    loop {
        let gate = scan_gate(&dev_state, &config).await;
        if gate != ScanGate::Active {
            if prev_gate != Some(gate) {
                match gate {
                    ScanGate::Disabled => {
                        info!("BLE proximity monitoring disabled by settings (case_popup_enabled=false)");
                    }
                    ScanGate::NoTrackedDevice => {
                        info!("BLE proximity monitoring idle: no paired/tracked AirPods");
                    }
                    ScanGate::Active => {}
                }
            }
            prev_gate = Some(gate);
            tokio::time::sleep(Duration::from_secs(3)).await;
            continue;
        }

        prev_gate = Some(ScanGate::Active);

        // Try modern BlueZ AdvertisementMonitor first: hardware/kernel pattern matching
        // without continuous generic discovery, eliminating RF congestion and BLE HID conflicts.
        match run_advertisement_monitor(&adapter, &dev_state, &ui_tx, &config, &lid_tracker).await {
            Ok(()) => {
                debug!("AdvertisementMonitor session completed normally");
            }
            Err(e) => {
                info!(
                    "AdvertisementMonitor unavailable ({}); using discovery stream fallback (Tip: enable 'Experimental = true' in /etc/bluetooth/main.conf for hardware pattern monitoring)",
                    e
                );
                if let Err(e2) =
                    run_discovery_stream(&adapter, &dev_state, &ui_tx, &config, &lid_tracker).await
                {
                    warn!("Discovery stream error ({}); retrying in 3s", e2);
                }
            }
        }

        tokio::time::sleep(Duration::from_secs(1)).await;
    }
}

/// Runs kernel/hardware pattern-matched monitoring via BlueZ AdvertisementMonitor1.
///
/// This registers an offloaded pattern monitor matching Apple Manufacturer Specific Data
/// (0x004C) with the 0x07 Proximity Pairing sub-type. It avoids running full-band active
/// discovery, eliminating conflicts with BLE keyboards/mice reconnecting.
async fn run_advertisement_monitor(
    adapter: &bluer::Adapter,
    dev_state: &Arc<RwLock<Option<AirPodsDeviceState>>>,
    ui_tx: &mpsc::UnboundedSender<UiEvent>,
    config: &Arc<std::sync::RwLock<AppConfig>>,
    lid_tracker: &Arc<Mutex<LidTracker>>,
) -> Result<(), bluer::Error> {
    let monitor_mgr = adapter.monitor().await?;
    let pattern = Pattern::new(
        data_type::MANUFACTURER_SPECIFIC_DATA,
        0,
        &[0x4C, 0x00, 0x07],
    );
    let monitor = Monitor {
        monitor_type: Type::OrPatterns,
        patterns: Some(vec![pattern]),
        ..Default::default()
    };
    let mut handle = monitor_mgr.register(monitor).await?;
    info!("Registered BlueZ AdvertisementMonitor for AirPods Proximity Beacons");

    let mut gate_tick = tokio::time::interval(Duration::from_secs(2));
    gate_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    gate_tick.tick().await;

    let mut active_devices: HashMap<bluer::Address, tokio::task::JoinHandle<()>> = HashMap::new();

    loop {
        tokio::select! {
            evt = handle.next() => {
                match evt {
                    Some(MonitorEvent::DeviceFound(device_id)) => {
                        let addr = device_id.device;
                        debug!("AdvertisementMonitor: device found {}", addr);

                        if let Ok(dev) = adapter.device(addr) {
                            if let Ok(Some(mfg)) = dev.manufacturer_data().await
                                && let Some(apple_data) = mfg.get(&76)
                            {
                                parse_proximity_message(
                                    apple_data,
                                    addr,
                                    dev_state,
                                    ui_tx,
                                    config,
                                    lid_tracker,
                                ).await;
                            }

                            // Watch subsequent property changes on this device while in range
                            // (e.g. lid opened, closed, battery level updates).
                            if let std::collections::hash_map::Entry::Vacant(entry) = active_devices.entry(addr) {
                                let dev_c = dev.clone();
                                let dev_state_c = dev_state.clone();
                                let ui_tx_c = ui_tx.clone();
                                let cfg_c = config.clone();
                                let lid_c = lid_tracker.clone();

                                let task = tokio::spawn(async move {
                                    if let Ok(mut events) = dev_c.events().await {
                                        while let Some(dev_evt) = events.next().await {
                                            if let bluer::DeviceEvent::PropertyChanged(bluer::DeviceProperty::ManufacturerData(mfg)) = dev_evt
                                                && let Some(apple_data) = mfg.get(&76)
                                            {
                                                parse_proximity_message(
                                                    apple_data,
                                                    addr,
                                                    &dev_state_c,
                                                    &ui_tx_c,
                                                    &cfg_c,
                                                    &lid_c,
                                                ).await;
                                            }
                                        }
                                    }
                                });
                                entry.insert(task);
                            }
                        }
                    }
                    Some(MonitorEvent::DeviceLost(device_id)) => {
                        let addr = device_id.device;
                        debug!("AdvertisementMonitor: device lost {}", addr);
                        if let Some(task) = active_devices.remove(&addr) {
                            task.abort();
                        }
                    }
                    Some(_) => {}
                    None => {
                        warn!("AdvertisementMonitor event stream ended");
                        break;
                    }
                }
            }
            _ = gate_tick.tick() => {
                if scan_gate(dev_state, config).await != ScanGate::Active {
                    info!("Scan gate no longer active, releasing AdvertisementMonitor");
                    break;
                }
                active_devices.retain(|_, task| !task.is_finished());
            }
        }
    }

    for (_, task) in active_devices {
        task.abort();
    }

    Ok(())
}

/// Fallback discovery stream used when AdvertisementMonitor1 is unsupported.
async fn run_discovery_stream(
    adapter: &bluer::Adapter,
    dev_state: &Arc<RwLock<Option<AirPodsDeviceState>>>,
    ui_tx: &mpsc::UnboundedSender<UiEvent>,
    config: &Arc<std::sync::RwLock<AppConfig>>,
    lid_tracker: &Arc<Mutex<LidTracker>>,
) -> Result<(), bluer::Error> {
    let le_filter = DiscoveryFilter {
        transport: DiscoveryTransport::Le,
        duplicate_data: true,
        ..Default::default()
    };
    if let Err(e) = adapter.set_discovery_filter(le_filter).await {
        warn!("Failed to set LE discovery filter ({}), trying Auto", e);
        let auto_filter = DiscoveryFilter {
            transport: DiscoveryTransport::Auto,
            duplicate_data: true,
            ..Default::default()
        };
        let _ = adapter.set_discovery_filter(auto_filter).await;
    }

    let mut stream = adapter.discover_devices_with_changes().await?;
    info!("BLE discovery stream active (duplicate_data=true fallback mode)");

    let mut gate_tick = tokio::time::interval(Duration::from_secs(2));
    gate_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    gate_tick.tick().await;

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
                            parse_proximity_message(
                                apple_data,
                                addr,
                                dev_state,
                                ui_tx,
                                config,
                                lid_tracker,
                            )
                            .await;
                        }
                    }
                    Some(_) => {}
                    None => {
                        warn!("BLE discovery stream ended");
                        break;
                    }
                }
            }
            _ = gate_tick.tick() => {
                if scan_gate(dev_state, config).await != ScanGate::Active {
                    break;
                }
            }
        }
    }

    Ok(())
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
            0xA => Some(100),
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
        && data.len() >= 27
    {
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

    let model_id = u16::from_le_bytes([data[3], data[4]]);
    let packet_model = crate::core::models::AirPodsModel::from_product_id(model_id);

    let mut lock = dev_state.write().await;
    if let Some(ref mut state) = *lock {
        let model_matches = packet_model == state.model;
        let is_target = state.mac_address == addr_str
            || rpa_verified
            || (irk_opt.is_none() && model_matches);

        if !is_target {
            debug!(
                "Ignoring proximity message from {} (tracked device is {}, rpa_verified=false, packet_model={:?})",
                device_addr, state.mac_address, packet_model
            );
            return;
        }

        info!(
            "Accepted Apple Proximity packet from {} (rpa_verified={}, connected={}, lid_open={}, counter={})",
            device_addr, rpa_verified, state.is_connected, lid_open, lid_counter
        );

        // Preserve AACP-authoritative in-ear state and battery when connected.
        // AACP provides exact 1%-granular battery readings directly from the earbuds.
        let aacp_wearing = state.is_connected && state.is_wearing_any();

        if !state.is_connected {
            // When disconnected, BLE proximity is the sole source of battery
            // for the popup window and device state.
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

            state.left_in_ear = EarStatus::Disconnected;
            state.right_in_ear = EarStatus::Disconnected;
        } else {
            // When connected, AACP is authoritative for earbud battery and in-ear status.
            // If AACP has not provided earbud battery yet, use BLE as initial fallback.
            if state.battery_left.is_none()
                && let Some(lb) = left_battery
            {
                state.battery_left = Some(lb);
                state.battery_left_charging = left_charging;
            }
            if state.battery_right.is_none()
                && let Some(rb) = right_battery
            {
                state.battery_right = Some(rb);
                state.battery_right_charging = right_charging;
            }
            // Only update case battery if the case is actively open or in-use.
            if (lid_open || is_this_pod_in_case) && case_battery.is_some() {
                state.battery_case = case_battery;
                state.battery_case_charging = case_charging;
            }
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

        // Connected AirPods remain Active so case lid open and case battery
        // updates are tracked even when headphones are in use.
        state.is_connected = true;
        *dev_state.write().await = Some(state.clone());
        assert_eq!(scan_gate(&dev_state, &config).await, ScanGate::Active);

        config.write().unwrap().case_popup_enabled = false;
        assert_eq!(scan_gate(&dev_state, &config).await, ScanGate::Disabled);
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

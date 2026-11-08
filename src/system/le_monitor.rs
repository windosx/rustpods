use crate::core::aacp::events::EarStatus;
use crate::core::device::AirPodsDeviceState;
use crate::ui::UiEvent;
use bluer::Session;
use futures::StreamExt;
use std::sync::Arc;
use tokio::sync::{mpsc, RwLock};
use tracing::{debug, info};

pub async fn start_le_monitor(
    dev_state: Arc<RwLock<Option<AirPodsDeviceState>>>,
    ui_tx: mpsc::UnboundedSender<UiEvent>,
) -> Result<(), String> {
    let session = Session::new()
        .await
        .map_err(|e| format!("Failed to create BlueZ session for LE monitor: {}", e))?;
    let adapter = session
        .default_adapter()
        .await
        .map_err(|e| format!("Failed to get default adapter for LE monitor: {}", e))?;

    let mut stream = adapter
        .discover_devices()
        .await
        .map_err(|e| format!("Failed to start BLE discovery: {}", e))?;

    info!("BLE Proximity Monitor (discovery mode) started and active");

    // Devices already given an event task, so a device that disappears and
    // reappears does not pile up a second task for the same address.
    let mut spawned: std::collections::HashSet<_> = std::collections::HashSet::new();

    while let Some(evt) = stream.next().await {
        if let bluer::AdapterEvent::DeviceAdded(addr) = evt {
            let Ok(dev) = adapter.device(addr) else {
                continue;
            };
            let mfg = dev
                .manufacturer_data()
                .await
                .ok()
                .flatten()
                .unwrap_or_default();
            if let Some(apple_data) = mfg.get(&76) {
                parse_proximity_message(apple_data, addr, &dev_state, &ui_tx).await;
            }

            // Every device in range would otherwise park one event task for
            // the lifetime of the process - unbounded growth in a busy RF
            // environment. Only Apple devices are interesting here, judged by
            // the current advertisement or the advertised alias; properties
            // that show up later arrive through the initial parse above on a
            // fresh DeviceAdded.
            let looks_like_airpods = mfg.contains_key(&76)
                || dev
                    .alias()
                    .await
                    .map(|a| a.to_lowercase().contains("airpods"))
                    .unwrap_or(false);
            if !looks_like_airpods || !spawned.insert(addr) {
                continue;
            }

            let dev_state_clone = dev_state.clone();
            let ui_tx_clone = ui_tx.clone();
            tokio::spawn(async move {
                if let Ok(mut events) = dev.events().await {
                    while let Some(ev) = events.next().await {
                        if let bluer::DeviceEvent::PropertyChanged(bluer::DeviceProperty::ManufacturerData(mfg)) = ev {
                            if let Some(apple_data) = mfg.get(&76) {
                                parse_proximity_message(apple_data, addr, &dev_state_clone, &ui_tx_clone).await;
                            }
                        }
                    }
                }
            });
        }
    }

    Ok(())
}

async fn parse_proximity_message(
    data: &[u8],
    device_addr: bluer::Address,
    dev_state: &Arc<RwLock<Option<AirPodsDeviceState>>>,
    ui_tx: &mpsc::UnboundedSender<UiEvent>,
) {
    // Proximity pairing message starts with 0x07 and is at least 11 bytes long
    if data.len() < 11 || data[0] != 0x07 {
        return;
    }

    let status = data[5] as usize;
    let pods_battery_byte = data[6];
    let flags_and_case = data[7];

    let primary_left = (status & 0x20) != 0;
    let are_values_flipped = !primary_left;
    let is_this_pod_in_case = (status & 0x40) != 0;

    let parse_nibble_battery = |nibble: u8| -> Option<u8> {
        match nibble {
            0x0..=0x9 => Some(nibble * 10),
            0xA..=0xE => Some(100),
            _ => None,
        }
    };

    let left_nibble = if are_values_flipped {
        (pods_battery_byte >> 4) & 0x0F
    } else {
        pods_battery_byte & 0x0F
    };
    let right_nibble = if are_values_flipped {
        pods_battery_byte & 0x0F
    } else {
        (pods_battery_byte >> 4) & 0x0F
    };
    let case_nibble = flags_and_case & 0x0F;

    let flags = (flags_and_case >> 4) & 0x0F;
    let right_charging = if are_values_flipped {
        (flags & 0x01) != 0
    } else {
        (flags & 0x02) != 0
    };
    let left_charging = if are_values_flipped {
        (flags & 0x02) != 0
    } else {
        (flags & 0x01) != 0
    };
    let case_charging = (flags & 0x04) != 0;

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
        // Only our own device: every AirPods case within range broadcasts the
        // same proximity message, and without this a neighbour's case being
        // opened rewrote the local battery and in-ear state.
        if state.mac_address != device_addr.to_string() {
            debug!(
                "Ignoring proximity message from {} (tracked device is {})",
                device_addr, state.mac_address
            );
            return;
        }
        if let Some(lb) = parse_nibble_battery(left_nibble) {
            state.battery_left = Some(lb);
            state.battery_left_charging = left_charging;
        }
        if let Some(rb) = parse_nibble_battery(right_nibble) {
            state.battery_right = Some(rb);
            state.battery_right_charging = right_charging;
        }
        if let Some(cb) = parse_nibble_battery(case_nibble) {
            state.battery_case = Some(cb);
            state.battery_case_charging = case_charging;
        }

        if left_in_ear {
            state.left_in_ear = EarStatus::InEar;
        } else if is_this_pod_in_case && primary_left {
            state.left_in_ear = EarStatus::InCase;
        } else {
            state.left_in_ear = EarStatus::OutOfEar;
        }

        if right_in_ear {
            state.right_in_ear = EarStatus::InEar;
        } else if is_this_pod_in_case && !primary_left {
            state.right_in_ear = EarStatus::InCase;
        } else {
            state.right_in_ear = EarStatus::OutOfEar;
        }

        debug!(
            "BLE Proximity Battery updated: L={:?} R={:?} Case={:?}",
            state.battery_left, state.battery_right, state.battery_case
        );

        let _ = ui_tx.send(UiEvent::DeviceStateUpdated(state.clone()));
    }
}

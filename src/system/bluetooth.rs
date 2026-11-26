use crate::core::models::{AirPodsModel, MacAddress};
#[cfg(target_os = "linux")]
use bluer::{Adapter, Session};
#[cfg(target_os = "linux")]
use tracing::info;
use uuid::Uuid;

pub const APPLE_AACP_UUID: Uuid = Uuid::from_u128(0x74ec2172_0bad_4d01_8f77_997b2be0722a);
pub const APPLE_VENDOR_ID: u32 = 0x004c;

#[derive(Debug, Clone)]
pub struct DiscoveredAirPods {
    pub address: MacAddress,
    pub name: String,
    pub model: AirPodsModel,
    pub model_inferred: bool,
    pub is_connected: bool,
}

#[cfg(target_os = "linux")]
pub struct BluetoothManager {
    session: Session,
}

#[cfg(target_os = "linux")]
impl BluetoothManager {
    pub async fn new() -> Result<Self, String> {
        let session = Session::new()
            .await
            .map_err(|e| format!("Failed to create BlueZ session: {}", e))?;
        Ok(Self { session })
    }

    pub async fn get_default_adapter(&self) -> Result<Adapter, String> {
        self.session
            .default_adapter()
            .await
            .map_err(|e| format!("Failed to get default Bluetooth adapter: {}", e))
    }

    pub async fn find_connected_airpods(&self) -> Result<Vec<DiscoveredAirPods>, String> {
        let adapter = self.get_default_adapter().await?;
        let addrs = adapter
            .device_addresses()
            .await
            .map_err(|e| format!("Failed to get device addresses: {}", e))?;

        let mut list = Vec::new();
        for addr in addrs {
            if let Ok(device) = adapter.device(addr) {
                let is_connected = device.is_connected().await.unwrap_or(false);
                let uuids = device.uuids().await.unwrap_or_default().unwrap_or_default();
                let modalias_opt = device.modalias().await.unwrap_or_default();

                // Vendor 0x004C alone matches every Apple product - a Magic
                // Mouse or Keyboard would be adopted as "AirPods" and the
                // daemon would hammer it with AACP connects. The modalias
                // fallback therefore requires a product id this project
                // actually knows; a future AirPods model still matches through
                // the AACP UUID. Beats ids are unmapped, so they stay out too.
                let modalias_model = modalias_opt
                    .as_ref()
                    .filter(|m| m.vendor == APPLE_VENDOR_ID)
                    .map(|m| AirPodsModel::from_product_id(m.product as u16));
                let is_airpods = uuids.contains(&APPLE_AACP_UUID)
                    || matches!(&modalias_model, Some(m) if !matches!(m, AirPodsModel::Unknown(_)));

                if is_airpods && is_connected {
                    let name = device
                        .alias()
                        .await
                        .ok()
                        .unwrap_or_else(|| "AirPods".to_string());

                    let model_inferred = modalias_model.is_none();
                    let model = modalias_model.unwrap_or(AirPodsModel::AirPods3);

                    list.push(DiscoveredAirPods {
                        address: addr.into(),
                        name,
                        model,
                        model_inferred,
                        is_connected,
                    });
                }
            }
        }
        Ok(list)
    }

    pub async fn find_paired_airpods(&self) -> Result<Vec<DiscoveredAirPods>, String> {
        let adapter = self.get_default_adapter().await?;
        let addrs = adapter
            .device_addresses()
            .await
            .map_err(|e| format!("Failed to get device addresses: {}", e))?;

        let mut list = Vec::new();
        for addr in addrs {
            if let Ok(device) = adapter.device(addr) {
                let is_connected = device.is_connected().await.unwrap_or(false);
                let uuids = device.uuids().await.unwrap_or_default().unwrap_or_default();
                let modalias_opt = device.modalias().await.unwrap_or_default();

                // Same rule as find_connected_airpods: vendor 0x004C alone
                // would adopt every Apple peripheral as an AirPods.
                let modalias_model = modalias_opt
                    .as_ref()
                    .filter(|m| m.vendor == APPLE_VENDOR_ID)
                    .map(|m| AirPodsModel::from_product_id(m.product as u16));
                let is_airpods = uuids.contains(&APPLE_AACP_UUID)
                    || matches!(&modalias_model, Some(m) if !matches!(m, AirPodsModel::Unknown(_)));

                if is_airpods {
                    let name = device
                        .alias()
                        .await
                        .ok()
                        .unwrap_or_else(|| "AirPods".to_string());

                    let model_inferred = modalias_model.is_none();
                    let model = modalias_model.unwrap_or(AirPodsModel::AirPods3);

                    list.push(DiscoveredAirPods {
                        address: addr.into(),
                        name,
                        model,
                        model_inferred,
                        is_connected,
                    });
                }
            }
        }
        Ok(list)
    }

    /// Brings a Classic-Bluetooth connection up, or verifies the existing one.
    ///
    /// The shell-out to `bluetoothctl connect` this replaces spawned a process
    /// per attempt and lost BlueZ's structured error; the API also skips the
    /// fixed 3 s sleep the shell version needed because `connect()` resolves
    /// when the link is actually up.
    pub async fn ensure_connected(&self, addr: MacAddress) -> Result<(), String> {
        let adapter = self.get_default_adapter().await?;
        let bluer_addr: bluer::Address = addr.into();
        let device = adapter
            .device(bluer_addr)
            .map_err(|e| format!("Device {} not found: {}", addr, e))?;
        if device.is_connected().await.unwrap_or(false) {
            return Ok(());
        }
        device
            .connect()
            .await
            .map_err(|e| format!("Bluetooth connect to {} failed: {}", addr, e))
    }

    pub async fn watch_connection_events<F>(
        &self,
        target_addr: MacAddress,
        mut callback: F,
    ) -> Result<(), String>
    where
        F: FnMut(bool) + Send + 'static,
    {
        let adapter = self.get_default_adapter().await?;
        let bluer_addr: bluer::Address = target_addr.into();
        let device = adapter
            .device(bluer_addr)
            .map_err(|e| format!("Device not found: {}", e))?;

        let mut events = device
            .events()
            .await
            .map_err(|e| format!("Failed to watch device events: {}", e))?;

        tokio::spawn(async move {
            let mut last_connected = device.is_connected().await.unwrap_or(false);
            callback(last_connected);

            use futures::StreamExt;
            while let Some(evt) = events.next().await {
                let bluer::DeviceEvent::PropertyChanged(prop) = evt;
                if let bluer::DeviceProperty::Connected(connected) = prop
                    && connected != last_connected {
                        last_connected = connected;
                        info!("AirPods connection state changed: connected = {}", connected);
                        callback(connected);
                    }
            }
        });

        Ok(())
    }
}

/// Extracts `(MacAddress, product_id)` from a Windows `BTHENUM` PnP device
/// instance ID when it carries Apple's Bluetooth SIG Vendor ID (`0x004C`).
///
/// Example instance ID:
/// `BTHENUM\{00001200-0000-1000-8000-00805f9b34fb}_VID&0002004c_PID&2024\7&1a2b3c&0&AABBCCDDEEFF_C00000000`
pub fn parse_bthenum_apple_pid(instance_id: &str) -> Option<(MacAddress, u16)> {
    let upper = instance_id.to_ascii_uppercase();
    let pid_hex = ["VID&0002004C_PID&", "VID&004C_PID&"]
        .iter()
        .find_map(|pat| {
            let pos = upper.find(pat)?;
            upper.get(pos + pat.len()..pos + pat.len() + 4)
        })?;
    let pid = u16::from_str_radix(pid_hex, 16).ok()?;

    let last_seg = upper.rsplit('\\').next()?;
    let mac_part = last_seg.rsplit('&').next()?.split('_').next()?;
    if mac_part.len() != 12 || !mac_part.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let mut bytes = [0u8; 6];
    for (i, slot) in bytes.iter_mut().enumerate() {
        *slot = u8::from_str_radix(&mac_part[i * 2..i * 2 + 2], 16).ok()?;
    }
    Some((MacAddress(bytes), pid))
}

#[cfg(target_os = "windows")]
fn query_bthenum_apple_pids() -> std::collections::HashMap<MacAddress, u16> {
    use std::collections::HashMap;
    use windows::core::HSTRING;
    use windows::Devices::Enumeration::{DeviceInformation, DeviceInformationKind};
    use windows::Foundation::Collections::IIterable;

    let mut map = HashMap::new();
    let filter = HSTRING::from("System.Devices.EnumeratorName:=\"BTHENUM\"");
    let Ok(op) = DeviceInformation::FindAllAsyncWithKindAqsFilterAndAdditionalProperties(
        &filter,
        Option::<&IIterable<HSTRING>>::None,
        DeviceInformationKind::Device,
    ) else {
        return map;
    };
    let Ok(items) = op.get() else {
        return map;
    };
    for item in items {
        if let Ok(id) = item.Id()
            && let Some((mac, pid)) = parse_bthenum_apple_pid(&id.to_string_lossy())
        {
            map.entry(mac).or_insert(pid);
        }
    }
    map
}

#[cfg(target_os = "windows")]
pub struct BluetoothManager {}

#[cfg(target_os = "windows")]
impl BluetoothManager {
    pub async fn new() -> Result<Self, String> {
        Ok(Self {})
    }

    pub async fn find_connected_airpods(&self) -> Result<Vec<DiscoveredAirPods>, String> {
        let paired = self.find_paired_airpods().await?;
        Ok(paired.into_iter().filter(|d| d.is_connected).collect())
    }

    pub async fn find_paired_airpods(&self) -> Result<Vec<DiscoveredAirPods>, String> {
        tokio::task::spawn_blocking(|| {
            use windows::Devices::Bluetooth::{BluetoothConnectionStatus, BluetoothDevice};
            use windows::Devices::Enumeration::{DeviceClass, DeviceInformation};

            let selector = BluetoothDevice::GetDeviceSelectorFromPairingState(true)
                .map_err(|e| format!("Failed to get device selector: {}", e))?;

            let devices = DeviceInformation::FindAllAsyncAqsFilter(&selector)
                .map_err(|e| format!("Failed to query paired Bluetooth devices: {}", e))?
                .get()
                .map_err(|e| format!("Failed to get device query results: {}", e))?;

            let active_audio_names: Vec<String> = if let Ok(op) = DeviceInformation::FindAllAsyncDeviceClass(DeviceClass::AudioRender) {
                if let Ok(audio_devs) = op.get() {
                    audio_devs.into_iter()
                        .filter(|d| d.IsEnabled().unwrap_or(false))
                        .filter_map(|d| d.Name().ok().map(|h| h.to_string().to_lowercase()))
                        .collect()
                } else {
                    Vec::new()
                }
            } else {
                Vec::new()
            };

            let pnp_pids = query_bthenum_apple_pids();

            let mut list = Vec::new();
            for dev_info in devices {
                let id = match dev_info.Id() {
                    Ok(id) => id,
                    Err(_) => continue,
                };
                let bt_device = match BluetoothDevice::FromIdAsync(&id) {
                    Ok(op) => match op.get() {
                        Ok(d) => d,
                        Err(_) => continue,
                    },
                    Err(_) => continue,
                };

                let name = bt_device
                    .Name()
                    .map(|h| h.to_string())
                    .unwrap_or_else(|_| "Bluetooth Device".to_string());

                let raw_addr = match bt_device.BluetoothAddress() {
                    Ok(a) => a,
                    Err(_) => continue,
                };
                let b = raw_addr.to_be_bytes();
                let address = MacAddress([b[2], b[3], b[4], b[5], b[6], b[7]]);

                let pnp_model = pnp_pids
                    .get(&address)
                    .map(|&pid| AirPodsModel::from_product_id(pid))
                    .filter(|m| !matches!(m, AirPodsModel::Unknown(_)));

                let (model, model_inferred) = match pnp_model {
                    Some(m) => (m, false),
                    None => match AirPodsModel::infer_from_name(&name) {
                        Some(m) => (m, true),
                        None => continue,
                    },
                };

                let lower_name = name.to_lowercase();
                let audio_connected = active_audio_names
                    .iter()
                    .any(|an| an.contains(&lower_name) || lower_name.contains(an));
                let bt_connected = bt_device
                    .ConnectionStatus()
                    .map(|s| s == BluetoothConnectionStatus::Connected)
                    .unwrap_or(false);

                let is_connected = audio_connected || bt_connected;

                list.push(DiscoveredAirPods {
                    address,
                    name,
                    model,
                    model_inferred,
                    is_connected,
                });
            }
            Ok(list)
        })
        .await
        .map_err(|e| format!("Task join error: {}", e))?
    }

    pub async fn ensure_connected(&self, addr: MacAddress) -> Result<(), String> {
        tokio::task::spawn_blocking(move || {
            use windows::core::GUID;
            use windows::Win32::Devices::Bluetooth::{
                BluetoothFindFirstRadio, BluetoothFindRadioClose, BluetoothSetServiceState,
                BLUETOOTH_DEVICE_INFO, BLUETOOTH_FIND_RADIO_PARAMS, BLUETOOTH_SERVICE_ENABLE,
            };
            use windows::Win32::Foundation::{CloseHandle, HANDLE};

            const AUDIO_SINK_UUID: GUID = GUID::from_u128(0x0000110b_0000_1000_8000_00805f9b34fb);
            const HANDSFREE_UUID: GUID = GUID::from_u128(0x0000111e_0000_1000_8000_00805f9b34fb);

            let b = addr.0;
            let raw_addr = u64::from_be_bytes([0, 0, b[0], b[1], b[2], b[3], b[4], b[5]]);

            let params = BLUETOOTH_FIND_RADIO_PARAMS {
                dwSize: std::mem::size_of::<BLUETOOTH_FIND_RADIO_PARAMS>() as u32,
            };
            let mut radio = HANDLE::default();
            let find = unsafe { BluetoothFindFirstRadio(&params, &mut radio) }
                .map_err(|e| format!("No Bluetooth radio found: {}", e))?;

            let mut dev_info = BLUETOOTH_DEVICE_INFO {
                dwSize: std::mem::size_of::<BLUETOOTH_DEVICE_INFO>() as u32,
                ..Default::default()
            };
            dev_info.Address.Anonymous.ullLong = raw_addr;

            let a2dp_rc = unsafe {
                BluetoothSetServiceState(
                    radio,
                    &dev_info,
                    &AUDIO_SINK_UUID,
                    BLUETOOTH_SERVICE_ENABLE,
                )
            };
            let hfp_rc = unsafe {
                BluetoothSetServiceState(
                    radio,
                    &dev_info,
                    &HANDSFREE_UUID,
                    BLUETOOTH_SERVICE_ENABLE,
                )
            };

            unsafe {
                let _ = CloseHandle(radio);
                let _ = BluetoothFindRadioClose(find);
            }

            if a2dp_rc == 0 || hfp_rc == 0 {
                Ok(())
            } else {
                Err(format!(
                    "BluetoothSetServiceState failed (A2DP={}, HFP={})",
                    a2dp_rc, hfp_rc
                ))
            }
        })
        .await
        .map_err(|e| format!("Task join error: {}", e))?
    }

    pub async fn watch_connection_events<F>(
        &self,
        target_addr: MacAddress,
        mut callback: F,
    ) -> Result<(), String>
    where
        F: FnMut(bool) + Send + 'static,
    {
        tokio::task::spawn_blocking(move || {
            use windows::Devices::Bluetooth::{BluetoothConnectionStatus, BluetoothDevice};
            use windows::Devices::Enumeration::{DeviceClass, DeviceInformation};
            let b = target_addr.0;
            let raw_addr = u64::from_be_bytes([0, 0, b[0], b[1], b[2], b[3], b[4], b[5]]);

            let bt_device = match BluetoothDevice::FromBluetoothAddressAsync(raw_addr) {
                Ok(op) => match op.get() {
                    Ok(d) => d,
                    Err(e) => {
                        tracing::warn!("Failed to get BluetoothDevice for {}: {}", target_addr, e);
                        return;
                    }
                },
                Err(e) => {
                    tracing::warn!("FromBluetoothAddressAsync error for {}: {}", target_addr, e);
                    return;
                }
            };

            let target_name = bt_device.Name().map(|h| h.to_string().to_lowercase()).unwrap_or_default();

            let check_connected = |bt_dev: &BluetoothDevice, name: &str| -> bool {
                let bt_conn = bt_dev
                    .ConnectionStatus()
                    .map(|s| s == BluetoothConnectionStatus::Connected)
                    .unwrap_or(false);
                if bt_conn {
                    return true;
                }
                if let Ok(op) = DeviceInformation::FindAllAsyncDeviceClass(DeviceClass::AudioRender)
                    && let Ok(audio_devs) = op.get()
                {
                    return audio_devs.into_iter()
                        .filter(|d| d.IsEnabled().unwrap_or(false))
                        .filter_map(|d| d.Name().ok().map(|h| h.to_string().to_lowercase()))
                        .any(|an| !name.is_empty() && (an.contains(name) || name.contains(&an)));
                }
                false
            };

            let mut last_connected = check_connected(&bt_device, &target_name);

            callback(last_connected);

            loop {
                std::thread::sleep(std::time::Duration::from_secs(2));
                let current = check_connected(&bt_device, &target_name);

                if current != last_connected {
                    last_connected = current;
                    tracing::info!(
                        "AirPods connection state changed (Windows): connected = {}",
                        current
                    );
                    callback(current);
                }
            }
        });

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_bthenum_apple_vid_and_pid_with_mac() {
        let id = r"BTHENUM\{00001200-0000-1000-8000-00805f9b34fb}_VID&0002004c_PID&2024\7&1a2b3c4d&0&AABBCCDDEEFF_C00000000";
        let parsed = parse_bthenum_apple_pid(id);
        assert_eq!(
            parsed,
            Some((MacAddress([0xAA, 0xBB, 0xCC, 0xDD, 0xEE, 0xFF]), 0x2024))
        );

        let short_vid = r"BTHENUM\{0000110b-0000-1000-8000-00805f9b34fb}_VID&004C_PID&2013\8&deadbeef&0&112233445566";
        assert_eq!(
            parse_bthenum_apple_pid(short_vid),
            Some((MacAddress([0x11, 0x22, 0x33, 0x44, 0x55, 0x66]), 0x2013))
        );

        let non_apple = r"BTHENUM\{0000110b-0000-1000-8000-00805f9b34fb}_VID&0002054c_PID&0ce6\7&1&0&112233445566_C00000000";
        assert_eq!(parse_bthenum_apple_pid(non_apple), None);
    }
}

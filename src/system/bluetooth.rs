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

                    let model = modalias_model.unwrap_or(AirPodsModel::AirPods3);

                    list.push(DiscoveredAirPods {
                        address: addr.into(),
                        name,
                        model,
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

                    let model = modalias_model.unwrap_or(AirPodsModel::AirPods3);

                    list.push(DiscoveredAirPods {
                        address: addr.into(),
                        name,
                        model,
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

                if let Some(model) = AirPodsModel::infer_from_name(&name) {
                    let raw_addr = match bt_device.BluetoothAddress() {
                        Ok(a) => a,
                        Err(_) => continue,
                    };
                    let b = raw_addr.to_be_bytes();
                    let address = MacAddress([b[2], b[3], b[4], b[5], b[6], b[7]]);
                    let lower_name = name.to_lowercase();
                    let audio_connected = active_audio_names.iter().any(|an| an.contains(&lower_name) || lower_name.contains(an));
                    let bt_connected = bt_device
                        .ConnectionStatus()
                        .map(|s| s == BluetoothConnectionStatus::Connected)
                        .unwrap_or(false);

                    let is_connected = audio_connected || bt_connected;

                    list.push(DiscoveredAirPods {
                        address,
                        name,
                        model,
                        is_connected,
                    });
                }
            }
            Ok(list)
        })
        .await
        .map_err(|e| format!("Task join error: {}", e))?
    }

    pub async fn ensure_connected(&self, _addr: MacAddress) -> Result<(), String> {
        Ok(())
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
                if let Ok(op) = DeviceInformation::FindAllAsyncDeviceClass(DeviceClass::AudioRender) {
                    if let Ok(audio_devs) = op.get() {
                        return audio_devs.into_iter()
                            .filter(|d| d.IsEnabled().unwrap_or(false))
                            .filter_map(|d| d.Name().ok().map(|h| h.to_string().to_lowercase()))
                            .any(|an| !name.is_empty() && (an.contains(name) || name.contains(&an)));
                    }
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

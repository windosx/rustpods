use crate::core::models::AirPodsModel;
use bluer::{Adapter, Address, Session};
use tracing::info;
use uuid::Uuid;

pub const APPLE_AACP_UUID: Uuid = Uuid::from_u128(0x74ec2172_0bad_4d01_8f77_997b2be0722a);
pub const APPLE_VENDOR_ID: u32 = 0x004c;

#[derive(Debug, Clone)]
pub struct DiscoveredAirPods {
    pub address: Address,
    pub name: String,
    pub model: AirPodsModel,
    pub is_connected: bool,
}

pub struct BluetoothManager {
    session: Session,
}

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
                        address: addr,
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
                        address: addr,
                        name,
                        model,
                        is_connected,
                    });
                }
            }
        }
        Ok(list)
    }

    pub async fn watch_connection_events<F>(
        &self,
        target_addr: Address,
        mut callback: F,
    ) -> Result<(), String>
    where
        F: FnMut(bool) + Send + 'static,
    {
        let adapter = self.get_default_adapter().await?;
        let device = adapter
            .device(target_addr)
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
                if let bluer::DeviceProperty::Connected(connected) = prop {
                    if connected != last_connected {
                        last_connected = connected;
                        info!("AirPods connection state changed: connected = {}", connected);
                        callback(connected);
                    }
                }
            }
        });

        Ok(())
    }
}

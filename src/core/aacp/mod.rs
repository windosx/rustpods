pub mod commands;
pub mod events;
pub mod packets;
pub mod remotehid;
pub mod rtbuddy;
pub mod session;

use bluer::l2cap::{Socket, SocketAddr};
use bluer::{Address, AddressType};
use commands::{ControlCommandId, MicrophoneMode, NoiseControlMode};
use events::AacpEvent;
use packets::*;
use crate::core::aacp::events::EarStatus;
use crate::core::head_tracking::{HeadGesture, HeadPose, HeadTracker};
use crate::core::spatial::{SpatialClaim, SpatialMode, SpatialSession};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{broadcast, mpsc, watch, Mutex, RwLock};
use tokio::time::sleep;
use tracing::{debug, error, info, warn};

const CONNECT_TIMEOUT: Duration = Duration::from_secs(8);
const POLL_INTERVAL: Duration = Duration::from_millis(150);

#[derive(Debug, Clone)]
pub struct AacpState {
    pub is_connected: bool,
    pub airpods_address: Option<Address>,
    pub local_address: Option<String>,
}

#[derive(Clone)]
pub struct AacpManager {
    state: Arc<RwLock<AacpState>>,
    send_tx: Arc<Mutex<Option<mpsc::Sender<Vec<u8>>>>>,
    event_tx: broadcast::Sender<AacpEvent>,
    /// Fed directly from the receive loop at wire rate. A `broadcast` channel
    /// would drop samples here, so the tracker lives next to the socket and
    /// readers pull the latest pose instead.
    head_tracker: Arc<Mutex<HeadTracker>>,
    head_feed: watch::Sender<Option<HeadPose>>,
    gesture_tx: broadcast::Sender<HeadGesture>,
    /// Raw short frames on the RTBuddy descriptor (descriptor replies, ACKs).
    /// Exposed so the handshake can wait for the 9-byte enable ACK.
    rtb_acp_tx: broadcast::Sender<Vec<u8>>,
    /// Identifiers from the `neorelay` advertisement, which the HID enable
    /// needs. Set by the receive loop as soon as that advertisement arrives.
    neorelay_ids: Arc<Mutex<Option<(u16, u32)>>>,
    /// Latest ear-detection status, cached so callers can tell "buds in case"
    /// from "buds stuck" without waiting for the next notification.
    ear_status: Arc<Mutex<Option<(EarStatus, EarStatus)>>>,
    /// Services the accessory announced on this connection, kept up to date by
    /// the receive loop.
    ///
    /// The announcement burst arrives during the handshake, well before
    /// `start_head_tracking` is called, and a broadcast channel only delivers
    /// messages sent after subscribing. Caching here is what makes the burst
    /// visible to whoever starts tracking later - without it every round reports
    /// zero services even though eight were announced.
    remotes: Arc<Mutex<remotehid::DeviceRegistry>>,
    /// Human-readable progress of the head-tracking negotiation, for the UI.
    ///
    /// The spatial-audio page had a hardcoded "waiting for data" string, so a
    /// failed negotiation looked identical to one still in progress. This carries
    /// what actually happened: how many services were announced, whether the
    /// interval write went out, and how many samples arrived.
    head_status: Arc<Mutex<String>>,
    /// Which application currently holds the spatial-audio claim.
    ///
    /// macOS only sends the enable once an app has claimed spatial audio in
    /// head-tracked mode, so the enable path is gated on this the same way.
    spatial: Arc<Mutex<SpatialSession>>,
}


/// `SOL_BLUETOOTH` / `BT_SECURITY` socket options, from the kernel headers.
const SOL_BLUETOOTH: libc::c_int = 274;
const BT_SECURITY: libc::c_int = 4;
/// `BT_SECURITY_MEDIUM`: request an encrypted (or higher) link.
const BT_SECURITY_MEDIUM: libc::c_int = 2;

/// Raises the requested link security on a Bluetooth L2CAP socket.
///
/// The kernel applies this as a requirement when the ACL link comes up, so it
/// must be set before `connect`. Failure is non-fatal: some BlueZ/controller
/// combinations already have the link encrypted, and the socket still works.
/// `SOL_L2CAP` / `L2CAP_OPTIONS`, from the kernel's `bluetooth/l2cap.h`.
///
/// These are **not** `SOL_BLUETOOTH`: the MTU lives in the L2CAP-level option
/// block, and setting it through the wrong level silently fails with
/// `ENOPROTOOPT`, which is why an earlier attempt at this appeared not to work.
const SOL_L2CAP: libc::c_int = 6;
const L2CAP_OPTIONS: libc::c_int = 1;

/// Receive MTU the accessory needs before it will announce the motion sensor.
///
/// With the kernel default (672) the `devmotion6` descriptor is too large to fit
/// and the accessory simply omits it from the DeviceConnect burst — it announces
/// three services instead of eight. At 2582 all eight appear. Established by
/// `starman-tech/mousepods` (`captures/airpods3_announces_mtu*.hex`) and
/// reproduced here.
const AACP_RECEIVE_MTU: u16 = 2582;

/// `struct l2cap_options` as the kernel lays it out: omtu, imtu, flush_to, mode,
/// then a few single-byte fields. Only omtu/imtu are read here.
const L2CAP_OPTIONS_LEN: usize = 12;

/// Raises the L2CAP receive MTU so the accessory will announce every sensor.
///
/// Must run **before** `connect()`: the MTU is advertised during L2CAP
/// configuration, so setting it afterwards has no effect on the negotiation.
/// The existing option block is read first and only the imtu field rewritten,
/// because the other fields carry the kernel's current settings.
fn raise_receive_mtu<Type>(socket: &Socket<Type>) {
    use std::os::unix::io::AsRawFd;

    let fd = socket.as_raw_fd();
    let mut opts = [0u8; L2CAP_OPTIONS_LEN];

    // SAFETY: `opts` is exactly the 12-byte l2cap_options block the kernel
    // expects for L2CAP_OPTIONS, and the fd outlives this call.
    let rc = unsafe {
        libc::getsockopt(
            fd,
            SOL_L2CAP,
            L2CAP_OPTIONS,
            opts.as_mut_ptr().cast(),
            &mut (L2CAP_OPTIONS_LEN as libc::socklen_t),
        )
    };
    if rc != 0 {
        let err = std::io::Error::last_os_error();
        warn!("Could not read L2CAP options; receive MTU left at default: {}", err);
        return;
    }

    // imtu is at byte offset 2 (omtu occupies 0..2), little-endian.
    opts[2..4].copy_from_slice(&AACP_RECEIVE_MTU.to_le_bytes());

    // SAFETY: same buffer and length as above.
    let rc = unsafe {
        libc::setsockopt(
            fd,
            SOL_L2CAP,
            L2CAP_OPTIONS,
            opts.as_ptr().cast(),
            L2CAP_OPTIONS_LEN as libc::socklen_t,
        )
    };
    if rc == 0 {
        debug!("Set AACP receive MTU to {}", AACP_RECEIVE_MTU);
    } else {
        let err = std::io::Error::last_os_error();
        warn!(
            "Could not raise the receive MTU ({}); the accessory will not announce \
             its motion sensor and head tracking cannot start",
            err
        );
    }
}

fn raise_link_security<Type>(socket: &Socket<Type>) {
    use std::os::unix::io::AsRawFd;

    let level: [u8; 2] = [BT_SECURITY_MEDIUM as u8, 0];
    // SAFETY: the fd is owned by `socket` and outlives this call; the buffer is
    // exactly the two bytes the kernel expects for `BT_SECURITY`.
    let rc = unsafe {
        libc::setsockopt(
            socket.as_raw_fd(),
            SOL_BLUETOOTH,
            BT_SECURITY,
            level.as_ptr().cast(),
            std::mem::size_of_val(&level) as libc::socklen_t,
        )
    };
    if rc == 0 {
        debug!("Requested BT_SECURITY_MEDIUM on the AACP socket");
    } else {
        let err = std::io::Error::last_os_error();
        debug!("Could not raise link security (continuing): {}", err);
    }
}

impl AacpManager {
    pub fn new() -> Self {
        let (event_tx, _) = broadcast::channel(256);
        let (head_feed, _) = watch::channel(None);
        let (gesture_tx, _) = broadcast::channel(16);
        let (rtb_acp_tx, _) = broadcast::channel(64);
        Self {
            state: Arc::new(RwLock::new(AacpState {
                is_connected: false,
                airpods_address: None,
                local_address: None,
            })),
            send_tx: Arc::new(Mutex::new(None)),
            event_tx,
            head_tracker: Arc::new(Mutex::new(HeadTracker::new())),
            head_feed,
            gesture_tx,
            rtb_acp_tx,
            neorelay_ids: Arc::new(Mutex::new(None)),
            ear_status: Arc::new(Mutex::new(None)),
            head_status: Arc::new(Mutex::new("head tracking not started".to_string())),
            remotes: Arc::new(Mutex::new(remotehid::DeviceRegistry::new())),
            spatial: Arc::new(Mutex::new(SpatialSession::new())),
        }
    }

    /// Latest head pose, or `None` while no stream is running.
    pub fn head_pose(&self) -> watch::Receiver<Option<HeadPose>> {
        self.head_feed.subscribe()
    }

    /// True once at least one sample has been received and calibrated.
    pub async fn is_head_tracking_active(&self) -> bool {
        let tracker = self.head_tracker.lock().await;
        tracker.active && tracker.is_calibrated()
    }

    /// Measured sample rate of the attitude stream, if it is running.
    pub async fn head_tracking_rate_hz(&self) -> Option<f32> {
        self.head_tracker.lock().await.sample_rate_hz()
    }

    /// How many attitude samples have been parsed since the stream started.
    pub async fn head_samples_seen(&self) -> u64 {
        self.head_tracker.lock().await.samples_seen()
    }

    /// Current head-tracking progress, for display.
    pub async fn head_tracking_status(&self) -> String {
        self.head_status.lock().await.clone()
    }

    /// Updates the progress string the UI shows.
    pub async fn set_head_tracking_status(&self, text: impl Into<String>) {
        *self.head_status.lock().await = text.into();
    }

    /// Last ear status the accessory reported, if any.
    pub async fn last_ear_status(&self) -> Option<(EarStatus, EarStatus)> {
        *self.ear_status.lock().await
    }

    /// Records an ear-detection observation for later queries.
    pub async fn note_ear_status(&self, left: EarStatus, right: EarStatus) {
        *self.ear_status.lock().await = Some((left, right));
    }

    /// Subscribe to detected nod/shake gestures.
    pub fn subscribe_gestures(&self) -> broadcast::Receiver<HeadGesture> {
        self.gesture_tx.subscribe()
    }

    /// Every short frame received on the RTBuddy descriptor (descriptor
    /// replies, HID ACKs). Used to observe the sensor service handshake.
    pub fn subscribe_rtbuddy_frames(&self) -> broadcast::Receiver<Vec<u8>> {
        self.rtb_acp_tx.subscribe()
    }

    pub async fn set_local_address(&self, mac: String) {
        let mut state = self.state.write().await;
        state.local_address = Some(mac);
    }

    pub fn subscribe(&self) -> broadcast::Receiver<AacpEvent> {
        self.event_tx.subscribe()
    }

    pub async fn is_connected(&self) -> bool {
        self.state.read().await.is_connected
    }

    /// Opens the AACP L2CAP channel without running the handshake.
    ///
    /// Used by the head-tracking diagnostic to reproduce the LibrePods
    /// sequence exactly: that implementation sends `INIT` then `START` as the
    /// first bytes on a fresh socket, with no handshake in between.
    pub async fn connect_without_handshake(&self, addr: Address) -> Result<(), String> {
        self.open(addr, false).await
    }

    pub async fn connect(&self, addr: Address) -> Result<(), String> {
        self.open(addr, true).await
    }

    async fn open(&self, addr: Address, handshake: bool) -> Result<(), String> {
        info!("Connecting AACP L2CAP socket to {} on PSM 0x{:04X}...", addr, PSM_AACP);
        let target_sa = SocketAddr::new(addr, AddressType::BrEdr, PSM_AACP);

        let socket = Socket::new_seq_packet()
            .map_err(|e| format!("Failed to create L2CAP seq_packet socket: {}", e))?;

        // BlueZ requires an encrypted link for PSM 0x1001, but a raw L2CAP
        // socket only asks for "low" security. Without this the connect can fail
        // with EACCES, or the accessory silently ignores sensor commands on the
        // resulting link.
        raise_link_security(&socket);
        // The MTU has to be in place before the L2CAP configuration exchange:
        // with the default 672 the accessory omits devmotion6 entirely.
        raise_receive_mtu(&socket);

        let seq_packet = tokio::time::timeout(CONNECT_TIMEOUT, socket.connect(target_sa))
            .await
            .map_err(|_| "L2CAP connection timed out".to_string())?
            .map_err(|e| format!("L2CAP connect error: {}", e))?;

        let seq_packet = Arc::new(seq_packet);

        // Wait until connection is fully established
        let start = tokio::time::Instant::now();
        loop {
            match seq_packet.peer_addr() {
                Ok(peer) if peer.cid != 0 => break,
                Ok(_) => {}
                Err(e) => {
                    return Err(format!("L2CAP peer error during handshake: {}", e));
                }
            }
            if start.elapsed() >= CONNECT_TIMEOUT {
                return Err("Timeout waiting for L2CAP connection CID".to_string());
            }
            sleep(POLL_INTERVAL).await;
        }

        info!("L2CAP AACP connected to {}", addr);

        let (tx, mut rx) = mpsc::channel::<Vec<u8>>(128);
        {
            let mut state = self.state.write().await;
            state.is_connected = true;
            state.airpods_address = Some(addr);
        }
        {
            let mut send_tx_guard = self.send_tx.lock().await;
            *send_tx_guard = Some(tx);
        }

        // Spawn send loop
        let seq_packet_send = Arc::clone(&seq_packet);
        tokio::spawn(async move {
            while let Some(packet) = rx.recv().await {
                if let Err(e) = seq_packet_send.send(&packet).await {
                    error!("Error sending AACP packet: {}", e);
                    break;
                }
            }
            debug!("AACP send loop terminated");
        });

        // Spawn receive loop
        let seq_packet_recv = Arc::clone(&seq_packet);
        let event_tx = self.event_tx.clone();
        let state_clone = Arc::clone(&self.state);
        let send_tx_cleanup = Arc::clone(&self.send_tx);
        let tracker = Arc::clone(&self.head_tracker);
        let head_feed = self.head_feed.clone();
        let gesture_tx = self.gesture_tx.clone();
        let rtb_acp_tx = self.rtb_acp_tx.clone();
        let neorelay_ids = Arc::clone(&self.neorelay_ids);
        let ear_cache = Arc::clone(&self.ear_status);
        let remotes = Arc::clone(&self.remotes);
        let self_head_status = Arc::clone(&self.head_status);

        tokio::spawn(async move {
            let mut buf = vec![0u8; 2048];
            // Log the first sensor frame, then only the occasional one: the
            // stream runs at ~50 Hz and would otherwise bury everything else.
            let mut sensor_frames: u64 = 0;
            loop {
                match seq_packet_recv.recv(&mut buf).await {
                    Ok(len) if len > 0 => {
                        let packet = &buf[..len];
                        if len > 4 && packet[4] == 0x17 {
                            if routes_to_xml(packet) {
                                // The accessory advertises its RTBuddy services as
                                // XML right after the handshake. This is the only
                                // authoritative source for which service ids
                                // exist, so surface it when debugging.
                                if let Some(ad) = rtbuddy::ServiceAdvertisement::parse(packet) {
                                    let name = rtbuddy::ServiceAdvertisement::service_name(packet)
                                        .unwrap_or("<unnamed>");
                                    info!(
                                        "Sensor service '{}': id=0x{:08X} cookie=0x{:08X}",
                                        name, ad.service_id, ad.cookie
                                    );
                                    if name == rtbuddy::SERVICE_NAME_NEORELAY {
                                        *neorelay_ids.lock().await =
                                            Some((ad.service_id, ad.cookie));
                                    }
                                }
                                if tracing::enabled!(tracing::Level::DEBUG) {
                                    // Hex, not text: the frame's header carries
                                    // assigned service ids and cookies as binary
                                    // values that a lossy UTF-8 decode destroys.
                                    let header_len = len.min(24);
                                    debug!(
                                        "RTBuddy XML descriptor ({} bytes), header {:02X?}",
                                        len,
                                        &packet[..header_len]
                                    );
                                } else {
                                    info!("Received RTBuddy XML descriptor ({} bytes)", len);
                                }
                                // XML bodies are both the SensorDataWX service
                                // advertisements *and* the RemoteHID
                                // DeviceConnect plists, so they still have to
                                // reach the tap.
                                rtb_acp_tx.send(packet.to_vec()).ok();
                                // Also record it here: the burst lands during
                                // the handshake, before anything subscribes to
                                // the tap, so a broadcast alone would lose it.
                                record_advertisement(&remotes, packet).await;
                                continue;
                            }
                            // Short frames on this descriptor are descriptor
                            // replies and the enable ACK, not samples.
                            let descriptor = if len >= 10 {
                                u32::from_le_bytes([packet[6], packet[7], packet[8], packet[9]])
                            } else {
                                0
                            };
                            // Short frames on descriptor 0x800 are control traffic
                            // (DeviceConnect plists, SetReport responses) and go
                            // to the tap only. Long ones carry a RemoteHID
                            // InputReport with its 58-byte motion block and must
                            // also reach the tracker, so they fall through.
                            let is_remote_hid = descriptor == rtbuddy::DESCRIPTOR_SENSOR;
                            if is_remote_hid {
                                rtb_acp_tx.send(packet.to_vec()).ok();
                                debug!(
                                    "RemoteHID frame: len={} {:02X?}",
                                    len,
                                    &packet[..len.min(24)]
                                );
                                if len < crate::core::head_tracking::SENSOR_V2_LEN {
                                    continue;
                                }
                            } else if len < crate::core::head_tracking::SENSOR_V2_LEN
                                && !packet
                                    .windows(crate::core::head_tracking::PAYLOAD_MARK.len())
                                    .any(|w| w == crate::core::head_tracking::PAYLOAD_MARK)
                            {
                                rtb_acp_tx.send(packet.to_vec()).ok();
                                debug!(
                                    "RTBuddy frame: descriptor=0x{:08X} len={} {:02X?}",
                                    descriptor,
                                    len,
                                    &packet[..len.min(24)]
                                );
                                continue;
                            }
                            sensor_frames += 1;
                            if sensor_frames == 1 {
                                info!("Head tracking stream started ({} byte samples)", len);
                            } else if sensor_frames % 500 == 0 {
                                debug!("Head tracking: {} samples received", sensor_frames);
                            }
                            // Feed the tracker at wire rate; consumers pull the
                            // latest pose instead of every sample.
                            let mut tracker = tracker.lock().await;
                            tracker.active = true;
                            if let Some((pose, gesture)) = tracker.parse_packet(packet) {
                                let _ = head_feed.send(Some(pose));
                                if let Some(gesture) = gesture {
                                    let _ = gesture_tx.send(gesture);
                                }
                            }
                            continue;
                        }
                        debug!("AACP raw recv {} bytes: {:02X?}", len, &packet[..len.min(16)]);
                        if let Some(event) = parse_aacp_packet(packet) {
                            // Version negotiation must be answered from the
                            // receive path: the accessory sends it once, right
                            // after the handshake, and will not expose the
                            // optional sensor services until it is echoed.
                            if let AacpEvent::VersionNegotiation(ref neg) = event {
                                let ack = neg.acknowledgement();
                                info!(
                                    "AACP version negotiation: echoing {} bytes {:02X?}",
                                    ack.len(),
                                    ack
                                );
                                if let Err(e) = seq_packet_recv.send(&ack).await {
                                    warn!("Failed to echo version negotiation: {}", e);
                                }
                            }
                            if let AacpEvent::EarDetection(ref ear) = event {
                                // Cache it: the recovery logic needs to tell a
                                // unit sitting in its case (sensors off, normal)
                                // from one that is worn but stuck.
                                *ear_cache.lock().await = Some((ear.left, ear.right));
                            }
                            let _ = event_tx.send(event);
                        }
                    }
                    Ok(_) => {
                        warn!("AACP connection closed by remote device");
                        break;
                    }
                    Err(e) => {
                        error!("AACP recv error: {}", e);
                        break;
                    }
                }
            }

            // Cleanup state on disconnect
            {
                let mut state = state_clone.write().await;
                state.is_connected = false;
            }
            {
                let mut send_tx_guard = send_tx_cleanup.lock().await;
                *send_tx_guard = None;
            }
            {
                let mut tracker = tracker.lock().await;
                tracker.active = false;
                tracker.reset();
            }
            // Per-connection caches go with the connection. The accessory
            // reassigns its service ids on every connect (rtbuddy's own docs),
            // so a registry carried across would aim the next session's
            // interval/feature reports at ids that no longer exist - and a
            // stale entry for a *named* service would mask the fresh one.
            *remotes.lock().await = remotehid::DeviceRegistry::new();
            *neorelay_ids.lock().await = None;
            *ear_cache.lock().await = None;
            *self_head_status.lock().await = "head tracking not started".to_string();
            let _ = head_feed.send(None);
            let _ = event_tx.send(AacpEvent::Disconnected);
            warn!("AACP receive loop exited, sent Disconnected event");
        });

        // Initialize handshake sequence
        if handshake {
            self.init_handshake().await?;
        }

        Ok(())
    }

    async fn init_handshake(&self) -> Result<(), String> {
        // AACP 1.3 handshake runs on both service 0 and service 4. Connecting
        // only service 4 (the legacy single handshake) leaves the sensor
        // services unavailable, so the accessory ignores sensor commands.
        for service in [0u16, 4u16] {
            debug!("AACP 1.3 handshake on service {}...", service);
            self.send_raw(make_service_connect(service)).await?;
            sleep(Duration::from_millis(120)).await;
            self.send_raw(make_service_caps_request(service)).await?;
            sleep(Duration::from_millis(120)).await;
        }

        debug!("Sending set feature flags...");
        self.send_raw(make_set_feature_flags_packet()).await?;
        sleep(Duration::from_millis(150)).await;

        // Take ownership of audio connection FIRST so AirPods pushes battery and sensor status
        debug!("Claiming connection ownership...");
        let _ = self
            .send_control_command(ControlCommandId::OwnsConnection, &[0x01])
            .await;
        sleep(Duration::from_millis(150)).await;

        debug!("Sending notification request...");
        self.send_raw(make_notification_request_packet()).await?;
        sleep(Duration::from_millis(150)).await;

        // Request Proximity Keys (IRK + EncKey)
        debug!("Requesting proximity keys...");
        let _ = self.send_raw(make_proximity_keys_request_packet()).await;
        sleep(Duration::from_millis(100)).await;

        // Enable stem press detection (single, double, triple, long press)
        debug!("Enabling stem press notifications...");
        let _ = self
            .send_control_command(ControlCommandId::StemConfig, &[0x0F])
            .await;
        sleep(Duration::from_millis(100)).await;

        // Subscribe to the sensor services. This is what makes the accessory
        // push its sensor descriptors and, subsequently, `SensorV2` samples.
        if let Err(e) = self.subscribe_rtbuddy_motion().await {
            warn!("Could not subscribe to RTBuddy sensor services: {}", e);
        }

        info!("AACP initial handshake complete");
        Ok(())
    }

    pub async fn request_battery_and_status(&self) -> Result<(), String> {
        self.send_raw(make_notification_request_packet()).await?;
        let _ = self
            .send_control_command(ControlCommandId::OwnsConnection, &[0x01])
            .await;
        Ok(())
    }

    pub async fn send_raw(&self, packet: Vec<u8>) -> Result<(), String> {
        let sender = {
            let guard = self.send_tx.lock().await;
            guard.clone()
        };
        if let Some(tx) = sender {
            tx.send(packet)
                .await
                .map_err(|e| format!("Failed to queue AACP packet: {}", e))
        } else {
            Err("AACP socket not connected".to_string())
        }
    }

    pub async fn send_control_command(
        &self,
        cmd: ControlCommandId,
        data: &[u8],
    ) -> Result<(), String> {
        let packet = make_control_command_packet(cmd, data);
        self.send_raw(packet).await
    }

    pub async fn set_noise_mode(&self, mode: NoiseControlMode) -> Result<(), String> {
        info!("Setting noise mode to {:?}", mode);
        self.send_control_command(ControlCommandId::ListeningMode, &[mode.to_u8()])
            .await
    }

    pub async fn set_adaptive_strength(&self, level: u8) -> Result<(), String> {
        info!("Setting adaptive noise strength to {}%", level);
        self.send_control_command(ControlCommandId::AutoAncStrength, &[level.min(100)])
            .await
    }

    pub async fn set_conversational_awareness(&self, enabled: bool) -> Result<(), String> {
        info!("Setting conversational awareness to {}", enabled);
        let val = if enabled { 0x01 } else { 0x02 };
        self.send_control_command(ControlCommandId::ConversationDetectConfig, &[val])
            .await
    }

    pub async fn set_ear_detection(&self, enabled: bool) -> Result<(), String> {
        info!("Setting in-ear detection to {}", enabled);
        let val = if enabled { 0x01 } else { 0x02 };
        self.send_control_command(ControlCommandId::EarDetection, &[val])
            .await
    }

    pub async fn set_microphone_mode(&self, mode: MicrophoneMode) -> Result<(), String> {
        info!("Setting microphone mode to {:?}", mode);
        self.send_control_command(ControlCommandId::MicMode, &[mode as u8])
            .await
    }

    pub async fn set_case_chime(&self, enabled: bool) -> Result<(), String> {
        info!("Setting case tone enabled to {}", enabled);
        let val = if enabled { 0x01 } else { 0x02 };
        self.send_control_command(ControlCommandId::InCaseToneConfig, &[val])
            .await
    }

    pub async fn rename(&self, new_name: &str) -> Result<(), String> {
        info!("Renaming AirPods to '{}'", new_name);
        let packet = make_rename_packet(new_name);
        self.send_raw(packet).await
    }

    /// Subscribes to the RTBuddy sensor services.
    ///
    /// The accessory answers each subscription with an XML descriptor naming the
    /// service and its HID report descriptor, then expects an acknowledgement.
    /// This three-service sweep plus ACK is what makes the accessory start
    /// reporting its sensor capabilities at all.
    pub async fn subscribe_rtbuddy_motion(&self) -> Result<(), String> {
        info!("Requesting all RTBuddy descriptors...");

        // This is the step that actually makes the accessory announce its
        // sensors. The per-service subscriptions below are kept because they set
        // the reporting interval, but on their own they produce no announcement
        // burst at all - confirmed against the reference client and by the
        // eight-service burst appearing right after these two frames.
        self.send_raw(rtbuddy::make_request_all_descriptors(0x42, false))
            .await?;
        sleep(Duration::from_millis(60)).await;
        self.send_raw(rtbuddy::make_request_all_descriptors(0x43, true))
            .await?;
        sleep(Duration::from_millis(120)).await;

        for (service, seq) in [
            (rtbuddy::SERVICE_NEORELAY, 100u64),
            (rtbuddy::SERVICE_IED, 101),
            (rtbuddy::SERVICE_MANDO, 102),
        ] {
            self.send_raw(rtbuddy::make_service_subscription(
                service,
                rtbuddy::INTERVAL_50HZ_US,
                seq,
            ))
            .await?;
            sleep(Duration::from_millis(80)).await;
        }

        // Acknowledge the service the accessory reported first.
        self.send_raw(rtbuddy::make_service_ack(rtbuddy::SERVICE_NEORELAY, 103))
            .await?;
        sleep(Duration::from_millis(80)).await;

        Ok(())
    }

    /// Declares that an application wants spatial audio.
    ///
    /// This is the Linux equivalent of the macOS step that always precedes the
    /// enable:
    ///
    /// ```text
    /// AudioLinkManager setSpatialModeWithBundleID: ..., mode:3, headtrack:1
    /// ```
    ///
    /// The claim is recorded even when it does not request head tracking, so
    /// `spatial_snapshot` can report why streaming is idle. Returns whether the
    /// accessory should now be streaming.
    pub async fn claim_spatial_audio(
        &self,
        app_id: &str,
        mode: SpatialMode,
        head_tracking: bool,
    ) -> bool {
        let claim = SpatialClaim::new(app_id, mode, head_tracking);
        let wants = {
            let mut session = self.spatial.lock().await;
            session.claim(claim);
            session.wants_head_tracking()
        };
        info!(
            "Spatial audio claimed by '{}' (mode={:?}, head tracking={}) -> streaming {}",
            app_id,
            mode,
            head_tracking,
            if wants { "requested" } else { "not requested" }
        );
        wants
    }

    /// Releases an application's spatial-audio claim.
    ///
    /// Returns `true` when the release should stop the stream.
    pub async fn release_spatial_audio(&self, app_id: &str) -> bool {
        let was = {
            let mut session = self.spatial.lock().await;
            session.release(app_id)
        };
        if was {
            info!("Spatial claim released by '{}'; stopping tracking", app_id);
            let _ = self.stop_head_tracking().await;
        }
        was
    }

    /// Whether a spatial-audio claim currently permits head tracking.
    pub async fn wants_head_tracking(&self) -> bool {
        self.spatial.lock().await.wants_head_tracking()
    }

    /// Free-text description of the current claim, for the UI and logs.
    pub async fn spatial_snapshot(&self) -> Option<(String, SpatialMode, bool)> {
        self.spatial
            .lock()
            .await
            .current()
            .map(|c| (c.app_id.clone(), c.mode, c.head_tracking))
    }

    /// Starts the head-tracking stream and keeps it alive.
    ///
    /// AirPods 3 does not implement `SensorDataWX`; it serves motion through
    /// RemoteHID on descriptor `0x800`. Two preconditions decide whether the
    /// accessory will hand samples over at all:
    ///
    /// 1. **Receive MTU 2582.** Set before `connect` in this module. At the
    ///    kernel default of 672 the accessory omits `devmotion6` from its
    ///    announcement burst and there is nothing to start. Reaching 2582 needs
    ///    a reconnect, because the MTU is fixed during L2CAP configuration.
    /// 2. **A head-tracked spatial claim.** macOS refuses to enable tracking
    ///    without one (`mode: 3, headtrack: 1`), so the claim is checked before
    ///    anything is written.
    ///
    /// The stream is started by writing Feature report 1 on `devmotion6` with the
    /// sample interval. Returns once the supervisor has been spawned; progress
    /// shows up as samples in the head tracker.
    pub async fn start_head_tracking(&self) -> Result<(), String> {
        if !self.is_connected().await {
            return Err("AACP socket not connected".to_string());
        }

        let sender = {
            let guard = self.send_tx.lock().await;
            guard.clone()
        };
        let Some(sender) = sender else {
            return Err("AACP socket not connected".to_string());
        };

        let tracker = Arc::clone(&self.head_tracker);
        let spatial = Arc::clone(&self.spatial);
        let frames = self.rtb_acp_tx.subscribe();
        let status = Arc::clone(&self.head_status);
        let remotes = Arc::clone(&self.remotes);
        let address = self.state.read().await.airpods_address;

        tokio::spawn(async move {
            if tracker.lock().await.samples_seen() > 0 {
                debug!("Head tracking already streaming");
                return;
            }
            {
                let session = spatial.lock().await;
                if !session.wants_head_tracking() {
                    info!("No spatial claim; not starting head tracking");
                    return;
                }
            }

            if let Err(e) = drive_remote_hid(sender, frames, tracker, address, status, remotes).await {
                warn!("RemoteHID could not be started: {}", e);
            }
        });

        Ok(())
    }

    pub async fn stop_head_tracking(&self) -> Result<(), String> {
        debug!("Stopping head tracking...");
        // Writing a zero interval is the documented way to stop a RemoteHID
        // stream; the SensorDataWX stop frame only applies to that transport.
        let stop = remotehid::make_set_interval(
            remotehid::Generation::default().next(),
            7, // devmotion6; corrected below if the real id differs
            0,
        );
        let _ = self.send_raw(stop).await;
        Ok(())
    }
}

/// Runs the RemoteHID negotiation and arms the motion stream.
///
/// Separated from [`AacpManager::start_head_tracking`] so the flow can be read in
/// one place: collect announcements, pick the motion service by name, write the
/// interval. All the waiting and retry policy lives in
/// [`session::StreamSupervisor`].
async fn drive_remote_hid(
    sender: mpsc::Sender<Vec<u8>>,
    mut frames: broadcast::Receiver<Vec<u8>>,
    _tracker: Arc<Mutex<HeadTracker>>,
    address: Option<Address>,
    status: Arc<Mutex<String>>,
    remotes: Arc<Mutex<remotehid::DeviceRegistry>>,
) -> Result<(), String> {
    let mut generation = remotehid::Generation::default();
    let mut supervisor = session::StreamSupervisor::new();

    // The announcement burst can lag the AACP session by several seconds on a
    // cold accessory, so this retries in place rather than giving up after one
    // window.
    for round in 1..=ROUNDS {
        supervisor.begin_attempt();

        // The announcement burst arrives during the handshake, so the cached
        // registry is already populated. Drain the live channel first to pick up
        // anything sent since, then read the cache.
        let deadline = tokio::time::Instant::now() + Duration::from_millis(1500);
        while tokio::time::Instant::now() < deadline {
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            let Ok(received) = tokio::time::timeout(remaining, frames.recv()).await else {
                break;
            };
            match received {
                Ok(_frame) => {}
                Err(broadcast::error::RecvError::Closed) => {
                    return Err("AACP receive channel closed".to_string());
                }
                Err(broadcast::error::RecvError::Lagged(n)) => {
                    // Lagging only means the announcement burst outran this
                    // receiver; the channel keeps working. Treating it as
                    // fatal used to abort the whole negotiation.
                    debug!("rtbuddy frame tap lagged, skipped {} frames", n);
                }
            }
            // The receive loop records these already; nothing to do per frame.
        }
        let registry = { remotes.lock().await.clone() };

        let announced: Vec<String> = registry
            .all()
            .map(|d| format!("{}={}", d.service, d.device))
            .collect();
        info!(
            "RemoteHID round {}: announced {} services {:?}",
            round,
            announced.len(),
            announced
        );
        {
            let mut st = status.lock().await;
            *st = if announced.is_empty() {
                format!("round {}/{}: the accessory announced no sensor services", round, ROUNDS)
            } else {
                format!(
                    "round {}/{}: {} services ({})",
                    round,
                    ROUNDS,
                    announced.len(),
                    announced.join(", ")
                )
            };
        }

        if let Some(packet) = supervisor.record_announcement(&registry, &mut generation) {
            if !packet.is_empty() {
                info!("Writing the sample interval for the motion service");
                *status.lock().await =
                    "motion service found; interval written, waiting for samples".to_string();
                sender
                    .send(packet)
                    .await
                    .map_err(|e| format!("could not queue the interval write: {}", e))?;
            }
            return Ok(());
        }

        if !announced.is_empty() {
            debug!("Partial announcement; the accessory is still coming up");
        }
    }

    // Deliberately no disconnect here. Cycling the Bluetooth link was tried and
    // it is destructive: the accessory ends up refusing paging outright
    // (`br-connection-page-timeout`) and only a physical case cycle brings it
    // back, while macOS never does this - it keeps one link and waits. Leaving
    // the connection alone lets the next attempt succeed as soon as the wearer
    // puts the buds back in.
    let _ = address;
    let message = format!(
        "no motion service after {} rounds. Put the buds in the case and take them \
         out again to re-arm the sensor announcement",
        ROUNDS
    );
    *status.lock().await = message.clone();
    Err(message)
}

/// How many announcement windows to watch before giving up on this attempt.
const ROUNDS: u32 = 4;

/// Records a DeviceConnect advertisement in the shared registry.
///
/// Called from the receive loop, which sees the burst during the handshake - well
/// before anything asks for the stream.
async fn record_advertisement(registry: &Mutex<remotehid::DeviceRegistry>, frame: &[u8]) {
    let mut guard = registry.lock().await;
    record_connect(&mut guard, frame);
}

/// Records a DeviceConnect frame in the registry, ignoring anything else.
fn record_connect(registry: &mut remotehid::DeviceRegistry, frame: &[u8]) {
    let Some((_g, header, _p)) = remotehid::parse_frame(frame) else {
        return;
    };
    if header.kind() != Some(remotehid::Kind::HostConnect) || header.is_response {
        return;
    }
    let Some(start) = frame.iter().position(|&b| b == b'<') else {
        return;
    };
    let text = String::from_utf8_lossy(&frame[start..]).to_string();
    let descriptor = text
        .find("<key>ReportDescriptor</key><data>")
        .and_then(|s| {
            let rest = &text[s + "<key>ReportDescriptor</key><data>".len()..];
            rest.find('<').map(|e| decode_b64(&rest[..e]))
        })
        .unwrap_or_default();
    registry.record(header.device, &text, &descriptor);
}

/// Bytes of motion data in a RemoteHID InputReport, after the report-id byte.
const MOTION_BLOCK_LEN: usize = crate::core::head_tracking::REMOTE_HID_MOTION_LEN;

/// Whether an opcode-0x17 frame is a RemoteHID InputReport carrying a motion
/// block, i.e. a sensor sample rather than XML or control traffic.
fn is_motion_input_report(packet: &[u8]) -> bool {
    let Some((_, header, payload)) = remotehid::parse_frame(packet) else {
        return false;
    };
    header.kind() == Some(remotehid::Kind::InputReport)
        && !header.is_response
        // payload starts at the report-id byte; the motion block follows it.
        && payload.len() >= 1 + MOTION_BLOCK_LEN
}

/// The receive-loop routing decision for an opcode-0x17 frame: XML path or
/// sensor path.
///
/// Motion samples are binary and their sensor bytes hit 0x3C (`<`) regularly -
/// the sample counter alone lands one there every 256 frames - so "contains
/// '<'" cannot by itself separate XML descriptors from sensor data. A genuine
/// InputReport carrying a full motion block is a sample; everything else with
/// a `<` is XML. Advertisements are recognised first: their cookie can
/// coincide with the InputReport kind bits in [`remotehid::parse_frame`], and
/// they must stay on the XML path.
fn routes_to_xml(packet: &[u8]) -> bool {
    let is_advertisement = rtbuddy::ServiceAdvertisement::parse(packet).is_some();
    let is_motion_report = !is_advertisement && is_motion_input_report(packet);
    packet.contains(&b'<') && !is_motion_report
}

/// Minimal base64 decoder for the descriptor blobs in DeviceConnect.
fn decode_b64(input: &str) -> Vec<u8> {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = Vec::new();
    let mut buffer: u32 = 0;
    let mut bits = 0;
    for byte in input.bytes() {
        let Some(index) = TABLE.iter().position(|&c| c == byte) else {
            continue;
        };
        buffer = (buffer << 6) | index as u32;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((buffer >> bits) as u8);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Builds a RemoteHID InputReport frame carrying a 58-byte motion block,
    /// in the layout the AirPods 3 actually streams.
    fn motion_frame(counter: u32, seed: u8) -> Vec<u8> {
        let mut block = vec![0u8; 58];
        block[44..46].copy_from_slice(&1000i16.to_le_bytes()); // gravity x
        block[54..58].copy_from_slice(&counter.to_le_bytes());
        for byte in block.iter_mut().skip(20).take(20) {
            *byte = seed;
        }

        let header = 7u32 | ((62u32) << 7) | (2u32 << 17); // device 7, kind 2
        let mut body = vec![0x00, 0x2A, 0x00, 0x00];
        body.extend_from_slice(&header.to_le_bytes());
        body.push(0x00); // report id byte
        body.extend_from_slice(&block);

        let mut frame = vec![0x04, 0x00, 0x04, 0x00, 0x17, 0x00];
        frame.extend_from_slice(&0x0000_0800u32.to_le_bytes());
        frame.extend_from_slice(&(body.len() as u16).to_le_bytes());
        frame.extend_from_slice(&body);
        frame
    }

    /// Builds a sensor service advertisement in the captured rtbuddy layout.
    fn advertisement_frame(cookie: u32) -> Vec<u8> {
        let mut frame = vec![
            0x04, 0x00, 0x04, 0x00, 0x17, 0x00, // AACP header
            0x00, 0x08, 0x00, 0x00, // descriptor 0x800
            0x40, 0x00, // body length
            0x00, 0x16, 0x00, 0x00, // flag, service id 22, reserved
        ];
        frame.extend_from_slice(&cookie.to_le_bytes());
        frame.extend_from_slice(
            b"<?xml?><key>AccessoryService</key><string>neorelay</string></dict>",
        );
        let len = (frame.len() - 12) as u16;
        frame[10..12].copy_from_slice(&len.to_le_bytes());
        frame
    }

    #[test]
    fn motion_sample_with_lt_byte_is_not_routed_to_xml() {
        // The sample counter's low byte cycles 0x00-0xFF, so one frame in 256
        // carries 0x3C ('<') - and seed bytes elsewhere in the block can too.
        // Each such frame used to be mistaken for an XML descriptor and
        // dropped from the tracker.
        let frame = motion_frame(0x0000_003C, 0x3C);
        assert!(frame.contains(&b'<'), "the frame must contain a '<'");
        assert!(is_motion_input_report(&frame));

        // The receive-loop predicate: contains '<' but rescued by the motion
        // check, so it reaches the tracker instead of the XML path.
        let routed_to_xml = routes_to_xml(&frame);
        assert!(!routed_to_xml);
    }

    #[test]
    fn advertisement_is_xml_even_when_its_cookie_looks_like_an_input_report() {
        // parse_frame reads body[4..8] as a sub-header, so a cookie whose
        // second byte sets the kind bits can coincide with InputReport (2).
        // The advertisement check runs first and must win.
        let frame = advertisement_frame(0x0004_3483); // kind bits come out as 2
        assert!(rtbuddy::ServiceAdvertisement::parse(&frame).is_some());
        assert!(is_motion_input_report(&frame)); // coincidence confirmed...
        let routed_to_xml = routes_to_xml(&frame);
        assert!(routed_to_xml, "the advertisement check must run first");
    }

    #[test]
    fn deviceconnect_is_xml_wherever_its_plist_starts() {
        // A DeviceConnect has kind 0, so it is never mistaken for a motion
        // sample - including if its plist does not start at a fixed offset.
        for plist_offset in [20usize, 21, 24] {
            let mut body = vec![0x00, 0x2A, 0x00, 0x00];
            body.extend_from_slice(&7u32.to_le_bytes()); // kind 0 sub-header
            body.resize(plist_offset - 12, 0xEE);
            body.extend_from_slice(b"<plist><key>AccessoryService</key><string>x</string>");

            let mut frame = vec![0x04, 0x00, 0x04, 0x00, 0x17, 0x00];
            frame.extend_from_slice(&0x0000_0800u32.to_le_bytes());
            frame.extend_from_slice(&(body.len() as u16).to_le_bytes());
            frame.extend_from_slice(&body);

            assert!(!is_motion_input_report(&frame));
            let routed_to_xml = routes_to_xml(&frame);
            assert!(routed_to_xml, "plist at offset {plist_offset} must stay XML");
        }
    }

    #[test]
    fn short_control_frames_stay_xml() {
        // SetReport responses and the enable ACK are short and kind 3; a '<'
        // inside them keeps them on the XML path (they are not samples).
        let mut body = vec![0x00, 0x2A, 0x00, 0x00];
        body.extend_from_slice(&((3u32 << 17) | (1u32 << 21)).to_le_bytes()); // kind 3, response
        body.push(0x02);
        body.extend_from_slice(&[0x3C, 0x3C, 0x00, 0x00]);

        let mut frame = vec![0x04, 0x00, 0x04, 0x00, 0x17, 0x00];
        frame.extend_from_slice(&0x0000_0800u32.to_le_bytes());
        frame.extend_from_slice(&(body.len() as u16).to_le_bytes());
        frame.extend_from_slice(&body);

        assert!(!is_motion_input_report(&frame));
        let routed_to_xml = routes_to_xml(&frame);
        assert!(routed_to_xml);
    }
}

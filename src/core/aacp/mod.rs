pub mod commands;
pub mod events;
pub mod packets;
pub mod remotehid;
pub mod rtbuddy;

use bluer::l2cap::{Socket, SocketAddr};
use bluer::{Address, AddressType};
use commands::{ControlCommandId, MicrophoneMode, NoiseControlMode};
use events::AacpEvent;
use packets::*;
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
                            if packet.contains(&b'<') {
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
                                continue;
                            }
                            // Short frames on this descriptor are descriptor
                            // replies and the enable ACK, not samples.
                            let descriptor = if len >= 10 {
                                u32::from_le_bytes([packet[6], packet[7], packet[8], packet[9]])
                            } else {
                                0
                            };
                            // Every RemoteHID frame belongs to the tap: on
                            // descriptor 0x800 the accessory sends DeviceConnect
                            // blobs that are hundreds of bytes long, so a size
                            // test alone would misroute them into the sample
                            // parser. Only the SensorDataWX descriptor carries
                            // attitude samples.
                            if descriptor == rtbuddy::DESCRIPTOR_SENSOR
                                || (len < crate::core::head_tracking::SENSOR_V2_LEN
                                    && !packet
                                        .windows(crate::core::head_tracking::PAYLOAD_MARK.len())
                                        .any(|w| w == crate::core::head_tracking::PAYLOAD_MARK))
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
        info!("Subscribing to RTBuddy sensor services...");

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
    /// Four things have to line up, all established from the macOS capture and
    /// from byte-exact comparison with a working session:
    ///
    /// 1. An application has claimed spatial audio in head-tracked mode. macOS
    ///    refuses to send the enable without this (`mode: 3, headtrack: 1`).
    /// 2. Subscribe to the sensor services, so the accessory advertises
    ///    `neorelay` and hands out a per-connection service id and cookie.
    /// 3. Send the 14-byte HID enable carrying those identifiers — this is what
    ///    arms the stream.
    /// 4. Keep re-sending, because the accessory only streams while both buds
    ///    are in ear and ignores anything sent while they were in the case.
    ///
    /// Returns immediately; the retry loop runs in the background until samples
    /// arrive, the claim is released, or the connection drops.
    pub async fn start_head_tracking(&self) -> Result<(), String> {
        if !self.is_connected().await {
            return Err("AACP socket not connected".to_string());
        }

        let retry_sender = {
            let guard = self.send_tx.lock().await;
            guard.clone()
        };
        let Some(sender) = retry_sender else {
            return Err("AACP socket not connected".to_string());
        };
        let tracker = Arc::clone(&self.head_tracker);
        let ids = Arc::clone(&self.neorelay_ids);
        let spatial = Arc::clone(&self.spatial);

        tokio::spawn(async move {
            let mut variant = 0usize;
            let mut attempts = 0u32;

            loop {
                // Stop retrying once samples are arriving or the socket is gone.
                if tracker.lock().await.samples_seen() > 0 {
                    debug!("Head-tracking stream is live; stopping the retries");
                    break;
                }

                // macOS will not send this enable unless an app holds a
                // head-tracked spatial claim. Stop as soon as it is released, so
                // we never keep poking an accessory nobody asked to track.
                {
                    let mut session = spatial.lock().await;
                    if !session.wants_head_tracking() {
                        debug!("No spatial claim permits head tracking; stopping retries");
                        break;
                    }
                    session.mark_enable_sent();
                }
                // The enable needs the identifiers from *this* connection's
                // advertisement; without them only the START probe can be sent.
                let identifiers = *ids.lock().await;

                let mut packets: Vec<Vec<u8>> = Vec::new();
                if let Some((service_id, cookie)) = identifiers {
                    packets.push(rtbuddy::make_sensor_v2_enable(
                        service_id,
                        cookie,
                        rtbuddy::INTERVAL_50HZ_US,
                    ));
                }
                packets.push(if variant == 0 {
                    rtbuddy::make_head_tracking_start(rtbuddy::INTERVAL_25HZ_US)
                } else {
                    rtbuddy::make_alternate_start_head_tracking(rtbuddy::INTERVAL_25HZ_US)
                });
                variant ^= 1;
                attempts += 1;

                if attempts == 1 {
                    match identifiers {
                        Some((service_id, cookie)) => info!(
                            "Starting head tracking: enable(service={}, cookie=0x{:08X}) + START",
                            service_id, cookie
                        ),
                        None => info!(
                            "Starting head tracking: START only (no advertisement yet)"
                        ),
                    }
                } else {
                    debug!("Head-tracking retry {}", attempts);
                }

                for packet in packets {
                    if sender.send(packet).await.is_err() {
                        debug!("AACP send channel closed; stopping retries");
                        return;
                    }
                    sleep(Duration::from_millis(60)).await;
                }

                // The reference implementation retries every 4 s while the buds
                // are in ear.
                sleep(Duration::from_secs(4)).await;
            }
        });

        Ok(())
    }

    /// Asks the AirPods to stop the head-tracking stream.
    pub async fn stop_head_tracking(&self) -> Result<(), String> {
        debug!("Stopping head tracking...");
        self.send_raw(rtbuddy::make_head_tracking_stop()).await
    }

}

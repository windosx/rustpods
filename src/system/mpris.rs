use std::collections::HashSet;
use std::sync::Arc;
use tokio::sync::Mutex;
use tracing::{debug, error, info};
use zbus::proxy;
use zbus::Connection;

#[proxy(
    default_service = "org.mpris.MediaPlayer2",
    interface = "org.mpris.MediaPlayer2.Player"
)]
trait MediaPlayer2Player {
    fn next(&self) -> zbus::Result<()>;
    fn previous(&self) -> zbus::Result<()>;
    fn pause(&self) -> zbus::Result<()>;
    fn play_pause(&self) -> zbus::Result<()>;
    fn play(&self) -> zbus::Result<()>;

    #[zbus(property)]
    fn playback_status(&self) -> zbus::Result<String>;

    #[zbus(property)]
    fn volume(&self) -> zbus::Result<f64>;

    #[zbus(property)]
    fn set_volume(&self, volume: f64) -> zbus::Result<()>;
}

#[derive(Clone)]
pub struct MprisController {
    paused_by_us: Arc<Mutex<HashSet<String>>>,
}

impl MprisController {
    pub fn new() -> Self {
        Self {
            paused_by_us: Arc::new(Mutex::new(HashSet::new())),
        }
    }

    pub async fn get_player_names(&self, conn: &Connection) -> Vec<String> {
        let dbus_proxy = zbus::fdo::DBusProxy::new(conn)
            .await
            .map_err(|e| error!("Failed to create DBusProxy: {}", e))
            .ok();

        if let Some(proxy) = dbus_proxy {
            if let Ok(names) = proxy.list_names().await {
                return names
                    .into_iter()
                    .filter(|name| name.starts_with("org.mpris.MediaPlayer2."))
                    .map(|n| n.to_string())
                    .collect();
            }
        }
        Vec::new()
    }

    pub async fn handle_ear_detection_pause(&self) {
        info!("Earbuds removed: auto-pausing media players...");
        let conn = match Connection::session().await {
            Ok(c) => c,
            Err(e) => {
                error!("Failed to connect to D-Bus session: {}", e);
                return;
            }
        };

        let players = self.get_player_names(&conn).await;
        let mut paused_set = self.paused_by_us.lock().await;

        for player_name in players {
            let proxy_res = MediaPlayer2PlayerProxy::builder(&conn)
                .destination(player_name.as_str())
                .and_then(|b| b.path("/org/mpris/MediaPlayer2"));

            if let Ok(builder) = proxy_res {
                if let Ok(proxy) = builder.build().await {
                    if let Ok(status) = proxy.playback_status().await {
                        if status == "Playing" {
                            if let Ok(_) = proxy.pause().await {
                                info!("Paused player: {}", player_name);
                                paused_set.insert(player_name);
                            }
                        }
                    }
                }
            }
        }
    }

    pub async fn handle_ear_detection_resume(&self) {
        info!("Earbuds put back: auto-resuming media players...");
        let conn = match Connection::session().await {
            Ok(c) => c,
            Err(e) => {
                error!("Failed to connect to D-Bus session: {}", e);
                return;
            }
        };

        let mut paused_set = self.paused_by_us.lock().await;
        for player_name in paused_set.drain() {
            let proxy_res = MediaPlayer2PlayerProxy::builder(&conn)
                .destination(player_name.as_str())
                .and_then(|b| b.path("/org/mpris/MediaPlayer2"));

            if let Ok(builder) = proxy_res {
                if let Ok(proxy) = builder.build().await {
                    // Only resume what is still paused. If the user resumed or
                    // stopped the player themselves while the buds were out,
                    // forcing play() here would start playback they ended.
                    let still_paused = matches!(
                        proxy.playback_status().await.as_deref(),
                        Ok("Paused")
                    );
                    if still_paused && proxy.play().await.is_ok() {
                        info!("Resumed player: {}", player_name);
                    }
                }
            }
        }
    }

    pub async fn play_pause(&self) {
        debug!("MPRIS Play/Pause triggered");
        let conn = match Connection::session().await {
            Ok(c) => c,
            Err(e) => {
                error!("Failed to connect to D-Bus: {}", e);
                return;
            }
        };
        let players = self.get_player_names(&conn).await;
        if let Some(primary) = players.first() {
            let proxy_res = MediaPlayer2PlayerProxy::builder(&conn)
                .destination(primary.as_str())
                .and_then(|b| b.path("/org/mpris/MediaPlayer2"));

            if let Ok(builder) = proxy_res {
                if let Ok(proxy) = builder.build().await {
                    let _ = proxy.play_pause().await;
                }
            }
        }
    }

    pub async fn next(&self) {
        debug!("MPRIS Next track triggered");
        let conn = match Connection::session().await {
            Ok(c) => c,
            Err(e) => {
                error!("Failed to connect to D-Bus: {}", e);
                return;
            }
        };
        let players = self.get_player_names(&conn).await;
        if let Some(primary) = players.first() {
            let proxy_res = MediaPlayer2PlayerProxy::builder(&conn)
                .destination(primary.as_str())
                .and_then(|b| b.path("/org/mpris/MediaPlayer2"));

            if let Ok(builder) = proxy_res {
                if let Ok(proxy) = builder.build().await {
                    let _ = proxy.next().await;
                }
            }
        }
    }

    pub async fn previous(&self) {
        debug!("MPRIS Previous track triggered");
        let conn = match Connection::session().await {
            Ok(c) => c,
            Err(e) => {
                error!("Failed to connect to D-Bus: {}", e);
                return;
            }
        };
        let players = self.get_player_names(&conn).await;
        if let Some(primary) = players.first() {
            let proxy_res = MediaPlayer2PlayerProxy::builder(&conn)
                .destination(primary.as_str())
                .and_then(|b| b.path("/org/mpris/MediaPlayer2"));

            if let Ok(builder) = proxy_res {
                if let Ok(proxy) = builder.build().await {
                    let _ = proxy.previous().await;
                }
            }
        }
    }
}

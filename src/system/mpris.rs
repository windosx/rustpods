use std::collections::HashSet;
use std::sync::Arc;
use tokio::sync::Mutex;
use tracing::{debug, info};
#[cfg(target_os = "linux")]
use tracing::error;

#[cfg(target_os = "linux")]
use zbus::proxy;
#[cfg(target_os = "linux")]
use zbus::Connection;

#[cfg(target_os = "linux")]
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

#[cfg(target_os = "linux")]
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

        if let Some(proxy) = dbus_proxy
            && let Ok(names) = proxy.list_names().await {
                return names
                    .into_iter()
                    .filter(|name| name.starts_with("org.mpris.MediaPlayer2."))
                    .map(|n| n.to_string())
                    .collect();
            }
        Vec::new()
    }
}

#[cfg(target_os = "linux")]
impl Default for MprisController {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(target_os = "linux")]
impl MprisController {
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

            if let Ok(builder) = proxy_res
                && let Ok(proxy) = builder.build().await
                    && let Ok(status) = proxy.playback_status().await
                        && status == "Playing"
                            && let Ok(_) = proxy.pause().await {
                                info!("Paused player: {}", player_name);
                                paused_set.insert(player_name);
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

            if let Ok(builder) = proxy_res
                && let Ok(proxy) = builder.build().await {
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

            if let Ok(builder) = proxy_res
                && let Ok(proxy) = builder.build().await {
                    let _ = proxy.play_pause().await;
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

            if let Ok(builder) = proxy_res
                && let Ok(proxy) = builder.build().await {
                    let _ = proxy.next().await;
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

            if let Ok(builder) = proxy_res
                && let Ok(proxy) = builder.build().await {
                    let _ = proxy.previous().await;
                }
        }
    }
}

#[cfg(target_os = "windows")]
mod win_media {
    use tracing::info;
    use windows::Media::Control::{
        GlobalSystemMediaTransportControlsSessionManager,
        GlobalSystemMediaTransportControlsSessionPlaybackStatus,
    };
    use windows::Win32::UI::Input::KeyboardAndMouse::{
        SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYEVENTF_EXTENDEDKEY,
        KEYEVENTF_KEYUP, VIRTUAL_KEY,
    };

    pub const VK_MEDIA_NEXT_TRACK: VIRTUAL_KEY = VIRTUAL_KEY(0xB0);
    pub const VK_MEDIA_PREV_TRACK: VIRTUAL_KEY = VIRTUAL_KEY(0xB1);
    pub const VK_MEDIA_PLAY_PAUSE: VIRTUAL_KEY = VIRTUAL_KEY(0xB3);

    pub fn send_media_key(vk: VIRTUAL_KEY) {
        let inputs = [
            INPUT {
                r#type: INPUT_KEYBOARD,
                Anonymous: INPUT_0 {
                    ki: KEYBDINPUT {
                        wVk: vk,
                        wScan: 0,
                        dwFlags: KEYEVENTF_EXTENDEDKEY,
                        time: 0,
                        dwExtraInfo: 0,
                    },
                },
            },
            INPUT {
                r#type: INPUT_KEYBOARD,
                Anonymous: INPUT_0 {
                    ki: KEYBDINPUT {
                        wVk: vk,
                        wScan: 0,
                        dwFlags: KEYEVENTF_EXTENDEDKEY | KEYEVENTF_KEYUP,
                        time: 0,
                        dwExtraInfo: 0,
                    },
                },
            },
        ];
        unsafe {
            SendInput(&inputs, std::mem::size_of::<INPUT>() as i32);
        }
    }

    pub fn try_gsmtc_pause() -> Option<String> {
        let manager = GlobalSystemMediaTransportControlsSessionManager::RequestAsync()
            .ok()?
            .get()
            .ok()?;
        let session = manager.GetCurrentSession().ok()?;
        let playback_info = session.GetPlaybackInfo().ok()?;
        if playback_info.PlaybackStatus().ok()?
            == GlobalSystemMediaTransportControlsSessionPlaybackStatus::Playing
        {
            let app_id = session
                .SourceAppUserModelId()
                .map(|h| h.to_string())
                .unwrap_or_else(|_| "UnknownApp".to_string());
            if let Ok(true) = session.TryPauseAsync().ok()?.get() {
                info!("Paused Windows media session: {}", app_id);
                return Some(app_id);
            }
        }
        None
    }

    pub fn try_gsmtc_resume() -> bool {
        let manager = match GlobalSystemMediaTransportControlsSessionManager::RequestAsync() {
            Ok(op) => match op.get() {
                Ok(m) => m,
                Err(_) => return false,
            },
            Err(_) => return false,
        };
        let session = match manager.GetCurrentSession() {
            Ok(s) => s,
            Err(_) => return false,
        };
        match session.TryPlayAsync() {
            Ok(op) => op.get().unwrap_or(false),
            Err(_) => false,
        }
    }

    pub fn try_gsmtc_play_pause() -> bool {
        let manager = match GlobalSystemMediaTransportControlsSessionManager::RequestAsync() {
            Ok(op) => match op.get() {
                Ok(m) => m,
                Err(_) => return false,
            },
            Err(_) => return false,
        };
        let session = match manager.GetCurrentSession() {
            Ok(s) => s,
            Err(_) => return false,
        };
        match session.TryTogglePlayPauseAsync() {
            Ok(op) => op.get().unwrap_or(false),
            Err(_) => false,
        }
    }

    pub fn try_gsmtc_next() -> bool {
        let manager = match GlobalSystemMediaTransportControlsSessionManager::RequestAsync() {
            Ok(op) => match op.get() {
                Ok(m) => m,
                Err(_) => return false,
            },
            Err(_) => return false,
        };
        let session = match manager.GetCurrentSession() {
            Ok(s) => s,
            Err(_) => return false,
        };
        match session.TrySkipNextAsync() {
            Ok(op) => op.get().unwrap_or(false),
            Err(_) => false,
        }
    }

    pub fn try_gsmtc_prev() -> bool {
        let manager = match GlobalSystemMediaTransportControlsSessionManager::RequestAsync() {
            Ok(op) => match op.get() {
                Ok(m) => m,
                Err(_) => return false,
            },
            Err(_) => return false,
        };
        let session = match manager.GetCurrentSession() {
            Ok(s) => s,
            Err(_) => return false,
        };
        match session.TrySkipPreviousAsync() {
            Ok(op) => op.get().unwrap_or(false),
            Err(_) => false,
        }
    }
}

#[cfg(target_os = "windows")]
impl MprisController {
    pub fn new() -> Self {
        Self {
            paused_by_us: Arc::new(Mutex::new(HashSet::new())),
        }
    }

    pub async fn handle_ear_detection_pause(&self) {
        info!("Earbuds removed: auto-pausing media players (Windows)...");
        let paused_by_us = self.paused_by_us.clone();
        let _ = tokio::task::spawn_blocking(move || {
            if let Some(app_id) = win_media::try_gsmtc_pause() {
                if let Ok(mut set) = paused_by_us.try_lock() {
                    set.insert(app_id);
                }
            } else {
                win_media::send_media_key(win_media::VK_MEDIA_PLAY_PAUSE);
                if let Ok(mut set) = paused_by_us.try_lock() {
                    set.insert("VK_MEDIA_KEY".to_string());
                }
            }
        })
        .await;
    }

    pub async fn handle_ear_detection_resume(&self) {
        info!("Earbuds put back: auto-resuming media players (Windows)...");
        let paused_by_us = self.paused_by_us.clone();
        let _ = tokio::task::spawn_blocking(move || {
            let mut set = match paused_by_us.try_lock() {
                Ok(s) => s,
                Err(_) => return,
            };
            if set.is_empty() {
                return;
            }
            set.clear();

            if !win_media::try_gsmtc_resume() {
                win_media::send_media_key(win_media::VK_MEDIA_PLAY_PAUSE);
            }
        })
        .await;
    }

    pub async fn play_pause(&self) {
        debug!("Media Play/Pause triggered (Windows)");
        let _ = tokio::task::spawn_blocking(|| {
            if !win_media::try_gsmtc_play_pause() {
                win_media::send_media_key(win_media::VK_MEDIA_PLAY_PAUSE);
            }
        })
        .await;
    }

    pub async fn next(&self) {
        debug!("Media Next track triggered (Windows)");
        let _ = tokio::task::spawn_blocking(|| {
            if !win_media::try_gsmtc_next() {
                win_media::send_media_key(win_media::VK_MEDIA_NEXT_TRACK);
            }
        })
        .await;
    }

    pub async fn previous(&self) {
        debug!("Media Previous track triggered (Windows)");
        let _ = tokio::task::spawn_blocking(|| {
            if !win_media::try_gsmtc_prev() {
                win_media::send_media_key(win_media::VK_MEDIA_PREV_TRACK);
            }
        })
        .await;
    }
}

#[cfg(target_os = "windows")]
impl Default for MprisController {
    fn default() -> Self {
        Self::new()
    }
}

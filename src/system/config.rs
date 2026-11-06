use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;
use tracing::{debug, error};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PauseBehavior {
    OneRemoved,
    BothRemoved,
    Never,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum StemAction {
    PlayPause,
    NextTrack,
    PreviousTrack,
    ToggleNoiseMode,
    SiriOrAssistant,
    None,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppConfig {
    pub ear_detection_enabled: bool,
    pub pause_behavior: PauseBehavior,
    pub conversational_awareness_enabled: bool,
    pub stem_single_click: StemAction,
    pub stem_double_click: StemAction,
    pub stem_triple_click: StemAction,
    pub stem_long_press: StemAction,
    pub start_minimized: bool,
    pub close_to_tray: bool,
    /// Answers ZCode `AskUserQuestion` prompts by counting head nods while a
    /// question is pending. The ZCode hook always falls back to the normal
    /// question UI when this is off, so the toggle only silences the feature.
    ///
    /// `serde(default)` keeps config files written before this field existed
    /// parseable - a missing field must fall back to the feature default, not
    /// invalidate the whole file (a parse failure here discards every saved
    /// preference and rewrites the file from scratch).
    #[serde(default = "default_true")]
    pub head_gesture_answers_enabled: bool,
}

fn default_true() -> bool {
    true
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            ear_detection_enabled: true,
            pause_behavior: PauseBehavior::OneRemoved,
            conversational_awareness_enabled: false,
            stem_single_click: StemAction::PlayPause,
            stem_double_click: StemAction::NextTrack,
            stem_triple_click: StemAction::PreviousTrack,
            stem_long_press: StemAction::ToggleNoiseMode,
            start_minimized: false,
            close_to_tray: true,
            head_gesture_answers_enabled: true,
        }
    }
}

pub fn get_config_dir() -> PathBuf {
    let dir = std::env::var("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
            PathBuf::from(home).join(".config")
        })
        .join("rustpods");
    let _ = fs::create_dir_all(&dir);
    dir
}

pub fn load_config() -> AppConfig {
    let path = get_config_dir().join("config.json");
    if path.exists() {
        match fs::read_to_string(&path) {
            Ok(content) => match serde_json::from_str(&content) {
                Ok(cfg) => return cfg,
                Err(e) => error!("Failed to parse config.json: {}", e),
            },
            Err(e) => error!("Failed to read config.json: {}", e),
        }
    }
    let default_cfg = AppConfig::default();
    save_config(&default_cfg);
    default_cfg
}

pub fn save_config(cfg: &AppConfig) {
    let path = get_config_dir().join("config.json");
    match serde_json::to_string_pretty(cfg) {
        Ok(json) => {
            if let Err(e) = fs::write(&path, json) {
                error!("Failed to write config.json: {}", e);
            } else {
                debug!("Saved config to {:?}", path);
            }
        }
        Err(e) => error!("Failed to serialize config: {}", e),
    }
}

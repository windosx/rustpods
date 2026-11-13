use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
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
    MouseLeftClick,
    MouseRightClick,
    ToggleMouseControl,
    ToggleMouseScroll,
    None,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum HeadMouseMode {
    #[default]
    Relative,
    Absolute,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum AppLanguage {
    #[default]
    Auto,
    ZhCn,
    EnUs,
}

impl AppLanguage {
    pub const ALL: [AppLanguage; 3] = [
        AppLanguage::Auto,
        AppLanguage::ZhCn,
        AppLanguage::EnUs,
    ];

    pub fn resolved_locale(self) -> &'static str {
        match self {
            AppLanguage::Auto => detect_system_locale(),
            AppLanguage::ZhCn => "zh-CN",
            AppLanguage::EnUs => "en",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            AppLanguage::Auto => "跟随系统 (Auto)",
            AppLanguage::ZhCn => "简体中文",
            AppLanguage::EnUs => "English",
        }
    }
}

/// Detects the system language on Linux (checking sys_locale and POSIX env vars).
pub fn detect_system_locale() -> &'static str {
    if let Some(locale) = sys_locale::get_locale() {
        if locale.to_lowercase().starts_with("zh") {
            return "zh-CN";
        }
    }
    for var in ["LC_ALL", "LC_MESSAGES", "LANG"] {
        if let Ok(val) = std::env::var(var) {
            if val.to_lowercase().starts_with("zh") {
                return "zh-CN";
            }
        }
    }
    "en"
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
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
    /// Gesture sensitivity, 1 (only deliberate, large nods) to 5 (light
    /// movements already count). Drives the nod counter's thresholds and the
    /// badge detector's amplitude gates; clamped at the use sites.
    #[serde(default = "default_gesture_sensitivity")]
    pub gesture_sensitivity: u8,
    /// Show an iOS-style floating popup card with 3D model animation when the
    /// AirPods case lid is opened nearby.
    #[serde(default = "default_true")]
    pub case_popup_enabled: bool,
    /// Cached Apple Proximity IRK (16 bytes, lowercase hex) for verifying BLE
    /// Resolvable Private Addresses when the case lid opens before Classic BT connects.
    #[serde(default)]
    pub proximity_irk: Option<String>,
    /// Cached Apple Proximity AES-128 key (16 bytes, lowercase hex) for decrypting
    /// exact battery levels from BLE advertisements.
    #[serde(default)]
    pub proximity_enc_key: Option<String>,
    /// Controls whether head orientation / angular velocity moves the mouse cursor.
    #[serde(default)]
    pub head_mouse_enabled: bool,
    #[serde(default)]
    pub head_mouse_mode: HeadMouseMode,
    #[serde(default = "default_mouse_speed")]
    pub head_mouse_speed: f32,
    #[serde(default = "default_mouse_accel")]
    pub head_mouse_accel: f32,
    #[serde(default = "default_mouse_smoothing")]
    pub head_mouse_smoothing: f32,
    #[serde(default = "default_mouse_deadzone")]
    pub head_mouse_deadzone: f32,
    #[serde(default)]
    pub head_mouse_invert_x: bool,
    #[serde(default)]
    pub head_mouse_invert_y: bool,
    #[serde(default)]
    pub head_mouse_roll_click: bool,
    #[serde(default)]
    pub head_mouse_dwell_click: bool,
    /// Interface display language (Auto follows system, ZhCn, or EnUs).
    #[serde(default)]
    pub language: AppLanguage,
}

fn default_true() -> bool {
    true
}

fn default_gesture_sensitivity() -> u8 {
    3
}

fn default_mouse_speed() -> f32 {
    20.0
}

fn default_mouse_accel() -> f32 {
    1.0
}

fn default_mouse_smoothing() -> f32 {
    0.35
}

fn default_mouse_deadzone() -> f32 {
    2.0
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
            gesture_sensitivity: 3,
            case_popup_enabled: true,
            proximity_irk: None,
            proximity_enc_key: None,
            head_mouse_enabled: false,
            head_mouse_mode: HeadMouseMode::Relative,
            head_mouse_speed: 20.0,
            head_mouse_accel: 1.0,
            head_mouse_smoothing: 0.35,
            head_mouse_deadzone: 2.0,
            head_mouse_invert_x: false,
            head_mouse_invert_y: false,
            head_mouse_roll_click: false,
            head_mouse_dwell_click: false,
            language: AppLanguage::Auto,
        }
    }
}

pub fn get_config_dir() -> PathBuf {
    // An empty-but-set XDG variable counts as unset (XDG spec); without this
    // a stray empty value would produce a relative config directory and
    // settings would silently stop persisting.
    let dir = std::env::var("XDG_CONFIG_HOME")
        .ok()
        .filter(|s| !s.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
            PathBuf::from(home).join(".config")
        })
        .join("rustpods");
    let _ = fs::create_dir_all(&dir);
    dir
}

/// What loading the config file ended up doing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfigLoad {
    /// Read and parsed successfully.
    Loaded,
    /// No file yet; defaults were produced. The caller creates the file.
    Created,
    /// The file existed but could not be parsed. Defaults are in effect and
    /// the original bytes were preserved beside the config for recovery.
    RecoveredFromInvalid,
    /// The file existed but could not be read (permissions, IO error).
    Unreadable,
}

pub fn load_config() -> AppConfig {
    let path = get_config_dir().join("config.json");
    let (cfg, outcome) = load_config_from(&path);
    if outcome == ConfigLoad::Created {
        // First run only: put the defaults on disk so the next start reads
        // the same values. Every other outcome must not write - in
        // particular a parse failure used to overwrite the user's whole
        // config with defaults here, destroying their settings.
        save_config(&cfg);
    }
    cfg
}

/// Loads the config from an explicit path, so tests and future callers do not
/// have to touch the global location.
pub fn load_config_from(path: &Path) -> (AppConfig, ConfigLoad) {
    match fs::read_to_string(path) {
        Ok(content) => match serde_json::from_str(&content) {
            Ok(cfg) => (cfg, ConfigLoad::Loaded),
            Err(e) => {
                // A config we cannot parse is still the user's data. Back it
                // up untouched and run on defaults; the next explicit save
                // from a settings toggle replaces the live file, and the
                // backup keeps whatever was there recoverable by hand.
                let backup = path.with_extension("json.invalid");
                error!(
                    "Failed to parse {}: {} - defaults in effect, original kept at {}",
                    path.display(),
                    e,
                    backup.display()
                );
                let _ = fs::copy(path, &backup);
                (AppConfig::default(), ConfigLoad::RecoveredFromInvalid)
            }
        },
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            (AppConfig::default(), ConfigLoad::Created)
        }
        Err(e) => {
            error!("Failed to read {}: {}", path.display(), e);
            (AppConfig::default(), ConfigLoad::Unreadable)
        }
    }
}

pub fn save_config(cfg: &AppConfig) {
    save_config_to(&get_config_dir().join("config.json"), cfg);
}

pub fn save_config_to(path: &Path, cfg: &AppConfig) {
    match serde_json::to_string_pretty(cfg) {
        Ok(json) => {
            // Write-then-rename: a crash or full disk mid-write must never
            // leave a half-written config behind, which the next start would
            // otherwise find unparseable.
            let tmp = path.with_extension("json.tmp");
            if let Err(e) = fs::write(&tmp, &json) {
                error!("Failed to write {}: {}", tmp.display(), e);
                return;
            }
            if let Err(e) = fs::rename(&tmp, path) {
                error!("Failed to move {} into place: {}", tmp.display(), e);
                let _ = fs::remove_file(&tmp);
            } else {
                debug!("Saved config to {:?}", path);
            }
        }
        Err(e) => error!("Failed to serialize config: {}", e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    /// A unique scratch path per call, so parallel tests never collide.
    fn scratch(tag: &str) -> PathBuf {
        static N: AtomicU32 = AtomicU32::new(0);
        std::env::temp_dir().join(format!(
            "rustpods-config-{}-{}-{}.json",
            std::process::id(),
            N.fetch_add(1, Ordering::SeqCst),
            tag
        ))
    }

    fn remove_scratch(path: &Path) {
        let _ = fs::remove_file(path);
        let _ = fs::remove_file(path.with_extension("json.tmp"));
        let _ = fs::remove_file(path.with_extension("json.invalid"));
    }

    #[test]
    fn missing_file_reports_created_without_writing() {
        let path = scratch("missing");
        let (cfg, outcome) = load_config_from(&path);
        assert_eq!(outcome, ConfigLoad::Created);
        assert_eq!(cfg, AppConfig::default());
        // The pure loader never writes; the wrapper's first-run save does.
        assert!(!path.exists(), "load must not create the file itself");
    }

    #[test]
    fn valid_file_loads_untouched() {
        let path = scratch("valid");
        let mut cfg = AppConfig::default();
        cfg.close_to_tray = false;
        cfg.pause_behavior = PauseBehavior::Never;
        save_config_to(&path, &cfg);
        let on_disk = fs::read_to_string(&path).unwrap();

        let (loaded, outcome) = load_config_from(&path);
        assert_eq!(outcome, ConfigLoad::Loaded);
        assert!(!loaded.close_to_tray);
        assert_eq!(loaded.pause_behavior, PauseBehavior::Never);
        // Loading must not rewrite a healthy file.
        assert_eq!(fs::read_to_string(&path).unwrap(), on_disk);
        remove_scratch(&path);
    }

    #[test]
    fn invalid_file_is_backed_up_and_not_overwritten() {
        let path = scratch("invalid");
        let broken = "{ this is not json";
        fs::write(&path, broken).unwrap();

        let (cfg, outcome) = load_config_from(&path);
        assert_eq!(outcome, ConfigLoad::RecoveredFromInvalid);
        assert_eq!(cfg, AppConfig::default());

        // The user's bytes survive in place and in the backup. This is the
        // regression guard: the old behaviour saved defaults over the file
        // right here, destroying whatever the user had configured.
        assert_eq!(fs::read_to_string(&path).unwrap(), broken);
        let backup = path.with_extension("json.invalid");
        assert_eq!(fs::read_to_string(&backup).unwrap(), broken);
        remove_scratch(&path);
    }

    #[test]
    fn save_then_load_roundtrips_every_field() {
        let path = scratch("roundtrip");
        let mut cfg = AppConfig::default();
        cfg.ear_detection_enabled = false;
        cfg.pause_behavior = PauseBehavior::BothRemoved;
        cfg.conversational_awareness_enabled = true;
        cfg.stem_long_press = StemAction::SiriOrAssistant;
        cfg.start_minimized = true;
        cfg.close_to_tray = false;
        cfg.head_gesture_answers_enabled = false;
        cfg.case_popup_enabled = false;
        cfg.proximity_irk = Some("0123456789abcdef0123456789abcdef".to_string());
        cfg.proximity_enc_key = Some("fedcba9876543210fedcba9876543210".to_string());
        cfg.head_mouse_enabled = true;
        cfg.head_mouse_mode = HeadMouseMode::Absolute;
        cfg.head_mouse_speed = 35.0;
        cfg.head_mouse_accel = 1.8;
        cfg.head_mouse_smoothing = 0.5;
        cfg.head_mouse_deadzone = 2.5;
        cfg.head_mouse_invert_x = true;
        cfg.head_mouse_invert_y = true;
        cfg.head_mouse_roll_click = true;
        cfg.head_mouse_dwell_click = true;
        cfg.language = AppLanguage::EnUs;
        save_config_to(&path, &cfg);

        let (loaded, outcome) = load_config_from(&path);
        assert_eq!(outcome, ConfigLoad::Loaded);
        assert_eq!(loaded, cfg);
        remove_scratch(&path);
    }

    #[test]
    fn language_resolution_and_locale_detection() {
        assert_eq!(AppLanguage::ZhCn.resolved_locale(), "zh-CN");
        assert_eq!(AppLanguage::EnUs.resolved_locale(), "en");
        let auto = AppLanguage::Auto.resolved_locale();
        assert!(auto == "zh-CN" || auto == "en");
    }

    #[test]
    fn translations_resolve_in_both_locales() {
        assert_eq!(rust_i18n::t!("nav.overview", locale = "zh-CN"), "概览首页");
        assert_eq!(rust_i18n::t!("nav.overview", locale = "en"), "Overview");
        assert_eq!(rust_i18n::t!("head_mouse.title", locale = "zh-CN"), "头部姿态鼠标");
        assert_eq!(rust_i18n::t!("head_mouse.title", locale = "en"), "Head Pose Mouse");
        assert_eq!(rust_i18n::t!("preferences.language", locale = "zh-CN"), "界面语言 (Language)");
        assert_eq!(rust_i18n::t!("preferences.language", locale = "en"), "Language");
    }

    #[test]
    fn older_config_json_without_head_mouse_deserializes_to_defaults() {
        let path = scratch("older");
        let json = r#"{
            "ear_detection_enabled": true,
            "pause_behavior": "OneRemoved",
            "conversational_awareness_enabled": false,
            "stem_single_click": "PlayPause",
            "stem_double_click": "NextTrack",
            "stem_triple_click": "PreviousTrack",
            "stem_long_press": "ToggleNoiseMode",
            "start_minimized": false,
            "close_to_tray": true
        }"#;
        fs::write(&path, json).unwrap();

        let (loaded, outcome) = load_config_from(&path);
        assert_eq!(outcome, ConfigLoad::Loaded);
        assert!(!loaded.head_mouse_enabled);
        assert_eq!(loaded.head_mouse_speed, 20.0);
        assert_eq!(loaded.head_mouse_accel, 1.0);
        assert_eq!(loaded.head_mouse_smoothing, 0.35);
        assert_eq!(loaded.head_mouse_deadzone, 2.0);
        assert_eq!(loaded.head_mouse_mode, HeadMouseMode::Relative);
        remove_scratch(&path);
    }
}

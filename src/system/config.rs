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
    None,
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
}

fn default_true() -> bool {
    true
}

fn default_gesture_sensitivity() -> u8 {
    3
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
        save_config_to(&path, &cfg);

        let (loaded, outcome) = load_config_from(&path);
        assert_eq!(outcome, ConfigLoad::Loaded);
        assert_eq!(loaded, cfg);
        remove_scratch(&path);
    }
}

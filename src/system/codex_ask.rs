//! Answers Codex / ChatGPT `ask_question` / `AskUserQuestion` prompts by counting head nods.
//!
//! Codex discovers hooks from `~/.codex/hooks.json` or inline tables in
//! `~/.codex/config.toml` (see https://learn.chatgpt.com/docs/hooks).
//! This module manages installing and uninstalling our PreToolUse hook into
//! `~/.codex/hooks.json`.

use crate::system::agent_hooks::HookInstallStatus;
use serde_json::json;
use std::path::{Path, PathBuf};
use tracing::info;

/// Path of the user-level Codex hooks file: `~/.codex/hooks.json`.
pub fn codex_hooks_json_path() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    PathBuf::from(home).join(".codex").join("hooks.json")
}

/// The argv marker that identifies our hook entry in Codex configuration.
pub const HOOK_ARG_MARKER: &str = "--codex-ask-hook";

/// Matcher string targeting question tools in Codex / MCP.
pub const HOOK_MATCHER: &str =
    "request_user_input|request_user_input_async|AskUserQuestion|ask_question|ask_user_question";

/// Canonical hook entry for Codex `hooks.json`.
fn canonical_hook_entry(command: &str) -> serde_json::Value {
    let full_command = if command.contains(HOOK_ARG_MARKER) || command.contains("--zcode-ask-hook") {
        command.to_string()
    } else {
        format!("{} {}", command, HOOK_ARG_MARKER)
    };
    json!([{
        "type": "command",
        "command": full_command,
        "timeout": 180,
        "statusMessage": "等待头部手势作答..."
    }])
}

/// Merges our PreToolUse hook entry into a Codex `hooks.json` document.
pub fn install_hook_into(config: &mut serde_json::Value, command: &str) -> Result<bool, String> {
    let Some(root) = config.as_object_mut() else {
        return Err("config 根节点不是 JSON 对象".to_string());
    };

    let hooks = root
        .entry("hooks")
        .or_insert_with(|| json!({}))
        .as_object_mut()
        .ok_or_else(|| "`hooks` 已存在且不是对象;请先手工修正 ~/.codex/hooks.json".to_string())?;

    let pre = hooks
        .entry("PreToolUse")
        .or_insert_with(|| json!([]))
        .as_array_mut()
        .ok_or_else(|| "`hooks.PreToolUse` 不是数组".to_string())?;

    let canonical = canonical_hook_entry(command);
    if let Some(existing) = pre.iter_mut().find(|e| entry_is_ours(e)) {
        let mut changed = false;
        if existing.get("matcher").and_then(|m| m.as_str()) != Some(HOOK_MATCHER) {
            existing["matcher"] = json!(HOOK_MATCHER);
            changed = true;
        }
        if existing.get("hooks") != Some(&canonical) {
            existing["hooks"] = canonical;
            changed = true;
        }
        return Ok(changed);
    }

    pre.push(json!({
        "matcher": HOOK_MATCHER,
        "hooks": canonical,
    }));
    Ok(true)
}

/// Removes our hook entry; returns whether anything changed.
pub fn uninstall_hook_from(config: &mut serde_json::Value) -> bool {
    let Some(root) = config.as_object_mut() else {
        return false;
    };
    let Some(hooks) = root.get_mut("hooks").and_then(|h| h.as_object_mut()) else {
        return false;
    };
    let Some(pre) = hooks.get_mut("PreToolUse").and_then(|p| p.as_array_mut()) else {
        return false;
    };

    let before = pre.len();
    pre.retain(|entry| !entry_is_ours(entry));
    if pre.len() == before {
        return false;
    }
    if pre.is_empty() {
        hooks.remove("PreToolUse");
    }
    if hooks.is_empty() {
        root.remove("hooks");
    }
    true
}

fn hook_command_path() -> Result<String, String> {
    std::env::current_exe()
        .map_err(|e| format!("could not locate the running binary: {}", e))
        .map(|p| p.to_string_lossy().into_owned())
}

/// Whether a PreToolUse entry is ours (matcher match + our marker).
fn entry_is_ours(entry: &serde_json::Value) -> bool {
    let is_matcher = entry.get("matcher").and_then(|m| m.as_str()).is_some_and(|m| {
        m == HOOK_MATCHER
            || m.contains("request_user_input")
            || m.contains("AskUserQuestion")
            || m.contains("ask_question")
    });
    let has_marker = entry
        .get("hooks")
        .and_then(|h| h.as_array())
        .is_some_and(|hooks| {
            hooks.iter().any(|h| {
                h.get("command")
                    .and_then(|c| c.as_str())
                    .is_some_and(|c| c.contains(HOOK_ARG_MARKER) || c.contains("--zcode-ask-hook"))
            })
        });
    is_matcher && has_marker
}

/// The command of our hook inside an entry we own.
fn hook_command_of(entry: &serde_json::Value) -> Option<String> {
    entry
        .get("hooks")
        .and_then(|h| h.as_array())
        .and_then(|hooks| {
            hooks
                .iter()
                .filter(|h| {
                    h.get("command")
                        .and_then(|c| c.as_str())
                        .is_some_and(|c| c.contains(HOOK_ARG_MARKER) || c.contains("--zcode-ask-hook"))
                })
                .find_map(|h| h.get("command").and_then(|c| c.as_str()))
        })
        .map(String::from)
}

/// Reads the install status out of a parsed Codex `hooks.json` document.
pub fn hook_status_from(config: &serde_json::Value) -> HookInstallStatus {
    let pre = config
        .get("hooks")
        .and_then(|h| h.get("PreToolUse"))
        .and_then(|p| p.as_array());
    match pre.and_then(|pre| pre.iter().find(|e| entry_is_ours(e)).and_then(hook_command_of)) {
        Some(cmd) => HookInstallStatus::Installed(cmd),
        None => HookInstallStatus::NotInstalled,
    }
}

/// Current install status, for the agent management UI.
pub fn hook_status() -> HookInstallStatus {
    match std::fs::read_to_string(codex_hooks_json_path()) {
        Ok(text) => match serde_json::from_str::<serde_json::Value>(&text) {
            Ok(value) => hook_status_from(&value),
            Err(e) => HookInstallStatus::ConfigUnreadable(e.to_string()),
        },
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => HookInstallStatus::NotInstalled,
        Err(e) => HookInstallStatus::ConfigUnreadable(e.to_string()),
    }
}

/// Writes Codex `hooks.json` through a temp file + rename.
fn write_codex_config(path: &Path, pretty: &str) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let tmp = path.with_extension("json.rustpods-tmp");
    std::fs::write(&tmp, pretty).map_err(|e| format!("could not write {}: {}", tmp.display(), e))?;
    std::fs::rename(&tmp, path).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        format!("could not move {} into place: {}", tmp.display(), e)
    })?;
    Ok(())
}

/// Writes `~/.codex/hooks.json` with our hook merged in, backing up the
/// previous file if present.
pub fn install_hook() -> Result<(), String> {
    let path = codex_hooks_json_path();
    let existing = std::fs::read_to_string(&path).unwrap_or_else(|_| "{}".to_string());
    let mut root: serde_json::Value = serde_json::from_str(&existing)
        .map_err(|e| format!("{} is not valid JSON ({}); fix it by hand first", path.display(), e))?;

    let command = hook_command_path()?;
    let changed = install_hook_into(&mut root, &command)?;
    if !changed {
        info!("Codex hook already installed");
        return Ok(());
    }

    if path.exists() {
        let backup = path.with_extension("json.rustpods-bak");
        std::fs::copy(&path, &backup)
            .map_err(|e| format!("could not back up {} : {}", path.display(), e))?;
        info!("Backed up previous config to {}", backup.display());
    }
    let pretty = serde_json::to_string_pretty(&root).map_err(|e| e.to_string())?;
    write_codex_config(&path, &pretty)?;
    info!("Installed Codex hook (command: {} {})", command, HOOK_ARG_MARKER);
    Ok(())
}

/// Removes our hook entry from `~/.codex/hooks.json`.
pub fn uninstall_hook() -> Result<(), String> {
    let path = codex_hooks_json_path();
    let existing = std::fs::read_to_string(&path).unwrap_or_else(|_| "{}".to_string());
    let mut root: serde_json::Value = serde_json::from_str(&existing)
        .map_err(|e| format!("{} is not valid JSON ({}); fix it by hand first", path.display(), e))?;
    if uninstall_hook_from(&mut root) {
        let pretty = serde_json::to_string_pretty(&root).map_err(|e| e.to_string())?;
        write_codex_config(&path, &pretty)?;
        info!("Removed Codex hook");
    } else {
        info!("No Codex hook to remove");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    #[test]
    fn install_into_empty_creates_canonical_structure() {
        let mut config: Value = json!({});
        assert!(install_hook_into(&mut config, "/usr/bin/rustpods").unwrap());

        let pre = &config["hooks"]["PreToolUse"];
        assert_eq!(pre.as_array().unwrap().len(), 1);
        assert_eq!(pre[0]["matcher"], HOOK_MATCHER);
        assert_eq!(
            pre[0]["hooks"][0]["command"],
            "/usr/bin/rustpods --codex-ask-hook"
        );
        assert_eq!(pre[0]["hooks"][0]["type"], "command");
        assert_eq!(pre[0]["hooks"][0]["timeout"], 180);
    }

    #[test]
    fn install_is_idempotent_and_preserves_siblings() {
        let mut config: Value = json!({
            "description": "My workspace",
            "hooks": {
                "SessionStart": [{
                    "matcher": "startup",
                    "hooks": [{"type": "command", "command": "echo start"}]
                }],
                "PreToolUse": [{
                    "matcher": "Bash",
                    "hooks": [{"type": "command", "command": "guard-bash.sh"}]
                }]
            }
        });

        assert!(install_hook_into(&mut config, "/opt/rustpods").unwrap());
        // Siblings untouched
        assert_eq!(config["hooks"]["SessionStart"].as_array().unwrap().len(), 1);
        assert_eq!(config["hooks"]["PreToolUse"].as_array().unwrap().len(), 2);
        assert_eq!(config["description"], "My workspace");

        // Repeated install is no-op
        assert!(!install_hook_into(&mut config, "/opt/rustpods").unwrap());
        assert_eq!(config["hooks"]["PreToolUse"].as_array().unwrap().len(), 2);

        // Path update updates command in-place
        assert!(install_hook_into(&mut config, "/usr/local/bin/rustpods").unwrap());
        assert_eq!(config["hooks"]["PreToolUse"].as_array().unwrap().len(), 2);
        let cmd = config["hooks"]["PreToolUse"]
            .as_array()
            .unwrap()
            .iter()
            .find(|e| entry_is_ours(e))
            .unwrap()["hooks"][0]["command"]
            .as_str()
            .unwrap();
        assert_eq!(cmd, "/usr/local/bin/rustpods --codex-ask-hook");
    }

    #[test]
    fn uninstall_removes_only_our_entry_and_preserves_siblings() {
        let mut config: Value = json!({
            "hooks": {
                "PreToolUse": [
                    {
                        "matcher": "Bash",
                        "hooks": [{"type": "command", "command": "guard-bash.sh"}]
                    },
                    {
                        "matcher": HOOK_MATCHER,
                        "hooks": [{
                            "type": "command",
                            "command": "/usr/bin/rustpods --codex-ask-hook"
                        }]
                    }
                ]
            }
        });

        assert!(uninstall_hook_from(&mut config));
        assert_eq!(config["hooks"]["PreToolUse"].as_array().unwrap().len(), 1);
        assert_eq!(config["hooks"]["PreToolUse"][0]["matcher"], "Bash");

        // Repeated uninstall returns false
        assert!(!uninstall_hook_from(&mut config));
    }

    #[test]
    fn uninstall_cleans_up_empty_hooks() {
        let mut config: Value = json!({
            "hooks": {
                "PreToolUse": [{
                    "matcher": HOOK_MATCHER,
                    "hooks": [{
                        "type": "command",
                        "command": "/usr/bin/rustpods --codex-ask-hook"
                    }]
                }]
            }
        });

        assert!(uninstall_hook_from(&mut config));
        assert!(config.get("hooks").is_none());
    }

    #[test]
    fn hook_status_reports_correctly() {
        let config: Value = json!({
            "hooks": {
                "PreToolUse": [{
                    "matcher": HOOK_MATCHER,
                    "hooks": [{
                        "type": "command",
                        "command": "/usr/bin/rustpods --codex-ask-hook"
                    }]
                }]
            }
        });

        assert_eq!(
            hook_status_from(&config),
            HookInstallStatus::Installed("/usr/bin/rustpods --codex-ask-hook".to_string())
        );

        let empty: Value = json!({});
        assert_eq!(hook_status_from(&empty), HookInstallStatus::NotInstalled);
    }
}

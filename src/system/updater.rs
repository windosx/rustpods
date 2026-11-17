use serde::Deserialize;
use std::path::PathBuf;
use std::process::Command;
use tracing::{debug, error, info};

pub const REPO_OWNER: &str = "windosx";
pub const REPO_NAME: &str = "rustpods";
pub const GITHUB_API_LATEST_RELEASE: &str =
    "https://api.github.com/repos/windosx/rustpods/releases/latest";

/// Upper bound for the in-app installer. It downloads a release tarball; a
/// hang (network stall, sudo prompt in a non-interactive child) must not leave
/// the UI stuck in "Updating" forever.
const INSTALL_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(300);

/// Status of update check.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum UpdateState {
    #[default]
    Idle,
    Checking,
    UpToDate {
        version: String,
    },
    UpdateAvailable {
        current_version: String,
        latest_version: String,
        release_url: String,
        changelog: String,
        is_package_managed: bool,
        /// False for system prefixes where the in-app installer would need
        /// root or would fight the package database.
        can_self_update: bool,
    },
    /// The in-app updater is downloading/installing the release.
    Installing {
        latest_version: String,
    },
    /// The update finished; the running process still uses the old binary
    /// until it is restarted.
    Updated {
        version: String,
    },
    Error(String),
}

#[derive(Debug, Deserialize)]
struct GitHubRelease {
    tag_name: String,
    html_url: String,
    #[serde(default)]
    body: Option<String>,
}

/// Parses a semver-like string into (major, minor, patch).
pub fn parse_version(v: &str) -> Option<(u64, u64, u64)> {
    let s = v.trim().trim_start_matches('v');
    let mut parts = s.split('.');
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next()?.parse().ok()?;
    let patch_part = parts.next().unwrap_or("0");
    let patch = patch_part.split('-').next()?.parse().ok()?;
    Some((major, minor, patch))
}

/// Returns true if `latest` is strictly newer than `current`.
pub fn is_newer_version(current: &str, latest: &str) -> bool {
    match (parse_version(current), parse_version(latest)) {
        (Some(c), Some(l)) => l > c,
        _ => false,
    }
}

/// Checks if the current binary is installed in a system package directory (e.g. /usr/bin).
pub fn is_package_managed() -> bool {
    if let Ok(exe) = std::env::current_exe() {
        let path = exe.to_string_lossy();
        path.starts_with("/usr/bin") && !path.starts_with("/usr/local/bin")
    } else {
        false
    }
}

/// First `rustpods` on PATH, mirroring `command -v rustpods` in update.sh.
fn path_rustpods() -> Option<PathBuf> {
    std::env::var_os("PATH").and_then(|value| {
        std::env::split_paths(&value)
            .map(|dir| dir.join("rustpods"))
            .find(|candidate| candidate.is_file())
    })
}

/// Binary the updater actually replaces. `update.sh` picks its install prefix
/// from the first `rustpods` on PATH (`command -v rustpods`), so restarting
/// that path is what makes the new version take effect. A dev build running
/// from `target/release` behind a `~/.local/bin` symlink is covered by the
/// PATH lookup; the `~/.local/bin` fallback handles a PATH that no longer
/// contains it.
pub fn restart_binary_path() -> PathBuf {
    if let Some(path) = path_rustpods() {
        return path;
    }
    if let Some(home) = std::env::var_os("HOME") {
        let local = PathBuf::from(home).join(".local/bin/rustpods");
        if local.is_file() {
            return local;
        }
    }
    std::env::current_exe().unwrap_or_else(|_| PathBuf::from("rustpods"))
}

/// Whether the in-app update button can safely replace this installation.
/// Package-managed and system-prefix installs require root and/or would fight
/// the distribution's package database, so they keep the manual hint instead.
///
/// The official updater only writes `~/.local` for user installs (it uses
/// `/usr/local` or the package manager otherwise). A custom `~/bin/rustpods`
/// entry would be left untouched by that script, so it does not qualify
/// either: the update would appear to succeed but the restart would run the
/// old binary.
pub fn can_self_update() -> bool {
    can_self_update_with(
        path_rustpods(),
        std::env::var_os("HOME").map(PathBuf::from),
        is_package_managed(),
    )
}

/// Pure decision behind [`can_self_update`], split out so the prefix rules can
/// be tested without touching PATH or the real installation.
fn can_self_update_with(
    path_rustpods: Option<PathBuf>,
    home: Option<PathBuf>,
    package_managed: bool,
) -> bool {
    if package_managed {
        return false;
    }
    let Some(home) = home else {
        return false;
    };
    match path_rustpods {
        Some(path) => path.starts_with(home.join(".local")),
        // No PATH copy: update.sh installs to ~/.local/bin/rustpods, and
        // restart_binary_path() picks that up after the update.
        None => true,
    }
}

/// Runs the same installer as `rustpods --update`, capturing its output so a
/// failure can be shown in the preferences page. The child handles the
/// package-manager guard and the actual download itself.
pub async fn install_update() -> Result<(), String> {
    if !can_self_update() {
        return Err(
            "当前安装方式不支持应用内升级，请使用发行版包管理器或更新脚本".to_string(),
        );
    }

    let exe = std::env::current_exe().map_err(|e| format!("无法定位当前程序: {e}"))?;
    let mut cmd = tokio::process::Command::new(exe);
    cmd.arg("--update")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true);

    let output = tokio::time::timeout(INSTALL_TIMEOUT, cmd.output())
        .await
        .map_err(|_| "升级超时，请检查网络后重试".to_string())?
        .map_err(|e| format!("无法启动升级程序: {e}"))?;

    if output.status.success() {
        info!("In-app update finished successfully");
        return Ok(());
    }

    // The installer prints curl/tar/sudo failures on stderr; keep a bounded
    // excerpt so the card cannot grow into a wall of text.
    let stderr = String::from_utf8_lossy(&output.stderr);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let detail = if stderr.trim().is_empty() {
        stdout.trim()
    } else {
        stderr.trim()
    };
    let detail: String = detail.chars().take(500).collect();
    if detail.is_empty() {
        Err(format!("升级失败（退出码 {:?}）", output.status.code()))
    } else {
        Err(format!("升级失败: {detail}"))
    }
}

/// Asynchronously checks GitHub Releases for a newer version.
pub async fn check_for_update() -> Result<Option<(String, String, String)>, String> {
    info!("Checking for RustPods updates via GitHub API...");

    let output = tokio::process::Command::new("curl")
        .args([
            "-fsSL",
            "-m",
            "10",
            "-H",
            "Accept: application/vnd.github.v3+json",
            "-H",
            "User-Agent: rustpods-updater",
            GITHUB_API_LATEST_RELEASE,
        ])
        .output()
        .await
        .map_err(|e| format!("无法执行 curl 检查更新: {e}"))?;

    if !output.status.success() {
        let err = String::from_utf8_lossy(&output.stderr);
        error!("GitHub API check failed: {}", err);
        return Err("连接 GitHub Releases API 失败".to_string());
    }

    let release: GitHubRelease = serde_json::from_slice(&output.stdout)
        .map_err(|e| format!("解析 GitHub API 响应失败: {e}"))?;

    let latest_version = release.tag_name.trim_start_matches('v').to_string();
    let current_version = env!("CARGO_PKG_VERSION");

    debug!(
        "Update check: current={}, latest={}",
        current_version, latest_version
    );

    if is_newer_version(current_version, &latest_version) {
        Ok(Some((
            latest_version,
            release.html_url,
            release.body.unwrap_or_default(),
        )))
    } else {
        Ok(None)
    }
}

/// Opens the given URL in the default web browser.
pub fn open_browser(url: &str) -> std::io::Result<()> {
    Command::new("xdg-open").arg(url).spawn().map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_version() {
        assert_eq!(parse_version("0.1.0"), Some((0, 1, 0)));
        assert_eq!(parse_version("v0.1.0"), Some((0, 1, 0)));
        assert_eq!(parse_version("1.2.3-beta"), Some((1, 2, 3)));
        assert_eq!(parse_version("v2.0"), Some((2, 0, 0)));
        assert_eq!(parse_version("invalid"), None);
    }

    #[test]
    fn test_is_newer_version() {
        assert!(is_newer_version("0.1.0", "0.2.0"));
        assert!(is_newer_version("0.1.0", "v0.1.1"));
        assert!(is_newer_version("0.1.9", "0.2.0"));
        assert!(is_newer_version("1.0.0", "2.0.0"));

        assert!(!is_newer_version("0.1.0", "0.1.0"));
        assert!(!is_newer_version("0.1.0", "v0.1.0"));
        assert!(!is_newer_version("0.2.0", "0.1.9"));
        assert!(!is_newer_version("0.1.0", "0.1.0-alpha"));
    }

    #[test]
    fn self_update_is_limited_to_user_local_installs() {
        let home = PathBuf::from("/home/alice");

        assert!(can_self_update_with(
            Some(home.join(".local/bin/rustpods")),
            Some(home.clone()),
            false
        ));
        // No PATH entry: update.sh creates ~/.local/bin/rustpods.
        assert!(can_self_update_with(None, Some(home.clone()), false));

        // Custom ~/bin and system prefixes are not touched by update.sh's
        // ~/.local install, so a button there would restart the old binary.
        assert!(!can_self_update_with(
            Some(home.join("bin/rustpods")),
            Some(home.clone()),
            false
        ));
        assert!(!can_self_update_with(
            Some(PathBuf::from("/usr/local/bin/rustpods")),
            Some(home.clone()),
            false
        ));
        assert!(!can_self_update_with(
            Some(PathBuf::from("/usr/bin/rustpods")),
            Some(home.clone()),
            false
        ));
        // Package-managed wins over everything.
        assert!(!can_self_update_with(
            Some(home.join(".local/bin/rustpods")),
            Some(home),
            true
        ));
    }
}

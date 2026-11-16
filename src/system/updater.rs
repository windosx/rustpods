use serde::Deserialize;
use std::process::Command;
use tracing::{debug, error, info};

pub const REPO_OWNER: &str = "windosx";
pub const REPO_NAME: &str = "rustpods";
pub const GITHUB_API_LATEST_RELEASE: &str =
    "https://api.github.com/repos/windosx/rustpods/releases/latest";

/// Status of update check.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UpdateState {
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
    },
    Error(String),
}

impl Default for UpdateState {
    fn default() -> Self {
        Self::Idle
    }
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
}

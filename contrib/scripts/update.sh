#!/usr/bin/env bash
set -euo pipefail

# RustPods In-Place / Remote Updater for Linux
# Fetches the latest release from GitHub, compares version, and installs.

REPO="windosx/rustpods"
API_URL="https://api.github.com/repos/${REPO}/releases/latest"

echo "==> Checking for latest RustPods release..."

if ! command -v curl >/dev/null 2>&1; then
    echo "Error: 'curl' is required to check and download updates." >&2
    exit 1
fi

RELEASE_JSON=$(curl -fsSL -H "Accept: application/vnd.github.v3+json" -H "User-Agent: rustpods-updater" "${API_URL}" 2>/dev/null || true)

if [ -z "${RELEASE_JSON}" ]; then
    echo "Error: Failed to reach GitHub Releases API (${API_URL}). Please check your network connection." >&2
    exit 1
fi

LATEST_TAG=$(echo "${RELEASE_JSON}" | grep -m1 '"tag_name":' | sed -E 's/.*"tag_name":[[:space:]]*"([^"]+)".*/\1/')
LATEST_VERSION="${LATEST_TAG#v}"

if [ -z "${LATEST_TAG}" ]; then
    echo "Error: Could not determine latest version from GitHub API response." >&2
    exit 1
fi

CURRENT_VERSION=""
if command -v rustpods >/dev/null 2>&1; then
    CURRENT_VERSION=$(rustpods --version 2>/dev/null | awk '{print $2}' || true)
fi

echo "  Current version: ${CURRENT_VERSION:-not installed}"
echo "  Latest release:  ${LATEST_VERSION} (${LATEST_TAG})"

if [ -n "${CURRENT_VERSION}" ] && [ "${CURRENT_VERSION}" = "${LATEST_VERSION}" ]; then
    echo "==> RustPods is already up to date (v${CURRENT_VERSION})."
    exit 0
fi

ARCH=$(uname -m)
ASSET_NAME="rustpods-v${LATEST_VERSION}-linux-${ARCH}.tar.gz"
DOWNLOAD_URL="https://github.com/${REPO}/releases/download/${LATEST_TAG}/${ASSET_NAME}"
CHECKSUMS_URL="https://github.com/${REPO}/releases/download/${LATEST_TAG}/SHA256SUMS"

echo "==> Downloading update: ${ASSET_NAME}..."

TMP_DIR=$(mktemp -d /tmp/rustpods-update.XXXXXX)
trap 'rm -rf "${TMP_DIR}"' EXIT

cd "${TMP_DIR}"
if ! curl -fsSL -o "${ASSET_NAME}" "${DOWNLOAD_URL}"; then
    echo "Error: Failed to download release asset from ${DOWNLOAD_URL}" >&2
    exit 1
fi

if curl -fsSL -o "SHA256SUMS" "${CHECKSUMS_URL}" 2>/dev/null; then
    echo "==> Verifying SHA256 checksum..."
    if grep "${ASSET_NAME}" SHA256SUMS | sha256sum -c --status 2>/dev/null; then
        echo "  Checksum OK."
    else
        echo "Warning: Checksum verification failed or not found in SHA256SUMS." >&2
    fi
fi

echo "==> Extracting update archive..."
tar -xzf "${ASSET_NAME}"
EXTRACTED_DIR="rustpods-v${LATEST_VERSION}-linux-${ARCH}"

if [ ! -d "${EXTRACTED_DIR}" ]; then
    echo "Error: Archive did not contain expected directory '${EXTRACTED_DIR}'." >&2
    exit 1
fi

cd "${EXTRACTED_DIR}"

INSTALL_PREFIX=""
if [ "$(id -u)" -eq 0 ]; then
    INSTALL_PREFIX="/usr/local"
elif command -v rustpods >/dev/null 2>&1; then
    BIN_PATH=$(command -v rustpods)
    case "${BIN_PATH}" in
        /usr/bin/rustpods)
            echo "Notice: Current rustpods is in /usr/bin (system package manager)."
            echo "        Installing to /usr/local will take precedence in PATH."
            INSTALL_PREFIX="/usr/local"
            ;;
        /usr/local/bin/rustpods)
            INSTALL_PREFIX="/usr/local"
            ;;
        *)
            INSTALL_PREFIX="${HOME}/.local"
            ;;
    esac
else
    INSTALL_PREFIX="${HOME}/.local"
fi

echo "==> Installing to: ${INSTALL_PREFIX}"
if [ "${INSTALL_PREFIX}" = "/usr/local" ] && [ "$(id -u)" -ne 0 ]; then
    echo "Root privileges required to install to /usr/local. Prompting with sudo:"
    sudo PREFIX="${INSTALL_PREFIX}" ./install.sh
else
    PREFIX="${INSTALL_PREFIX}" ./install.sh
fi

if command -v systemctl >/dev/null 2>&1; then
    if systemctl --user is-active --quiet rustpods.service 2>/dev/null; then
        echo "==> Restarting rustpods systemd service..."
        systemctl --user restart rustpods.service || true
    fi
fi

echo "==> Update to RustPods v${LATEST_VERSION} completed successfully!"

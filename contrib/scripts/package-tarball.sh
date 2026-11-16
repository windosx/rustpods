#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT_DIR="$(cd "${SCRIPT_DIR}/../.." && pwd)"

VERSION="${1:-$(grep -m1 '^version = ' "${ROOT_DIR}/Cargo.toml" | cut -d '"' -f2)}"
ARCH="${2:-$(uname -m)}"
TARGET_NAME="rustpods-v${VERSION}-linux-${ARCH}"
DIST_DIR="${ROOT_DIR}/dist"
STAGE_DIR="${DIST_DIR}/${TARGET_NAME}"

echo "==> Packaging ${TARGET_NAME}.tar.gz"

mkdir -p "${STAGE_DIR}"
rm -rf "${STAGE_DIR:?}"/*

# 1. Binary
if [ ! -f "${ROOT_DIR}/target/release/rustpods" ]; then
    echo "target/release/rustpods not found, running cargo build --release..."
    (cd "${ROOT_DIR}" && cargo build --release)
fi
cp "${ROOT_DIR}/target/release/rustpods" "${STAGE_DIR}/"
strip "${STAGE_DIR}/rustpods" 2>/dev/null || true

# 2. Scripts
cp "${SCRIPT_DIR}/install.sh" "${STAGE_DIR}/"
cp "${SCRIPT_DIR}/uninstall.sh" "${STAGE_DIR}/"
cp "${SCRIPT_DIR}/update.sh" "${STAGE_DIR}/"
chmod +x "${STAGE_DIR}/install.sh" "${STAGE_DIR}/uninstall.sh" "${STAGE_DIR}/update.sh"

# 3. Contrib
mkdir -p "${STAGE_DIR}/contrib/systemd" "${STAGE_DIR}/contrib/udev" "${STAGE_DIR}/contrib/pipewire"
cp "${ROOT_DIR}/contrib/io.github.rustpods.RustPods.desktop" "${STAGE_DIR}/contrib/"
cp "${ROOT_DIR}/contrib/systemd/rustpods.service" "${STAGE_DIR}/contrib/systemd/"
cp "${ROOT_DIR}/contrib/udev/70-rustpods.rules" "${STAGE_DIR}/contrib/udev/"
cp "${ROOT_DIR}/contrib/pipewire/99-rustpods-hrtf.conf" "${STAGE_DIR}/contrib/pipewire/"

# 4. Assets (Icons)
mkdir -p "${STAGE_DIR}/assets"
cp -r "${ROOT_DIR}/assets/icons" "${STAGE_DIR}/assets/"

# 5. Metadata / Licenses
cp "${ROOT_DIR}/LICENSE" "${STAGE_DIR}/"
cp "${ROOT_DIR}/NOTICE" "${STAGE_DIR}/"
cp "${ROOT_DIR}/README.md" "${STAGE_DIR}/"
cp "${ROOT_DIR}/README_zh.md" "${STAGE_DIR}/"

# 6. Tarball
mkdir -p "${DIST_DIR}"
tar -czvf "${DIST_DIR}/${TARGET_NAME}.tar.gz" -C "${DIST_DIR}" "${TARGET_NAME}"

# Generate SHA256
(cd "${DIST_DIR}" && sha256sum "${TARGET_NAME}.tar.gz" > "${TARGET_NAME}.tar.gz.sha256")

echo "==> Successfully created: ${DIST_DIR}/${TARGET_NAME}.tar.gz"

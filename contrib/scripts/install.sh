#!/usr/bin/env bash
set -euo pipefail

# Determine script directory
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT_DIR="$(cd "${SCRIPT_DIR}/../.." 2>/dev/null && pwd || echo "${SCRIPT_DIR}")"

# Find binary: prefer local ./rustpods (in tarball), then target/release/rustpods (in repo)
if [ -f "${SCRIPT_DIR}/rustpods" ]; then
    BIN_SRC="${SCRIPT_DIR}/rustpods"
elif [ -f "${ROOT_DIR}/rustpods" ]; then
    BIN_SRC="${ROOT_DIR}/rustpods"
elif [ -f "${ROOT_DIR}/target/release/rustpods" ]; then
    BIN_SRC="${ROOT_DIR}/target/release/rustpods"
else
    echo "Error: rustpods binary not found in '${SCRIPT_DIR}' or '${ROOT_DIR}/target/release'." >&2
    echo "Please build the project first ('cargo build --release') or run this from a release archive." >&2
    exit 1
fi

# Locate assets and contrib directories
if [ -d "${ROOT_DIR}/assets" ]; then
    ASSETS_DIR="${ROOT_DIR}/assets"
elif [ -d "${SCRIPT_DIR}/assets" ]; then
    ASSETS_DIR="${SCRIPT_DIR}/assets"
else
    echo "Error: assets directory not found." >&2
    exit 1
fi

if [ -d "${ROOT_DIR}/contrib" ]; then
    CONTRIB_DIR="${ROOT_DIR}/contrib"
elif [ -d "${SCRIPT_DIR}/contrib" ]; then
    CONTRIB_DIR="${SCRIPT_DIR}/contrib"
else
    echo "Error: contrib directory not found." >&2
    exit 1
fi

# Determine install prefix: default to /usr/local if root, ~/.local if non-root
if [ "$(id -u)" -eq 0 ]; then
    PREFIX="${PREFIX:-/usr/local}"
    IS_SYSTEM=1
else
    PREFIX="${PREFIX:-${HOME}/.local}"
    IS_SYSTEM=0
fi

echo "==> Installing RustPods to: ${PREFIX}"

# Install binary
BIN_DIR="${PREFIX}/bin"
mkdir -p "${BIN_DIR}"
install -Dm755 "${BIN_SRC}" "${BIN_DIR}/rustpods"
echo "  Installed: ${BIN_DIR}/rustpods"

# Install Desktop Entry
DESKTOP_DIR="${PREFIX}/share/applications"
mkdir -p "${DESKTOP_DIR}"
install -Dm644 "${CONTRIB_DIR}/io.github.rustpods.RustPods.desktop" \
    "${DESKTOP_DIR}/io.github.rustpods.RustPods.desktop"
echo "  Installed: ${DESKTOP_DIR}/io.github.rustpods.RustPods.desktop"

# Install Icons
ICONS_BASE="${PREFIX}/share/icons/hicolor"
if [ -f "${ASSETS_DIR}/icons/hicolor/scalable/apps/rustpods.svg" ]; then
    install -Dm644 "${ASSETS_DIR}/icons/hicolor/scalable/apps/rustpods.svg" \
        "${ICONS_BASE}/scalable/apps/io.github.rustpods.RustPods.svg"
fi

for size in 16 24 32 48 64 128 256 512 1024; do
    icon_file="${ASSETS_DIR}/icons/hicolor/${size}x${size}/apps/rustpods.png"
    if [ -f "${icon_file}" ]; then
        install -Dm644 "${icon_file}" \
            "${ICONS_BASE}/${size}x${size}/apps/io.github.rustpods.RustPods.png"
    fi
done
echo "  Installed icons to: ${ICONS_BASE}"

# Install PipeWire HRTF configuration template
DATA_DIR="${PREFIX}/share/rustpods"
mkdir -p "${DATA_DIR}/pipewire"
if [ -f "${CONTRIB_DIR}/pipewire/99-rustpods-hrtf.conf" ]; then
    install -Dm644 "${CONTRIB_DIR}/pipewire/99-rustpods-hrtf.conf" \
        "${DATA_DIR}/pipewire/99-rustpods-hrtf.conf"
    echo "  Installed PipeWire HRTF config template to: ${DATA_DIR}/pipewire"
fi

# Install systemd user service
if [ "${IS_SYSTEM}" -eq 1 ] || [ "${PREFIX}" != "${HOME}/.local" ]; then
    SYSTEMD_USER_DIR="${PREFIX}/lib/systemd/user"
    mkdir -p "${SYSTEMD_USER_DIR}"
    sed "s|/usr/bin/rustpods|${BIN_DIR}/rustpods|" \
        "${CONTRIB_DIR}/systemd/rustpods.service" > "${SYSTEMD_USER_DIR}/rustpods.service"
    chmod 644 "${SYSTEMD_USER_DIR}/rustpods.service"
    echo "  Installed systemd user service: ${SYSTEMD_USER_DIR}/rustpods.service"
else
    USER_SYSTEMD_DIR="${HOME}/.config/systemd/user"
    mkdir -p "${USER_SYSTEMD_DIR}"
    sed "s|/usr/bin/rustpods|${BIN_DIR}/rustpods|" \
        "${CONTRIB_DIR}/systemd/rustpods.service" > "${USER_SYSTEMD_DIR}/rustpods.service"
    chmod 644 "${USER_SYSTEMD_DIR}/rustpods.service"
    echo "  Installed systemd user service: ${USER_SYSTEMD_DIR}/rustpods.service"
fi

# Install udev rule (requires root)
if [ "${IS_SYSTEM}" -eq 1 ]; then
    UDEV_RULES_DIR="/etc/udev/rules.d"
    if [ -d "/usr/lib/udev/rules.d" ] && [ "${PREFIX}" = "/usr" ]; then
        UDEV_RULES_DIR="/usr/lib/udev/rules.d"
    fi
    mkdir -p "${UDEV_RULES_DIR}"
    install -Dm644 "${CONTRIB_DIR}/udev/70-rustpods.rules" "${UDEV_RULES_DIR}/70-rustpods.rules"
    echo "  Installed udev rule: ${UDEV_RULES_DIR}/70-rustpods.rules"
    if command -v udevadm >/dev/null 2>&1; then
        udevadm control --reload-rules 2>/dev/null || true
        udevadm trigger /dev/uinput 2>/dev/null || true
    fi
else
    echo "  [Note] Skipping udev rule install (requires root)."
    echo "         To enable Head Mouse without password prompts, run:"
    echo "         sudo cp ${CONTRIB_DIR}/udev/70-rustpods.rules /etc/udev/rules.d/"
    echo "         sudo udevadm control --reload-rules && sudo udevadm trigger /dev/uinput"
fi

# Update desktop & icon caches
if command -v update-desktop-database >/dev/null 2>&1; then
    update-desktop-database "${DESKTOP_DIR}" 2>/dev/null || true
fi
if command -v gtk-update-icon-cache >/dev/null 2>&1; then
    gtk-update-icon-cache -f -t "${ICONS_BASE}" 2>/dev/null || true
fi

echo "==> Installation complete!"
echo "    Run 'rustpods' to start the application."
echo "    Or run 'systemctl --user enable --now rustpods' for background tray service."

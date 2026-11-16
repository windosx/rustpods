#!/usr/bin/env bash
set -euo pipefail

# Determine prefix
if [ "$(id -u)" -eq 0 ]; then
    PREFIX="${PREFIX:-/usr/local}"
    IS_SYSTEM=1
else
    PREFIX="${PREFIX:-${HOME}/.local}"
    IS_SYSTEM=0
fi

echo "==> Uninstalling RustPods from: ${PREFIX}"

# Stop systemd service if running
if command -v systemctl >/dev/null 2>&1; then
    systemctl --user stop rustpods.service 2>/dev/null || true
    systemctl --user disable rustpods.service 2>/dev/null || true
fi

# Remove binary
rm -f "${PREFIX}/bin/rustpods"
echo "  Removed: ${PREFIX}/bin/rustpods"

# Remove desktop entry
rm -f "${PREFIX}/share/applications/io.github.rustpods.RustPods.desktop"
rm -f "${PREFIX}/share/applications/rustpods.desktop"

# Remove icons
rm -f "${PREFIX}/share/icons/hicolor/scalable/apps/io.github.rustpods.RustPods.svg"
rm -f "${PREFIX}/share/icons/hicolor/scalable/apps/rustpods.svg"
for size in 16 24 32 48 64 128 256 512 1024; do
    rm -f "${PREFIX}/share/icons/hicolor/${size}x${size}/apps/io.github.rustpods.RustPods.png"
    rm -f "${PREFIX}/share/icons/hicolor/${size}x${size}/apps/rustpods.png"
done

# Remove data directory
rm -rf "${PREFIX}/share/rustpods"

# Remove systemd service
if [ "${IS_SYSTEM}" -eq 1 ] || [ "${PREFIX}" != "${HOME}/.local" ]; then
    rm -f "${PREFIX}/lib/systemd/user/rustpods.service"
    rm -f "/etc/udev/rules.d/70-rustpods.rules"
    rm -f "/usr/lib/udev/rules.d/70-rustpods.rules"
    if command -v udevadm >/dev/null 2>&1; then
        udevadm control --reload-rules 2>/dev/null || true
        udevadm trigger /dev/uinput 2>/dev/null || true
    fi
else
    rm -f "${HOME}/.config/systemd/user/rustpods.service"
fi

# Update desktop & icon caches
if command -v update-desktop-database >/dev/null 2>&1; then
    update-desktop-database "${PREFIX}/share/applications" 2>/dev/null || true
fi
if command -v gtk-update-icon-cache >/dev/null 2>&1; then
    gtk-update-icon-cache -f -t "${PREFIX}/share/icons/hicolor" 2>/dev/null || true
fi

echo "==> RustPods has been successfully uninstalled."

#!/usr/bin/env bash
# Builds one .deb from release binaries already built on the target release (Ubuntu 24.04 or later, Debian 13), with
# the same files as dist/arch/PKGBUILD: the daemon, session and mount; the window is andromeda-link-app. The daemon and the
# mount start with the graphical session here, since only Andromeda has andromeda-session.target.
# Usage: build-deb.sh TARGET_RELEASE_DIR OUT_DIR
set -euo pipefail
ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)
BIN=${1:?target release dir}
OUT=${2:?output dir}
VERSION=${LINK_VERSION:-"0~r$(git -C "$ROOT" rev-list --count HEAD).g$(git -C "$ROOT" rev-parse --short HEAD)"}
STAGE=$(mktemp -d)
trap 'rm -rf "$STAGE"' EXIT
P=$STAGE/andromeda-link

for b in andromeda-linkd andromeda-link-mount andromeda-link-session; do
  install -Dm755 "$BIN/$b" "$P/usr/bin/$b"
done
U=$P/usr/lib/systemd/user
for s in andromeda-linkd andromeda-link-mount andromeda-link-session; do
  install -Dm644 "$ROOT/dist/$s.service" "$U/$s.service"
  install -d "$U/graphical-session.target.wants"
  ln -s "../$s.service" "$U/graphical-session.target.wants/$s.service"
done
# systemd 255 (Ubuntu 24.04) cannot drop capabilities in a user unit and exits 218/CAPABILITIES; each of these options
# drops some. A user process has none to drop, and NoNewPrivileges still stops it gaining any.
sed -i -E '/^(CapabilityBoundingSet|ProtectClock|ProtectKernelModules|ProtectKernelLogs|PrivateDevices)=/d' \
  "$U/andromeda-linkd.service"
install -Dm644 "$ROOT/dist/org.andromeda.Link1.service" "$P/usr/share/dbus-1/services/org.andromeda.Link1.service"
install -Dm644 "$ROOT/api/org.andromeda.Link1.xml" "$P/usr/share/dbus-1/interfaces/org.andromeda.Link1.xml"
install -Dm644 "$ROOT/dist/andromeda-link.ufw" "$P/etc/ufw/applications.d/andromeda-link"
install -Dm644 "$ROOT/dist/andromeda-link.firewalld.xml" "$P/usr/lib/firewalld/services/andromeda-link.xml"
install -Dm644 "$ROOT/dist/andromeda-link-localsend.firewalld.xml" \
  "$P/usr/lib/firewalld/services/andromeda-link-localsend.xml"
install -Dm644 "$ROOT/dist/60-andromeda-link-bluetooth.conf" \
  "$P/usr/share/wireplumber/wireplumber.conf.d/60-andromeda-link-bluetooth.conf"
install -Dm644 "$ROOT/dist/60-andromeda-link-uinput.rules" "$P/usr/lib/udev/rules.d/60-andromeda-link-uinput.rules"
install -Dm644 "$ROOT/LICENSE" "$P/usr/share/doc/andromeda-link/copyright"
cp -r "$ROOT/dist/icons" "$P/usr/share/"

install -d "$P/DEBIAN"
# andromeda-link-app below 1 is the old Qt app package; 2026.x is the Andromeda Link window.
cat > "$P/DEBIAN/control" << EOF
Package: andromeda-link
Version: $VERSION
Architecture: amd64
Maintainer: Andromeda <sitish@akintechnologies.com>
Depends: libc6, libgcc-s1, libxkbcommon0, dbus, fuse3, gstreamer1.0-plugins-base, gstreamer1.0-plugins-good, gstreamer1.0-plugins-bad, gstreamer1.0-pipewire
Replaces: andromeda-link-app (<< 1)
Breaks: andromeda-link-app (<< 1)
Recommends: andromeda-link-app
Suggests: bluez, network-manager
Description: Andromeda Link: phone and desktop continuity
 Clipboard, notifications, media, files, texts, and screen mirroring between an Android phone and the desktop.
EOF
echo /etc/ufw/applications.d/andromeda-link > "$P/DEBIAN/conffiles"
# Phones reach the daemon only through the firewall; open whichever one runs, as pkg/andromeda-link.install does.
cat > "$P/DEBIAN/postinst" << 'EOF'
#!/bin/sh
set -e
if command -v ufw > /dev/null && ufw status 2> /dev/null | grep -q '^Status: active'; then
  ufw allow 'Andromeda Link' > /dev/null 2>&1 || echo "ufw: phones cannot pair until you run: sudo ufw allow 'Andromeda Link'"
fi
if command -v firewall-cmd > /dev/null && firewall-cmd --state > /dev/null 2>&1; then
  firewall-cmd --reload > /dev/null 2>&1 || true
  firewall-cmd --permanent --add-service=andromeda-link > /dev/null 2>&1 && firewall-cmd --reload > /dev/null 2>&1 ||
    echo "firewalld: phones cannot pair until you run: sudo firewall-cmd --permanent --add-service=andromeda-link"
fi
EOF
chmod 755 "$P/DEBIAN/postinst"
mkdir -p "$OUT"
dpkg-deb --root-owner-group --build "$P" "$OUT/andromeda-link_${VERSION}_amd64.deb"

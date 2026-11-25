#!/bin/bash
# Builds every Link package for a release into target/dist: the Android APK (signed when
# ~/.config/andromeda-link/signing.env exists), the Ubuntu 24.04 .deb and Fedora RPM (each built in
# its distrobox, ubuntu-link and fedora-link, so they link the target's libraries), and the Flatpak bundle of the app.
# Usage: release.sh [android deb rpm]; no argument builds them all.
set -euo pipefail
ROOT=$(cd "$(dirname "$0")/.." && pwd)
OUT=$ROOT/target/dist
VERSION="r$(git -C "$ROOT" rev-list --count HEAD).g$(git -C "$ROOT" rev-parse --short HEAD)"
# A build with uncommitted changes gets its time, so it never takes a committed build's name.
git -C "$ROOT" diff --quiet HEAD || VERSION+=".dirty$(date +%m%d%H%M)"
SIGNING=$HOME/.config/andromeda-link/signing.env
SERVICES=(-p link-daemon -p link-session -p link-mount)
mkdir -p "$OUT"
cd "$ROOT"

android() {
  (
    [[ -f $SIGNING ]] && source "$SIGNING"
    cd apps/android && ./gradlew -q :app:assembleRelease
  )
  local apk=apps/android/app/build/outputs/apk/release
  if [[ -f $apk/app-release.apk ]]; then
    cp "$apk/app-release.apk" "$OUT/andromeda-link-$VERSION.apk"
  else
    echo "android: no $SIGNING, so the APK is unsigned and cannot be installed" >&2
    cp "$apk/app-release-unsigned.apk" "$OUT/andromeda-link-$VERSION-unsigned.apk"
  fi
}

deb() {
  local target=$HOME/.cache/ubuntu-link/target
  distrobox enter ubuntu-link -- bash -c "
    set -e
    export RUSTUP_HOME=$HOME/.local/share/ubuntu-link/rustup CARGO_HOME=$HOME/.local/share/ubuntu-link/cargo
    export PATH=$HOME/.local/share/ubuntu-link/cargo/bin:\$PATH CARGO_TARGET_DIR=$target
    cd '$ROOT'
    cargo build --release --locked ${SERVICES[*]}
    dist/deb/build-deb.sh $target/release '$OUT'"
}

rpm() {
  local target=$HOME/.cache/fedora-link/target
  distrobox enter fedora-link -- bash -c "
    export CARGO_TARGET_DIR=$target PATH=$HOME/.cargo/bin:\$PATH
    cd '$ROOT' && cargo build --release --locked ${SERVICES[*]} &&
    rpmbuild -bb dist/rpm/andromeda-link.spec --define 'linkroot $ROOT' --define 'linkbin $target/release' \
      --define 'linkversion 0~$VERSION' --define '_rpmdir $OUT'"
}

for kind in "${@:-android deb rpm}"; do
  for step in $kind; do
    echo "== $step"
    "$step"
  done
done
ls -la "$OUT"

#!/usr/bin/env bash
# Builds the Link window (window/) into DESTDIR, linked with the static libdesk of a desktop checkout at
# dist/window/libdesk-commit:  dist/window/build.sh DESKTOP_DIR DESTDIR
set -euo pipefail
link=$(realpath "$(dirname "$0")/../..")
desktop=$(realpath "$1") dest=$(realpath -m "$2")
# WINDOW_BUILD_DIR keeps the build dirs between runs (just window); by default they are temporary.
if [ -n "${WINDOW_BUILD_DIR:-}" ]; then build=$(realpath -m "$WINDOW_BUILD_DIR"); else
  build=$(mktemp -d)
  trap 'rm -rf "$build"' EXIT
fi
pin=$(cat "$link/dist/window/libdesk-commit")
if have=$(git -C "$desktop" rev-parse HEAD 2> /dev/null) && [ "$have" != "$pin" ]; then
  echo "build.sh: $desktop is at $have, the pin is $pin" >&2
  [ -n "${LIBDESK_ANY_COMMIT:-}" ] || exit 1
fi
[ -d "$build/desk" ] || meson setup "$build/desk" "$desktop/libdesk-app" -Ddevel=true -Dlibdesk-ui:dev_tokens=false \
  --prefix="$build/libdesk" --libdir=lib --buildtype=release
meson compile -C "$build/desk"
meson install -C "$build/desk" --no-rebuild --tags devel > /dev/null
[ -d "$build/window" ] || PKG_CONFIG_PATH="$build/libdesk/lib/pkgconfig" meson setup "$build/window" "$link/window" --prefix=/usr --libdir=lib \
  --buildtype=release
meson compile -C "$build/window"
meson test -C "$build/window" --no-rebuild --print-errorlogs
DESTDIR="$dest" meson install -C "$build/window" --no-rebuild

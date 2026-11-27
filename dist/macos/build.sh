#!/usr/bin/env bash
# Builds "Andromeda Link.app" for arm64 macOS into OUT_DIR, on a Mac or cross on Linux: the static libdesk of the
# pinned desktop commit (dist/window/libdesk-commit), the window, andromeda-linkd, link-session in its helper bundle,
# and the now-playing dylib, with every non-system dylib inside and an ad-hoc signature. VERSION: at most 3 numbers.
#   dist/macos/build.sh DESKTOP_DIR OUT_DIR [VERSION]
# The desktop's tools/macos/deps.py fetches the Homebrew bottles (on Linux also the Darwin SDK from xtool).
set -euo pipefail
link=$(realpath "$(dirname "$0")/../..")
desktop=$(realpath "$1") out=$(realpath -m "$2") version=${3:-0.0.0}
# MAC_BUILD_DIR keeps the build dirs between runs (just mac-app); by default they are temporary.
if [ -n "${MAC_BUILD_DIR:-}" ]; then build=$(realpath -m "$MAC_BUILD_DIR"); else
  build=$(mktemp -d)
  trap 'rm -rf "$build"' EXIT
fi
pin=$(cat "$link/dist/window/libdesk-commit")
if have=$(git -C "$desktop" rev-parse HEAD 2> /dev/null) && [ "$have" != "$pin" ]; then
  echo "build.sh: $desktop is at $have, the pin is $pin" >&2
  [ -n "${LIBDESK_ANY_COMMIT:-}" ] || exit 1
fi

cross=$HOME/.cache/andromeda-mac/cross.ini
deps=$HOME/.cache/andromeda-mac/deps
[ -f "$cross" ] || "$desktop/tools/macos/deps.py"
sdk=$(sed -n "s/^cpp = .*'-isysroot', '\([^']*\)'.*/\1/p" "$cross")
mkdir -p "$build"
[ -d "$build/desk" ] || meson setup "$build/desk" "$desktop/libdesk-app" --cross-file "$cross" -Dplatform=macos \
  -Ddevel=true -Dlibdesk-ui:dev_tokens=false --prefix="$build/libdesk" --libdir=lib --buildtype=release
meson compile -C "$build/desk"
meson install -C "$build/desk" --no-rebuild --tags devel > /dev/null

export MACOSX_DEPLOYMENT_TARGET=26.0 CARGO_TARGET_DIR=$build/cargo
export CARGO_TARGET_AARCH64_APPLE_DARWIN_LINKER=clang CC_aarch64_apple_darwin=clang CFLAGS_aarch64_apple_darwin="-isysroot $sdk"
export CARGO_TARGET_AARCH64_APPLE_DARWIN_RUSTFLAGS="-C link-arg=-target -C link-arg=arm64-apple-macos26 -C link-arg=-isysroot -C link-arg=$sdk -C link-arg=-fuse-ld=lld"
printf "[properties]\npkg_config_libdir = ['%s', '%s', '%s']\n" "$build/libdesk/lib/pkgconfig" "$deps/lib/pkgconfig" \
  "$deps/share/pkgconfig" > "$build/window.ini"
[ -d "$build/window" ] || meson setup "$build/window" "$link/window" --cross-file "$cross" --cross-file "$build/window.ini" \
  -Dcargo_target=aarch64-apple-darwin --buildtype=release
meson compile -C "$build/window"
# The bottles' dylibs carry @@HOMEBREW_PREFIX@@ install names until the bundle step rewrites them: dyld finds them
# by leaf name here.
[ "$(uname)" != Darwin ] || DYLD_LIBRARY_PATH=$deps/lib meson test -C "$build/window" --no-rebuild --print-errorlogs
cargo build -q --release --locked --target aarch64-apple-darwin --manifest-path "$link/Cargo.toml" \
  -p link-daemon -p link-session -p link-nowplaying
bin=$build/cargo/aarch64-apple-darwin/release

app="$out/Andromeda Link.app"
contents=$app/Contents
helper="$contents/Library/LoginItems/Andromeda Link Helper.app/Contents"
rm -rf "$app"
mkdir -p "$contents/MacOS" "$contents/Frameworks" "$contents/Resources/LaunchAgents" "$contents/Resources/translations/link" \
  "$helper/MacOS" "$helper/Resources"
cp "$build/window/andromeda-link-app" "$bin/andromeda-linkd" "$contents/MacOS/"
# linkd hands it to Apple's perl, whose identity MediaRemote still serves (crates/link-nowplaying).
cp "$bin/liblink_nowplaying.dylib" "$contents/Frameworks/"
# link-session runs from its own bundle, so Accessibility and notifications name it "Andromeda Link Helper".
cp "$bin/andromeda-link-session" "$helper/MacOS/"
sed "s/@VERSION@/$version/" "$link/dist/macos/Helper-Info.plist" > "$helper/Info.plist"
sed "s/@VERSION@/$version/" "$link/dist/macos/Info.plist" > "$contents/Info.plist"
cp "$link/dist/macos/AppIcon.icns" "$helper/Resources/"
cp "$link/dist/macos/AppIcon.icns" "$link/dist/macos/install-agents.sh" "$contents/Resources/"
cp "$link/dist/macos"/org.andromeda.*.plist "$contents/Resources/LaunchAgents/"
cp -R "$(PKG_CONFIG_LIBDIR=$build/libdesk/lib/pkgconfig pkg-config --variable=assetsdir andromeda-desk)" "$contents/Resources/assets"
cp "$link/window/translations/en.json" "$contents/Resources/translations/link/"

"$desktop/tools/macos/bundle-libdesk.sh" "$app" "" "$contents/MacOS/andromeda-link-app" "$contents/MacOS/andromeda-linkd" \
  "$helper/MacOS/andromeda-link-session"
llvm-strip -S "$contents/MacOS/andromeda-link-app"
# Install-name changes broke the old signatures; a Mac has codesign, Linux rcodesign.
if command -v rcodesign > /dev/null; then
  rcodesign sign "$app" > /dev/null
else
  codesign --force --deep --sign - "$app"
fi
echo "build.sh: $app ($(du -sh "$app" | cut -f1))"

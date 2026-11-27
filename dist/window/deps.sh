#!/usr/bin/env bash
# Installs what build.sh and the packaging need, in a fresh container of the target: deps.sh arch|ubuntu|debian|fedora
set -euo pipefail
case ${1:?distro} in
  arch)
    pacman -Syu --noconfirm --needed base-devel git meson ninja python dbus cairo pango fontconfig freetype2 harfbuzz fribidi \
      librsvg glib2 libxkbcommon nlohmann-json zlib wayland wayland-protocols libglvnd sdbus-cpp systemd-libs libcups \
      libxcrypt qrencode ffmpeg libva tomlplusplus md4c libwebp libjxl stb ;;
  ubuntu|debian)
    export DEBIAN_FRONTEND=noninteractive
    apt-get update -qq
    apt-get install -y -qq --no-install-recommends git meson ninja-build g++ pkgconf python3 ca-certificates curl dpkg-dev dbus \
      libfreetype-dev libfontconfig-dev libcairo2-dev libpango1.0-dev libharfbuzz-dev libfribidi-dev librsvg2-dev \
      libxkbcommon-dev libglib2.0-dev nlohmann-json3-dev zlib1g-dev libwayland-dev wayland-protocols libegl-dev \
      libgles-dev libsdbus-c++-dev libsystemd-dev libcups2-dev libcrypt-dev libqrencode-dev libavcodec-dev \
      libavutil-dev libavfilter-dev libswscale-dev libva-dev libtomlplusplus-dev libmd4c-dev libwebp-dev libjxl-dev \
      libstb-dev ;;
  fedora)
    dnf install -y -q git meson ninja-build gcc-c++ libasan pkgconf python3 rpm-build dbus-daemon freetype-devel fontconfig-devel \
      cairo-devel pango-devel harfbuzz-devel fribidi-devel librsvg2-devel libxkbcommon-devel glib2-devel json-devel \
      zlib-ng-compat-devel wayland-devel wayland-protocols-devel libglvnd-devel sdbus-cpp-devel systemd-devel \
      cups-devel libxcrypt-devel qrencode-devel libavcodec-free-devel libavutil-free-devel libavfilter-free-devel \
      libswscale-free-devel libva-devel tomlplusplus-devel md4c-devel libwebp-devel libjxl-devel \
      stb_image_resize2-devel stb_image-devel stb_image_write-devel ;;
  *) echo "deps.sh: unknown distro $1" >&2; exit 2 ;;
esac

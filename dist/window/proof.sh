#!/usr/bin/env bash
# In a clean container of the target, installs Link and its window from the release packages in PKG_DIR with only
# the distro's own repos, starts andromeda-linkd and the window in a headless sway, and decodes the pairing QR code
# the window shows with no phone paired, with its own catalog loaded. Writes pairing.png and the logs to OUT_DIR.
#   dist/window/proof.sh arch|ubuntu|debian|fedora PKG_DIR OUT_DIR
set -euo pipefail
distro=${1:?distro} pkgs=$(realpath "${2:?package dir}") out=$(realpath -m "${3:?output dir}")
mkdir -p "$out"
pick() { find "$pkgs" -type f -name "$1" ! -name '*-debug-*' | head -1; }
case $distro in
  arch)
    link=$(pick 'andromeda-link-[0-9]*.pkg.tar.zst') window=$(pick 'andromeda-link-window-*.pkg.tar.zst')
    pacman -Syu --noconfirm --needed sway grim zbar mesa
    pacman -U --noconfirm "$link" "$window" ;;
  ubuntu|debian)
    link=$(pick 'andromeda-link_*.deb') window=$(pick "andromeda-link-window-${distro}_*.deb")
    export DEBIAN_FRONTEND=noninteractive
    apt-get update -qq
    apt-get install -y -qq --no-install-recommends sway grim zbar-tools libgl1-mesa-dri libegl-mesa0 dbus "$link" "$window" ;;
  fedora)
    link=$(pick 'andromeda-link-[0-9]*.rpm') window=$(pick 'andromeda-link-window-*.rpm')
    dnf install -y -q sway grim zbar mesa-dri-drivers mesa-libEGL dbus-daemon "$link" "$window" ;;
  *) echo "proof.sh: unknown distro $distro" >&2; exit 2 ;;
esac
echo "installed: $(basename "$link") $(basename "$window")"

export XDG_RUNTIME_DIR=$(mktemp -d) XDG_STATE_HOME=$(mktemp -d)
chmod 700 "$XDG_RUNTIME_DIR"
dbus-run-session -- bash -euo pipefail -c '
  out=$1
  # 30 s deadline per wait, at 0.5 s steps.
  wait_for() { local what=$1 i; shift; for i in $(seq 60); do "$@" && return 0; sleep 0.5; done
    echo "proof.sh: $what" >&2; tail -n 20 "$out"/*.log >&2; return 1; }
  WLR_BACKENDS=headless WLR_RENDERER=pixman WLR_LIBINPUT_NO_DEVICES=1 sway -c /dev/null > "$out/sway.log" 2>&1 &
  wait_for "sway did not start" compgen -G "$XDG_RUNTIME_DIR/wayland-*" > /dev/null
  export WAYLAND_DISPLAY=$(basename "$(compgen -G "$XDG_RUNTIME_DIR/wayland-?" | head -1)")
  andromeda-linkd > "$out/linkd.log" 2>&1 &
  owned() { dbus-send --session --print-reply --dest=org.freedesktop.DBus / org.freedesktop.DBus.NameHasOwner \
    string:org.andromeda.Link1 | grep -q "boolean true"; }
  wait_for "andromeda-linkd did not take org.andromeda.Link1" owned
  andromeda-link-app > "$out/window.log" 2>&1 &
  qr() { grim "$out/pairing.png" && zbarimg -q --raw "$out/pairing.png" > "$out/uri.txt"; }
  wait_for "the window showed no pairing QR code" qr
  echo "pairing QR: $(cut -d"&" -f1 "$out/uri.txt")"
  if grep "no English catalog" "$out/window.log" >&2; then exit 1; fi
' proof "$out"

#!/usr/bin/env bash
# On a clean Mac: installs Andromeda Link from the release zip into /Applications, starts the window, which loads
# andromeda-linkd and link-session as launchd agents, and decodes the pairing QR code the window shows with no phone
# paired, with its own catalog loaded. Writes pairing.png and the logs to OUT_DIR. Needs zbarimg.
#   dist/macos/proof.sh ZIP OUT_DIR
set -euo pipefail
zip=$(realpath "${1:?zip}")
mkdir -p "${2:?output dir}"
out=$(realpath "$2")
app="/Applications/Andromeda Link.app"
rm -rf "$app"
ditto -x -k "$zip" /Applications
codesign --verify --deep --strict "$app"
echo "installed: $(du -sh "$app" | cut -f1)"

logs="$HOME/Library/Logs/Andromeda Link"
# 60 s deadline per wait, at 0.5 s steps.
wait_for() {
  local what=$1 i
  shift
  for i in $(seq 120); do "$@" > /dev/null 2>&1 && return 0; sleep 0.5; done
  echo "proof.sh: $what" >&2
  screencapture -x "$out/failure.png" || true
  cp "$logs"/*.log "$out/" 2> /dev/null || true
  tail -n 30 "$out"/*.log >&2 || true
  exit 1
}
"$app/Contents/MacOS/andromeda-link-app" > "$out/window.log" 2>&1 &
wait_for "the window loaded no linkd agent" launchctl print "gui/$(id -u)/org.andromeda.linkd"
wait_for "andromeda-linkd made no socket" test -S "$HOME/Library/Application Support/Andromeda Link/link.sock"
qr() { screencapture -x "$out/pairing.png" && zbarimg -q --raw "$out/pairing.png" > "$out/uri.txt"; }
wait_for "the window showed no pairing QR code" qr
echo "pairing QR: $(cut -d'&' -f1 "$out/uri.txt")"
cp "$logs"/*.log "$out/" 2> /dev/null || true
if grep "no English catalog" "$out/window.log" >&2; then exit 1; fi

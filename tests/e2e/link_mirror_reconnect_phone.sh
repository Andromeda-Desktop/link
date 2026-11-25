#!/usr/bin/env bash
# A Link1 OpenMirror viewer gets mirroring back after the network drops, with a real phone through
# tests/e2e/udp_impair.py, which drops every packet for a while and then lets them through again.
#
# Proves:
# 1. The viewer goes live with the phone's screen (bytes flow on the OpenMirror socket, video size known).
# 2. Total loss: within seconds the bytes stop (the viewer marks the stream stalled after 2 s). After 12 s the link is
#    clean again and bytes flow on the same socket, with no new consent on the phone and no second ask.
# Needs ANDROID_SERIAL and the debug APKs. Keeps the phone's other pairings. Holds /tmp/andromeda-link-phone.lock, as
# other phone runs do. Writes steps.txt, viewer.log, linkd.log, relay.tsv to $OUT (default
# ./artifacts/link-mirror-reconnect-phone).
set -euo pipefail
OUT=${OUT:-$(pwd)/artifacts/link-mirror-reconnect-phone}
source "$(dirname "$0")/link_phone_lib.sh"
RELAY_PORT=${RELAY_PORT:-48307}

[[ -n ${LINK_PHONE_LOCKED:-} ]] || exec env LINK_PHONE_LOCKED=1 flock --close /tmp/andromeda-link-phone.lock "$0" "$@"
rm -rf "$OUT"
mkdir -p "$OUT"
RUNTIME=$(mktemp -d /tmp/link-mirror-reconnect.XXXX)
trap 'adb shell input keyevent KEYCODE_HOME; forget_phone; kill $(jobs -p) 2>/dev/null || true; wait 2>/dev/null; rm -rf "$RUNTIME"' EXIT
step() { printf '%s\n' "$*" | tee -a "$OUT/steps.txt"; }
[[ $(adb get-state 2>/dev/null) == device ]] || fail "no phone at $SERIAL"
[[ -s $APK && -s $FIXTURE ]] || fail "no APKs at $APK and $FIXTURE"

private_bus system
start_linkd
HOST=$(ip -4 route get 1.1.1.1 | grep -o 'src [0-9.]*' | cut -d' ' -f2)
echo clean > "$RUNTIME/impair"
python3 "$ROOT/tests/e2e/udp_impair.py" "$RELAY_PORT" "$PORT" "$RUNTIME/impair" "$OUT/relay.tsv" &
install_app
pair_phone "$HOST:$RELAY_PORT"
phone_switches screen

# The viewer logs "live W H" when bytes flow, "stalled" after 2 s without any, and "lost" when the socket closes.
python3 -I -c '
import os, select, sys, time
import dbus
link = dbus.Interface(dbus.SessionBus().get_object("org.andromeda.Link1", "/org/andromeda/Link1"), "org.andromeda.Link1")
fd, width, height = link.OpenMirror(sys.argv[1], timeout=60)
stream, state, last = fd.take(), "", time.monotonic()
while True:
    if select.select([stream], [], [], 0.2)[0]:
        if not os.read(stream, 65536):
            print("lost", flush=True)
            break
        last = time.monotonic()
        if state != "live":
            state = "live"
            print("live", int(width), int(height), flush=True)
    elif state == "live" and time.monotonic() - last > 2:
        state = "stalled"
        print("stalled", flush=True)' "$ID" > "$OUT/viewer.log" 2>&1 &
state() { tail -1 "$OUT/viewer.log" | cut -d' ' -f1; }
asks() { grep -c "asking the phone to mirror" "$OUT/linkd.log" || true; }
allow_mirror first
wait_for 30 "the viewer did not go live: $(tail -3 "$OUT/viewer.log")" eval '[[ $(state) == live ]]'
step "live: $(cut -d' ' -f2- <<< "$(grep ^live "$OUT/viewer.log" | head -1)")"

echo "loss=1.0" > "$RUNTIME/impair"
T0=$(now_ms)
wait_for 10 "the viewer still sees bytes during the loss: $(state)" eval '[[ $(state) == stalled ]]'
step "stalled $(( ($(now_ms) - T0) / 1000 )) s after the drop began"
read -rt 12 < /dev/zero 2> /dev/null || true
echo clean > "$RUNTIME/impair"
T0=$(now_ms)
# A drop shorter than the session's idle timeout ends nothing: the same stream goes on, with no new consent.
wait_for 60 "the viewer did not go live again: $(tail -3 "$OUT/viewer.log")" eval '[[ $(state) == live ]]'
(( $(asks) == 1 )) || fail "the phone was asked again: $(asks) asks"
step "live again $(( ($(now_ms) - T0) / 1000 )) s after the link came back, same stream"
echo "PASS; artifacts: $OUT"

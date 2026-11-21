#!/usr/bin/env bash
# Wi-Fi Direct with real radios: the phone stays on its Wi-Fi, this laptop leaves its own, so Bluetooth is the only
# path until the phone forms a Wi-Fi Direct group that the laptop joins through the real NetworkManager.
#
# Proves:
# 1. With the laptop off its Wi-Fi, the phone's session comes over Bluetooth.
# 2. An 8 MiB SendFiles creates a group the phone owns instead of the hotspot: linkd joins its DIRECT- SSID with the
#    passphrase on the idle Wi-Fi device, the session moves to IP on the group's subnet (192.168.49.0/24), no
#    HotspotRequested is signalled, and the file arrives with a matching SHA-256. results.jsonl records the join path.
# 3. An 8 MiB share from the phone reaches the desktop intact over the group (phone -> desktop throughput).
# 4. A text share made while the desktop's linkd is down is queued on the phone and arrives exactly once after.
# 5. With LINK_IDLE_STEP=1: with the screen off the group idles out to Bluetooth; the next 8 MiB rejoins it (rejoin_ms,
#    budget 3 s). This locks the phone, so it runs last and only on request.
# Takes the laptop off its Wi-Fi for a few minutes. Stops the user's andromeda-linkd for the run (it holds Link's
# Bluetooth profile and port 4717) and starts it again after, and brings the laptop's Wi-Fi back even on failure.
# Writes results.jsonl, linkd.log, signals.txt, nm.txt, logcat.txt to $OUT (default ./artifacts/link-wifi-direct-phone).
set -euo pipefail
OUT=${OUT:-$(pwd)/artifacts/link-wifi-direct-phone}
source "$(dirname "$0")/link_phone_lib.sh"

[[ -n ${LINK_PHONE_LOCKED:-} ]] || exec env LINK_PHONE_LOCKED=1 flock --close /tmp/andromeda-link-phone.lock "$0" "$@"
rm -rf "$OUT"
mkdir -p "$OUT"
RUNTIME=$(mktemp -d /var/tmp/link-wifi-direct-phone.XXXX)
HOME_WIFI=$(nmcli -t -f NAME,TYPE con show --active | grep ':802-11-wireless$' | head -1 | cut -d: -f1)
[[ -n $HOME_WIFI ]] || fail "the laptop is not on Wi-Fi"
USER_LINKD=$(systemctl --user is-active andromeda-linkd || true)
restore() {
  # First: a failing step below would end the trap and leave these jobs holding the phone lock.
  kill $(jobs -p) 2>/dev/null || true
  wait 2>/dev/null
  forget_phone || true
  nmcli -t -f NAME con show --active | grep -qxF "$HOME_WIFI" || nmcli con up id "$HOME_WIFI" > /dev/null || true
  [[ $USER_LINKD != active ]] || systemctl --user start andromeda-linkd
  rm -rf "$RUNTIME"
}
trap restore EXIT
[[ $(adb get-state 2>/dev/null) == device ]] || fail "no phone at $SERIAL"
[[ -s $APK && -s $FIXTURE ]] || fail "no APKs at $APK and $FIXTURE"
[[ $(adb get-devpath) == usb* ]] || fail "adb must run over USB: the laptop leaves its Wi-Fi"
! adb shell dumpsys window | grep -q 'isKeyguardShowing=true' || fail "unlock the phone: pairing and the share sheet need it"

systemctl --user stop andromeda-linkd 2>/dev/null || true
private_bus
start_linkd
wait_for 10 "BlueZ did not take Link's profile" grep -q 'Bluetooth: listening on' "$OUT/linkd.log"
nmcli monitor > >(stamp > "$OUT/nm.txt") &

install_app
adb logcat -c
"$ADB" -s "$SERIAL" logcat -v time > "$OUT/logcat.txt" 2>&1 &
HOST=$(ip -4 route get 1.1.1.1 | grep -o 'src [0-9.]*' | cut -d' ' -f2)
pair_phone "$HOST:$PORT"
DESKTOP_NAME=$(adb shell run-as "$PACKAGE" cat files/devices.json \
  | python3 -c 'import json,sys; print(json.load(sys.stdin)["peers"][-1]["name"])')
record "{\"step\":\"paired\",\"device\":\"$ID\",\"home_wifi\":\"$HOME_WIFI\"}"

# A new session logs "connected over", a session that moves logs "the session is on".
last_path() { grep -E "connected over|the session is on" "$OUT/linkd.log" | tail -1; }
on_bluetooth() { last_path | grep -q "Bluetooth$"; }
on_group() { last_path | grep -q "IP from .*192\.168\.49\."; }
delivered() {
  [[ $(adb shell sha256sum "/sdcard/Download/$1" 2>/dev/null | cut -d' ' -f1) == $(sha256sum < "$2" | cut -d' ' -f1) ]]
}
# phone_receives <name> <file>: without auto-accept on the phone, its offer notification takes the tap.
phone_receives() {
  local arrived=""
  for _ in $(seq 30); do
    delivered "$1" "$2" && { arrived=1; break; }
    adb shell dumpsys notification --noredact | grep -q "wants to send" && break
    sleep 1
  done
  [[ -n $arrived ]] && return
  wait_for 30 "no offer notification" eval 'adb shell dumpsys notification --noredact | grep -q "wants to send"'
  adb shell cmd statusbar expand-notifications
  tap_text offer "*wants to send*"
  tap_text accept Accept
  adb shell cmd statusbar collapse
  wait_for 120 "$1 did not arrive intact" delivered "$1" "$2"
}
# xfer_ms <log> <regex for a line's ms>: accept to done of the last transfer the log shows, in ms.
xfer_ms() {
  python3 - "$1" "$2" <<'PY2'
import re, sys
from datetime import datetime
times = {}
for line in open(sys.argv[1], errors="replace"):
    m = re.match(sys.argv[2] + r".*: (accepted transfer|transfer \w+ from it: done)", line)
    if m:
        t = m.group(1)
        t = int(t) if t.isdigit() else datetime.strptime("2000-" + t, "%Y-%m-%d %H:%M:%S.%f").timestamp() * 1000
        times["accepted" if m.group(2).startswith("accepted") else "done"] = t
print(int(times["done"] - times["accepted"]) if len(times) == 2 and times["done"] > times["accepted"] else 0)
PY2
}
# share_from_phone <file>: the phone's share sheet sends <file> (pushed to Download) to the desktop.
share_from_phone() {
  local name id
  name=$(basename "$1")
  adb push "$1" "/sdcard/Download/$name" > /dev/null 2>&1
  adb shell content call --uri content://media --method scan_volume --arg external_primary > /dev/null
  id=$(adb shell content query --uri content://media/external/file --projection _id \
    --where "\"_display_name='$name'\"" | grep -o '_id=[0-9]*' | cut -d= -f2 | tr -d '\r' | tail -1 || true)
  [[ -n $id ]] || fail "$name is not in the media store"
  adb shell am start -W -n "$PACKAGE/.share.ShareActivity" -a android.intent.action.SEND -t application/octet-stream \
    --eu android.intent.extra.STREAM "content://media/external/file/$id" > /dev/null
  choose_desktop
}
# choose_desktop: with the user's own desktop also paired, the share sheet asks; it lists desktops oldest first, so
# the run's is last.
choose_desktop() {
  for _ in $(seq 20); do
    adb shell dumpsys activity activities | grep -m1 topResumedActivity | grep -q ShareActivity || return 0
    ui share
    if grep -q 'Send to which desktop' "$OUT/ui-share.xml"; then
      at=$(python3 - "$OUT/ui-share.xml" "$DESKTOP_NAME" <<'PY2'
import re, sys
xml = open(sys.argv[1]).read()
name = sys.argv[2]
last = None
for node in re.finditer(r'<node [^>]*>', xml):
    attrs = dict(re.findall(r'(\w[\w-]*)="([^"]*)"', node.group(0)))
    if name and attrs.get("text") == name:
        l, t, r, b = map(int, re.findall(r"\d+", attrs["bounds"]))
        last = f"{(l + r) // 2} {(t + b) // 2}"
print(last or "")
PY2
)
      [[ -n $at ]] || fail "the share sheet does not list $DESKTOP_NAME"
      adb shell input tap $at
      return 0
    fi
    sleep 0.25
  done
}
# desktop_receives <file>: accepts the phone's offer unless auto-accept did, then waits for the intact copy.
desktop_receives() {
  local name offer
  name=$(basename "$1")
  for _ in $(seq 600); do
    cmp -s "$1" "$RUNTIME/downloads/$name" && return
    offer=$(grep -A1 'member=TransferOffered' "$OUT/signals.txt" | grep -o '"[0-9a-f]\{32\}"' | tail -1 | tr -d '"' || true)
    [[ -n $offer && $offer != "${ACCEPTED:-}" ]] && { link AcceptTransfer "'$offer'" > /dev/null || true; ACCEPTED=$offer; }
    sleep 0.1
  done
  fail "$name did not reach the desktop intact"
}


T0=$(now_ms)
nmcli con down id "$HOME_WIFI" > /dev/null
wait_for 120 "the phone did not come over Bluetooth" on_bluetooth
record "{\"step\":\"rfcomm-session\",\"after_wifi_off_ms\":$(( $(now_ms) - T0 ))}"

adb shell rm -f /sdcard/Download/direct-8m.bin
head -c 8388608 /dev/urandom > "$RUNTIME/direct-8m.bin"
link SetAutoAccept "'$ID'" true > /dev/null || true
T0=$(now_ms)
python3 "$ROOT/tests/e2e/link_send_files.py" "$ID" "$RUNTIME/direct-8m.bin" > "$RUNTIME/transfer" || fail "SendFiles 8 MiB"
wait_for 90 "the session did not move to a Wi-Fi Direct group: $(grep -E 'Direct|hotspot' "$OUT/linkd.log" | tail -2)" on_group
JOINED=$(( $(now_ms) - T0 ))
phone_receives direct-8m.bin "$RUNTIME/direct-8m.bin"
! grep -q 'member=HotspotRequested' "$OUT/signals.txt" || fail "the phone offered its hotspot instead of a group"
JOIN_PATH=leave-wifi
grep -q "the Wi-Fi device is idle" "$OUT/linkd.log" && JOIN_PATH=idle-wifi
grep -q "added linkwifi0" "$OUT/linkd.log" && JOIN_PATH=second-interface
MHZ=$(grep -o 'wifi-direct group on [0-9]* MHz' "$OUT/logcat.txt" | tail -1 | grep -o '[0-9]\+' || echo 0)
# The bytes' own time: from the phone's accept to its last byte, by the phone's clock.
XFER_MS=$(xfer_ms "$OUT/logcat.txt" '(\d\d-\d\d \d\d:\d\d:\d\d\.\d+).*link_core::transfer: \w+')
record "{\"step\":\"8mib-over-wifi-direct\",\"path\":\"$JOIN_PATH\",\"mhz\":$MHZ,\"join_ms\":$JOINED,\"xfer_ms\":$XFER_MS,\"done_ms\":$(( $(now_ms) - T0 ))}"
adb shell rm -f /sdcard/Download/direct-8m.bin

# Phone -> desktop over the group: the bytes' own time by the desktop's clock.
head -c 8388608 /dev/urandom > "$RUNTIME/up-8m.bin"
on_group || fail "the session left the group before the phone's send"
T0=$(now_ms)
share_from_phone "$RUNTIME/up-8m.bin"
desktop_receives "$RUNTIME/up-8m.bin"
on_group || fail "the phone's 8 MiB did not go over the group: $(last_path)"
record "{\"step\":\"phone-8mib-over-wifi-direct\",\"xfer_ms\":$(xfer_ms "$OUT/linkd.log" '(\d+) '),\"done_ms\":$(( $(now_ms) - T0 ))}"
adb shell rm -f /sdcard/Download/up-8m.bin

# Outbox: a share made while the desktop is away waits on the phone and arrives once after the desktop returns.
LINKD=$(pgrep -xf "$BIN/andromeda-linkd")
kill "$LINKD"
while kill -0 "$LINKD" 2>/dev/null; do sleep 0.1; done
TOKEN="link-outbox-$(now_ms)"
adb shell am start -W -n "$PACKAGE/.share.ShareActivity" -a android.intent.action.SEND -t text/plain \
  --es android.intent.extra.TEXT "$TOKEN" > /dev/null
choose_desktop
wait_for 30 "the phone did not queue the share" grep -q 'share [0-9a-f]* queued' "$OUT/logcat.txt"
XDG_STATE_HOME=$RUNTIME/desktop XDG_DOWNLOAD_DIR=$RUNTIME/downloads RUST_LOG=info,zbus=warn,tracing=warn \
  "$BIN/andromeda-linkd" 2>&1 | stamp >> "$OUT/linkd.log" &
T0=$(now_ms)
wait_for 120 "the queued share did not reach the desktop" grep -q "$TOKEN" "$OUT/signals.txt"
DELIVERED_MS=$(( $(now_ms) - T0 ))
wait_for 30 "the phone did not see the share acknowledged" grep -q 'acknowledged' "$OUT/logcat.txt"
sleep 3
COPIES=$(grep -c "$TOKEN" "$OUT/signals.txt" || true)
(( COPIES == 1 )) || fail "the queued share arrived $COPIES times"
record "{\"step\":\"outbox\",\"after_desktop_back_ms\":$DELIVERED_MS}"

# Rejoin: with the screen off the group idles out (60 s) and the session falls back to Bluetooth; the next 8 MiB
# forms the group again with the saved credentials. Budget: SendFiles to the session on the group <= 3 s.
# The outbox step ended the session and with it the group: the next 8 MiB forms it again (rejoin after a session).
T0=$(now_ms)
python3 "$ROOT/tests/e2e/link_send_files.py" "$ID" "$RUNTIME/direct-8m.bin" > "$RUNTIME/transfer" || fail "SendFiles 8 MiB after the outbox"
wait_for 90 "the session did not join the group again" on_group
AFTER_SESSION_MS=$(( $(now_ms) - T0 ))
phone_receives direct-8m.bin "$RUNTIME/direct-8m.bin"
adb shell rm -f /sdcard/Download/direct-8m.bin
if [[ -z ${LINK_IDLE_STEP:-} ]]; then
  record "{\"step\":\"rejoin\",\"after_session_ms\":$AFTER_SESSION_MS,\"budget_ms\":3000}"
else
  MOVES=$(grep -cE "connected over|the session is on" "$OUT/linkd.log")
  adb shell input keyevent KEYCODE_SLEEP
  T0=$(now_ms)
  moved_to_bluetooth() { (( $(grep -cE "connected over|the session is on" "$OUT/linkd.log") > MOVES )) && on_bluetooth; }
  wait_for 150 "the group did not idle out with the screen off" moved_to_bluetooth
  IDLE_MS=$(( $(now_ms) - T0 ))
  wait_for 30 "the phone did not stay connected after the idle-out: $(devices)" connected
  adb shell input keyevent KEYCODE_WAKEUP
  T0=$(now_ms)
  python3 "$ROOT/tests/e2e/link_send_files.py" "$ID" "$RUNTIME/direct-8m.bin" > "$RUNTIME/transfer" || fail "SendFiles 8 MiB again"
  wait_for 90 "the session did not rejoin the group" on_group
  REJOIN_MS=$(( $(now_ms) - T0 ))
  MHZ=$(grep -o 'wifi-direct group on [0-9]* MHz' "$OUT/logcat.txt" | tail -1 | grep -o '[0-9]\+' || echo 0)
  # The lock screen is back after the sleep, so nobody taps the offer: the join is what this step measures.
  link CancelTransfer "'$(cat "$RUNTIME/transfer")'" > /dev/null || true
  record "{\"step\":\"rejoin\",\"after_session_ms\":$AFTER_SESSION_MS,\"idle_out_ms\":$IDLE_MS,\"rejoin_ms\":$REJOIN_MS,\"mhz\":$MHZ,\"budget_ms\":3000}"
fi
cat "$OUT/results.jsonl"
echo "PASS; artifacts: $OUT"

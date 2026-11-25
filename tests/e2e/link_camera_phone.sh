#!/usr/bin/env bash
# Take a photo through Link1 TakePhoto with the emulator's camera, through a private andromeda-linkd. Proves:
# 1. TakePhoto opens the phone's camera (Link may draw over apps, so it starts at once).
# 2. After the shot (KEYCODE_CAMERA, then the camera's Done), TakePhoto returns the photo's path, and ReadFile on it
#    gives a JPEG, whole (its size matches StatFile).
# Needs ANDROID_SERIAL (the emulator's virtual camera, or the OnePlus camera) and the debug APKs. Run under flock
# /tmp/link-emulator.lock. Writes steps.txt, linkd.log, take.log, ui-*.xml and photo.jpg to $OUT (default
# ./artifacts/link-camera-phone).
set -euo pipefail
OUT=${OUT:-$(pwd)/artifacts/link-camera-phone}
source "$(dirname "$0")/link_phone_lib.sh"

[[ -n ${LINK_PHONE_LOCKED:-} ]] || exec env LINK_PHONE_LOCKED=1 flock --close /tmp/andromeda-link-phone.lock "$0" "$@"
rm -rf "$OUT"
mkdir -p "$OUT"
RUNTIME=$(mktemp -d /tmp/link-camera-phone.XXXX)
scan() { adb shell content call --method scan_volume --uri content://media --arg external_primary > /dev/null 2>&1 || true; }
trap 'adb shell "rm -f /sdcard/Pictures/Link/Link_*.jpg"; scan; adb shell input keyevent KEYCODE_HOME; forget_phone; kill $(jobs -p) 2>/dev/null || true; wait 2>/dev/null; rm -rf "$RUNTIME"' EXIT
step() { printf '%s\n' "$*" | tee -a "$OUT/steps.txt"; }
[[ $(adb get-state 2>/dev/null) == device ]] || fail "no phone at $SERIAL"
[[ -s $APK && -s $FIXTURE ]] || fail "no APKs at $APK and $FIXTURE"

private_bus
start_linkd
HOST=$(ip -4 route get 1.1.1.1 | grep -o 'src [0-9.]*' | cut -d' ' -f2)
install_app
adb shell appops set "$PACKAGE" SYSTEM_ALERT_WINDOW allow
pair_phone "$HOST:$PORT"
phone_switches browse
link SetGrant "$ID" browse true > /dev/null
# TakePhoto, then ReadFile to its end into $OUT/photo.jpg; take.log gets the phone path and the StatFile size.
python3 -I -c '
import sys
import dbus
device, out = sys.argv[1:]
link = dbus.Interface(dbus.SessionBus().get_object("org.andromeda.Link1", "/org/andromeda/Link1"), "org.andromeda.Link1")
path = link.TakePhoto(device, timeout=300)
print("taken", path, flush=True)
size = int(link.StatFile(device, path)[2])
with open(out, "wb") as f:
    while chunk := bytes(link.ReadFile(device, path, dbus.UInt64(f.tell()), dbus.UInt32(1024 * 1024))):
        f.write(chunk)
print("read", size, flush=True)' "$ID" "$OUT/photo.jpg" > "$OUT/take.log" 2>&1 &
camera_up() { adb shell dumpsys window | grep -E "mCurrentFocus|mFocusedApp" | grep -qi camera; }
wait_for 30 "the camera never opened" camera_up
step "the camera opened on the phone"

# First runs of the camera ask about locations and tips; then the shutter key takes the shot and Done hands it back.
shot=""
for try in $(seq 40); do
  ui "camera-$try"
  for text in "No thanks" "NEXT" "Next" "Got it" "While using the app" "Only this time" "Don't allow"; do
    at=$(bounds "camera-$try" "$text")
    [[ -z $at ]] || { adb shell input tap $at; continue 2; }
  done
  at=$(bounds "camera-$try" Done)
  [[ -n $at ]] || at=$(bounds "camera-$try" "*done*")
  [[ -n $at || -z $shot ]] || at=$(bounds "camera-$try" done_button)
  if [[ -n $at ]]; then adb shell input tap $at; break; fi
  if [[ -z $shot ]] && [[ -n $(bounds "camera-$try" "Shutter") ]]; then
    adb shell input keyevent KEYCODE_CAMERA
    shot=1
  elif [[ -z $shot ]] && at=$(bounds "camera-$try" shutter_button) && [[ -n $at ]]; then
    adb shell input tap $at
    shot=1
  fi
  camera_up || break
  sleep 1
done
[[ -n $shot ]] || fail "the camera never showed its shutter"
step "took the shot and tapped Done"

wait_for 60 "TakePhoto never returned a photo: $(tail -3 "$OUT/take.log")" grep -q '^read ' "$OUT/take.log"
SIZE=$(sed -n 's/^read //p' "$OUT/take.log")
(( SIZE > 0 )) && [[ $(stat -c %s "$OUT/photo.jpg") == "$SIZE" ]] || fail "ReadFile gave $(stat -c %s "$OUT/photo.jpg") bytes, StatFile says $SIZE"
file -b "$OUT/photo.jpg" | grep -q "JPEG image" || fail "photo.jpg is not a JPEG: $(file -b "$OUT/photo.jpg")"
step "TakePhoto gave $(sed -n 's/^taken //p' "$OUT/take.log"); ReadFile read it whole, $SIZE bytes of JPEG"
echo PASS

#!/usr/bin/env bash
# Browsing writes through Link1 against the emulator, through a private andromeda-linkd. Proves:
# 1. With All files access and the browse switch on, WriteFile copies a 150 KB file (4 writes of at most 48 KiB) into
#    /Storage/Download on the phone, byte for byte, and ListFiles lists it with its size.
# 2. DeleteFile removes it from the phone, and ListFiles no longer lists it.
# Needs ANDROID_SERIAL (the emulator) and the debug APKs. Run under flock /tmp/link-emulator.lock. Writes steps.txt and
# linkd.log to $OUT (default ./artifacts/link-files-phone).
set -euo pipefail
OUT=${OUT:-$(pwd)/artifacts/link-files-phone}
source "$(dirname "$0")/link_phone_lib.sh"
NAME=e2e-copy-$$.txt

[[ -n ${LINK_PHONE_LOCKED:-} ]] || exec env LINK_PHONE_LOCKED=1 flock --close /tmp/andromeda-link-phone.lock "$0" "$@"
rm -rf "$OUT"
mkdir -p "$OUT"
RUNTIME=$(mktemp -d /tmp/link-files-phone.XXXX)
trap 'adb shell rm -f /sdcard/Download/$NAME; forget_phone; kill $(jobs -p) 2>/dev/null || true; wait 2>/dev/null; rm -rf "$RUNTIME"' EXIT
step() { printf '%s\n' "$*" | tee -a "$OUT/steps.txt"; }
[[ $(adb get-state 2>/dev/null) == device ]] || fail "no phone at $SERIAL"
[[ -s $APK && -s $FIXTURE ]] || fail "no APKs at $APK and $FIXTURE"
head -c 150000 /dev/urandom > "$RUNTIME/$NAME"

private_bus
start_linkd
HOST=$(ip -4 route get 1.1.1.1 | grep -o 'src [0-9.]*' | cut -d' ' -f2)
install_app
adb shell appops set "$PACKAGE" MANAGE_EXTERNAL_STORAGE allow
pair_phone "$HOST:$PORT"
phone_switches browse
storage() { link ListFiles "'$ID'" "'/'" 2>/dev/null | grep -q Storage; }
wait_for 30 "the phone's roots never showed Storage" storage
link SetGrant "$ID" browse true > /dev/null

# upload LOCAL PHONE_PATH: WriteFile in chunks of 48 KiB; prints the number of writes and the size after the last one.
upload() {
  python3 -I -c '
import sys
import dbus
device, local, path = sys.argv[1:]
link = dbus.Interface(dbus.SessionBus().get_object("org.andromeda.Link1", "/org/andromeda/Link1"), "org.andromeda.Link1")
data, writes, size = open(local, "rb").read(), 0, 0
for offset in range(0, len(data), 48 * 1024):
    size = link.WriteFile(device, path, dbus.UInt64(offset), dbus.ByteArray(data[offset:offset + 48 * 1024]))
    writes += 1
print(writes, int(size))' "$ID" "$1" "$2"
}
listed() { link ListFiles "'$ID'" "'/Storage/Download'" | grep -q "'$NAME', false, \(uint64 \)\?150000,"; }

COPY=$(upload "$RUNTIME/$NAME" "/Storage/Download/$NAME") || fail "WriteFile refused the copy"
[[ $COPY == "4 150000" ]] || fail "WriteFile: '$COPY' (writes, size), not '4 150000'"
on_phone() { [[ $(adb shell "md5sum /sdcard/Download/$NAME 2>/dev/null" | cut -d' ' -f1) == $(md5sum < "$RUNTIME/$NAME" | cut -d' ' -f1) ]]; }
wait_for 60 "the copy never reached the phone whole" on_phone
step "$NAME (150000 bytes, 4 writes) is on the phone, same md5"
wait_for 30 "ListFiles never listed $NAME: $(link ListFiles "'$ID'" "'/Storage/Download'")" listed
step "ListFiles lists it in /Storage/Download with its size"

link DeleteFile "'$ID'" "'/Storage/Download/$NAME'" > /dev/null || fail "DeleteFile refused"
gone() { ! adb shell "ls /sdcard/Download/$NAME" > /dev/null 2>&1; }
wait_for 30 "$NAME is still on the phone" gone
step "DeleteFile removed it from the phone"
wait_for 30 "ListFiles still lists $NAME" eval '! listed'
step "ListFiles no longer lists it"
echo PASS

#!/usr/bin/env bash
# Link1 Calls and Dial against the emulator's call log, through a private andromeda-linkd. Proves:
# 1. Calls seeded into the emulator's call log (a missed, an incoming, an outgoing) reach D-Bus Calls with their kind,
#    newest first.
# 2. Link1 Dial places the call on the emulator (its call state goes off-hook); after it ends, the log's new outgoing
#    call reaches D-Bus Calls through calls-changed.
# Dial only on the emulator: it places a real call on a phone. Needs ANDROID_SERIAL=emulator-* and the debug APKs. Run
# under flock /tmp/link-emulator.lock. Writes steps.txt and linkd.log to $OUT (default ./artifacts/link-calls-phone).
set -euo pipefail
OUT=${OUT:-$(pwd)/artifacts/link-calls-phone}
source "$(dirname "$0")/link_phone_lib.sh"
DIALED=5550199

[[ $SERIAL == emulator-* ]] || fail "dials a number: run it on an emulator, not on $SERIAL"
[[ -n ${LINK_PHONE_LOCKED:-} ]] || exec env LINK_PHONE_LOCKED=1 flock --close /tmp/andromeda-link-phone.lock "$0" "$@"
rm -rf "$OUT"
mkdir -p "$OUT"
RUNTIME=$(mktemp -d /tmp/link-calls-phone.XXXX)
unseed() { adb shell content delete --uri content://call_log/calls --where "\"number LIKE '555014%' OR number='$DIALED'\""; }
trap 'adb shell input keyevent KEYCODE_ENDCALL; unseed; forget_phone; kill $(jobs -p) 2>/dev/null || true; wait 2>/dev/null; rm -rf "$RUNTIME"' EXIT
step() { printf '%s\n' "$*" | tee -a "$OUT/steps.txt"; }
[[ $(adb get-state 2>/dev/null) == device ]] || fail "no emulator at $SERIAL"
[[ -s $APK && -s $FIXTURE ]] || fail "no APKs at $APK and $FIXTURE"

# seed NUMBER NAME TYPE MINUTES_AGO DURATION: one call in the log; TYPE 1 incoming, 2 outgoing, 3 missed.
seed() {
  adb shell content insert --uri content://call_log/calls --bind "number:s:$1" --bind "name:s:'$2'" \
    --bind "type:i:$3" --bind "date:l:$(( $(now_ms) - $4 * 60000 ))" --bind "duration:i:$5" --bind new:i:1
}
unseed
seed 5550141 "E2E Missed" 3 5 0
seed 5550142 "E2E Incoming" 1 90 125
seed 5550143 "E2E Outgoing" 2 1500 40

private_bus
start_linkd
HOST=$(ip -4 route get 1.1.1.1 | grep -o 'src [0-9.]*' | cut -d' ' -f2)
install_app
pair_phone "$HOST:$PORT"
calls() { prop Calls; }
wait_for 30 "D-Bus Calls never listed the seeded calls: $(calls)" eval 'calls | grep -q "5550143"'
for want in "'5550141', 'E2E Missed'.*'missed'" "'5550142', 'E2E Incoming'.*'incoming'" "'5550143', 'E2E Outgoing'.*'outgoing'"; do
  calls | grep -q "$want" || fail "D-Bus Calls lacks $want: $(calls)"
done
step "D-Bus Calls holds the 3 seeded calls with their kinds"

# before NEWER OLDER: the phone's list holds NEWER ahead of OLDER (Calls is newest first).
before() { python3 -I -c 'import sys; s = sys.stdin.read(); a, b = s.find(sys.argv[1]), s.find(sys.argv[2]); sys.exit(not 0 <= a < b)' "$1" "$2"; }
calls | before 5550141 5550142 && calls | before 5550142 5550143 || fail "D-Bus Calls not newest first: $(calls)"
step "D-Bus Calls lists them newest first"

link Dial "$ID" "$DIALED" > /dev/null
offhook() { adb shell dumpsys telephony.registry | grep -q "mCallState=2"; }
wait_for 20 "the emulator never went off-hook for $DIALED" offhook
step "the emulator placed the call to $DIALED"
adb shell input keyevent KEYCODE_ENDCALL
wait_for 30 "D-Bus Calls never listed the outgoing call to $DIALED: $(calls)" eval \
  'calls | grep -q "'"'"'$DIALED'"'"'.*'"'"'outgoing'"'"'" && calls | before "$DIALED" 5550141'
step "the ended call reached D-Bus Calls through calls-changed"
echo PASS

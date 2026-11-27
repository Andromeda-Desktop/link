#!/usr/bin/env bash
# The fake phone against a running andromeda-linkd that serves the API on its socket, as the macOS app does:
#   tests/e2e/link_macos.sh PHONE_BINARY PAIRING_URI
# Proves: the phone pairs by the pairing URI (on the Mac, the one the window's QR code shows) and the desktop lists it;
# it connects and stays; text travels both ways; a file from the phone, accepted, lands in Downloads intact; the
# desktop lists the phone's photos; the mirror streams the phone's screen. Needs python3 with jeepney and ffmpeg.
# Writes results.jsonl, signals.jsonl, hold.jsonl, hold.log and transcript.jsonl to $OUT (default ./artifacts/link-macos).
set -euo pipefail
phone=$(realpath "${1:?phone binary}")
uri=${2:?pairing URI}
here=$(cd "$(dirname "$0")" && pwd)
OUT=${OUT:-$PWD/artifacts/link-macos}
mkdir -p "$OUT"
OUT=$(cd "$OUT" && pwd)
work=$(mktemp -d)
python=${PYTHON:-python3}
link1() { "$python" "$here/link1_socket.py" "$@"; }
pids=()
cleanup() {
  for pid in "${pids[@]}"; do kill "$pid" 2> /dev/null || true; done
  rm -rf "$work"
}
trap cleanup EXIT
fail() {
  echo "FAIL: $*" >&2
  tail -n 20 "$OUT/hold.log" >&2 || true
  exit 1
}
record() { printf '%s\n' "$1" >> "$OUT/results.jsonl"; }
# wait_for SECONDS WHAT COMMAND...: polls every 0.2 s.
wait_for() {
  local seconds=$1 what=$2 i
  shift 2
  for ((i = 0; i < seconds * 5; i++)); do "$@" > /dev/null 2>&1 && return 0; sleep 0.2; done
  fail "$what"
}
: > "$OUT/results.jsonl"

link1 watch > "$OUT/signals.jsonl" 2> "$OUT/watch.log" &
pids+=($!)
signal() { grep -F "\"signal\": \"$1\"" "$OUT/signals.jsonl" | grep -F -- "${2:-}"; }

"$phone" --state "$work/phone" --name "Mac E2E phone" pair --uri "$uri" > "$OUT/pair.log" 2>&1 || fail "pairing: $(cat "$OUT/pair.log")"
wait_for 10 "the desktop lists no paired phone" eval "link1 get Devices | grep -q '\"Mac E2E phone\"'"
id=$(link1 get Devices | "$python" -c 'import json,sys; print(next(d[0] for d in json.load(sys.stdin) if d[1] == "Mac E2E phone"))')
record "{\"step\":\"paired\"}"

# The phone grants the desktop its photos and its screen, as the app's switches do.
"$python" - "$work/phone/devices.json" << 'PY'
import json, sys
store = json.load(open(sys.argv[1]))
store["peers"][0].setdefault("grants", {}).update(browse=True, screen=True)
json.dump(store, open(sys.argv[1], "w"))
PY
mkdir -p "$work/photos/Camera"
"$python" - "$work/photos/Camera/beach.png" << 'PY'
import struct, sys, zlib
def chunk(kind, data): return struct.pack(">I", len(data)) + kind + data + struct.pack(">I", zlib.crc32(kind + data))
row = b"\0" + b"\x30\x90\xd0" * 8
png = b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", 8, 8, 8, 2, 0, 0, 0))
open(sys.argv[1], "wb").write(png + chunk(b"IDAT", zlib.compress(row * 8)) + chunk(b"IEND", b""))
PY
ffmpeg -loglevel error -f lavfi -i testsrc=size=64x64:rate=30 -t 2 -c:v libx264 -x264-params aud=1 \
  -bsf:v h264_mp4toannexb -f h264 "$work/screen.h264"
mkfifo "$work/stdin"
RUST_LOG=info "$phone" --state "$work/phone" --name "Mac E2E phone" --transcript "$OUT/transcript.jsonl" \
  --photos "$work/photos" --mirror-file "$work/screen.h264" --mirror-size 64x64 \
  hold --on-offer accept --status 80,1,wifi < "$work/stdin" > "$OUT/hold.jsonl" 2> "$OUT/hold.log" &
pids+=($!)
exec 3> "$work/stdin"
wait_for 30 "the phone never connected" grep -q '"event":"connected"' "$OUT/hold.jsonl"
wait_for 10 "the desktop does not show the phone connected" eval "link1 get Devices | grep -q '\"Mac E2E phone\", true'"
record "{\"step\":\"connected\"}"

echo "text hello from the phone" >&3
wait_for 10 "the phone's text did not reach the desktop" signal Received "hello from the phone"
link1 call Share "\"$id\"" '"text"' '"hello from the Mac"' > /dev/null
wait_for 10 "the desktop's text did not reach the phone" grep -q '"text":"hello from the Mac"' "$OUT/hold.jsonl"
record "{\"step\":\"text-both-ways\"}"

head -c 3000000 /dev/urandom > "$work/report.bin"
echo "send $work/report.bin" >&3
wait_for 20 "no transfer offer reached the desktop" signal TransferOffered "report.bin"
transfer=$(signal TransferOffered report.bin | head -n 1 | "$python" -c 'import json,sys; print(json.load(sys.stdin)["args"][0])')
link1 call AcceptTransfer "\"$transfer\"" > /dev/null
wait_for 60 "the transfer did not finish" signal TransferFinished "$transfer"
landed=$(find "${DOWNLOADS:-$HOME/Downloads}" -maxdepth 1 -name 'report*.bin' -newer "$work/report.bin" | head -n 1)
[[ -n $landed ]] || fail "the file is not in Downloads"
cmp -s "$landed" "$work/report.bin" || fail "the file in Downloads differs from the phone's"
rm -f "$landed"
record "{\"step\":\"file-to-desktop\",\"bytes\":3000000}"

photos=$(link1 call RecentPhotos "\"$id\"" 5) || fail "RecentPhotos: $photos"
[[ $photos == *beach.png* ]] || fail "the desktop does not list the phone's photo: $photos"
record "{\"step\":\"photos\"}"

mirror=$(link1 mirror "$id" 4) || fail "OpenMirror: $mirror"
"$python" -c 'import json,sys; m=json.loads(sys.argv[1]); sys.exit(m["bytes"] < 1000)' "$mirror" \
  || fail "the mirror streamed nothing: $mirror"
record "{\"step\":\"mirror\",\"stream\":$mirror}"
echo "PASS; artifacts: $OUT"

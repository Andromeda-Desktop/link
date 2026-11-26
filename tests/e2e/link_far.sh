#!/usr/bin/env bash
# Link between networks: the desktop and the phone each sit behind their own NAT (nft masquerade), with a relay
# (andromeda-link-relay) on the "internet" between them and 20 ms one-way delay per NAT uplink.
# MODE=eim (default): both NATs map endpoint-independently, as most home routers do.
# MODE=symmetric: the phone's NAT maps every flow to a random port, so no punch can work.
#
# Proves:
# 1. The desktop holds a relay connection from <state>/relay and its hello carries the relay and its own public
#    address; the phone stores both at pairing.
# 2. With no direct path, the held phone opens its session through the relay (via "relay").
# 3. eim: the phone opens its NAT, asks for a punch with its relay-observed address, the desktop punches, and the
#    session moves to the desktop's public address with its streams; a share then arrives over the direct path.
#    symmetric: the session stays on the relay and a share still arrives.
# 4. eim: when the direct path dies, the overdue heartbeat moves the same session back onto its relay link.
# Writes results.jsonl, hold.jsonl, linkd.log, hold.log, relay.log to $OUT (default ./artifacts/link-far).
set -euo pipefail
ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)
OUT=${OUT:-$(pwd)/artifacts/link-far}
BIN=${LINK_BIN:-$ROOT/target/debug}
MODE=${MODE:-eim}
if [[ -z ${LINK_IN_NS:-} ]]; then
  rm -rf "$OUT"
  mkdir -p "$OUT"
  exec env LINK_IN_NS=1 OUT="$OUT" BIN="$BIN" MODE="$MODE" unshare --user --map-root-user --net bash "$0"
fi

RUNTIME=$(mktemp -d /tmp/link-far.XXXX)
trap 'kill $(jobs -p) 2>/dev/null || true; wait 2>/dev/null; rm -rf "$RUNTIME"' EXIT
source "$(dirname "$0")/link_lib.sh"

# This namespace is the internet: relay 10.9.1.2, desktop NAT 10.9.2.2 (LAN 192.168.2.0/24), phone NAT 10.9.3.2
# (LAN 192.168.3.0/24).
mkns() {
  unshare --net sleep infinity &
  local pid=$!
  for _ in $(seq 50); do [[ $(readlink /proc/$pid/ns/net) != "$(readlink /proc/self/ns/net)" ]] && break; sleep 0.02; done
  printf -v "$1" %s "$pid"
}
in_ns() { local pid=$1; shift; nsenter -t "$pid" -n -- "$@"; }
for name in RELAY_NS DESK_NAT PHONE_NAT DESK_NS PHONE_NS; do mkns "$name"; done
ip link set lo up
sysctl -qw net.ipv4.ip_forward=1
uplink() {
  ip link add "$1" type veth peer name wan
  ip link set wan netns "$2"
  ip addr add "$3/24" dev "$1"
  ip link set "$1" up
  in_ns "$2" ip addr add "$4/24" dev wan
  in_ns "$2" ip link set wan up
  in_ns "$2" ip link set lo up
  in_ns "$2" ip route add default via "$3"
}
uplink r0 "$RELAY_NS" 10.9.1.1 10.9.1.2
uplink nd0 "$DESK_NAT" 10.9.2.1 10.9.2.2
uplink np0 "$PHONE_NAT" 10.9.3.1 10.9.3.2
lan() {
  ip link add lan type veth peer name eth0
  ip link set lan netns "$1"
  ip link set eth0 netns "$2"
  in_ns "$1" ip addr add "192.168.$3.1/24" dev lan
  in_ns "$1" ip link set lan up
  in_ns "$1" sysctl -qw net.ipv4.ip_forward=1
  in_ns "$2" ip addr add "192.168.$3.2/24" dev eth0
  in_ns "$2" ip link set eth0 up
  in_ns "$2" ip link set lo up
  in_ns "$2" ip route add default via "192.168.$3.1"
}
lan "$DESK_NAT" "$DESK_NS" 2
lan "$PHONE_NAT" "$PHONE_NS" 3
masquerade() {
  in_ns "$1" nft -f - <<NFT
table ip nat {
  chain post {
    type nat hook postrouting priority 100
    oifname "wan" masquerade $2
  }
}
NFT
}
masquerade "$DESK_NAT" ""
if [[ $MODE == symmetric ]]; then masquerade "$PHONE_NAT" "random,fully-random"; else masquerade "$PHONE_NAT" ""; fi
for dev in nd0 np0; do tc qdisc add dev "$dev" root netem delay 20ms; done
# Each NAT's outside, for triage: which side's packet crossed first.
for nat in DESK_NAT PHONE_NAT; do
  in_ns "${!nat}" tcpdump -l -n -i wan -tt udp > "$OUT/${nat,,}.txt" 2> /dev/null &
done

private_bus system

mkdir -p "$RUNTIME/relay" "$RUNTIME/desktop/andromeda-link" "$RUNTIME/downloads" "$RUNTIME/phone-downloads"
touch "$RUNTIME/relay/allow"
RUST_LOG=info in_ns "$RELAY_NS" "$BIN/andromeda-link-relay" --listen 10.9.1.2:4433 --state "$RUNTIME/relay" \
  > "$OUT/relay.log" 2>&1 &
wait_for 10 "the relay did not start" grep -q 'key [0-9a-f]\{64\}' "$OUT/relay.log"
KEY=$(grep -o 'key [0-9a-f]\{64\}' "$OUT/relay.log" | head -1 | cut -d' ' -f2)
echo "10.9.1.2:4433#$KEY" > "$RUNTIME/desktop/andromeda-link/relay"

XDG_STATE_HOME=$RUNTIME/desktop XDG_DOWNLOAD_DIR=$RUNTIME/downloads RUST_LOG=info \
  in_ns "$DESK_NS" "$BIN/andromeda-linkd" >> "$OUT/linkd.log" 2>&1 &
wait_for 10 "andromeda-linkd did not come up" grep -q 'listening on port' "$OUT/linkd.log"
DESK_ID=$(grep -o '^.*[0-9a-f]\{32\} listening on port' "$OUT/linkd.log" | grep -o '[0-9a-f]\{32\}' | head -1)
PORT=$(grep -o 'listening on port [0-9]*' "$OUT/linkd.log" | head -1 | grep -o '[0-9]*$')
echo "$DESK_ID" >> "$RUNTIME/relay/allow"
# The daemon dials the relay again 10 s after the refusal before the allow line.
wait_for 25 "the desktop never got its address from the relay" grep -q "seen at Some(10.9.2.2:$PORT)" "$OUT/linkd.log"
record "{\"step\":\"desktop-on-the-relay\",\"public\":\"10.9.2.2:$PORT\"}"

# Pairing needs a direct path: a cable between the two LANs, cut right after.
ip link add pairing type veth peer name pairing-p
ip link set pairing netns "$DESK_NS"
ip link set pairing-p netns "$PHONE_NS"
in_ns "$DESK_NS" ip addr add 10.77.0.1/24 dev pairing
in_ns "$DESK_NS" ip link set pairing up
in_ns "$PHONE_NS" ip addr add 10.77.0.2/24 dev pairing-p
in_ns "$PHONE_NS" ip link set pairing-p up
phone() {
  in_ns "$PHONE_NS" "$BIN/andromeda-link-phone" --state "$RUNTIME/phone" --name phone \
    --downloads "$RUNTIME/phone-downloads" "$@"
}
CODE=$(link StartPairing | sed -E "s/^\('([0-9]+)', .*/\1/")
PAIRED=$(phone pair --code "$CODE" --addr "10.77.0.1:$PORT") || fail "pairing: $PAIRED"
PHONE_ID=$(devices | grep -o "'[0-9a-f]\{32\}', 'phone'" | cut -d"'" -f2)
[[ -n $PHONE_ID ]] || fail "desktop does not list the phone: $(devices)"
echo "$PHONE_ID" >> "$RUNTIME/relay/allow"
in_ns "$DESK_NS" ip link del pairing
STORE=$(cat "$RUNTIME/phone/"*.json 2> /dev/null || true)
grep -q "\"relay\": *\"10.9.1.2:4433#$KEY\"" <<< "$STORE" || fail "the phone did not store the relay: $STORE"
grep -q "\"public\": *\"10.9.2.2:$PORT\"" <<< "$STORE" || fail "the phone did not learn the desktop's public address: $STORE"
record '{"step":"phone-stores-relay-and-public-address"}'

events() { grep -c "\"event\":\"$1\"" "$OUT/hold.jsonl" || true; }
START=$(date +%s)
RUST_LOG=info in_ns "$PHONE_NS" "$BIN/andromeda-link-phone" --state "$RUNTIME/phone" --name phone \
  --downloads "$RUNTIME/phone-downloads" hold --on-offer accept < /dev/null > "$OUT/hold.jsonl" 2> "$OUT/hold.log" &
wait_for 30 "the held phone never connected" eval '[[ $(events connected) -ge 1 ]]'
grep '"event":"connected"' "$OUT/hold.jsonl" | head -1 | grep -q '"via":"relay"' \
  || fail "the first session did not run through the relay: $(grep connected "$OUT/hold.jsonl" | head -1)"
record "{\"step\":\"session-through-the-relay\",\"seconds\":$(( $(date +%s) - START ))}"

if [[ $MODE == eim ]]; then
  wait_for 20 "the session never moved to a direct path" eval '[[ $(events connected) -ge 2 ]]'
  grep '"event":"connected"' "$OUT/hold.jsonl" | tail -1 | grep -q "\"addr\":\"10.9.2.2:$PORT\"" \
    || fail "the move went elsewhere: $(grep connected "$OUT/hold.jsonl" | tail -1)"
  grep -q 'punching toward \[10.9.3.2:' "$OUT/linkd.log" || fail "the desktop never punched toward the phone's NAT"
  grep -q 'moving the session from the relay to' "$OUT/hold.log" || fail "the phone did not move its session"
  record "{\"step\":\"punch-moves-direct\",\"seconds\":$(( $(date +%s) - START ))}"
else
  wait_for 20 "the desktop never punched" grep -q 'punching toward \[10.9.3.2:' "$OUT/linkd.log"
  wait_for 20 "the probe never finished" grep -q 'last-known addresses of' "$OUT/hold.log"
  [[ $(events connected) == 1 ]] || fail "a symmetric NAT session left the relay: $(grep connected "$OUT/hold.jsonl")"
  record '{"step":"symmetric-stays-on-the-relay"}'
fi

link Share "$PHONE_ID" text "'from afar'" > /dev/null || fail "Share"
wait_for 10 "the phone missed the share" grep -q '"text":"from afar"' "$OUT/hold.jsonl"
record '{"step":"share-arrives"}'

if [[ $MODE == eim ]]; then
  # The direct path dies (the desktop's NAT drops the phone's packets); the relay still forwards.
  in_ns "$DESK_NAT" nft -f - <<NFT
table ip cut {
  chain forward {
    type filter hook forward priority 0
    iifname "wan" ip saddr 10.9.3.2 drop
  }
}
NFT
  wait_for 30 "the session never fell back to the relay" eval '[[ $(events connected) -ge 3 ]]'
  grep '"event":"connected"' "$OUT/hold.jsonl" | tail -1 | grep -q '"via":"relay"' \
    || fail "the fallback went elsewhere: $(grep connected "$OUT/hold.jsonl" | tail -1)"
  grep -q 'moving the session to its standby Relay link' "$OUT/hold.log" || fail "the phone did not use its standby relay link"
  link Share "$PHONE_ID" text "'after the cut'" > /dev/null || fail "Share after the cut"
  wait_for 10 "the phone missed the share after the cut" grep -q '"text":"after the cut"' "$OUT/hold.jsonl"
  record "{\"step\":\"direct-path-dies-back-on-relay\",\"seconds\":$(( $(date +%s) - START ))}"
fi
cat "$OUT/results.jsonl"
echo "PASS ($MODE); artifacts: $OUT"

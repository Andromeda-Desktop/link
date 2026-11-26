# Shared by the Link E2E scripts: a private bus, a private andromeda-linkd on it, and the Link1 D-Bus helpers.
# Source it after setting OUT and RUNTIME.
ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)
BIN=${LINK_BIN:-$ROOT/target/debug}

fail() { echo "FAIL: $*" >&2; exit 1; }
record() { printf '%s\n' "$1" >> "$OUT/results.jsonl"; }
wait_for() {
  local tries=$(( $1 * 10 )) what=$2; shift 2
  for _ in $(seq "$tries"); do "$@" && return; sleep 0.1; done
  fail "$what"
}
now_ms() { date +%s%3N; }
# stamp: each line of stdin with the Unix time in ms in front.
stamp() { while IFS= read -r line; do printf '%s %s\n' "$(date +%s%3N)" "$line"; done; }
link() { gdbus call --session -d org.andromeda.Link1 -o /org/andromeda/Link1 -m "org.andromeda.Link1.$1" "${@:2}"; }
prop() { gdbus call --session -d org.andromeda.Link1 -o /org/andromeda/Link1 -m org.freedesktop.DBus.Properties.Get org.andromeda.Link1 "$1"; }
devices() { prop Devices; }
connected() { devices | grep -q "', true)"; }

# private_bus: a session bus of the run's own; with "system", also its system bus.
private_bus() {
  cat > "$RUNTIME/bus.conf" <<CONF
<busconfig>
  <type>session</type>
  <listen>unix:path=$RUNTIME/bus</listen>
  <auth>EXTERNAL</auth>
  <policy context="default">
    <allow user="*"/>
    <allow send_destination="*" eavesdrop="true"/>
    <allow eavesdrop="true"/>
    <allow own="*"/>
  </policy>
</busconfig>
CONF
  dbus-daemon --config-file="$RUNTIME/bus.conf" --nofork 2> "$OUT/dbus.log" &
  for _ in $(seq 50); do [[ -S $RUNTIME/bus ]] && break; sleep 0.02; done
  export DBUS_SESSION_BUS_ADDRESS=unix:path=$RUNTIME/bus
  [[ ${1:-} == system ]] && export DBUS_SYSTEM_BUS_ADDRESS=unix:path=$RUNTIME/bus
  true
}

linkd_up() { link CancelPairing > /dev/null 2>&1; }

# run_linkd [VAR=VALUE]...: a private andromeda-linkd with those variables, its log in $OUT/linkd.log (each line through
# $LINKD_LOG_FILTER, default cat), state in $RUNTIME/desktop and downloads in $RUNTIME/downloads; returns once it answers.
run_linkd() {
  mkdir -p "$RUNTIME/downloads"
  env XDG_STATE_HOME="$RUNTIME/desktop" XDG_DOWNLOAD_DIR="$RUNTIME/downloads" RUST_LOG="${RUST_LOG:-info,zbus=warn,tracing=warn}" \
    "$@" "$BIN/andromeda-linkd" 2>&1 | ${LINKD_LOG_FILTER:-cat} > "$OUT/linkd.log" &
  wait_for 5 "andromeda-linkd did not come up" linkd_up
}

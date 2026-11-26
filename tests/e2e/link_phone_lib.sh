# Shared by the real-phone E2E scripts (link_*_phone.sh): the phone lock, a private bus, a private andromeda-linkd, the
# debug app paired with it, and taps on Android's own UI through uiautomator. Source it after setting OUT.
# Needs ANDROID_SERIAL, the debug APKs, and the phone on USB debugging.
source "$(dirname "${BASH_SOURCE[0]}")/link_lib.sh"
ADB=${ADB:-$HOME/Android/Sdk/platform-tools/adb}
SERIAL=${ANDROID_SERIAL:?set ANDROID_SERIAL to the phone}
APK=${APK:-$ROOT/apps/android/app/build/outputs/apk/debug/app-debug.apk}
FIXTURE=${FIXTURE:-$ROOT/apps/android/fixture/build/outputs/apk/debug/fixture-debug.apk}
PACKAGE=org.andromeda.link
export ANDROMEDA_LINK_NO_USB=1 # a test linkd runs no adb reverse, so the phone keeps the path the test set up

adb() { "$ADB" -s "$SERIAL" "$@"; }
# A server first started under the phone lock keeps the lock's descriptor and holds the lock for good.
[[ -n ${LINK_PHONE_LOCKED:-} ]] || "$ADB" start-server > /dev/null 2>&1 || true

start_linkd() {
  # The firewall admits only Link's own port, so the user's daemon gives it up for the run; the EXIT trap (set before
  # this call) starts it again.
  if systemctl --user is-active -q andromeda-linkd; then
    # Masked first: a session client calling org.andromeda.Link1 bus-activates the daemon, and that start cancels the stop.
    systemctl --user mask --runtime -q andromeda-linkd
    systemctl --user stop andromeda-linkd
    eval "set -- $(trap -p EXIT)"
    # set +e: a failing cleanup step must not skip the restart.
    trap "set +e; ${3:-}
systemctl --user unmask --runtime -q andromeda-linkd
systemctl --user start andromeda-linkd" EXIT
  fi
  LINKD_LOG_FILTER=stamp run_linkd
  wait_for 5 "andromeda-linkd logged no port" grep -q 'listening on port' "$OUT/linkd.log"
  dbus-monitor --session "type='signal',interface='org.andromeda.Link1'" > "$OUT/signals.txt" 2>&1 &
  # The user allows every hotspot question, as a tap on "Use hotspot".
  gdbus monitor --session -d org.andromeda.Link1 | while read -r line; do
    [[ $line =~ HotspotRequested\ \(\'([0-9a-f]+)\',\ \'[^\']+\'\) ]] && link AnswerHotspot "${BASH_REMATCH[1]}" true > /dev/null
  done &
  PORT=$(grep -o 'listening on port [0-9]*' "$OUT/linkd.log" | grep -o '[0-9]*$')
}

# install_app: Link replaced by the debug build (a release build's key differs, so it goes first), with its runtime
# permissions granted at install, since ColorOS refuses `pm grant`.
install_app() {
  adb shell input keyevent KEYCODE_WAKEUP
  adb shell cmd statusbar collapse
  # An update, not a fresh install: the phone keeps the pairings of other runs and of the user.
  adb install -r -g "$APK" > /dev/null || fail "installing $APK; a release build on the phone has another key"
  adb install -r "$FIXTURE" > /dev/null || fail "installing $FIXTURE"
  record '{"step":"installed"}'
}

ui() { adb shell uiautomator dump /sdcard/ui.xml > /dev/null 2>&1; adb shell cat /sdcard/ui.xml > "$OUT/ui-$1.xml"; }
# bounds NAME TEXT: the centre of the first node whose text or description is TEXT (any case), as "x y", from dump
# NAME; a prefix match with a trailing '*', a substring match with '*' at both ends.
bounds() {
  python3 - "$OUT/ui-$1.xml" "$2" <<'PY'
import re, sys
xml, want = open(sys.argv[1]).read(), sys.argv[2].lower()
for node in re.finditer(r'<node [^>]*>', xml):
    attrs = dict(re.findall(r'(\w[\w-]*)="([^"]*)"', node.group(0)))
    # Some vendor apps label buttons only by id, e.g. com.oplus.camera:id/shutter_button.
    labels = [attrs.get(key, "").lower() for key in ("text", "content-desc")] + [attrs.get("resource-id", "").split("/")[-1]]
    def matches(label):
        if want.startswith("*") and want.endswith("*"):
            return want[1:-1] in label
        return label.startswith(want[:-1]) if want.endswith("*") else label == want
    if any(matches(label) for label in labels):
        l, t, r, b = map(int, re.findall(r"\d+", attrs["bounds"]))
        print((l + r) // 2, (t + b) // 2)
        break
PY
}
# tap_text NAME TEXT...: wait for any of TEXT on screen and tap it.
tap_text() {
  local name=$1; shift
  for _ in $(seq 40); do
    ui "$name"
    for text in "$@"; do
      local at
      at=$(bounds "$name" "$text")
      if [[ -n $at ]]; then adb shell input tap $at; return; fi
    done
    sleep 0.5
  done
  fail "none of $* appeared on screen"
}

# pair_phone ADDRESS: pairs the app with the private daemon at ADDRESS (host:port), plus its Bluetooth adapter when
# the daemon has one; sets ID.
pair_phone() {
  local uri
  uri=$(link StartPairing | sed -E "s/^\('[0-9]+', '([^']+)'\)$/\1/" | sed -E "s/&a=[^&]*//g")
  uri="$uri&a=$1"
  adb shell am start -W -a android.intent.action.VIEW -d "'$uri'" "$PACKAGE" > /dev/null
  # The clipboard watcher asks for log access when the app opens, which can cover the pairing prompt.
  for _ in 1 2; do
    tap_text pair "Allow one-time access" Pair
    [[ -n $(bounds pair "Allow one-time access") ]] || break
  done
  wait_for 30 "D-Bus does not show the phone connected: $(devices)" connected
  ID=$(devices | grep -o "'[0-9a-f]\{32\}'" | head -1 | tr -d "'")
  DESKTOP=$(phone_peers | tail -1)
}
# phone_peers: the ids of the desktops the phone is paired with, oldest first.
phone_peers() {
  adb shell run-as "$PACKAGE" cat files/devices.json \
    | python3 -c 'import json,sys; print(*(p["id"] for p in json.load(sys.stdin)["peers"]), sep="\n")'
}
# forget_phone: removes the run's desktop from the stopped app's store, and keeps every other pairing.
forget_phone() {
  [[ -n ${DESKTOP:-} ]] || return 0
  adb shell am force-stop "$PACKAGE"
  adb shell run-as "$PACKAGE" cat files/devices.json > "$RUNTIME/forget.json" || return 0
  python3 - "$RUNTIME/forget.json" "$DESKTOP" <<'PY'
import json, sys
store = json.load(open(sys.argv[1]))
store["peers"] = [peer for peer in store["peers"] if peer["id"] != sys.argv[2]]
json.dump(store, open(sys.argv[1], "w"))
PY
  adb shell run-as "$PACKAGE" sh -c "'cat > files/devices.json'" < "$RUNTIME/forget.json"
}

# phone_switches SWITCH...: turns those of the phone's switches for the desktop on, and Stay connected, with the app
# stopped, then starts it and waits for its new session.
phone_switches() {
  local sessions
  sessions=$(grep -c "connected over" "$OUT/linkd.log")
  adb shell am force-stop "$PACKAGE"
  adb shell run-as "$PACKAGE" cat files/devices.json > "$RUNTIME/phone.json"
  python3 - "$RUNTIME/phone.json" "$@" <<'PY'
import json, sys
store = json.load(open(sys.argv[1]))
for switch in sys.argv[2:]:
    store["peers"][-1].setdefault("grants", {})[switch] = True
json.dump(store, open(sys.argv[1], "w"))
PY
  adb shell run-as "$PACKAGE" sh -c "'cat > files/devices.json'" < "$RUNTIME/phone.json"
  printf '<?xml version="1.0" encoding="utf-8" standalone="yes" ?>\n<map><boolean name="stay_connected" value="true" /></map>\n' \
    | adb shell run-as "$PACKAGE" sh -c "'mkdir -p shared_prefs && cat > shared_prefs/presence.xml'"
  # A fresh task: the old one still holds the pairing link, which would open again.
  adb shell am start -W --activity-clear-task -n "$PACKAGE/.MainActivity" > /dev/null
  wait_for 30 "the app did not reconnect" eval '(( $(grep -c "connected over" "$OUT/linkd.log") > sessions ))'
}

# allow_mirror NAME [REQUEST STARTED]: Allow on the request, as a heads-up or in the shade (ColorOS can keep a pinned
# heads-up out of the shade), then Start on Android's capture prompt. Goes home first: a fullscreen app can hide both.
# REQUEST is the notification's text, STARTED the linkd log line of a started capture (default: the mirror's).
allow_mirror() {
  local request=${2:-wants to show your screen} started=${3:-allowed mirroring}
  # With a root or Shizuku helper the phone starts the capture itself, with no notification or prompt.
  local before
  before=$(grep -c "$started" "$OUT/linkd.log" || true)
  wait_for 75 "no capture request notification" eval '(( $(grep -c "$started" "$OUT/linkd.log") > before )) ||
    adb shell dumpsys notification --noredact | grep -qF "$request"'
  (( $(grep -c "$started" "$OUT/linkd.log") > before )) && return
  adb shell input keyevent KEYCODE_HOME
  local at="" was="" try text
  # The shade moves while it opens, so a position counts only once two dumps a second apart agree on it. A tap on a
  # request inside a group of Link's notifications only opens the group, so the request is tapped until the prompt shows.
  for try in $(seq 30); do
    ui "prompt-$1"
    for text in Start "Start now" "Share screen"; do
      at=$(bounds "prompt-$1" "$text")
      [[ -z $at ]] || { adb shell input tap $at; adb shell cmd statusbar collapse; return; }
    done
    at=$(bounds "prompt-$1" Allow)
    [[ -n $at ]] || at=$(bounds "prompt-$1" "*$request*")
    if [[ -n $at && $at == "$was" ]]; then
      adb shell input tap $at
      was=""
      sleep 1.5
    elif [[ -n $at ]]; then
      was=$at
      sleep 1
    else
      was=""
      if (( try % 2 )); then adb shell cmd statusbar expand-notifications; else adb shell cmd statusbar collapse; fi
      sleep 0.7
    fi
  done
  fail "the capture request did not lead to the capture prompt"
}

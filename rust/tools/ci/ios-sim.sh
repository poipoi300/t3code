#!/usr/bin/env bash
# Installs and launches the iOS simulator build in a simulator, waits for its connect screen by
# reading the screen's text, and captures it.
#
#   ios-sim.sh --app "<out>/T3 Code Dev.app" --out <dir> --label <name> [--device <name or UDID>] [--shutdown]
#
# Without --device it uses a booted iPhone simulator, else boots "iPhone 16" (or the first iPhone)
# on the newest installed iOS runtime. Writes <out>/<label>-ios-connect.{png,txt} (txt: the text OCR
# read) and <label>-ios-app.log (the app's unified log). Leaves the simulator booted for the next
# capture unless --shutdown.
set -euo pipefail
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

app='' out='' label='' device='' shutdown=0
while (($#)); do
  case $1 in
    --app) app=$2 && shift ;;
    --out) out=$2 && shift ;;
    --label) label=$2 && shift ;;
    --device) device=$2 && shift ;;
    --shutdown) shutdown=1 ;;
    *) ci_die "unknown argument $1" ;;
  esac
  shift
done
[[ -n $app && -n $out && -n $label ]] || ci_die "usage: ios-sim.sh --app <path.app> --out <dir> --label <name> [--device <name|udid>] [--shutdown]"
[[ -d $app ]] || ci_die "no app bundle at $app (rust/shells/ui.ps1 ios-sim)"
[[ $(uname -s) == Darwin ]] || ci_die "macOS only"
mkdir -p "$out"
out=$(cd "$out" && pwd)
prefix="$out/$label-ios"
bundle=$(/usr/libexec/PlistBuddy -c 'Print :CFBundleIdentifier' "$app/Info.plist")

udid=$(xcrun simctl list devices available -j | python3 -I -c '
import json, re, sys
wanted = sys.argv[1]
runtimes = json.load(sys.stdin)["devices"]
def version(key):
    return [int(n) for n in re.findall(r"\d+", key.rsplit("iOS", 1)[-1])]
phones = [(version(key), device) for key, devices in runtimes.items() if ".iOS-" in key
          for device in devices if device["name"].startswith("iPhone")]
if wanted:
    match = [d for _, d in phones if wanted in (d["udid"], d["name"])]
else:
    match = [d for _, d in phones if d["state"] == "Booted"]
    newest = max((v for v, _ in phones), default=None)
    match += [d for v, d in phones if v == newest and d["name"] == "iPhone 16"]
    match += [d for v, d in phones if v == newest]
print(match[0]["udid"] if match else "")
' "$device")
[[ -n $udid ]] || ci_die "no matching iPhone simulator (xcrun simctl list devices available)"
ci_log "simulator $udid"
xcrun simctl boot "$udid" 2> /dev/null || true
xcrun simctl bootstatus "$udid" -b > /dev/null
xcrun simctl install "$udid" "$app"

xcrun simctl spawn "$udid" log stream --level debug --style compact \
  --predicate "subsystem == \"$bundle\" OR process == \"$(basename "$app" .app)\"" > "$prefix-app.log" 2>&1 &
log_pid=$!
cleanup() {
  xcrun simctl terminate "$udid" "$bundle" 2> /dev/null || true
  kill "$log_pid" 2> /dev/null || true
  ((shutdown)) && xcrun simctl shutdown "$udid" || true
}
trap cleanup EXIT
xcrun simctl launch --terminate-running-process "$udid" "$bundle"

shows() {
  xcrun simctl io "$udid" screenshot --type=png "$prefix-$1.png" > /dev/null 2>&1 &&
    ci_probe ocr "$prefix-$1.png" > "$prefix-$1.txt" && ocr_has "$prefix-$1.txt" "$2"
}
if ! wait_until 180 "the connect screen" shows connect "Connect to T3 Code"; then
  ci_log "last text read from the screen:" && cat "$prefix-connect.txt" >&2 2> /dev/null || true
  xcrun simctl spawn "$udid" launchctl list | grep -q "UIKitApplication:$bundle" || ci_log "the app is not running"
  find "$HOME/Library/Logs/DiagnosticReports" -name '*.ips' -newer "$prefix-app.log" \
    -exec cp {} "$out/" \; 2> /dev/null || true
  ci_die "the connect screen did not show; see $prefix-app.log and any crash report in $out"
fi
ci_log "connect screen: $prefix-connect.png"

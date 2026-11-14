#!/usr/bin/env bash
# Starts the macOS desktop app (the `desktop` shell, AppKit), waits for its connect screen by
# reading the window's text, and captures it. With --pair-url it then pairs through the app's own
# form (System Events, the Accessibility API) and captures the thread list.
#
#   macos-desktop.sh --app <t3-desktop> --out <dir> --label <name> [--pair-url <url>] [--pairing-optional]
#
# Writes <out>/<label>-macos-connect.{png,txt} (txt: the text OCR read), -shell.{png,txt} when paired,
# and -app.log. The app's data goes to <out>/<label>-home; its storage key still goes to the login
# keychain as "T3 Code client storage". Exit 0: connect screen shown (and paired, unless optional).
set -euo pipefail
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

app='' out='' label='' pair_url='' pairing_optional=0
while (($#)); do
  case $1 in
    --app) app=$2 && shift ;;
    --out) out=$2 && shift ;;
    --label) label=$2 && shift ;;
    --pair-url) pair_url=$2 && shift ;;
    --pairing-optional) pairing_optional=1 ;;
    *) ci_die "unknown argument $1" ;;
  esac
  shift
done
[[ -n $app && -n $out && -n $label ]] || ci_die "usage: macos-desktop.sh --app <t3-desktop> --out <dir> --label <name> [--pair-url <url>] [--pairing-optional]"
[[ -x $app ]] || ci_die "no desktop app at $app (rust/shells/ui.ps1 desktop-build)"
[[ $(uname -s) == Darwin ]] || ci_die "macOS only"
mkdir -p "$out"
out=$(cd "$out" && pwd)
prefix="$out/$label-macos"

(
  isolate_home "$out/$label-home"
  exec "$app"
) > "$prefix-app.log" 2>&1 &
pid=$!
ci_log "desktop app PID $pid"
stop_app() {
  kill "$pid" 2> /dev/null || true
  [[ -n ${CI:-} ]] || ci_log "note: this run may have added a \"T3 Code client storage\" item to your login keychain"
}
trap stop_app EXIT

alive() { kill -0 "$pid" 2> /dev/null || ci_die "the app exited; see $prefix-app.log"; }
window_id() { alive && wid=$(ci_probe window "$pid"); }
# shows <name> <phrase>: captures the window and reads it; true once the phrase is on screen.
shows() {
  alive
  screencapture -x -o -l "$wid" "$prefix-$1.png" && ci_probe ocr "$prefix-$1.png" > "$prefix-$1.txt" && ocr_has "$prefix-$1.txt" "$2"
}
give_up() {
  screencapture -x "$prefix-$1-screen.png" || true
  ci_log "last text read from the window:" && cat "$prefix-$1.txt" >&2 2> /dev/null || true
  ci_die "$2"
}

wid=''
wait_until 120 "the app's window" window_id || give_up window "no window appeared"
ci_log "window $wid"
wait_until 120 "the connect screen" shows connect "Connect to T3 Code" || give_up connect "the connect screen did not show"
ci_log "connect screen: $prefix-connect.png"
[[ -n $pair_url ]] || exit 0

# Types the link into the "Pairing link" field and presses "Connect", found by their accessibility
# labels (AXDescription), as the Windows proof does through UI Automation.
pair() {
  osascript - "$pid" "$pair_url" << 'APPLESCRIPT'
on find(theWindow, theRole, theLabel)
  repeat with element in (entire contents of theWindow)
    try
      if role of element is theRole and description of element is theLabel then return element
    end try
  end repeat
  error "no " & theRole & " labelled " & theLabel
end find
on run argv
  set appPid to (item 1 of argv) as integer
  set link to item 2 of argv
  tell application "System Events"
    set proc to first process whose unix id is appPid
    set frontmost of proc to true
    set theWindow to window 1 of proc
    set field to my find(theWindow, "AXTextField", "Pairing link")
    set focused of field to true
    keystroke link
    perform action "AXPress" of my find(theWindow, "AXButton", "Connect")
  end tell
end run
APPLESCRIPT
}
pairing_failed() {
  if ((pairing_optional)); then
    echo "::warning::macOS pairing through the app's form failed: $1"
    exit 0
  fi
  give_up shell "$1"
}
pair || pairing_failed "System Events could not drive the form (Accessibility permission?)"
wait_until 60 "the seeded thread in the sidebar" shows shell "Replay showcase" || pairing_failed "the thread list did not show after pairing"
ci_log "paired: $prefix-shell.png"

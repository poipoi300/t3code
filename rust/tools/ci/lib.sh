# shellcheck shell=bash
# Helpers shared by the scripts in this directory (sourced, not run).

CI_DIR=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
# shellcheck disable=SC2034 # used by the scripts that source this file
REPO_ROOT=$(cd "$CI_DIR/../../.." && pwd)

ci_log() { printf '[%s] %s\n' "$(date +%H:%M:%S)" "$*" >&2; }
ci_die() {
  ci_log "error: $*"
  exit 1
}

# wait_until <seconds> <what> <command...>: runs the command every half second until it succeeds.
wait_until() {
  local seconds=$1 what=$2
  shift 2
  local deadline=$(($(date +%s) + seconds))
  until "$@"; do
    if (($(date +%s) >= deadline)); then
      ci_log "timed out after ${seconds}s waiting for $what"
      return 1
    fi
    sleep 0.5
  done
}

# A path as the programs we start expect it: Windows form under Git Bash, unchanged elsewhere.
native_path() {
  if command -v cygpath > /dev/null; then cygpath -w "$1"; else printf '%s' "$1"; fi
}

# Points every home and data directory at <dir> and drops T3 Code's own variables, so nothing
# the started program does reads or writes the real account's state. Call in a subshell.
isolate_home() {
  local dir=$1
  mkdir -p "$dir/AppData/Roaming" "$dir/AppData/Local"
  export HOME USERPROFILE APPDATA LOCALAPPDATA XDG_CONFIG_HOME XDG_DATA_HOME
  HOME=$(native_path "$dir")
  USERPROFILE=$HOME
  APPDATA=$(native_path "$dir/AppData/Roaming")
  LOCALAPPDATA=$(native_path "$dir/AppData/Local")
  XDG_CONFIG_HOME="$dir/.config"
  XDG_DATA_HOME="$dir/.local/share"
  unset VITE_DEV_SERVER_URL VITE_HTTP_URL VITE_WS_URL T3_UI_BACKEND
  local name
  for name in $(compgen -e); do
    [[ $name == T3CODE_* ]] && unset "$name"
  done
  return 0
}

# apple-probe (macOS only): compiled once from apple-probe.swift into $T3_CI_BIN.
ci_probe() {
  local bin_dir=${T3_CI_BIN:-${TMPDIR:-/tmp}/t3-ci-bin}
  local bin="$bin_dir/apple-probe" src="$CI_DIR/apple-probe.swift"
  if [[ ! -x $bin || $src -nt $bin ]]; then
    mkdir -p "$bin_dir"
    xcrun swiftc -O "$src" -o "$bin" >&2 || ci_die "cannot compile $src"
  fi
  "$bin" "$@"
}

# ocr_has <text-file> <phrase>: the OCR lines, joined with spaces, contain the phrase (any case).
ocr_has() {
  local text phrase
  text=$(tr '\n' ' ' < "$1" | tr -s ' ' | tr '[:upper:]' '[:lower:]')
  phrase=$(tr '[:upper:]' '[:lower:]' <<< "$2")
  [[ $text == *"$phrase"* ]]
}

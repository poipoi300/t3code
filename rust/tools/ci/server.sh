#!/usr/bin/env bash
# The Rust server in dev mode (the Replay (dev) provider), on a scratch home, for the app captures.
#
#   server.sh start <run-dir> --t3 <t3 binary> [--port <n>]   start, wait until it is ready
#   server.sh seed  <run-dir> --seed <seed_thread binary>     create the "Replay showcase" thread
#   server.sh pair  <run-dir>                                 print a fresh pairing URL for an app
#   server.sh stop  <run-dir>                                 stop the server started above
#
# Every home the server sees is <run-dir>/home. The PID captured at start is the only one stopped.
# Build the binaries from rust/: `cargo build --locked -p t3-server --bin t3`, and from
# rust/crates/t3-ui: `cargo build --locked --example seed_thread`.
set -euo pipefail
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

usage() { ci_die "usage: server.sh start|seed|pair|stop <run-dir> [--t3 <bin>] [--seed <bin>] [--port <n>]"; }
(($# >= 2)) || usage
command=$1 run=$2
shift 2
t3='' seed='' port=0
while (($#)); do
  case $1 in
    --t3) t3=$2 ;;
    --seed) seed=$2 ;;
    --port) port=$2 ;;
    *) usage ;;
  esac
  shift 2
done
mkdir -p "$run"
run=$(cd "$run" && pwd)
log="$run/server.log"

# The value after "<key>: " on the first line of the server's output that has it.
startup() { sed -n "s/^$1: //p" "$log" | head -n 1 | tr -d '\r'; }
alive() { kill -0 "$(cat "$run/server.pid")" 2> /dev/null; }
ready() {
  alive || {
    cat "$log" >&2
    ci_die "the server exited during startup"
  }
  grep -q '^Pairing URL: ' "$log"
}

case $command in
  start)
    [[ -n $t3 ]] || usage
    [[ -x $t3 ]] || ci_die "no server binary at $t3"
    [[ ! -f $run/server.pid ]] || ! alive || ci_die "a server from $run is still running (PID $(cat "$run/server.pid"))"
    printf '%s\n' "$t3" > "$run/t3-path"
    ci_log "starting: $t3 serve --home-dir $run/home --host 127.0.0.1 --port $port --dev-url http://127.0.0.1:5733"
    # A dev URL is what turns on dev mode; nothing is served there, and the apps pair with the
    # server's own origin.
    (
      isolate_home "$run/home"
      exec "$t3" serve --home-dir "$(native_path "$run/home")" --host 127.0.0.1 --port "$port" \
        --dev-url http://127.0.0.1:5733
    ) > "$log" 2>&1 &
    echo $! > "$run/server.pid"
    ci_log "server PID $(cat "$run/server.pid"), log $log"
    ci_log "waiting (up to 120 s) for the server's \"Pairing URL:\" line in $log"
    wait_until 120 "the server's startup lines" ready
    startup 'Connection string' > "$run/origin"
    startup Token > "$run/startup-token"
    ci_log "server ready at $(cat "$run/origin")"
    ;;
  seed)
    [[ -n $seed ]] || usage
    [[ -x $seed ]] || ci_die "no seed_thread binary at $seed"
    # The startup token is single-use: the seeding client spends it, apps get their own (pair).
    ci_log "seeding: $seed $(cat "$run/origin") <startup token> $run/seed-client replay-showcase \"Replay showcase\" devReplay showcase"
    "$seed" "$(cat "$run/origin")" "$(cat "$run/startup-token")" "$(native_path "$run/seed-client")" \
      replay-showcase "Replay showcase" devReplay showcase
    ci_log "seeded the thread \"Replay showcase\""
    ;;
  pair)
    ci_log "minting a pairing link: t3 pair --home-dir $run/home --label ci"
    out=$(
      isolate_home "$run/home"
      "$(cat "$run/t3-path")" pair --home-dir "$(native_path "$run/home")" --label ci
    )
    # In dev mode `t3 pair` also prints the server's own origin, which the native apps pair with.
    url=$(sed -n 's/^Server pairing URL: //p' <<< "$out" | tr -d '\r')
    [[ -n $url ]] || url=$(sed -n 's/^Pairing URL: //p' <<< "$out" | tr -d '\r')
    [[ -n $url ]] || ci_die "t3 pair printed no pairing URL"
    printf '%s\n' "$url"
    ;;
  stop)
    [[ -f $run/server.pid ]] || exit 0
    pid=$(cat "$run/server.pid")
    if kill -0 "$pid" 2> /dev/null; then
      ci_log "stopping server PID $pid (SIGTERM)"
      kill "$pid"
      wait_until 30 "server PID $pid to exit" bash -c "! kill -0 $pid 2> /dev/null" || kill -9 "$pid"
      ci_log "stopped server PID $pid"
    fi
    rm -f "$run/server.pid"
    ;;
  *) usage ;;
esac

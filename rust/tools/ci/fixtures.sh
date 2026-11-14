#!/usr/bin/env bash
# The generated fixtures under rust/fixtures that git ignores (samples/, non-scalar/,
# contracts.schema.json, differential.jsonl, about 95 MB) and that crates embed with include_str!.
#
#   fixtures.sh generate   run the exporter; needs the Node version package.json names and the server's
#                          workspace dependencies (`vp install --filter=t3...`)
#   fixtures.sh check      fail unless every generated file is present (after an artifact download)
set -euo pipefail
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

generated=(samples non-scalar contracts.schema.json differential.jsonl)
fixtures="$REPO_ROOT/rust/fixtures"

present() {
  local name missing=0
  for name in "${generated[@]}"; do
    if [[ -e $fixtures/$name ]]; then
      ci_log "present: rust/fixtures/$name ($(du -sh "$fixtures/$name" | cut -f1))"
    else
      ci_log "missing: rust/fixtures/$name"
      missing=1
    fi
  done
  return "$missing"
}

case ${1:-} in
  generate)
    ci_log "node $(node --version), exporter rust/tools/export/main.ts (seed 29747, deterministic)"
    log=${RUNNER_TEMP:-${TMPDIR:-/tmp}}/export.log
    ci_log "running the exporter, output in $log"
    if ! node "$REPO_ROOT/rust/tools/export/main.ts" > "$log" 2>&1; then
      tail -n 60 "$log" >&2
      ci_die "the exporter failed; see $log"
    fi
    tail -n 1 "$log"
    present || ci_die "the exporter did not write every generated file"
    # The tracked fixtures were generated from the same TypeScript; a difference means drift between the
    # committed fixtures and the contracts, which the owners should know, not a reason to stop here.
    drift=$(git -C "$REPO_ROOT" status --porcelain -- rust/fixtures)
    if [[ -n $drift ]]; then
      echo "::warning::the exporter changed tracked fixtures (committed fixtures and TypeScript have drifted):"
      echo "$drift"
    else
      ci_log "tracked fixtures unchanged by the export"
    fi
    ;;
  check)
    present || ci_die "generated fixtures are missing (the fixtures job's artifact rust-fixtures)"
    ;;
  *) ci_die "usage: fixtures.sh generate | check" ;;
esac

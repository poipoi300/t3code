#!/usr/bin/env bash
# The one-line change of the Apple proof: one string in the shared UI crate, on the connect
# screen, which both Apple apps show without pairing. Nothing in a shell changes.
#
#   one-change.sh apply                                   make the edit; fail unless it is the only change
#   one-change.sh revert                                  undo it (for a developer's checkout)
#   one-change.sh verify <before.txt> <after.txt> <name>  check two OCR readings of the connect screen
set -euo pipefail
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

file=rust/crates/t3-ui/src/screens/connect.rs
before_text='Paste the pairing link your server printed, or enter its address and a pairing code.'
after_text='One change reached every platform.'
title='Connect to T3 Code'

replace() { # <from> <to>
  local count
  count=$(grep -cF -- "t(\"$1\")" "$REPO_ROOT/$file" || true)
  [[ $count == 1 ]] || ci_die "expected t(\"$1\") once in $file, found ${count:-0}: update this script with the screen"
  FROM="t(\"$1\")" TO="t(\"$2\")" perl -0pi -e 's/\Q$ENV{FROM}\E/$ENV{TO}/' "$REPO_ROOT/$file"
}

case ${1:-} in
  apply)
    ci_log "applying the one change in $file: t(\"$before_text\") -> t(\"$after_text\")"
    status_before=$(git -C "$REPO_ROOT" status --porcelain -- rust .github)
    replace "$before_text" "$after_text"
    status_after=$(git -C "$REPO_ROOT" status --porcelain -- rust .github)
    numstat=$(git -C "$REPO_ROOT" diff --numstat -- "$file")
    [[ $numstat == $'1\t1\t'"$file" ]] || ci_die "the edit is not one line: $numstat"
    changed=$(comm -13 <(sort <<< "$status_before") <(sort <<< "$status_after"))
    [[ -z $changed || $changed == " M $file" ]] || ci_die "files other than $file changed: $changed"
    git -C "$REPO_ROOT" diff -- "$file"
    ;;
  revert)
    replace "$after_text" "$before_text"
    ;;
  verify)
    (($# == 4)) || ci_die "usage: one-change.sh verify <before.txt> <after.txt> <name>"
    before=$2 after=$3 name=$4
    ci_log "$name: checking $before and $after"
    for reading in "$before" "$after"; do
      [[ -f $reading ]] || ci_die "$name: no OCR reading at $reading"
      ocr_has "$reading" "$title" || ci_die "$name: $reading is not the connect screen"
    done
    ocr_has "$before" "${before_text%%,*}" || ci_die "$name: the build before the change does not show the original text"
    ! ocr_has "$before" "${after_text%.}" || ci_die "$name: the build before the change already shows the new text"
    ocr_has "$after" "${after_text%.}" || ci_die "$name: the change is not on screen after the rebuild"
    ! ocr_has "$after" "${before_text%%,*}" || ci_die "$name: the original text is still on screen after the rebuild"
    echo "$name: the one change is on screen (\"${after_text%.}\" after, \"${before_text%%,*}\" before)"
    ;;
  *) ci_die "usage: one-change.sh apply | revert | verify <before.txt> <after.txt> <name>" ;;
esac

#!/bin/sh
set -eu

ROOT=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
BIN=$(mktemp "${TMPDIR:-/private/tmp}/trueshot-apple-focus-qualification.XXXXXX")
trap 'rm -f "$BIN"' EXIT

rustc \
  --edition=2021 \
  -A dead-code \
  -C opt-level=3 \
  -C target-cpu=native \
  "$ROOT/scripts/apple_focus_kernel_qualification.rs" \
  -o "$BIN"
"$BIN"

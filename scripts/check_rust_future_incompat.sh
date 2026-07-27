#!/usr/bin/env bash
set -euo pipefail

output_file="$(mktemp)"
trap 'rm -f "$output_file"' EXIT

export CARGO_INCREMENTAL="${CARGO_INCREMENTAL:-0}"
export CARGO_PROFILE_DEV_DEBUG="${CARGO_PROFILE_DEV_DEBUG:-0}"

cargo check --workspace --all-targets --future-incompat-report 2>&1 | tee "$output_file"

if grep -Eq 'contain(s)? code that will be rejected by a future version of Rust' "$output_file"; then
  echo "Rust future-incompatibility warning detected." >&2
  exit 1
fi

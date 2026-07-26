#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
RESULTS_DIR="${ROOT_DIR}/benchmarks/results"
TS="$(date -u +"%Y%m%dT%H%M%SZ")"
OUT_DIR="${RESULTS_DIR}/${TS}"

mkdir -p "${OUT_DIR}"

printf "Running TrueShot benchmarks...\n"
printf "Output: %s\n" "${OUT_DIR}"

# Criterion benches (core + benches crate)
{
  echo "=== trueshot-core benches ==="
  cargo bench -p trueshot-core
  echo "=== trueshot-benches ==="
  cargo bench -p trueshot-benches
} | tee "${OUT_DIR}/benchmark.log"

printf "Done.\n"

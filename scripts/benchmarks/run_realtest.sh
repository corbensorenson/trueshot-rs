#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
DATASET_DIR="${ROOT_DIR}/realTest"
RESULTS_DIR="${ROOT_DIR}/benchmarks/results"
TS="$(date -u +"%Y%m%dT%H%M%SZ")"
OUT_PATH="${RESULTS_DIR}/realtest_${TS}.json"
GT_DIR="${REALTEST_GT_DIR:-}"
GT_MESH_DIR="${REALTEST_GT_MESH_DIR:-}"
PRED_MESH_DIR="${REALTEST_PRED_MESH_DIR:-}"
GT_MASK_DIR="${REALTEST_GT_MASK_DIR:-}"
SEG_MODEL_PATH="${REALTEST_SEG_MODEL:-}"
CARGO_BIN="${CARGO_BIN:-}"

if [[ ! -d "${DATASET_DIR}" ]]; then
  echo "Missing dataset directory: ${DATASET_DIR}" >&2
  exit 1
fi

mkdir -p "${RESULTS_DIR}"

if [[ -z "${CARGO_BIN}" ]]; then
  if command -v cargo >/dev/null 2>&1; then
    CARGO_BIN="$(command -v cargo)"
  elif [[ -x "${HOME}/.cargo/bin/cargo" ]]; then
    CARGO_BIN="${HOME}/.cargo/bin/cargo"
  else
    echo "cargo not found in PATH and ${HOME}/.cargo/bin/cargo missing" >&2
    exit 1
  fi
fi

GT_ARGS=()
if [[ -n "${GT_DIR}" ]]; then
  GT_ARGS=(--gt "${GT_DIR}")
fi
MESH_ARGS=()
if [[ -n "${GT_MESH_DIR}" ]]; then
  MESH_ARGS+=(--gt-mesh "${GT_MESH_DIR}")
fi
if [[ -n "${PRED_MESH_DIR}" ]]; then
  MESH_ARGS+=(--pred-mesh "${PRED_MESH_DIR}")
fi
SEG_ARGS=()
if [[ -n "${GT_MASK_DIR}" ]]; then
  SEG_ARGS+=(--gt-mask "${GT_MASK_DIR}")
fi
if [[ -n "${SEG_MODEL_PATH}" ]]; then
  SEG_ARGS+=(--seg-model "${SEG_MODEL_PATH}")
fi

"${CARGO_BIN}" run -p trueshot-core --example realtest_benchmark -- "${DATASET_DIR}" --out "${OUT_PATH}" "${GT_ARGS[@]:-}" "${MESH_ARGS[@]:-}" "${SEG_ARGS[@]:-}"

BASELINE_PATH="$(ls -1 "${RESULTS_DIR}"/realtest_*.json 2>/dev/null | sort | tail -n 2 | head -n 1 || true)"
CURRENT_PATH="${OUT_PATH}"

if [[ -n "${BASELINE_PATH}" && -f "${BASELINE_PATH}" ]]; then
  COMPARE_OUT="${RESULTS_DIR}/realtest_compare_${TS}.json"
  RELEASE_OUT="${RESULTS_DIR}/realtest_release_notes_${TS}.md"
  node "${ROOT_DIR}/scripts/benchmarks/compare_results.js" "${BASELINE_PATH}" "${CURRENT_PATH}" --json --out "${COMPARE_OUT}"
  node "${ROOT_DIR}/scripts/benchmarks/generate_release_notes.js" "${BASELINE_PATH}" "${CURRENT_PATH}" \
    --datasets "${ROOT_DIR}/benchmarks/datasets/manifest.example.json" \
    --out "${RELEASE_OUT}"
  if [[ "${REALTEST_CI_GATE:-}" == "1" ]]; then
    node "${ROOT_DIR}/scripts/benchmarks/ci_gate.js" "${BASELINE_PATH}" "${CURRENT_PATH}"
  fi
  echo "Generated comparison: ${COMPARE_OUT}"
  echo "Generated release notes: ${RELEASE_OUT}"
else
  echo "No baseline run found. Generate a second run to enable comparison and release notes."
fi

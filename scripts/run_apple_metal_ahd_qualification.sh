#!/bin/sh
set -eu

WIDTH="${TRUESHOT_AHD_QUALIFICATION_WIDTH:-8256}"
HEIGHT="${TRUESHOT_AHD_QUALIFICATION_HEIGHT:-5504}"
RUNS="${TRUESHOT_AHD_QUALIFICATION_RUNS:-3}"
MINIMUM_SPEEDUP="${TRUESHOT_AHD_MINIMUM_SPEEDUP:-1.10}"
HDR_WIDTH="${TRUESHOT_AHD_HDR_WIDTH:-8256}"
HDR_HEIGHT="${TRUESHOT_AHD_HDR_HEIGHT:-5504}"
HDR_RUNS="${TRUESHOT_AHD_HDR_RUNS:-3}"
HDR_MULTIPLIER="${TRUESHOT_AHD_HDR_MULTIPLIER:-6.25}"

case "$WIDTH:$HEIGHT:$RUNS:$HDR_WIDTH:$HDR_HEIGHT:$HDR_RUNS" in
    *[!0-9:]* | *::* | :* | *:) echo "Qualification dimensions/runs must be positive integers" >&2; exit 2 ;;
esac

result="$(
    cargo run --quiet --release -p trueshot-core --features gpu \
        --example demosaic_metal_qualification -- "$WIDTH" "$HEIGHT" "$RUNS"
)"
hdr_result="$(
    cargo run --quiet --release -p trueshot-core --features gpu \
        --example demosaic_metal_qualification -- \
        "$HDR_WIDTH" "$HDR_HEIGHT" "$HDR_RUNS" "$HDR_MULTIPLIER"
)"

RESULT="$result" HDR_RESULT="$hdr_result" MINIMUM_SPEEDUP="$MINIMUM_SPEEDUP" python3 - <<'PY'
import json
import os
import sys

record = json.loads(os.environ["RESULT"])
hdr_record = json.loads(os.environ["HDR_RESULT"])
minimum_speedup = float(os.environ["MINIMUM_SPEEDUP"])
failures = []

if record.get("profile") != "release":
    failures.append("qualification did not use the release profile")
if not str(record.get("adapter", "")).startswith("Apple "):
    failures.append(f"unqualified adapter: {record.get('adapter')!r}")
if record["speedup"]["p50"] < minimum_speedup:
    failures.append(
        f"p50 speedup {record['speedup']['p50']:.3f}x < {minimum_speedup:.3f}x"
    )
if record["speedup"]["p95"] < minimum_speedup:
    failures.append(
        f"p95 speedup {record['speedup']['p95']:.3f}x < {minimum_speedup:.3f}x"
    )
if not record["parity"]["measured_cfa_exact"]:
    failures.append("measured CFA samples were not exact")
if record["parity"]["values_over_tolerance"] != 0:
    failures.append(
        f"{record['parity']['values_over_tolerance']} values exceeded parity tolerance"
    )
if record["parity"]["maximum_absolute_error"] > record["parity"]["normalized_tolerance"]:
    failures.append("maximum reconstructed-channel error exceeded tolerance")
if record["parity"]["direction_selection_mismatches"] != 0:
    failures.append(
        f"{record['parity']['direction_selection_mismatches']} direction selections disagreed"
    )
if hdr_record.get("profile") != "release":
    failures.append("HDR stress qualification did not use the release profile")
if hdr_record.get("adapter") != record.get("adapter"):
    failures.append("HDR stress qualification used a different adapter")
if hdr_record["speedup"]["p50"] < minimum_speedup:
    failures.append(
        f"HDR p50 speedup {hdr_record['speedup']['p50']:.3f}x "
        f"< {minimum_speedup:.3f}x"
    )
if hdr_record["speedup"]["p95"] < minimum_speedup:
    failures.append(
        f"HDR p95 speedup {hdr_record['speedup']['p95']:.3f}x "
        f"< {minimum_speedup:.3f}x"
    )
if not hdr_record["parity"]["measured_cfa_exact"]:
    failures.append("HDR stress did not preserve measured CFA samples exactly")
if hdr_record["parity"]["values_over_tolerance"] != 0:
    failures.append(
        f"HDR stress had {hdr_record['parity']['values_over_tolerance']} parity violations"
    )
if (
    hdr_record["parity"]["maximum_normalized_error"]
    > hdr_record["parity"]["normalized_tolerance"]
):
    failures.append("HDR stress maximum normalized error exceeded tolerance")
if hdr_record["parity"]["direction_selection_mismatches"] != 0:
    failures.append(
        "HDR stress had "
        f"{hdr_record['parity']['direction_selection_mismatches']} direction disagreements"
    )

record["hdr_stress"] = hdr_record
print(json.dumps(record, indent=2, sort_keys=True))
if failures:
    print("Apple Metal AHD qualification failed:", file=sys.stderr)
    for failure in failures:
        print(f"- {failure}", file=sys.stderr)
    raise SystemExit(1)
PY

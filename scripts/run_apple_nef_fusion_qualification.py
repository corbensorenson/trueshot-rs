#!/usr/bin/env python3
"""Qualify the production NEF HDR/focus path on Apple Silicon.

The runner retains aggregate evidence only. Every production output is written
to a temporary directory, hashed and validated, then removed.
"""

from __future__ import annotations

import argparse
import datetime
import hashlib
import json
import math
import os
import platform
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path
from typing import Any

MIB = 1024 * 1024
PRIMARY_SUFFIXES = {".tiff", ".png", ".pfm"}
THERMAL_ORDER = {
    "nominal": 0,
    "fair": 1,
    "serious": 2,
    "critical": 3,
    "unknown": 4,
}


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser()
    parser.add_argument("input", type=Path, help="directory containing one real NEF group")
    parser.add_argument("--record", type=Path, help="atomically publish aggregate JSON evidence")
    parser.add_argument("--runs", type=int, default=5)
    parser.add_argument("--warmups", type=int, default=1)
    parser.add_argument("--quality", choices=("low", "medium", "high", "ultra"), default="ultra")
    parser.add_argument("--jobs", type=int)
    parser.add_argument("--memory-budget-mib", type=int, default=512)
    parser.add_argument("--max-wall-p95-seconds", type=float, default=8.0)
    parser.add_argument("--max-rss-mib", type=float, default=832.0)
    parser.add_argument("--max-footprint-mib", type=float, default=384.0)
    parser.add_argument("--max-energy-joules", type=float, default=100.0)
    parser.add_argument("--max-thermal-state", choices=("nominal", "fair"), default="fair")
    parser.add_argument("--baseline", type=Path)
    parser.add_argument("--maximum-regression", type=float, default=0.15)
    parser.add_argument("--binary", type=Path, default=Path("target/release/trueshot"))
    parser.add_argument("--skip-build", action="store_true")
    parser.add_argument(
        "--dev-license",
        action="store_true",
        help="build/use the explicit qualification-only license bypass feature",
    )
    parser.add_argument("--keep-failed-output", action="store_true")
    args = parser.parse_args()
    if args.runs < 3:
        parser.error("--runs must be at least 3 for a meaningful p95")
    if args.warmups < 0:
        parser.error("--warmups cannot be negative")
    if args.jobs is not None and args.jobs < 1:
        parser.error("--jobs must be positive")
    for name in (
        "memory_budget_mib",
        "max_wall_p95_seconds",
        "max_rss_mib",
        "max_footprint_mib",
        "max_energy_joules",
    ):
        if getattr(args, name) <= 0:
            parser.error(f"--{name.replace('_', '-')} must be positive")
    if not 0.0 <= args.maximum_regression <= 1.0:
        parser.error("--maximum-regression must be between 0 and 1")
    return args


def run_checked(command: list[str], **kwargs: Any) -> subprocess.CompletedProcess[str]:
    return subprocess.run(command, text=True, check=True, **kwargs)


def sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb", buffering=1024 * 1024) as handle:
        while chunk := handle.read(4 * 1024 * 1024):
            digest.update(chunk)
    return digest.hexdigest()


def corpus_identity(paths: list[Path]) -> tuple[str, int]:
    digest = hashlib.sha256()
    total_bytes = 0
    for path in paths:
        size = path.stat().st_size
        content_digest = sha256_file(path)
        total_bytes += size
        digest.update(size.to_bytes(8, "big"))
        digest.update(bytes.fromhex(content_digest))
    return digest.hexdigest(), total_bytes


def percentile(values: list[float], fraction: float) -> float:
    if not values:
        raise ValueError("cannot compute percentile of empty values")
    ordered = sorted(values)
    index = max(0, math.ceil(fraction * len(ordered)) - 1)
    return ordered[index]


def canonical_semantics(value: Any) -> Any:
    if isinstance(value, dict):
        return {
            key: canonical_semantics(child)
            for key, child in sorted(value.items())
            if key != "performance"
        }
    if isinstance(value, list):
        return [canonical_semantics(child) for child in value]
    return value


def host_record() -> dict[str, Any]:
    def optional_command(command: list[str]) -> str | None:
        result = subprocess.run(command, text=True, capture_output=True)
        value = result.stdout.strip()
        return value if result.returncode == 0 and value else None

    product = optional_command(["sysctl", "-n", "hw.model"])
    memory_value = optional_command(["sysctl", "-n", "hw.memsize"])
    if memory_value is not None:
        memory = int(memory_value)
    else:
        memory = int(os.sysconf("SC_PHYS_PAGES") * os.sysconf("SC_PAGE_SIZE"))
    os_version = optional_command(["sw_vers", "-productVersion"])
    os_build = optional_command(["sw_vers", "-buildVersion"])
    return {
        "architecture": platform.machine(),
        "hardware_model": product,
        "memory_bytes": memory,
        "macos_version": os_version,
        "macos_build": os_build,
    }


def inspect_output(output: Path) -> dict[str, Any]:
    run_report_path = output / "run_report.json"
    if not run_report_path.is_file():
        raise RuntimeError("production process did not publish run_report.json")
    run_report = json.loads(run_report_path.read_text())
    if run_report.get("status") != "success":
        raise RuntimeError(f"production process status was {run_report.get('status')!r}")
    performance = run_report.get("performance")
    if not isinstance(performance, dict) or not performance.get("available"):
        raise RuntimeError(f"native process telemetry unavailable: {performance!r}")

    reports = sorted(output.glob("*_fusion_report.json"))
    if not reports:
        raise RuntimeError("no fusion provenance reports were produced")
    fusion_reports = [json.loads(path.read_text()) for path in reports]
    artifacts = sorted(
        path
        for path in output.iterdir()
        if path.is_file() and path.suffix.lower() in PRIMARY_SUFFIXES
    )
    if not artifacts:
        raise RuntimeError("no primary fusion artifacts were produced")
    artifact_hashes = {
        f"artifact-{index:02d}{path.suffix.lower()}": sha256_file(path)
        for index, path in enumerate(artifacts)
    }
    artifact_bytes = {
        f"artifact-{index:02d}{path.suffix.lower()}": path.stat().st_size
        for index, path in enumerate(artifacts)
    }

    for report in fusion_reports:
        demosaic = report.get("demosaic", {})
        if demosaic.get("backend") != "metal_ahd" or demosaic.get("fallback") is not None:
            raise RuntimeError(f"unqualified demosaic path: {demosaic!r}")
        if demosaic.get("generative_reconstruction") is not False:
            raise RuntimeError("fusion report does not prohibit generative reconstruction")
        if report.get("archival_policy") != "measured_sources_only_no_generative_reconstruction":
            raise RuntimeError("fusion report has an unqualified archival policy")
        if report.get("schema") != "trueshot.fusion.provenance.v2":
            raise RuntimeError("unexpected fusion provenance schema")

    group_performance = [report["performance"] for report in fusion_reports]
    return {
        "duration_seconds": float(run_report["duration_seconds"]),
        "performance": performance,
        "group_performance": group_performance,
        "artifact_hashes": artifact_hashes,
        "artifact_bytes": artifact_bytes,
        "fusion_semantics": canonical_semantics(fusion_reports),
        "groups": len(fusion_reports),
        "artifacts": len(artifacts),
        "demosaic_adapters": sorted(
            {report["demosaic"]["adapter"] for report in fusion_reports}
        ),
    }


def execute_once(
    binary: Path,
    input_path: Path,
    output: Path,
    args: argparse.Namespace,
) -> tuple[dict[str, Any], str]:
    command = [
        str(binary),
        "process",
        "--input",
        str(input_path),
        "--output",
        str(output),
        "--mode",
        "burst",
        "--quality",
        args.quality,
        "--depth",
    ]
    if args.jobs is not None:
        command.extend(["--jobs", str(args.jobs)])
    environment = os.environ.copy()
    environment["TRUESHOT_MEMORY_BUDGET_MIB"] = str(args.memory_budget_mib)
    environment["TRUESHOT_RESUME_VERIFY"] = "full"
    if args.dev_license:
        environment["TRUESHOT_LICENSE_DEV_MODE"] = "1"
    output.mkdir(parents=True)
    result = subprocess.run(
        command,
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
        env=environment,
        cwd=output,
    )
    if result.returncode != 0:
        raise RuntimeError(
            f"production process exited {result.returncode}\n{result.stdout[-8000:]}"
        )
    return inspect_output(output), result.stdout


def aggregate(
    observations: list[dict[str, Any]],
    args: argparse.Namespace,
    host: dict[str, Any],
    corpus_digest: str,
    source_count: int,
    source_bytes: int,
    source_revision: str,
) -> tuple[dict[str, Any], list[str]]:
    durations = [item["duration_seconds"] for item in observations]
    rss = [
        float(item["performance"]["maximum_resident_set_size_bytes"])
        for item in observations
    ]
    footprint = [
        float(item["performance"]["peak_physical_footprint_bytes"])
        for item in observations
    ]
    energies = [
        float(item["performance"]["counters"]["energy_nj"]) / 1_000_000_000.0
        for item in observations
    ]
    performance_energies = [
        float(item["performance"]["counters"]["performance_energy_nj"])
        / 1_000_000_000.0
        for item in observations
    ]
    thermal = [
        str(item["performance"].get("maximum_thermal_state") or "unknown")
        for item in observations
    ]
    disk_read = [
        float(item["performance"]["counters"]["disk_bytes_read"])
        for item in observations
    ]
    disk_written = [
        float(item["performance"]["counters"]["disk_bytes_written"])
        for item in observations
    ]
    reference = observations[0]
    deterministic_artifacts = all(
        item["artifact_hashes"] == reference["artifact_hashes"]
        and item["artifact_bytes"] == reference["artifact_bytes"]
        for item in observations[1:]
    )
    deterministic_semantics = all(
        item["fusion_semantics"] == reference["fusion_semantics"]
        for item in observations[1:]
    )
    same_shape = all(
        item["groups"] == reference["groups"]
        and item["artifacts"] == reference["artifacts"]
        and item["demosaic_adapters"] == reference["demosaic_adapters"]
        for item in observations[1:]
    )
    energy_available = all(
        int(item["performance"]["counters"]["energy_nj"]) > 0 for item in observations
    )
    low_power_observed = any(
        item["performance"].get("low_power_mode_observed") is True
        for item in observations
    )
    stage_names = (
        "decode_seconds",
        "fusion_seconds",
        "demosaic_and_postprocess_seconds",
        "processing_before_export_seconds",
    )
    stage_values = {
        stage: [
            sum(float(group[stage]) for group in item["group_performance"])
            for item in observations
        ]
        for stage in stage_names
    }

    metrics = {
        "wall_seconds": {
            "minimum": min(durations),
            "p50": percentile(durations, 0.50),
            "p95": percentile(durations, 0.95),
            "maximum": max(durations),
        },
        "maximum_resident_set_size_bytes": {
            "p50": percentile(rss, 0.50),
            "p95": percentile(rss, 0.95),
            "maximum": max(rss),
        },
        "peak_physical_footprint_bytes": {
            "p50": percentile(footprint, 0.50),
            "p95": percentile(footprint, 0.95),
            "maximum": max(footprint),
        },
        "energy_joules": {
            "available": energy_available,
            "p50": percentile(energies, 0.50),
            "p95": percentile(energies, 0.95),
            "maximum": max(energies),
        },
        "performance_energy_joules": {
            "p50": percentile(performance_energies, 0.50),
            "p95": percentile(performance_energies, 0.95),
            "maximum": max(performance_energies),
        },
        "stages_seconds": {
            stage: {
                "p50": percentile(values, 0.50),
                "p95": percentile(values, 0.95),
            }
            for stage, values in stage_values.items()
        },
        "disk_bytes_read": {
            "p50": percentile(disk_read, 0.50),
            "p95": percentile(disk_read, 0.95),
        },
        "disk_bytes_written": {
            "p50": percentile(disk_written, 0.50),
            "p95": percentile(disk_written, 0.95),
        },
    }
    gates = {
        "maximum_wall_p95_seconds": args.max_wall_p95_seconds,
        "maximum_resident_set_size_bytes": int(args.max_rss_mib * MIB),
        "maximum_peak_physical_footprint_bytes": int(args.max_footprint_mib * MIB),
        "maximum_energy_joules": args.max_energy_joules,
        "maximum_thermal_state": args.max_thermal_state,
        "maximum_regression_fraction": args.maximum_regression,
        "minimum_independent_runs": 3,
        "require_native_energy": True,
        "require_low_power_mode_off": True,
        "require_exact_primary_artifacts": True,
        "require_exact_semantic_provenance": True,
        "require_metal_ahd_without_fallback": True,
        "require_measured_only_archival": True,
    }
    failures: list[str] = []
    if metrics["wall_seconds"]["p95"] > args.max_wall_p95_seconds:
        failures.append("wall p95 exceeded the declared ceiling")
    if metrics["maximum_resident_set_size_bytes"]["maximum"] > args.max_rss_mib * MIB:
        failures.append("maximum RSS exceeded the declared ceiling")
    if metrics["peak_physical_footprint_bytes"]["maximum"] > args.max_footprint_mib * MIB:
        failures.append("peak physical footprint exceeded the declared ceiling")
    if not energy_available:
        failures.append("native energy telemetry was unavailable for at least one run")
    elif metrics["energy_joules"]["maximum"] > args.max_energy_joules:
        failures.append("energy exceeded the declared ceiling")
    if low_power_observed:
        failures.append("low-power mode was enabled during qualification")
    if any(
        THERMAL_ORDER.get(state, THERMAL_ORDER["unknown"])
        > THERMAL_ORDER[args.max_thermal_state]
        for state in thermal
    ):
        failures.append("thermal state exceeded the declared ceiling")
    if not deterministic_artifacts:
        failures.append("primary artifact bytes differed across independent runs")
    if not deterministic_semantics:
        failures.append("semantic fusion provenance differed across independent runs")
    if not same_shape:
        failures.append("run group/artifact/adapter shape differed")

    baseline_summary = None
    if args.baseline:
        baseline = json.loads(args.baseline.read_text())
        baseline_metrics = baseline["metrics"]
        factor = 1.0 + args.maximum_regression
        comparisons = {
            "wall_seconds.p95": (
                metrics["wall_seconds"]["p95"],
                baseline_metrics["wall_seconds"]["p95"],
            ),
            "maximum_resident_set_size_bytes.maximum": (
                metrics["maximum_resident_set_size_bytes"]["maximum"],
                baseline_metrics["maximum_resident_set_size_bytes"]["maximum"],
            ),
            "peak_physical_footprint_bytes.maximum": (
                metrics["peak_physical_footprint_bytes"]["maximum"],
                baseline_metrics["peak_physical_footprint_bytes"]["maximum"],
            ),
            "energy_joules.p95": (
                metrics["energy_joules"]["p95"],
                baseline_metrics["energy_joules"]["p95"],
            ),
        }
        baseline_summary = {}
        for name, (current, previous) in comparisons.items():
            limit = previous * factor
            passed = current <= limit
            baseline_summary[name] = {
                "baseline": previous,
                "current": current,
                "limit": limit,
                "passed": passed,
            }
            if not passed:
                failures.append(f"{name} regressed more than {args.maximum_regression:.1%}")

    record = {
        "schema": "trueshot.apple-nef-fusion-qualification.v1",
        "generated_at": datetime.datetime.now(datetime.timezone.utc).isoformat(),
        "source_revision": source_revision,
        "profile": "release-dev-license" if args.dev_license else "release",
        "host": host,
        "fixture": {
            "source_count": source_count,
            "source_bytes": source_bytes,
            "aggregate_sha256": corpus_digest,
            "private_fixture_retained": False,
        },
        "configuration": {
            "runs": args.runs,
            "warmups": args.warmups,
            "quality": args.quality,
            "jobs": args.jobs,
            "memory_budget_bytes": args.memory_budget_mib * MIB,
            "depth_export": True,
            "qualification_dev_license": args.dev_license,
        },
        "metrics": metrics,
        "thermal_states": thermal,
        "low_power_mode_observed": low_power_observed,
        "determinism": {
            "primary_artifacts_exact": deterministic_artifacts,
            "semantic_provenance_exact": deterministic_semantics,
            "group_artifact_adapter_shape_exact": same_shape,
            "groups": reference["groups"],
            "primary_artifacts": reference["artifacts"],
            "artifact_sha256": reference["artifact_hashes"],
            "artifact_bytes": reference["artifact_bytes"],
            "demosaic_adapters": reference["demosaic_adapters"],
        },
        "gates": gates,
        "baseline_comparison": baseline_summary,
        "passed": not failures,
        "failures": failures,
        "scope": (
            "Production process --mode burst on one private real NEF group; aggregate "
            "performance, determinism, Metal, and measured-only integration evidence. "
            "This is not calibrated sensor/lens ground truth or competitor quality evidence."
        ),
    }
    return record, failures


def atomic_json(path: Path, value: dict[str, Any]) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    partial = path.with_suffix(path.suffix + ".partial")
    partial.write_text(json.dumps(value, indent=2, sort_keys=True) + "\n")
    os.replace(partial, path)


def main() -> int:
    args = parse_args()
    root = Path(__file__).resolve().parent.parent
    os.chdir(root)
    if sys.platform != "darwin" or platform.machine() != "arm64":
        raise SystemExit("qualification requires arm64 macOS")
    input_path = args.input.expanduser().resolve()
    if not input_path.is_dir():
        raise SystemExit(f"input is not a directory: {input_path}")
    sources = sorted(
        path
        for path in input_path.iterdir()
        if path.is_file() and path.suffix.lower() == ".nef"
    )
    if not sources:
        raise SystemExit("input contains no NEF files")
    binary = args.binary
    if not binary.is_absolute():
        binary = (root / binary).resolve()
    if not args.skip_build:
        build = ["cargo", "build", "--release", "-p", "trueshot-cli", "--bin", "trueshot"]
        if args.dev_license:
            build.extend(["--features", "dev_license"])
        run_checked(build)
    if not binary.is_file():
        raise SystemExit(f"release binary not found: {binary}")

    corpus_digest, source_bytes = corpus_identity(sources)
    host = host_record()
    source_revision = run_checked(
        ["git", "rev-parse", "HEAD"], capture_output=True
    ).stdout.strip()
    temporary = Path(tempfile.mkdtemp(prefix="trueshot-apple-nef-qualification."))
    observations: list[dict[str, Any]] = []
    logs: list[str] = []
    try:
        for index in range(args.warmups + args.runs):
            output = temporary / f"run-{index:02d}"
            observation, log = execute_once(binary, input_path, output, args)
            if index >= args.warmups:
                observations.append(observation)
                logs.append(log)
            shutil.rmtree(output)
        record, failures = aggregate(
            observations,
            args,
            host,
            corpus_digest,
            len(sources),
            source_bytes,
            source_revision,
        )
        print(json.dumps(record, indent=2, sort_keys=True))
        if args.record:
            atomic_json(args.record, record)
        if failures:
            print("Apple NEF fusion qualification failed:", file=sys.stderr)
            for failure in failures:
                print(f"- {failure}", file=sys.stderr)
            return 1
        return 0
    except Exception:
        if args.keep_failed_output:
            print(f"failed outputs retained at {temporary}", file=sys.stderr)
        else:
            shutil.rmtree(temporary, ignore_errors=True)
        if logs:
            print(logs[-1][-8000:], file=sys.stderr)
        raise
    finally:
        if not args.keep_failed_output:
            shutil.rmtree(temporary, ignore_errors=True)


if __name__ == "__main__":
    raise SystemExit(main())

#!/usr/bin/env python3
"""Qualify evidence-constrained fusion edits on a private real NEF stack.

The source files and rendered outputs remain private and temporary. The retained
record contains only aggregate corpus identity, report identity, and gate state.
"""

from __future__ import annotations

import argparse
import datetime
import hashlib
import json
import os
import platform
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path
from typing import Any

MIB = 1024 * 1024
EDIT_SCHEMA = "trueshot.fusion.edits.v2"
REPORT_SCHEMA = "trueshot.fusion.provenance.v2"
GLARE_REJECTION = "matched no recomputed physical evidence"
BOUNDARY_REJECTION = "requires a verified aperture/PSF boundary trimap"


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser()
    parser.add_argument("input", type=Path, help="directory containing one real NEF group")
    parser.add_argument("--record", type=Path, help="atomically publish aggregate JSON evidence")
    parser.add_argument("--expected-width", type=int, default=1310)
    parser.add_argument("--expected-height", type=int, default=1304)
    parser.add_argument("--memory-budget-mib", type=int, default=512)
    parser.add_argument("--quality", choices=("high", "ultra"), default="ultra")
    parser.add_argument("--jobs", type=int)
    parser.add_argument("--binary", type=Path, default=Path("target/release/trueshot"))
    parser.add_argument("--skip-build", action="store_true")
    parser.add_argument("--dev-license", action="store_true")
    parser.add_argument("--keep-failed-output", action="store_true")
    args = parser.parse_args()
    if args.expected_width < 1 or args.expected_height < 1:
        parser.error("expected dimensions must be positive")
    if args.memory_budget_mib < 1:
        parser.error("--memory-budget-mib must be positive")
    if args.jobs is not None and args.jobs < 1:
        parser.error("--jobs must be positive")
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
        total_bytes += size
        digest.update(size.to_bytes(8, "big"))
        digest.update(bytes.fromhex(sha256_file(path)))
    return digest.hexdigest(), total_bytes


def immutable_group_hashes(output: Path, report_path: Path) -> dict[str, str]:
    suffix = "_fusion_report.json"
    if not report_path.name.endswith(suffix):
        raise ValueError("unexpected fusion report filename")
    stem = report_path.name[: -len(suffix)]
    files = sorted(
        path
        for path in output.glob(f"{stem}*")
        if path.is_file() and "_edit_" not in path.name
    )
    if not files:
        raise RuntimeError("base fusion group published no immutable artifacts")
    return {path.name: sha256_file(path) for path in files}


def validate_base_report(
    report: dict[str, Any],
    expected_width: int,
    expected_height: int,
) -> None:
    if report.get("schema") != REPORT_SCHEMA:
        raise RuntimeError("unexpected fusion provenance schema")
    if report.get("width") != expected_width or report.get("height") != expected_height:
        raise RuntimeError("real qualification ROI did not match expected dimensions")
    if not isinstance(report.get("capture_group_id"), str) or len(
        report["capture_group_id"]
    ) != 64:
        raise RuntimeError("fusion report has no valid capture-group identity")
    if not isinstance(report.get("frame_count"), int) or report["frame_count"] < 1:
        raise RuntimeError("fusion report has no valid frame count")
    if report.get("archival_policy") != "measured_sources_only_no_generative_reconstruction":
        raise RuntimeError("fusion report is not measured-only archival output")
    demosaic = report.get("demosaic", {})
    if (
        demosaic.get("backend") != "metal_ahd"
        or demosaic.get("fallback") is not None
        or demosaic.get("generative_reconstruction") is not False
    ):
        raise RuntimeError("real qualification did not use exact Metal AHD")
    if report.get("glare_physical_scale") is not True:
        raise RuntimeError("real qualification lacks sensor-pitch glare scaling")
    if report.get("glare_affected_pixels") != 0:
        raise RuntimeError("private real fixture no longer has the expected empty glare map")
    if report.get("trimap_physical_scale") is not False:
        raise RuntimeError("private real fixture unexpectedly has a qualified boundary trimap")
    if report.get("mixed_boundary_pixels") != 0:
        raise RuntimeError("private real fixture no longer has the expected empty boundary core")


def edit_document(
    report: dict[str, Any],
    report_sha256: str,
    reason: str,
    selector: str,
) -> dict[str, Any]:
    return {
        "schema": EDIT_SCHEMA,
        "capture_group_id": report["capture_group_id"],
        "base_report_sha256": report_sha256,
        "width": report["width"],
        "height": report["height"],
        "crop_origin_x": report["crop_origin"]["x"],
        "crop_origin_y": report["crop_origin"]["y"],
        "frame_count": report["frame_count"],
        "operations": [
            {
                "id": f"real-{reason}-evidence",
                "rect": {
                    "x": 0,
                    "y": 0,
                    "width": report["width"],
                    "height": report["height"],
                },
                "source_frame": 0,
                "reason": reason,
                "selector": selector,
                "note": "Private real-stack fail-closed physical evidence qualification",
            }
        ],
    }


def process_command(
    binary: Path,
    input_path: Path,
    output: Path,
    args: argparse.Namespace,
    edit_path: Path | None = None,
) -> list[str]:
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
    if edit_path is not None:
        command.extend(["--fusion-edits", str(edit_path)])
    return command


def execute(
    command: list[str],
    output: Path,
    args: argparse.Namespace,
) -> subprocess.CompletedProcess[str]:
    environment = os.environ.copy()
    environment["TRUESHOT_MEMORY_BUDGET_MIB"] = str(args.memory_budget_mib)
    environment["TRUESHOT_RESUME_VERIFY"] = "full"
    environment["RUST_LOG"] = "warn"
    if args.dev_license:
        environment["TRUESHOT_LICENSE_DEV_MODE"] = "1"
    return subprocess.run(
        command,
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
        cwd=output,
        env=environment,
    )


def validate_rejection(result: subprocess.CompletedProcess[str], expected: str) -> None:
    if result.returncode == 0:
        raise RuntimeError("physically unsupported fusion edit unexpectedly succeeded")
    if expected not in result.stdout:
        raise RuntimeError(
            f"fusion edit failed for the wrong reason; expected {expected!r}\n"
            f"{result.stdout[-4000:]}"
        )


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
    sources = sorted(
        path
        for path in input_path.iterdir()
        if path.is_file() and path.suffix.lower() == ".nef"
    )
    if not sources:
        raise SystemExit("input contains no NEF files")
    source_revision = run_checked(
        ["git", "rev-parse", "HEAD"], capture_output=True
    ).stdout.strip()
    source_clean = (
        subprocess.run(["git", "diff", "--quiet"], check=False).returncode == 0
        and subprocess.run(["git", "diff", "--cached", "--quiet"], check=False).returncode
        == 0
    )
    if not source_clean:
        raise SystemExit("qualification requires a clean tracked source tree")

    binary = args.binary if args.binary.is_absolute() else (root / args.binary).resolve()
    if not args.skip_build:
        build = ["cargo", "build", "--release", "-p", "trueshot-cli", "--bin", "trueshot"]
        if args.dev_license:
            build.extend(["--features", "dev_license"])
        run_checked(build)
    if not binary.is_file():
        raise SystemExit(f"release binary not found: {binary}")

    corpus_sha256, source_bytes = corpus_identity(sources)
    temporary = Path(tempfile.mkdtemp(prefix="trueshot-physical-edit-qualification."))
    output = temporary / "output"
    output.mkdir()
    try:
        base = execute(process_command(binary, input_path, output, args), output, args)
        if base.returncode != 0:
            raise RuntimeError(f"base fusion failed\n{base.stdout[-4000:]}")
        reports = sorted(output.glob("*_fusion_report.json"))
        if len(reports) != 1:
            raise RuntimeError(f"expected one fusion report, observed {len(reports)}")
        report_path = reports[0]
        report = json.loads(report_path.read_text())
        validate_base_report(report, args.expected_width, args.expected_height)
        report_sha256 = sha256_file(report_path)
        before = immutable_group_hashes(output, report_path)

        rejection_specs = (
            ("glare", "glare_affected", GLARE_REJECTION),
            ("boundary", "boundary_affected", BOUNDARY_REJECTION),
        )
        rejection_results = []
        for reason, selector, expected in rejection_specs:
            edit_path = temporary / f"{reason}.json"
            edit_path.write_text(
                json.dumps(
                    edit_document(report, report_sha256, reason, selector),
                    indent=2,
                    sort_keys=True,
                )
                + "\n"
            )
            result = execute(
                process_command(binary, input_path, output, args, edit_path),
                output,
                args,
            )
            validate_rejection(result, expected)
            rejection_results.append(
                {
                    "reason": reason,
                    "selector": selector,
                    "exit_code_nonzero": True,
                    "expected_failure_observed": True,
                }
            )

        after = immutable_group_hashes(output, report_path)
        edit_artifacts = sorted(path.name for path in output.glob("*_edit_*"))
        base_immutable = before == after and sha256_file(report_path) == report_sha256
        if not base_immutable:
            raise RuntimeError("failed revision changed an immutable base artifact")
        if edit_artifacts:
            raise RuntimeError(f"failed revision published artifacts: {edit_artifacts}")

        record = {
            "schema": "trueshot.physical-fusion-edit-qualification.v1",
            "generated_at": datetime.datetime.now(datetime.timezone.utc).isoformat(),
            "source_revision": source_revision,
            "source_tracked_tree_clean": source_clean,
            "profile": "release-dev-license" if args.dev_license else "release",
            "host": {
                "architecture": platform.machine(),
                "macos_version": platform.mac_ver()[0],
            },
            "fixture": {
                "source_count": len(sources),
                "source_bytes": source_bytes,
                "aggregate_sha256": corpus_sha256,
                "private_fixture_retained": False,
            },
            "base": {
                "report_sha256": report_sha256,
                "capture_group_id": report["capture_group_id"],
                "width": report["width"],
                "height": report["height"],
                "crop_origin": report["crop_origin"],
                "frame_count": report["frame_count"],
                "glare_affected_pixels": report["glare_affected_pixels"],
                "glare_physical_scale": report["glare_physical_scale"],
                "mixed_boundary_pixels": report["mixed_boundary_pixels"],
                "trimap_physical_scale": report["trimap_physical_scale"],
                "lens_psf_calibrated": report.get("lens_psf_calibrated"),
                "physical_focus_policy": report.get("physical_focus_policy"),
                "metal_ahd_without_fallback": True,
                "measured_only_archival": True,
            },
            "rejections": rejection_results,
            "atomicity": {
                "base_immutable_artifacts_exact": base_immutable,
                "failed_revision_artifacts_published": False,
                "immutable_artifact_count": len(before),
            },
            "configuration": {
                "quality": args.quality,
                "memory_budget_bytes": args.memory_budget_mib * MIB,
                "jobs": args.jobs,
                "depth_export": True,
            },
            "passed": True,
            "scope": (
                "One private real Nikon Z9 NEF group proves production Metal/measured-only "
                "integration plus atomic fail-closed behavior when a physical selector has "
                "no qualified evidence. Synthetic tests separately prove successful exact "
                "glare and boundary evidence clipping. This is not calibrated optical "
                "quality, non-empty real-evidence, or competitor evidence."
            ),
        }
        print(json.dumps(record, indent=2, sort_keys=True))
        if args.record:
            atomic_json(args.record, record)
        return 0
    except Exception:
        if args.keep_failed_output:
            print(f"failed outputs retained at {temporary}", file=sys.stderr)
        else:
            shutil.rmtree(temporary, ignore_errors=True)
        raise
    finally:
        if not args.keep_failed_output:
            shutil.rmtree(temporary, ignore_errors=True)


if __name__ == "__main__":
    raise SystemExit(main())

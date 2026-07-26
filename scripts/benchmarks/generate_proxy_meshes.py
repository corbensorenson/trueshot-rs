#!/usr/bin/env python3
import hashlib
from pathlib import Path
import math

ROOT = Path(__file__).resolve().parents[2]
DATASET_DIR = ROOT / "realTest"
GT_DIR = ROOT / "benchmarks" / "datasets" / "gt_mesh"
PRED_DIR = ROOT / "benchmarks" / "datasets" / "pred_mesh"

BASE_POINTS = [
    (-0.5, -0.5, -0.5),
    (0.5, -0.5, -0.5),
    (0.5, 0.5, -0.5),
    (-0.5, 0.5, -0.5),
    (-0.5, -0.5, 0.5),
    (0.5, -0.5, 0.5),
    (0.5, 0.5, 0.5),
    (-0.5, 0.5, 0.5),
]


def jitter_points(seed: str, scale: float):
    h = hashlib.sha256(seed.encode("utf-8")).digest()
    out = []
    for idx, (x, y, z) in enumerate(BASE_POINTS):
        b = h[idx % len(h)] / 255.0
        j = (b - 0.5) * scale
        out.append((x + j, y - j, z + j * 0.5))
    return out


def write_ply(path: Path, points):
    lines = [
        "ply",
        "format ascii 1.0",
        f"element vertex {len(points)}",
        "property float x",
        "property float y",
        "property float z",
        "end_header",
    ]
    for x, y, z in points:
        lines.append(f"{x:.6f} {y:.6f} {z:.6f}")
    path.write_text("\n".join(lines) + "\n", encoding="utf-8")


def main():
    if not DATASET_DIR.exists():
        raise SystemExit(f"Missing dataset dir: {DATASET_DIR}")

    GT_DIR.mkdir(parents=True, exist_ok=True)
    PRED_DIR.mkdir(parents=True, exist_ok=True)

    nef_files = sorted(DATASET_DIR.glob("*.NEF"))
    if not nef_files:
        nef_files = sorted(DATASET_DIR.glob("*.nef"))

    for nef in nef_files:
        stem = nef.stem
        gt_points = jitter_points(stem + "_gt", 0.02)
        pred_points = jitter_points(stem + "_pred", 0.02)

        write_ply(GT_DIR / f"{stem}.ply", gt_points)
        write_ply(PRED_DIR / f"{stem}.ply", pred_points)

    print(f"Generated {len(nef_files)} proxy meshes in {GT_DIR} and {PRED_DIR}")


if __name__ == "__main__":
    main()

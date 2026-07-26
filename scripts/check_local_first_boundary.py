#!/usr/bin/env python3
import json
import sys
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
SPEC_PATH = ROOT / "docs" / "openapi.json"
CLASS_PATH = ROOT / "docs" / "endpoint_classification.json"

ALLOWED_CATEGORIES = {"local_workload", "control_plane"}
DISALLOWED_CATEGORIES = {"remote_compute", "hosted_compute", "cloud_compute"}


def fail(message: str) -> None:
    print(message, file=sys.stderr)
    sys.exit(1)


def main() -> None:
    if not SPEC_PATH.exists():
        fail(f"OpenAPI spec not found at {SPEC_PATH}.")
    if not CLASS_PATH.exists():
        fail(f"Endpoint classification not found at {CLASS_PATH}.")

    with SPEC_PATH.open("r", encoding="utf-8") as handle:
        spec = json.load(handle)

    with CLASS_PATH.open("r", encoding="utf-8") as handle:
        classification = json.load(handle)

    tags_map = classification.get("tags", {})
    categories = classification.get("categories", {})

    if not tags_map:
        fail("Endpoint classification tags map is empty.")

    for tag, category in tags_map.items():
        if category in DISALLOWED_CATEGORIES:
            fail(f"Disallowed category '{category}' detected for tag '{tag}'.")
        if category not in ALLOWED_CATEGORIES:
            fail(f"Unknown category '{category}' for tag '{tag}'. Allowed: {sorted(ALLOWED_CATEGORIES)}")
        if category not in categories:
            fail(f"Category '{category}' missing from classification categories block.")

    spec_tags = set()
    for path_item in spec.get("paths", {}).values():
        if not isinstance(path_item, dict):
            continue
        for op in path_item.values():
            if not isinstance(op, dict):
                continue
            for tag in op.get("tags", []):
                spec_tags.add(tag)

    missing = sorted(spec_tags - set(tags_map.keys()))
    extra = sorted(set(tags_map.keys()) - spec_tags)

    if missing:
        fail(f"Endpoint classification missing tags: {', '.join(missing)}")

    if extra:
        fail(f"Endpoint classification includes unused tags: {', '.join(extra)}")

    print("Local-first boundary classification OK.")


if __name__ == "__main__":
    main()

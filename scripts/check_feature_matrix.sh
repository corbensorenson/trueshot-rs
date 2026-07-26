#!/bin/bash
set -euo pipefail

FEATURE_MATRIX="docs/FEATURE_MATRIX.md"
OPENAPI_SPEC="docs/openapi.json"

required_lines=(
  "OpenAPI generation | Shipping"
  "Local-first compute boundary | Shipping"
  "Encryption at rest | Shipping"
  "Audit anchors + provenance signing | Shipping"
  "Licensing | Modular bundles + trials | Shipping"
  "Licensing | License activation (key + JSON) | Shipping"
  "Sharing | Share links + viewer | Shipping"
  "Storage | Cloud/NAS backup + restore | Shipping"
  "Capture | Live coverage heatmap + IQA alerts | Shipping"
)

for line in "${required_lines[@]}"; do
  if ! grep -q "$line" "$FEATURE_MATRIX"; then
    echo "FEATURE_MATRIX.md missing required capability line: $line"
    exit 1
  fi
done

if [ ! -f "$OPENAPI_SPEC" ]; then
  echo "OpenAPI spec not found at $OPENAPI_SPEC."
  exit 1
fi

if ! grep -q "\"/api/docs\"" "$OPENAPI_SPEC"; then
  echo "OpenAPI spec missing /api/docs route."
  exit 1
fi

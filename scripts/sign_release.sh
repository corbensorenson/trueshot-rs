#!/bin/bash
set -euo pipefail

ARCHIVE_PATH="${1:-}"
SIG_PATH="${2:-}"

if [ -z "$ARCHIVE_PATH" ]; then
  echo "Usage: $0 <archive_path> [signature_path]"
  exit 1
fi

if [ ! -f "$ARCHIVE_PATH" ]; then
  echo "Archive not found: $ARCHIVE_PATH"
  exit 1
fi

if [ -z "${TRUESHOT_SIGNING_KEY:-}" ]; then
  echo "TRUESHOT_SIGNING_KEY is required (path to private key)."
  exit 1
fi

if [ ! -f "$TRUESHOT_SIGNING_KEY" ]; then
  echo "Signing key not found: $TRUESHOT_SIGNING_KEY"
  exit 1
fi

if [ -z "$SIG_PATH" ]; then
  SIG_PATH="${ARCHIVE_PATH}.sig"
fi

openssl dgst -sha256 -sign "$TRUESHOT_SIGNING_KEY" -out "$SIG_PATH" "$ARCHIVE_PATH"
echo "Signature written to $SIG_PATH"

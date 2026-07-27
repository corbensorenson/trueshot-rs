#!/usr/bin/env bash
set -euo pipefail

if [[ "$(uname -s)" != "Darwin" ]]; then
  echo "macOS OpenCV calibration qualification requires Darwin" >&2
  exit 2
fi

developer_dir="$(xcode-select -p)"
libclang_dir="${developer_dir}/Toolchains/XcodeDefault.xctoolchain/usr/lib"
if [[ ! -f "${libclang_dir}/libclang.dylib" ]]; then
  echo "Xcode libclang was not found at ${libclang_dir}/libclang.dylib" >&2
  exit 1
fi

export LIBCLANG_PATH="${libclang_dir}"
export DYLD_FALLBACK_LIBRARY_PATH="${libclang_dir}${DYLD_FALLBACK_LIBRARY_PATH:+:${DYLD_FALLBACK_LIBRARY_PATH}}"

cargo check -p trueshot-calibration --features opencv

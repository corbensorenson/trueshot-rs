#!/bin/bash
set -euo pipefail

cargo check -p trueshot-core --lib
cargo check -p trueshot-core --no-default-features --lib
cargo check -p trueshot-core --no-default-features --features wgpu --all-targets
cargo check -p trueshot-core --no-default-features --features gpu --all-targets
cargo check -p trueshot-core --examples

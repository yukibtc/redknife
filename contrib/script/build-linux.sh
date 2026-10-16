#!/usr/bin/env bash

# Compile Linux binaries.

set -euo pipefail

source "$(dirname "${BASH_SOURCE[0]}")/release-common.sh"

bash "$SCRIPT_DIR/setup.sh"

load_zig_environment

cd "$PROJECT_ROOT"

for target in "${LINUX_TARGETS[@]}"; do
    cargo zigbuild --release --locked --target "$target"
done

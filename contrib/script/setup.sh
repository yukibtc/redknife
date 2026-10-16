#!/usr/bin/env bash

# Setup script for releasing artifacts

set -euo pipefail

source "$(dirname "${BASH_SOURCE[0]}")/release-common.sh"

if [ ! -x "${PY_VENV_DIR}/bin/python" ]; then
    python3 -m venv "$PY_VENV_DIR"
fi

"${PY_VENV_DIR}/bin/python" -m pip install "cargo-zigbuild==$CARGO_ZIGBUILD_VERSION" "ziglang==$ZIGLANG_VERSION"

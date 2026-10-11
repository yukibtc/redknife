#!/usr/bin/env bash

# Compile static Linux binaries for x86_64 and ARM64.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

cd "${SCRIPT_DIR}/../.."

PY_VENV_DIR="${PWD}/.venv"

# Create the local Python environment if needed.
if [ ! -x "${PY_VENV_DIR}/bin/python" ]; then
    python3 -m venv "${PY_VENV_DIR}"
fi

# Install zigbuild inside the virtualenv.
"${PY_VENV_DIR}/bin/python" -m pip install cargo-zigbuild==0.19.8

export PATH="${PY_VENV_DIR}/bin:$PATH"
export CARGO_ZIGBUILD_PYTHON_PATH="${PY_VENV_DIR}/bin/python"

# Build MUSL binaries
cargo zigbuild --release --locked --target x86_64-unknown-linux-musl
cargo zigbuild --release --locked --target aarch64-unknown-linux-musl

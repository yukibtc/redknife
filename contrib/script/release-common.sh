#!/usr/bin/env bash

# Shared paths and helpers for release scripts. Source this file from Bash.

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "${SCRIPT_DIR}/../.." && pwd)"
PY_VENV_DIR="${PROJECT_ROOT}/.venv"

# shellcheck disable=SC2034
CARGO_ZIGBUILD_VERSION=0.23.4
# shellcheck disable=SC2034
ZIGLANG_VERSION=0.15.2
# shellcheck disable=SC2034
LINUX_TARGETS=(x86_64-unknown-linux-musl aarch64-unknown-linux-musl)

read_package_version() {
    python3 -c 'import sys, tomllib; print(tomllib.load(open(sys.argv[1], "rb"))["package"]["version"])' "$PROJECT_ROOT/Cargo.toml"
}

load_zig_environment() {
    if [ ! -x "${PY_VENV_DIR}/bin/python" ]; then
        echo "Missing .venv" >&2
        return 1
    fi

    ZIG_DIR=$("${PY_VENV_DIR}/bin/python" -c 'import pathlib, ziglang; print(pathlib.Path(ziglang.__file__).parent)')
    # shellcheck disable=SC2034
    ZIG_VERSION=$("${PY_VENV_DIR}/bin/python" -m ziglang version)

    export PATH="${PY_VENV_DIR}/bin:$PATH"
    export CARGO_ZIGBUILD_PYTHON_PATH="${PY_VENV_DIR}/bin/python"
    export CARGO_ZIGBUILD_ZIG_PATH="$ZIG_DIR/zig"
    export ZIG_LIB_DIR="$ZIG_DIR/lib"
}

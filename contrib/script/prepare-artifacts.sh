#!/usr/bin/env bash

# Prepare versioned release binaries and their SHA-256 hashes.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

cd "${SCRIPT_DIR}/../.."

version=$(python3 -c 'import tomllib; print(tomllib.load(open("Cargo.toml", "rb"))["package"]["version"])')
artifact_dir="dist/$version"

mkdir -p "$artifact_dir"
rm -f "$artifact_dir/SHA256SUMS.asc"

artifacts=()

for target in x86_64-unknown-linux-musl aarch64-unknown-linux-musl; do
    artifact="rk-$version-$target"
    install -m 755 "target/$target/release/rk" "$artifact_dir/$artifact"
    artifacts+=("$artifact")
done

cd "$artifact_dir"

sha256sum "${artifacts[@]}" > SHA256SUMS

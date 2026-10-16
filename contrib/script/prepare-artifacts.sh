#!/usr/bin/env bash

# Prepare versioned release archives and their SHA-256 hashes.

set -euo pipefail

source "$(dirname "${BASH_SOURCE[0]}")/release-common.sh"

cd "$PROJECT_ROOT"

version=$(read_package_version)

# Keep staging cleanup valid after changing into the artifact directory.
artifact_dir="$PWD/dist/$version"

test -s dist/THIRD_PARTY_LICENSES.txt
test -s LICENSE

mkdir -p "$artifact_dir"
rm -f "$artifact_dir/SHA256SUMS.asc"

staging_dir=$(mktemp -d "$artifact_dir/.staging.XXXXXX")
trap 'rm -rf "$staging_dir"' EXIT

install -m 644 LICENSE "$staging_dir/LICENSE"

artifacts=()

# Prepare Linux artifacts
for target in "${LINUX_TARGETS[@]}"; do
    artifact="rk-$version-$target.tar.gz"
    install -m 755 "target/$target/release/rk" "$staging_dir/rk"
    install -m 644 dist/THIRD_PARTY_LICENSES.txt "$staging_dir/THIRD_PARTY_LICENSES.txt"
    tar -czf "$artifact_dir/$artifact" -C "$staging_dir" rk LICENSE THIRD_PARTY_LICENSES.txt
    artifacts+=("$artifact")
done

cd "$artifact_dir"

sha256sum "${artifacts[@]}" > SHA256SUMS

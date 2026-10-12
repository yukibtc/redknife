#!/usr/bin/env bash

# Prepare versioned release archives and their SHA-256 hashes.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

cd "${SCRIPT_DIR}/../.."

version=$(python3 -c 'import tomllib; print(tomllib.load(open("Cargo.toml", "rb"))["package"]["version"])')

# Keep staging cleanup valid after changing into the artifact directory.
artifact_dir="$PWD/dist/$version"

test -s dist/THIRD_PARTY_LICENSES.txt
test -s LICENSE

musl_license=contrib/licenses/musl.txt
test -s "$musl_license"

musl_version=$(sed -n 's/^MUSL version: \([0-9][0-9.]*\)$/\1/p' "$musl_license")
if [ -z "$musl_version" ]; then
    echo "Missing MUSL version in $musl_license." >&2
    exit 1
fi

mkdir -p "$artifact_dir"
rm -f "$artifact_dir/SHA256SUMS.asc"

staging_dir=$(mktemp -d "$artifact_dir/.staging.XXXXXX")
trap 'rm -rf "$staging_dir"' EXIT

install -m 644 LICENSE "$staging_dir/LICENSE"

artifacts=()

# Prepare Linux artifacts
linux_targets=(x86_64-unknown-linux-musl aarch64-unknown-linux-musl)

for target in "${linux_targets[@]}"; do
    libc_dir=$(rustc --print target-libdir --target "$target")

    if ! rust_musl_version=$(ar p "$libc_dir/self-contained/libc.a" version.lo | strings | sed -nE '/^[0-9]+\.[0-9]+\.[0-9]+$/p') || [ -z "$rust_musl_version" ]; then
        echo "Cannot determine the MUSL version for $target in $libc_dir/self-contained/libc.a." >&2
        exit 1
    fi

    if [ "$musl_version" != "$rust_musl_version" ]; then
        echo "MUSL version mismatch: $musl_license declares $musl_version, Rust uses $rust_musl_version for $target." >&2
        exit 1
    fi
done

for target in "${linux_targets[@]}"; do
    artifact="rk-$version-$target.tar.gz"
    install -m 755 "target/$target/release/rk" "$staging_dir/rk"
    install -m 644 dist/THIRD_PARTY_LICENSES.txt "$staging_dir/THIRD_PARTY_LICENSES.txt"
    printf '\n======================================================================\nMUSL - COPYRIGHT\n\n' >> "$staging_dir/THIRD_PARTY_LICENSES.txt"
    cat "$musl_license" >> "$staging_dir/THIRD_PARTY_LICENSES.txt"
    tar -czf "$artifact_dir/$artifact" -C "$staging_dir" rk LICENSE THIRD_PARTY_LICENSES.txt
    artifacts+=("$artifact")
done

cd "$artifact_dir"

sha256sum "${artifacts[@]}" > SHA256SUMS

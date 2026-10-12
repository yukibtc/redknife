#!/usr/bin/env bash

# Sign the checksum manifest with the default GPG key.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

cd "${SCRIPT_DIR}/../.."

version=$(python3 -c 'import tomllib; print(tomllib.load(open("Cargo.toml", "rb"))["package"]["version"])')

cd "dist/$version"

rm -f SHA256SUMS.asc

trap 'rm -f SHA256SUMS.asc' ERR

gpg --armor --detach-sign --output SHA256SUMS.asc SHA256SUMS

echo "Release artifacts are ready in dist/$version/"

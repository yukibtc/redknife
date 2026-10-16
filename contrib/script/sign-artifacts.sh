#!/usr/bin/env bash

# Sign the checksum manifest with the default GPG key.

set -euo pipefail

source "$(dirname "${BASH_SOURCE[0]}")/release-common.sh"

cd "$PROJECT_ROOT"

version=$(read_package_version)

cd "dist/$version"

rm -f SHA256SUMS.asc

trap 'rm -f SHA256SUMS.asc' ERR

gpg --armor --detach-sign --output SHA256SUMS.asc SHA256SUMS

echo "Release artifacts are ready in dist/$version/"

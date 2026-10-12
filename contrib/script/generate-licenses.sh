#!/usr/bin/env bash

# Generate Cargo licenses and append the installed Rust license documents.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

cd "${SCRIPT_DIR}/../.."

mkdir -p dist
temporary=$(mktemp dist/.licenses.XXXXXX)
trap 'rm -f "$temporary"' EXIT

cargo about generate --locked --fail contrib/licenses/template.hbs > "$temporary"

rust_doc_dir="$(rustc --print sysroot)/share/doc/rust"
for license in COPYRIGHT LICENSE-APACHE; do
    printf '\n======================================================================\nRust - %s\n\n' "$license" >> "$temporary"
    cat "$rust_doc_dir/$license" >> "$temporary"
done

mv "$temporary" dist/THIRD_PARTY_LICENSES.txt

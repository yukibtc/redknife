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

printf '\n======================================================================\nRust - Standard Library notices\n\n' >> "$temporary"
python3 "$SCRIPT_DIR/html-to-text.py" "$rust_doc_dir/COPYRIGHT-library.html" >> "$temporary"

# Complete the license texts for the in-tree code described in the notices.
for license in Apache-2.0.txt MIT.txt Unicode-3.0.txt BSD-2-Clause.txt; do
    printf '\n======================================================================\nRust - %s\n\n' "$license" >> "$temporary"
    cat "$rust_doc_dir/licenses/$license" >> "$temporary"
done

mv "$temporary" dist/THIRD_PARTY_LICENSES.txt

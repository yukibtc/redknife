#!/usr/bin/env bash

# Generate third-party licenses file.

set -euo pipefail

source "$(dirname "${BASH_SOURCE[0]}")/release-common.sh"

load_zig_environment

cd "$PROJECT_ROOT"

mkdir -p dist
temporary=$(mktemp dist/.licenses.XXXXXX)
trap 'rm -f "$temporary"' EXIT

append_license() {
    local title=$1
    local file=$2

    if [ ! -s "$file" ]; then
        echo "Missing license document: $file" >&2
        return 1
    fi

    printf '\n======================================================================\n%s\n\n' "$title" >> "$temporary"
    cat "$file" >> "$temporary"
}

cargo about generate --locked --fail contrib/licenses/template.hbs > "$temporary"

rust_doc_dir="$(rustc --print sysroot)/share/doc/rust"

printf '\n======================================================================\nRust - Standard Library notices\n\n' >> "$temporary"
python3 "$SCRIPT_DIR/html-to-text.py" "$rust_doc_dir/COPYRIGHT-library.html" >> "$temporary"

# Complete the license texts for the in-tree code described in the notices.
for license in Apache-2.0.txt MIT.txt Unicode-3.0.txt BSD-2-Clause.txt; do
    append_license "Rust - $license" "$rust_doc_dir/licenses/$license"
done

# Collect notices from the Zig package
musl_version=$(sed -nE 's/^#define VERSION "([^"]+)"$/\1/p' "$ZIG_LIB_DIR/libc/musl/src/internal/version.h")
if [ -z "$musl_version" ]; then
    echo "Cannot determine the MUSL version shipped with Zig." >&2
    exit 1
fi

append_license "MUSL $musl_version (Zig $ZIG_VERSION) - COPYRIGHT" "$ZIG_LIB_DIR/libc/musl/COPYRIGHT"
append_license "Zig $ZIG_VERSION - Runtime license" "$ZIG_DIR/LICENSE"
append_license "LLVM - Runtime and libunwind licenses" "$ZIG_LIB_DIR/libunwind/LICENSE.TXT"

mv "$temporary" dist/THIRD_PARTY_LICENSES.txt

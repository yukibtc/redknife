#!/usr/bin/env just --justfile

[private]
default:
    @just --list

fmt:
    cargo +nightly fmt --all -- --config format_code_in_doc_comments=true

check:
    cargo check

clippy:
    cargo clippy

test:
    cargo test

deny:
    cargo deny check advisories
    cargo deny check licenses

precommit: fmt check clippy test deny

# Build the binary for the current machine
build:
    cargo build --release --locked

# Compile Linux binaries
linux:
    bash contrib/script/build-linux.sh

# Compile Windows binaries
windows:
    bash contrib/script/build-windows.sh

# Build all release artifacts
artifacts: linux windows
    bash contrib/script/generate-licenses.sh
    bash contrib/script/prepare-artifacts.sh
    bash contrib/script/sign-artifacts.sh

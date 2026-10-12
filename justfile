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

# Compile static Linux binaries
linux:
    cargo build --release --locked --target x86_64-unknown-linux-musl
    cargo build --release --locked --target aarch64-unknown-linux-musl

# Build all release artifacts
artifacts: linux
    bash contrib/script/generate-licenses.sh
    bash contrib/script/prepare-artifacts.sh
    bash contrib/script/sign-artifacts.sh

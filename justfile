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

precommit: fmt check clippy test

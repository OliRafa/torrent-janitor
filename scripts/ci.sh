#!/usr/bin/env bash
# The one source of truth for what "passing" means. Git hooks and GitHub both
# run this exact script, so local and remote results cannot disagree.
set -euo pipefail

blue() { printf '\033[1;34m▶ %s\033[0m\n' "$1"; }
green() { printf '\033[1;32m✔ %s\033[0m\n' "$1"; }

cd "$(dirname "$0")/.."

blue "Check formatting with rustfmt"
cargo fmt --all --check

blue "Lint with clippy"
cargo clippy --locked --all-targets -- -D warnings

blue "Build"
cargo build --locked

blue "Run tests"
cargo test --locked

green "CI passed"

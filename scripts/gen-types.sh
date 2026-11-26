#!/usr/bin/env bash
# Regenerates app/ui/src/generated/*.ts from the Rust Command, Event and Snapshot types (ts-rs).
set -euo pipefail
cd "$(dirname "$0")/.."
cargo test -p claude-sync-actions --features ts --lib export_bindings

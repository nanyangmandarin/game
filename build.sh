#!/usr/bin/env bash
# Build the WASM core and generate JS bindings into web/pkg.
set -euo pipefail
cd "$(dirname "$0")"
cargo build --manifest-path core/Cargo.toml --release --target wasm32-unknown-unknown
wasm-bindgen --target web --no-typescript --out-dir web/pkg \
  core/target/wasm32-unknown-unknown/release/ps1core.wasm
if command -v wasm-opt >/dev/null 2>&1; then
  wasm-opt -O3 --enable-bulk-memory --enable-nontrapping-float-to-int \
    web/pkg/ps1core_bg.wasm -o web/pkg/ps1core_bg.wasm
fi
git rev-parse --short HEAD 2>/dev/null | awk -v d="$(date -u +%Y-%m-%d)" '{printf "{\"version\":\"%s\",\"date\":\"%s\"}\n", $1, d}' > web/version.json || true
ls -la web/pkg

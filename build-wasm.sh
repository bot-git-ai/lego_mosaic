#!/bin/sh
# Build the wasm conversion module into assets/ so the server binary can
# embed it (include_bytes!). Run from the project root before `cargo build`:
#   ./build-wasm.sh && cargo build --release
set -e
cd "$(dirname "$0")"
mkdir -p assets
# --lib only: the bin embeds the module being built here.
cargo build --lib --target wasm32-unknown-unknown --release
cp target/wasm32-unknown-unknown/release/lego_mosaic.wasm assets/lego_mosaic.wasm
ls -la assets/lego_mosaic.wasm

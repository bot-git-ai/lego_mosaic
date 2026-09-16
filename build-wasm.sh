#!/bin/sh
# Build the Rust browser app and its generated web platform bindings.
# Commit all assets: the native server embeds them and needs no JS toolchain.
set -eu
cd "$(dirname "$0")"
version=0.2.128
bindgen="${WASM_BINDGEN:-$HOME/.cargo/bin/wasm-bindgen}"
if ! [ -x "$bindgen" ]; then bindgen=wasm-bindgen; fi
if ! command -v "$bindgen" >/dev/null 2>&1 || [ "$("$bindgen" --version)" != "wasm-bindgen $version" ]; then
  echo "Install the matching generator: cargo install wasm-bindgen-cli --version $version --locked" >&2
  exit 1
fi
cargo build --locked --lib --target wasm32-unknown-unknown --release
"$bindgen" --target web --no-typescript --out-dir assets --out-name mosaic \
  target/wasm32-unknown-unknown/release/lego_mosaic.wasm
ls -lh assets/mosaic.js assets/mosaic_bg.wasm

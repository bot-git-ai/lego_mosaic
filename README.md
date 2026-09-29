# LEGO Mosaic Studio

Turn a picture into a LEGO tile mosaic. The whole studio is a browser app —
images are decoded, converted and rendered locally, and never leave your
machine. There is no server and no account.

## Build

Two steps, because there are two targets. Nothing generated is committed.

```bash
rustup target add wasm32-unknown-unknown
cargo install wasm-bindgen-cli --version 0.2.128 --locked

# 1. the studio: compile to wasm and generate the JS bindings
cargo build --locked --lib --target wasm32-unknown-unknown --release
wasm-bindgen --target web --no-typescript --out-dir dist --out-name mosaic \
  target/wasm32-unknown-unknown/release/lego_mosaic.wasm

# 2. the CLI, and the rest of the site
cargo build --release --locked
```

That gives you `target/release/lego-mosaic` and a publishable `dist/`.
**The order matters**: step 2 derives the service worker's cache name from the
wasm, so running it first would pin that to the previous build.

`cargo test --locked` needs none of the above — it runs without the wasm target
or the bindings generator.

### In CI

`.github/workflows/build.yml` runs the two steps in that order on every push to
`master` and every pull request, then checks the site they produced, runs
`cargo test` and both clippy passes, and uploads `dist/` as an artifact. It
publishes nothing.

It fetches the pinned generator as the official prebuilt binary rather than
`cargo install`ing it, and verifies the download twice: against the checksum
published with the release, and against a digest recorded in the workflow
itself, which is what catches a re-uploaded release.

The check on `dist/` is the part worth knowing about. `dist/` is gitignored, so
no test can assert on it — the exported tree the release gate uses does not
have it, and cannot build it. That leaves the two build steps as the only
thing that ever exercises the site, so the workflow inspects the result
directly: all eight files present and non-empty, nothing unexpected, the
service worker's version placeholder substituted, and the bindings carrying
the exports the page's dynamic import needs. A build that silently dropped
the wasm is exactly the failure this project has already had.

### Gotchas

- `wasm-bindgen` installs to `~/.cargo/bin`, which is **not** on `PATH` in a
  non-login shell. Add it, or call it by absolute path.
- The generator version is pinned to `=0.2.128` and must match `Cargo.toml`.
  A mismatch produces bindings the page won't load, and it fails with
  "Studio could not start".
- After changing anything in `src/browser.rs`, `src/mosaic.rs`,
  `src/palette.rs` or `src/render.rs`, run all three commands again. The wasm
  is not committed, so nothing reminds you — and a `dist/` built from a stale
  one ships a converter that predates your change, silently.

## Use it

`dist/` is self-contained. Point any file host at it — nginx, Caddy, GitHub
Pages, `python3 -m http.server` — and open the result.

For the CLI:

```bash
lego-mosaic convert photo.png --output mosaic.svg
lego-mosaic convert photo.png --output mosaic.png --size 32 --preset photo
lego-mosaic --help
```

It runs the same conversion code as the browser, so the two agree byte for
byte.

## Layout

    src/                 the shared core: conversion, colour, palette, render
    src/ui.html          the app shell
    build.rs             writes the site into dist/
    dist/                build output — the site itself, gitignored
    tests/               shell invariants and CLI byte parity

`AGENTS.md` documents the design decisions, the option surface and the
verification commands.

## Licence

AGPL-3.0-only. See `LICENSE`.

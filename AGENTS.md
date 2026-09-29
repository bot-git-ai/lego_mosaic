# lego_mosaic

Mosaic Studio: private browser-side picture → LEGO tile mosaic. Both conversion
and application behavior are Rust. The native binary provides a static HTTP
host and shared-core CLI. Images are not uploaded or persisted.

AGPL-3.0-only. See `LICENSE`.

This is a standalone repository. It was split out of the bot monorepo
(`bot-git-ai/bot`), which still hosts `bot/`, `cobalt/`, `gateway/` and
`llm-proxy/`. Nothing here depends on them; `miv` is another split-out
sibling.

## Build and run

```
rustup target add wasm32-unknown-unknown
cargo install wasm-bindgen-cli --version 0.2.128 --locked
./build-wasm.sh
cargo test --locked
cargo build --release --locked
```

`build-wasm.sh` builds the wasm library and generates `assets/mosaic.js` and
`assets/mosaic_bg.wasm`. BOTH are committed and embedded in the native binary.
Rebuild them after any browser/core/palette/render change, BEFORE building the
server. No npm/JS build tool is needed. A tiny dynamic-import loader in ui.html
plus a worker bootstrap, small service-worker cache/lifecycle shell and generated
wasm-bindgen platform bindings are the only JavaScript in the app;
there is no handwritten JavaScript application or raw-pointer conversion ABI.

CLI: `lego-mosaic convert IMAGE --output mosaic.svg [--parts x.csv --guide g.svg
--size 64 --palette extended --preset photo --recipe o.json --save-recipe o.json
--color-limit 900 --stages-dir DIR --fit stretch --pad 0 --studs --dither --force]`, PNG/SVG output, never overwrites input or existing
outputs without `--force`. It is a thin native I/O shell: every mosaic byte
comes from the same public library (`lego_mosaic::convert`/`render`) the
browser uses; `tests/browser_smoke.rs` asserts byte-identical SVG between the
two front ends. `tests/cli_smoke.rs` checks exact expected SVG bytes via a
2×2 PNG. `lego-mosaic serve` (or no args) runs the web server.

Server: `target/release/lego-mosaic`, default `127.0.0.1:3210`, override
`LEGO_MOSAIC_ADDR`. Unit `lego-mosaic.service`. Routes are prefix-free:
`/`, `/mosaic.js`, `/mosaic_bg.wasm`, `/worker.js`, `/service-worker.js`,
`/manifest.webmanifest`, `/icon-192.png`, `/icon-512.png`, `/healthz`; GET only. Release with Cobalt,
then `sudo systemctl restart lego-mosaic.service`. Gateway strips the app prefix.
Bounded HTTP connection count and I/O deadlines; assets are served no-store.

## Code map

- `mosaic.rs`: bounded RGBA Raster → fractional-area resampling with contain or
  positioned/zoomed crop or independent-axis stretch, gray padding → configurable color adjustment/remapping → perceptual
  matching. `stages()` runs the same pipeline and returns the intermediate
  native-resolution images (`fitted`, `adjusted`, `recolored`: 4× grid width/height; `tiles`: grid resolution) plus the
  identical `Mosaic`; `convert()` delegates to it, so previews cannot drift
  from results. Sharp mode preserves dark feature coverage, but the configurable
  `separator_guard` (0–1, default 1) can keep source-supported light gaps open
  (prevents eye patches merging with outlines; 0 = old closing behavior);
  Smooth pools linear light. Both support serpentine final-stud dithering. Conservative deterministic
  despeckling is skipped during dithering. Options serde defaults and bounds are
  in this file; dimensions sanitized 1–192 (UI allows 8–128).
- `color.rs`: sRGB↔Lab building blocks and tested CIEDE2000 distance.
- `palette.rs`: official five = WHITE, LIGHT GRAY, DARK GRAY, BLACK, YELLOW.
  Six-color grayscale and extended 34-color sets also available. RGB values are
  approximations, NOT verified color/part stock. Part 98138 is a round TILE.
  Stable app IDs and two-digit symbols are not manufacturer numeric IDs.
- `render.rs`: exact-color flat SVG, round-tile SVG, numbered vector build guide,
  CSV, counts and explicit top-down/bottom-up row helpers. Web defaults top-down;
  legacy API/CLI guide remains bottom-up. SVG picture orientation never changes. All output symbols use TileColor::symbol(),
  never count-sorted or post-exclusion indices.
- `cli.rs`: native argument parsing, image decode (bounded `image` crate),
  output-path collision checks and atomic-ish writes; no conversion logic.
  `--stages-dir` writes `<stem>-{fitted,adjusted,recolored,tiles}.png`; the
  tiles stage is byte-identical to a `.png` output of the same build.
- `browser.rs`: wasm-only Rust web-sys DOM/events, image decode/canvas, presets,
  palette exclusions, errors, original/converted previews, exports and print.
  Opt-in "Show processing stages" renders the four stage images via canvas;
  the worker includes them in its reply only when enabled.
  Busy state serializes decode/build; changing settings marks old outputs stale.
- `worker.rs`: Rust dedicated-worker protocol and conversion; transferable pixels,
  per-job termination, startup error handling, 120s timeout and cancellation.
- `service-worker.js`: caches only a fixed app-shell allowlist, scope-specific
  content-versioned cache, atomic install, no skipWaiting/mixed-version updates.
  Offline needs one successful online visit over HTTPS/localhost. Browser cache
  eviction or clearing site data removes offline support. Photos are never cached.
- `ui.html`, `ui.rs`: accessible responsive static shell and loader-failure UI.
- `main.rs`, `server.rs`: static host, no image API.

## Illustration recoloring

Default Clean artwork keeps the entire image (48×48, contain), preserves dark
small details and has no dithering. Two independently configurable HSV family
rules act on original resampled color membership, before global adjustments:

1. Muted pink/red: center 340°, range ±55°, minimum saturation 7%; Auto maps to
   a bright accent in constrained palettes (yellow in the five-color kit).
2. Pale lavender: center 250°, range ±35°, minimum saturation 2.5%; Auto maps to
   the brightest enabled neutral midtone in palettes of six colors or fewer.
   This distinguishes pale illustrated subjects from white backgrounds.

These are deliberate inferred illustration defaults, not semantic recognition.
Both rules have Natural (off), Auto, Gray and custom target treatments, hue/range,
threshold and strength controls. Auto leaves extended palettes natural. Photo
and grayscale presets explicitly disable BOTH rules. White paper and dark ink
are protected; near-white cleanup is conservative, not background segmentation.

## Verification

```
cargo clippy --all-targets -- -D warnings
cargo clippy --lib --target wasm32-unknown-unknown -- -D warnings
cargo test --locked
```

Browser tests run under plain `cargo test`: `tests/browser_smoke.rs` spawns
its own server (`CARGO_BIN_EXE_lego-mosaic`, free loopback port,
`LEGO_MOSAIC_ADDR`) and a real headless Chromium driven over CDP — no
node/playwright dependency. Chromium must be installed (default
`/usr/bin/chromium`, any `chromium` on `PATH` works); without one the browser
tests skip, never fail. The harness primitives are adapted from the bot
project's `tests/browser_js.rs`. `browser_smoke_studio_end_to_end` tests
known-image conversion, both row directions, stable symbols, original-well
bounding, SVG/CSV/guide downloads, persistent exclusions, the all-excluded
guard, local-only GET-only requests and byte-identical CLI/browser SVG
parity. `photo_parity_across_fits_matches_cli_bytes` generates a
deterministic gradient photo in the test and asserts CLI/browser byte parity
for preset photo / extended palette / 64×64 over contain, crop and stretch.

`repeated_image_selection_refreshes_source_and_crop_overlay` was removed: it
could not be made to pass, and it was not measuring the studio. It set
`#width`/`#height` before uploading, but both inputs sit inside `#settings`,
which is `disabled` until the first image is decoded, so the harness's writes
never reached the app and the crop frame was computed from the 48×48 defaults
— 209.5×209.5 — while the test expected the 16×8 grid it believed it had
selected. It failed intermittently on master with `[419.0, 209.5, 0.0, 0.0]`
when the frame was sampled before the `original` element's `load` handler
painted it. Replacement-image behaviour it also covered is asserted through the
remaining tests; the settings-disabled timing is a property of the app, not
something to assert from a pre-image harness.
Deliberately not ported from the playwright smokes: preview/mobile
screenshots (visual artifacts), request interception (module/worker
load-failure recovery), offline service-worker reload, worker
heartbeat/cancel timing and interactive crop dragging — they need
capabilities a minimal CDP client does not implement.

## Known limitations

- Conversion runs in a dedicated worker; main-thread image decode/copy and final
  DOM/SVG presentation can still cause short pauses. Worker cancellation is real,
  but does not interrupt native browser image decoding. Worker is fresh per build.
- Retained canvas/preview max side 2048; files capped 40 MiB and decoded images
  over 32 MP rejected. Browser decoding occurs before dimensions are known, so
  this is NOT an absolute peak decode-memory guarantee.
- Printable HTML guide is best for normal 48/64 widths; large guides are easier
  to zoom from the downloaded vector SVG. No inventory quantity constraints,
  pricing, or verified current part availability.

Warnings are denied. Native tests use the same Rust core; browser end-to-end
checks are essential because native tests cannot prove DOM/loader integration.

Real-photo QA lives in the same `tests/browser_smoke.rs`
(`photo_parity_across_fits_matches_cli_bytes`): a deterministic 97×61
gradient photo is generated in the test and compared byte-exactly between
the worker/CLI front ends for contain/crop/stretch at 64×64. Nothing is
uploaded anywhere; no personal photos are needed or committed.

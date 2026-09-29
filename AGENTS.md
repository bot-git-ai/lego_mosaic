# lego_mosaic

Mosaic Studio: private browser-side picture → LEGO tile mosaic. Both conversion
and application behavior are Rust. `cargo build` generates the **static PWA**
into `./dist` — a front-end-only site any file host can serve, with no server
and no runtime dependency on the binary. The binary itself is a conversion CLI
and nothing more. Images are not uploaded or persisted.

AGPL-3.0-only. See `LICENSE`.

This is a standalone repository. It was split out of the bot monorepo
(`bot-git-ai/bot`), which still hosts `bot/`, `cobalt/`, `gateway/` and
`llm-proxy/`. Nothing here depends on them; `miv` is another split-out
sibling.

## Build and run

```
rustup target add wasm32-unknown-unknown
cargo test --locked
cargo build --release --locked    # also writes the publishable site to ./dist
```

There is **no run step and no server**: `build.rs` writes `dist/` while the
crate compiles, so a build leaves a publishable site behind.

`assets/mosaic.js` and `assets/mosaic_bg.wasm` are **generated but committed**,
so a clean checkout builds and publishes the site with nothing but cargo. They
are the output of a wasm build plus a `wasm-bindgen` post-pass, and the
generator is an external CLI rather than a cargo dependency, so that one step
is spelled out here rather than hidden in a script:

```
cargo install wasm-bindgen-cli --version 0.2.128 --locked
rustup target add wasm32-unknown-unknown
cargo build --locked --lib --target wasm32-unknown-unknown --release
wasm-bindgen --target web --no-typescript --out-dir assets --out-name mosaic \
  target/wasm32-unknown-unknown/release/lego_mosaic.wasm
```

`wasm-bindgen` installs to `~/.cargo/bin`, which is on `PATH` in a normal
login shell; in a bare or non-login shell call it by absolute path
(`~/.cargo/bin/wasm-bindgen`) or set `WASM_BINDGEN` to its location.

Rebuild and commit both artifacts after any browser/core/palette/render change,
**before** building the binary that embeds them. The version is pinned to
`=0.2.128` in `Cargo.toml` and must match the CLI exactly; a mismatched
generator produces bindings the runtime will not load, and the page then fails
to start with "Studio could not start". No npm/JS build tool is needed. A tiny
dynamic-import loader in ui.html
plus a worker bootstrap, small service-worker cache/lifecycle shell and generated
wasm-bindgen platform bindings are the only JavaScript in the app;
there is no handwritten JavaScript application or raw-pointer conversion ABI.

## The static site

The studio is a browser application; there is no server and no service.
`build.rs` writes `dist/` during the build:

```
cargo build --release      # -> dist/, 8 files, ~528K
```

Seven of those files are committed as they are (`src/ui.html`,
`src/worker.js`, `src/service-worker.js`, the two icons and the two generated
wasm artifacts). Two are derived at build time: `manifest.webmanifest`, which
is now the committed `src/manifest.webmanifest` rather than a Rust string
literal, and `service-worker.js`, whose cache name is pinned to a version
derived from the bytes of every *other* shell file **and its own source** — so
a change to the caching logic invalidates the cache too.

The tree is built in a staging directory and swapped in by rename, so a failed
build cannot leave a half-updated site where a host is serving. `build.rs`
writes the same bytes to `OUT_DIR/dist`, which is what the tests read.

Everything the shell references is relative (`./mosaic.js`,
`new URL('./', self.location.href)`, `start_url: "./"`), so one build works
from any subdirectory. Any file host can publish it: nginx, Caddy, GitHub
Pages, `python3 -m http.server`.

`dist/` is build output and is gitignored. It is reproducible: a clean
checkout's `cargo build` produces the same bytes.

## Tests

`tests/shell.rs` asserts the invariants of the *generated* site — that the page
loads the generated bindings rather than a hand-written wasm ABI, that every
referenced file is present and non-empty, that the worker cache is pinned to a
real version, that URLs are relative so the site mounts anywhere, that the
shipped wasm is the committed artifact byte for byte, and that `dist/` stays
ignored and untracked. There are no browser tests: the shell is written by the
build, so nothing else would notice it breaking.

CLI: `lego-mosaic convert IMAGE --output mosaic.svg [--parts x.csv --guide g.svg
--size 64 --palette extended --preset photo --recipe o.json --save-recipe o.json
--color-limit 900 --stages-dir DIR --fit stretch --pad 0 --studs --dither --force]`, PNG/SVG output, never overwrites input or existing
outputs without `--force`. It is a thin native I/O shell: every mosaic byte
comes from the same public library (`lego_mosaic::convert`/`render`) the
browser uses. `tests/cli_smoke.rs` checks exact expected SVG bytes via a
2×2 PNG.

There is no `serve` subcommand, no unit, and no `/healthz`: the binary is a
conversion tool, and publishing is the build's job.

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
- `assets.rs`: the app shell as a single ordered list of typed files, shared by
  the browser bindings the page loads.
- `build.rs`: writes the eight-file shell to `dist/` and to `OUT_DIR/dist`,
  deriving the manifest file and the worker's content-derived cache version.
- `main.rs`: argument handling only. `server.rs`, `ui.rs`, `assets.rs` and
  `dist.rs` are gone with the host and the run step.

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

`tests/shell.rs` runs under plain `cargo test` with no browser and no
external tool. It asserts the invariants of the generated site rather than
driving it; the browser suites that used to do so needed the now-deleted
server binary as their host, and the app has no logic a unit test cannot
reach — the conversion is the same library the CLI tests already cover.

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

Warnings are denied, for the native target and for `wasm32-unknown-unknown`.

The conversion is the same library the CLI tests exercise, and `tests/shell.rs`
covers what the build emits, so the suite needs neither a browser nor an
external tool. What this no longer covers is DOM and loader integration in a
real browser; that is the cost of deleting the browser suites, which needed the
server binary this project no longer has.

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

Nothing is uploaded anywhere; no personal photos are needed or committed.

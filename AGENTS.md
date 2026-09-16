# lego_mosaic (`lego_mosaic/`)

LEGO Mosaic Maker: picture → round-plate mosaic web app. The whole
conversion runs **in the browser**: a zero-dependency Rust→wasm module
does the pixel work, the server binary only serves static assets.

## Running

`target/release/lego-mosaic` listens on `127.0.0.1:3210` (override with
`LEGO_MOSAIC_ADDR`). Deployed as the `lego-mosaic.service` systemd unit;
bot-web reverse-proxies it under the `/lego-mosaic` prefix, so it's at
`https://bot.<tailnet>.ts.net/lego-mosaic/`. The proxy only ever relays
three GETs (`/`, `/mosaic.wasm`, `/healthz`) — no POST endpoints exist,
so proxy/body/header quirks can't break conversions.

## Building

Both targets build from this one crate:

```
./build-wasm.sh            # wasm module → assets/lego_mosaic.wasm (committed)
cargo build --release      # server binary (embeds assets/lego_mosaic.wasm)
```

The wasm module needs `rustup target add wasm32-unknown-unknown` once.
`assets/lego_mosaic.wasm` is committed so the released binary is
self-contained; `main.rs` test asserts the embedded bytes start with the
`\0asm` magic.

## Design

- `color.rs` — sRGB → CIELAB (D65) and CIEDE2000. Matching is perceptual,
  never raw RGB: that's the main reason the official converter looks muddy.
- `palette.rs` — tile palettes: the 5 official Mosaic Maker colors,
  a grayscale ramp, and a 34-color extended round-plate (98138) palette.
  Hex values are LDraw-style approximations.
- `mosaic.rs` — pipeline on a plain RGBA8 `Raster`: crop-to-aspect (studs
  are square; crop, never stretch) → area-filter downscale to a 4× hi-res
  grid → brightness / contrast / saturation → optional white-background
  forcing → mapping. Two orders: `Sharp` (quantize hi-res, majority-vote
  per stud — flat art, default) and `Smooth` (average per stud — photos);
  optional serpentine Floyd–Steinberg dithering for gradients.
- `render.rs` — SVG stud preview, parts counts, bottom-up build rows.
- `lib.rs` — the wasm ABI: `convert` plus `palettes_json`/`defaults_json`
  metadata exports. Hand-rolled C ABI (no wasm-bindgen): strings come back
  through one output slot (`output_ptr`/`output_len`), buffers are
  `alloc_buf`/`drop_buf`. `main.rs`'s old `json_string` escaping lives here.
- `server.rs` — minimal HTTP/1.1, GET-only, static assets only.
- `main.rs` — routes: `/` (page), `/mosaic.wasm` (module), `/healthz`.
- `ui.rs` + `ui.html` — single static page, vanilla JS: decodes the image
  on a canvas (`createImageBitmap`), copies the RGBA bytes into the module
  and calls `convert`; palette swatches and form defaults come from the
  module, never duplicated in the HTML.

## Conventions

- Zero runtime dependencies (no `image`, no serde, no wasm-bindgen): image
  decoding is the browser's canvas; JSON is hand-rolled.
- `[lints.rust] warnings = "deny"` — keep `cargo build` warning-free.
- Tests are hermetic; `cargo test` covers the full conversion pipeline on
  the host target. The wasm ABI is exercised the same way in a browser or
  `node -e` (instantiation + `convert` round-trip).

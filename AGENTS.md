# lego_mosaic (`lego_mosaic/`)

LEGO Mosaic Maker: picture → round-plate mosaic web app in pure Rust.
Converts an uploaded image into a stud grid (default 48×48, the Mosaic
Maker set size), renders a stud preview, parts list and build rows.

## Running

`target/release/lego-mosaic` listens on `127.0.0.1:3210` (override with
`LEGO_MOSAIC_ADDR`). Deployed as the `lego-mosaic.service` systemd unit;
bot-web reverse-proxies it under the `/lego-mosaic` prefix, so it's at
`https://bot.<tailnet>.ts.net/lego-mosaic/`.

## Design

- `color.rs` — sRGB → CIELAB (D65) and CIEDE2000. Matching is perceptual,
  never raw RGB: that's the main reason the official converter looks muddy.
- `palette.rs` — tile palettes: the 5 official Mosaic Maker colors,
  a grayscale ramp, and a 34-color extended round-plate (98138) palette.
  Hex values are LDraw-style approximations.
- `mosaic.rs` — pipeline: crop-to-aspect (studs are square; crop, never
  stretch) → area-filter downscale to a 4× hi-res grid → brightness /
  contrast / saturation → optional white-background forcing → mapping.
  Two orders: `Sharp` (quantize hi-res, majority-vote per stud — flat art,
  default) and `Smooth` (average per stud — photos); optional serpentine
  Floyd–Steinberg dithering for gradients.
- `render.rs` — SVG stud preview, parts counts, bottom-up build rows.
- `multipart.rs` — minimal multipart/form-data parsing (binary-safe).
- `server.rs` — minimal HTTP/1.1 (same style as bot-web's `http` module).
- `ui.rs` — single page, vanilla JS, no build step.

## Conventions

- Pure Rust, no serde/clap: hand-rolled JSON escaping (`json_string`) and
  form parsing; the API surface is one endpoint.
- `[lints.rust] warnings = "deny"` — keep `cargo build` warning-free.
- Tests are hermetic (no network, no filesystem); `convert_handler_end_to_end`
  exercises the full multipart → JSON path.

// Copyright (c) 2026 Witalis Domitrz <witekdomitrz@gmail.com>
// AGPL License

//! Mosaic conversion and the browser application, both written in Rust.
//! The native binary serves embedded static assets only. On wasm the
//! wasm-bindgen start function installs the UI and calls the core directly:
//! no raw pointer ABI or hand-maintained JavaScript conversion protocol.
// Native builds exercise the core through unit tests; the static server
// intentionally does not call it or decode uploaded images.
#![cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]

pub(crate) mod color;
pub(crate) mod mosaic;
pub(crate) mod palette;
pub(crate) mod render;

#[cfg(target_arch = "wasm32")]
mod browser;

// One public core shared by the native CLI and the Rust browser front end.
pub use color::Srgb;
pub use mosaic::{stages, FitMode, HueMode, HueRemap, Mosaic, Options, Order, Raster, Stages};
pub use palette::{all as palettes, TileColor};
pub use render::{
    build_rows, build_rows_order, flat_svg, guide_svg, guide_svg_order, parts_csv, stud_svg,
};

pub fn convert(image: &Raster, palette_id: &str, options: &Options) -> Result<Mosaic, String> {
    let tiles = palette::all()
        .iter()
        .find(|p| p.0 == palette_id)
        .ok_or_else(|| format!("Unknown palette: {palette_id}"))?
        .2;
    Ok(mosaic::convert(image, tiles, options))
}

#[cfg(target_arch = "wasm32")]
mod worker;

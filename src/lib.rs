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

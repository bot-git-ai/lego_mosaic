// Copyright (c) 2026 Witalis Domitrz <witekdomitrz@gmail.com>
// AGPL License

//! The conversion core, compiled to a wasm module (`wasm32-unknown-unknown`,
//! no wasm-bindgen, no malloc'd return strings: every export writes into a
//! caller-allocated buffer or returns an offset/length pair into the
//! module's own allocation, which the JS side copies out and frees).
//!
//! Protocol for string-returning exports:
//! - `*_len() -> u32` reports the byte length of the last produced string
//!   (0 on error), and `*_ptr() -> *mut u8` its location. The JS glue
//!   allocates, calls the producer, checks the length, copies the bytes and
//!   drops the buffer back via `drop_buf`.
//!
//! Every call is serialized behind a mutex because the exports share one
//! static output slot — the UI runs one conversion at a time anyway.

pub(crate) mod color;
pub(crate) mod mosaic;
pub(crate) mod palette;
pub(crate) mod render;

use std::fmt::Write as _;
use std::sync::Mutex;

/// The single string slot shared by all exports, guarded for reentrancy.
static OUTPUT: Mutex<Vec<u8>> = Mutex::new(Vec::new());

fn set_output(value: String) {
    *OUTPUT.lock().unwrap_or_else(|e| e.into_inner()) = value.into_bytes();
}

fn output() -> std::sync::MutexGuard<'static, Vec<u8>> {
    OUTPUT.lock().unwrap_or_else(|e| e.into_inner())
}

/// Byte length of the last produced string (0 when the last call failed).
#[no_mangle]
pub extern "C" fn output_len() -> u32 {
    output().len() as u32
}

/// Location of the last produced string; valid until the next export call.
#[no_mangle]
pub extern "C" fn output_ptr() -> *const u8 {
    output().as_ptr()
}

/// Return a `drop_buf`-allocated buffer to the module.
///
/// # Safety
/// `ptr` must come from `drop_buf` and `len` must match its allocation.
#[no_mangle]
pub unsafe extern "C" fn drop_buf(ptr: *mut u8, len: usize) {
    if !ptr.is_null() && len != 0 {
        drop(Vec::from_raw_parts(ptr, len, len));
    }
}

/// Allocate a buffer for JS to copy into / pass back.
///
/// # Safety
/// The returned pointer must be released with `drop_buf(ptr, len)`.
#[no_mangle]
pub unsafe extern "C" fn alloc_buf(len: usize) -> *mut u8 {
    let mut buf = Vec::<u8>::with_capacity(len);
    let ptr = buf.as_mut_ptr();
    std::mem::forget(buf);
    ptr
}

/// Palette ids, labels and swatch lists for the UI, as JSON:
/// `[["id", "label", [["name", "#hex"], …]], …]`.
#[no_mangle]
pub extern "C" fn palettes_json() {
    let mut out = String::from("[");
    for (i, (id, label, tiles)) in palette::all().iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        write!(out, "[\"{id}\",\"{label}\",[").expect("writing to String cannot fail");
        for (j, tile) in tiles.iter().enumerate() {
            if j > 0 {
                out.push(',');
            }
            write!(out, "[\"{}\",\"{}\"]", tile.name, tile.rgb.hex())
                .expect("writing to String cannot fail");
        }
        out.push_str("]]");
    }
    out.push(']');
    set_output(out);
}

/// One option per form field, packed as JSON for the wasm caller.
/// `options_json` emits the default knobs so the page never duplicates the
/// Rust defaults.
#[no_mangle]
pub extern "C" fn defaults_json() {
    let o = mosaic::Options::default();
    set_output(format!(
        r#"{{"size":{},"saturation":{},"contrast":{},"order":"{}","dither":{},"whiteBackground":{}}}"#,
        o.width,
        o.saturation,
        o.contrast,
        if matches!(o.order, mosaic::Order::Smooth) {
            "smooth"
        } else {
            "sharp"
        },
        o.dither,
        o.white_background
    ));
}

/// Run a conversion.
///
/// `pixels` holds `width × height × 4` RGBA8 bytes (already copied into the
/// module via `alloc_buf`); the other pointers are `u32` inputs by value:
/// `grid` (studs per side), `palette_id_ptr/len`, `order_ptr/len`,
/// saturation ×1000, contrast ×1000, `dither`, `white_background`, and
/// `excluded_ptr/len` — a `u32` array of palette indices to exclude.
///
/// # Safety
/// `pixels` must be a live `alloc_buf(len)` buffer of exactly
/// `width*height*4` bytes, freed by the caller with `drop_buf` after this
/// returns. `palette_id`/`order` must be valid UTF-8 buffers with matching
/// lengths. `excluded` must point at `excluded_len` valid `u32`s.
#[no_mangle]
#[allow(clippy::too_many_arguments)]
pub unsafe extern "C" fn convert(
    pixels: *const u8,
    width: u32,
    height: u32,
    grid: u32,
    palette_id: *const u8,
    palette_id_len: usize,
    order: *const u8,
    order_len: usize,
    saturation_milli: u32,
    contrast_milli: u32,
    dither: u32,
    white_background: u32,
    excluded: *const u32,
    excluded_len: usize,
) {
    let pixels = std::slice::from_raw_parts(pixels, width as usize * height as usize * 4);
    let palette_id =
        String::from_utf8_lossy(std::slice::from_raw_parts(palette_id, palette_id_len))
            .into_owned();
    let order = String::from_utf8_lossy(std::slice::from_raw_parts(order, order_len)).into_owned();
    let excluded = if excluded_len == 0 {
        Vec::new()
    } else {
        std::slice::from_raw_parts(excluded, excluded_len)
            .iter()
            .map(|&i| i as usize)
            .collect()
    };

    let Some(palette) = palette::all()
        .iter()
        .find(|(id, _, _)| *id == palette_id)
        .map(|(_, _, tiles)| *tiles)
    else {
        set_output("{\"error\": \"unknown palette\"}".to_string());
        return;
    };

    let options = mosaic::Options {
        width: grid as usize,
        height: grid as usize,
        saturation: f64::from(saturation_milli) / 1000.0,
        contrast: f64::from(contrast_milli) / 1000.0,
        brightness: 0,
        order: mosaic::Order::from_str(&order),
        dither: dither != 0,
        white_background: white_background != 0,
        // The despeckle pass only fixes anti-aliasing stragglers; dithering
        // intentionally produces speckle, so never undo it.
        despeckle: dither == 0,
        excluded,
    };

    // Degenerate inputs can't happen from the UI (size 8–192, canvas
    // guarantees nonempty pixels), but a wasm module shouldn't panic on
    // hostile calls either: guard the zero-size cases.
    if width == 0 || height == 0 || grid == 0 {
        set_output("{\"error\": \"empty image\"}".to_string());
        return;
    }

    let raster = mosaic::Raster {
        width: width as usize,
        height: height as usize,
        pixels: pixels.to_vec(),
    };
    let result = mosaic::convert(&raster, palette, &options);
    let svg = render::stud_svg(&result, 20);
    let parts_list = render::parts_list(&result);
    let rows = render::build_rows(&result);

    let mut out = format!(
        r#"{{"svg":{},"width":{},"height":{},"tiles":{},"parts":["#,
        json_string(&svg),
        result.width,
        result.height,
        result.grid.len()
    );
    for (i, part) in parts_list.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        write!(
            out,
            r#"{{"name":{},"hex":"{}","count":{}}}"#,
            json_string(&part.name),
            part.hex,
            part.count
        )
        .expect("writing to String cannot fail");
    }
    out.push_str("],\"rows\":[");
    for (i, row) in rows.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        out.push('[');
        for (j, (name, hex)) in row.iter().enumerate() {
            if j > 0 {
                out.push(',');
            }
            write!(out, "[{},\"{}\"]", json_string(name), hex)
                .expect("writing to String cannot fail");
        }
        out.push(']');
    }
    out.push_str("]}");
    set_output(out);
}

/// Escape a string as a JSON string literal (hand-rolled: no serde).
fn json_string(value: &str) -> String {
    let mut out = String::with_capacity(value.len() + 2);
    out.push('"');
    for c in value.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => {
                let code = c as u32;
                write!(out, "\\u{code:04x}").expect("writing to String cannot fail");
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_string_escapes() {
        assert_eq!(json_string("say \"hi\"\n"), r#""say \"hi\"\n""#);
        assert_eq!(json_string("tab\there"), r#""tab\there""#);
    }
}

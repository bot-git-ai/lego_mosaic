// Copyright (c) 2026 Witalis Domitrz <witekdomitrz@gmail.com>
// AGPL License

//! Output rendering: an SVG stud preview (each tile drawn as a round stud
//! with a highlight, so the mosaic reads like the real plate), the parts
//! list, and the row-by-row build instructions.

// The SVG builder writes pre-computed u8 colors and small u32 geometry
// (grid ≤ 192 studs), so the casts below cannot truncate or lose sign.
#![allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss
)]

use std::fmt::Write as _;

use crate::color::Srgb;
use crate::mosaic::Mosaic;

/// Stud-preview SVG. `stud` is the stud diameter in px; gaps between studs
/// come out of the spacing so the mosaic looks like a plate of round
/// 1×1 tiles. Cell shading gives the studs a subtle 3D look.
pub(crate) fn stud_svg(mosaic: &Mosaic, stud: u32) -> String {
    let pad = stud / 8; // gap between studs
    let cell = stud + pad;
    let out_w = mosaic.width as u32 * cell + pad;
    let out_h = mosaic.height as u32 * cell + pad;
    let mut out = String::with_capacity(mosaic.grid.len() * 96 + 256);
    write!(
        out,
        "<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 {out_w} {out_h}\" \
         width=\"{out_w}\" height=\"{out_h}\" shape-rendering=\"crispEdges\">"
    )
    .expect("writing to String cannot fail");
    // Plate background (the gaps between studs).
    write!(
        out,
        "<rect width=\"{out_w}\" height=\"{out_h}\" fill=\"#1a1a1a\"/>"
    )
    .expect("writing to String cannot fail");
    for (i, &index) in mosaic.grid.iter().enumerate() {
        let tile = &mosaic.palette[index];
        let x = (i % mosaic.width) as u32 * cell + pad;
        let y = (i / mosaic.width) as u32 * cell + pad;
        let hex = tile.rgb.hex();
        write!(
            out,
            "<rect x=\"{x}\" y=\"{y}\" width=\"{stud}\" height=\"{stud}\" fill=\"{hex}\"/>"
        )
        .expect("writing to String cannot fail");
        // Stud cylinder: a slightly lighter circle with a darker rim and a
        // highlight arc, so it reads as a round tile under light.
        let cx = x + stud / 2;
        let cy = y + stud / 2;
        let r = stud / 2 - 1;
        let light = lighten(tile.rgb, 18).hex();
        let dark = lighten(tile.rgb, -22).hex();
        write!(
            out,
            "<circle cx=\"{cx}\" cy=\"{cy}\" r=\"{r}\" fill=\"{light}\"/>"
        )
        .expect("writing to String cannot fail");
        write!(
            out,
            "<circle cx=\"{cx}\" cy=\"{cy}\" r=\"{r}\" fill=\"none\" stroke=\"{dark}\" \
             stroke-width=\"{}\"/>",
            (stud / 16).max(1)
        )
        .expect("writing to String cannot fail");
        // Highlight: a small ellipse offset toward the top-left.
        let hx = cx - r / 3;
        let hy = cy - r / 3;
        let hr = (r / 4).max(1);
        write!(
            out,
            "<circle cx=\"{hx}\" cy=\"{hy}\" r=\"{hr}\" fill=\"#ffffff\" opacity=\"0.35\"/>"
        )
        .expect("writing to String cannot fail");
    }
    out.push_str("</svg>");
    out
}

/// Mix a color toward white (`amount` > 0) or black (`amount` < 0);
/// `amount` is a percentage.
fn lighten(c: Srgb, amount: i32) -> Srgb {
    let mix = |v: u8| -> u8 {
        let v = i32::from(v);
        let target = if amount >= 0 { 255 } else { 0 };
        let a = amount.abs();
        (v * (100 - a) / 100 + target * a / 100).clamp(0, 255) as u8
    };
    Srgb::new(mix(c.r), mix(c.g), mix(c.b))
}

/// One row of the parts list: color, count.
pub(crate) struct PartCount {
    pub(crate) name: String,
    pub(crate) hex: String,
    pub(crate) count: usize,
}

/// Aggregate the grid into per-color counts, sorted by count descending.
pub(crate) fn parts_list(mosaic: &Mosaic) -> Vec<PartCount> {
    let mut counts = vec![0_usize; mosaic.palette.len()];
    for &index in &mosaic.grid {
        counts[index] += 1;
    }
    let mut parts: Vec<_> = mosaic
        .palette
        .iter()
        .zip(counts)
        .filter(|(_, count)| *count > 0)
        .map(|(tile, count)| PartCount {
            name: tile.name.to_string(),
            hex: tile.hex(),
            count,
        })
        .collect();
    parts.sort_by(|a, b| b.count.cmp(&a.count).then(a.name.cmp(&b.name)));
    parts
}

/// The grid as rows of color names — the build instruction rows, matching
/// what the builder places stud by stud from bottom to top (like LEGO
/// instructions, row 1 is the bottom of the mosaic).
pub(crate) fn build_rows(mosaic: &Mosaic) -> Vec<Vec<(String, String)>> {
    let mut rows = Vec::with_capacity(mosaic.height);
    for y in (0..mosaic.height).rev() {
        let row = (0..mosaic.width)
            .map(|x| {
                let tile = &mosaic.palette[mosaic.grid[y * mosaic.width + x]];
                (tile.name.to_string(), tile.hex())
            })
            .collect();
        rows.push(row);
    }
    rows
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mosaic::{convert, Options, Raster};
    use crate::palette::MOSAIC_MAKER;

    fn solid(w: usize, h: usize, rgba: [u8; 4]) -> Raster {
        Raster {
            width: w,
            height: h,
            pixels: rgba.iter().copied().cycle().take(w * h * 4).collect(),
        }
    }

    #[test]
    fn svg_contains_every_stud_and_correct_dimensions() {
        let image = solid(32, 32, [10, 10, 10, 255]);
        let mosaic = convert(
            &image,
            MOSAIC_MAKER,
            &Options {
                width: 8,
                height: 8,
                ..Options::default()
            },
        );
        let svg = stud_svg(&mosaic, 20);
        assert_eq!(svg.matches("<circle").count(), 8 * 8 * 3);
        // cell = stud + pad = 22; width = 8·22 + pad(2) = 178.
        assert!(
            svg.contains("viewBox=\"0 0 178 178\""),
            "svg header: {}",
            &svg[..120]
        );
    }

    #[test]
    fn parts_list_totals_the_grid() {
        let image = solid(32, 32, [128, 128, 128, 255]);
        let mosaic = convert(
            &image,
            MOSAIC_MAKER,
            &Options {
                width: 6,
                height: 6,
                ..Options::default()
            },
        );
        let parts = parts_list(&mosaic);
        assert_eq!(parts.iter().map(|p| p.count).sum::<usize>(), 36);
        assert_eq!(parts.len(), 1);
    }

    #[test]
    fn build_rows_are_bottom_up() {
        // Bottom half black, top half white.
        let mut image = solid(16, 16, [255, 255, 255, 255]);
        for y in 8..16 {
            for x in 0..16 {
                image.put(x, y, [0, 0, 0, 255]);
            }
        }
        let mosaic = convert(
            &image,
            MOSAIC_MAKER,
            &Options {
                width: 4,
                height: 4,
                ..Options::default()
            },
        );
        let rows = build_rows(&mosaic);
        assert_eq!(rows.len(), 4);
        // First build row = bottom of the image = black.
        assert_eq!(rows[0][0].0, "Black");
        assert_eq!(rows[3][0].0, "White");
    }
}

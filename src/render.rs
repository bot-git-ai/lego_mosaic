// Copyright (c) 2026 Witalis Domitrz <witekdomitrz@gmail.com>
// AGPL License

//! Exact-color previews, vector build guides and parts exports for round
//! 1×1 tiles (part 98138). Screen swatches are approximate; availability is
//! not verified. Every output uses the same final mosaic grid.

#![allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss
)]

use std::fmt::Write as _;

use crate::mosaic::Mosaic;

/// Round-tile preview. `stud` is the tile diameter in px. Small neutral
/// gaps and a subtle rim depict a smooth round TILE, not a square plate
/// with an oversized bright stud. Tile faces retain the exact palette RGB.
pub fn stud_svg(mosaic: &Mosaic, stud: u32) -> String {
    let stud = stud.clamp(2, 256);
    let pad = (stud / 8).max(1);
    let cell = stud + pad;
    let out_w = mosaic.width as u32 * cell + pad;
    let out_h = mosaic.height as u32 * cell + pad;
    let mut out = svg_start(out_w, out_h, "Round tile mosaic preview");
    write!(
        out,
        "<rect width=\"{out_w}\" height=\"{out_h}\" fill=\"#b8b8b8\"/>"
    )
    .expect("writing to String cannot fail");
    for (i, &index) in mosaic.grid.iter().enumerate() {
        let cx =
            (i % mosaic.width) as f64 * f64::from(cell) + f64::from(pad) + f64::from(stud) / 2.0;
        let cy =
            (i / mosaic.width) as f64 * f64::from(cell) + f64::from(pad) + f64::from(stud) / 2.0;
        let radius = f64::from(stud) * 0.48;
        let rim = f64::from(stud) * 0.025;
        write!(out, "<circle cx=\"{cx}\" cy=\"{cy}\" r=\"{radius}\" fill=\"{}\" stroke=\"#000\" stroke-opacity=\"0.18\" stroke-width=\"{rim}\"/>", mosaic.palette[index].hex())
            .expect("writing to String cannot fail");
    }
    out.push_str("</svg>");
    out
}

/// Unshaded square-cell preview, useful for judging conversion quality
/// without gaps or simulated lighting. The final grid is never modified.
pub fn flat_svg(mosaic: &Mosaic, cell: u32) -> String {
    let cell = cell.clamp(1, 256);
    let mut out = svg_start(
        mosaic.width as u32 * cell,
        mosaic.height as u32 * cell,
        "Flat exact-color mosaic preview",
    );
    out.push_str("<g shape-rendering=\"crispEdges\">");
    for (i, &index) in mosaic.grid.iter().enumerate() {
        let x = i % mosaic.width * cell as usize;
        let y = i / mosaic.width * cell as usize;
        write!(
            out,
            "<rect x=\"{x}\" y=\"{y}\" width=\"{cell}\" height=\"{cell}\" fill=\"{}\"/>",
            mosaic.palette[index].hex()
        )
        .expect("writing to String cannot fail");
    }
    out.push_str("</g></svg>");
    out
}

fn svg_start(width: u32, height: u32, title: &str) -> String {
    format!("<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 {width} {height}\" width=\"{width}\" height=\"{height}\" role=\"img\"><title>{title}</title>")
}

/// Standalone, scalable vector guide. Row 1 is the bottom image row;
/// column 1 is the left. Bold boundaries divide bottom-anchored 16×16
/// sections, including partial sections at the top/right. Color numbers
/// remain stable even when exclusions reorder the effective palette.
/// The caller chooses SVG download or browser print/PDF scaling/pagination.
pub fn guide_svg(mosaic: &Mosaic, cell: u32) -> String {
    let cell = cell.clamp(16, 128);
    let left = cell * 2;
    let top = cell * 4;
    let grid_w = mosaic.width as u32 * cell;
    let grid_h = mosaic.height as u32 * cell;
    let parts = parts_list(mosaic);
    let legend_top = top + grid_h + cell * 3;
    let out_w = (grid_w + left * 2).max(800);
    let out_h = legend_top + parts.len() as u32 * 26 + 88;
    let mut out = svg_start(out_w, out_h, "Mosaic build guide: row 1 at bottom");
    write!(out, "<rect width=\"{out_w}\" height=\"{out_h}\" fill=\"white\"/><g font-family=\"Arial, sans-serif\" fill=\"#17243a\">")
        .expect("writing to String cannot fail");
    text(
        &mut out,
        left,
        28,
        20,
        "start",
        &format!(
            "Mosaic build guide — {} × {} tiles",
            mosaic.width, mosaic.height
        ),
    );
    text(
        &mut out,
        left,
        49,
        12,
        "start",
        "Row 1 is at the bottom. Build left to right, then upward.",
    );
    // Text badges stay legible in monochrome print and on dark tile fills.
    write!(out, "<g transform=\"translate({left} {top})\">")
        .expect("writing to String cannot fail");
    guide_cells(&mut out, mosaic, cell);
    guide_coordinates(&mut out, mosaic, cell);
    out.push_str("</g>");
    text(
        &mut out,
        left,
        legend_top - 14,
        16,
        "start",
        "Color key / parts needed — numbers are app symbols, not LEGO color IDs",
    );
    for (i, part) in parts.iter().enumerate() {
        let tile = mosaic
            .palette
            .iter()
            .find(|t| t.name == part.name)
            .expect("parts always come from the mosaic palette");
        let y = legend_top + i as u32 * 26;
        write!(
            out,
            "<rect x=\"{left}\" y=\"{y}\" width=\"20\" height=\"20\" fill=\"{}\" stroke=\"#777\"/>",
            tile.hex()
        )
        .expect("writing to String cannot fail");
        text(
            &mut out,
            left + 30,
            y + 15,
            14,
            "start",
            &format!("{} — {} — {} tiles", tile.symbol(), tile.name, part.count),
        );
    }
    let foot = legend_top + parts.len() as u32 * 26 + 22;
    text(
        &mut out,
        left,
        foot,
        12,
        "start",
        &format!(
            "Total: {} × 1×1 round tile (part 98138). Bold lines mark 16×16 sections.",
            mosaic.grid.len()
        ),
    );
    text(
        &mut out,
        left,
        foot + 20,
        12,
        "start",
        "RGBs approximate plastic. Part/color availability and stock are NOT verified.",
    );
    text(
        &mut out,
        left,
        foot + 40,
        12,
        "start",
        "Vector SVG: zoom freely. For PDF, choose an appropriate print scale or larger paper.",
    );
    out.push_str("</g></svg>");
    out
}

fn guide_cells(out: &mut String, mosaic: &Mosaic, cell: u32) {
    for (i, &index) in mosaic.grid.iter().enumerate() {
        let tile = &mosaic.palette[index];
        let x = (i % mosaic.width) as u32 * cell;
        let y = (i / mosaic.width) as u32 * cell;
        let cx = x + cell / 2;
        let cy = y + cell / 2;
        write!(out, "<g data-column=\"{}\" data-row=\"{}\" data-color=\"{}\"><rect x=\"{x}\" y=\"{y}\" width=\"{cell}\" height=\"{cell}\" fill=\"{}\" stroke=\"#999\" stroke-width=\"0.5\"/>", i % mosaic.width + 1, mosaic.height - i / mosaic.width, xml_escape(&tile.id()), tile.hex())
            .expect("writing to String cannot fail");
        let badge_w = cell * 3 / 4;
        let badge_h = cell * 3 / 5;
        write!(out, "<rect x=\"{}\" y=\"{}\" width=\"{badge_w}\" height=\"{badge_h}\" rx=\"2\" fill=\"white\"/>", cx - badge_w / 2, cy - badge_h / 2)
            .expect("writing to String cannot fail");
        text(
            out,
            cx,
            cy + cell / 7,
            cell * 2 / 5,
            "middle",
            &tile.symbol(),
        );
        out.push_str("</g>");
    }
}

fn guide_coordinates(out: &mut String, mosaic: &Mosaic, cell: u32) {
    let w = mosaic.width as u32 * cell;
    let h = mosaic.height as u32 * cell;
    let font = cell * 2 / 5;
    for x in 0..mosaic.width {
        let cx = x as u32 * cell + cell / 2;
        // Negative coordinates are intentional: labels sit outside the grid.
        write!(
            out,
            "<text x=\"{cx}\" y=\"-8\" font-size=\"{font}\" text-anchor=\"middle\">{}</text>",
            x + 1
        )
        .expect("writing to String cannot fail");
        text(out, cx, h + cell, font, "middle", &(x + 1).to_string());
    }
    for y in 0..mosaic.height {
        let cy = y as u32 * cell + cell / 2 + cell / 7;
        write!(
            out,
            "<text x=\"-8\" y=\"{cy}\" font-size=\"{font}\" text-anchor=\"end\">{}</text>",
            mosaic.height - y
        )
        .expect("writing to String cannot fail");
        text(
            out,
            w + 8,
            cy,
            font,
            "start",
            &(mosaic.height - y).to_string(),
        );
    }
    out.push_str(
        "<g class=\"section-boundaries\" stroke=\"#17243a\" stroke-width=\"2\" fill=\"none\">",
    );
    for x in (16..mosaic.width).step_by(16) {
        let x = x as u32 * cell;
        write!(out, "<path d=\"M{x} 0V{h}\"/>").expect("writing to String cannot fail");
    }
    for row in (16..mosaic.height).step_by(16) {
        let y = (mosaic.height - row) as u32 * cell;
        write!(out, "<path d=\"M0 {y}H{w}\"/>").expect("writing to String cannot fail");
    }
    write!(out, "<rect width=\"{w}\" height=\"{h}\"/></g>").expect("writing to String cannot fail");
}

fn text(out: &mut String, x: u32, y: u32, size: u32, anchor: &str, value: &str) {
    write!(
        out,
        "<text x=\"{x}\" y=\"{y}\" font-size=\"{size}\" text-anchor=\"{anchor}\">{}</text>",
        xml_escape(value)
    )
    .expect("writing to String cannot fail");
}

fn xml_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

/// Quoted CSV fields, CRLF records and explicit unverified availability.
/// This is a design parts count, not a stock check or an order submission.
pub fn parts_csv(mosaic: &Mosaic) -> String {
    let mut out =
        String::from("Color ID,Symbol,Color,Hex,Quantity,Part,Description,Availability\r\n");
    for part in parts_list(mosaic) {
        let tile = mosaic
            .palette
            .iter()
            .find(|t| t.name == part.name)
            .expect("parts always come from the mosaic palette");
        let fields = [
            tile.id(),
            tile.symbol(),
            part.name,
            part.hex,
            part.count.to_string(),
            "98138".to_string(),
            "Tile, Round 1 x 1".to_string(),
            "Not verified".to_string(),
        ];
        let fields: Vec<_> = fields
            .iter()
            .map(|value| format!("\"{}\"", value.replace('"', "\"\"")))
            .collect();
        out.push_str(&fields.join(","));
        out.push_str("\r\n");
    }
    out
}

/// One row of the parts list: color, count.
pub struct PartCount {
    pub name: String,
    pub hex: String,
    pub count: usize,
}

/// Aggregate the grid into per-color counts, sorted by count descending.
pub fn parts_list(mosaic: &Mosaic) -> Vec<PartCount> {
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
pub fn build_rows(mosaic: &Mosaic) -> Vec<Vec<(String, String)>> {
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
        assert_eq!(svg.matches("<circle").count(), 8 * 8);
        assert_eq!(svg.matches("<rect").count(), 1);
        assert!(!svg.contains("#ffffff"));
        assert!(svg.contains("fill=\"#05131d\""));
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
    fn fixture(width: usize, height: usize, grid: Vec<usize>) -> Mosaic {
        Mosaic {
            width,
            height,
            grid,
            palette: MOSAIC_MAKER.to_vec(),
        }
    }

    #[test]
    fn flat_preview_uses_exact_colors_and_no_gaps() {
        let mosaic = fixture(2, 2, vec![0, 4, 3, 2]);
        let svg = flat_svg(&mosaic, 20);
        assert!(svg.contains("viewBox=\"0 0 40 40\""));
        assert_eq!(svg.matches("<rect").count(), 4);
        assert!(!svg.contains("circle"));
        assert!(svg.contains("x=\"20\" y=\"0\" width=\"20\" height=\"20\" fill=\"#f2cd37\""));
        assert!(svg.contains("x=\"0\" y=\"20\" width=\"20\" height=\"20\" fill=\"#05131d\""));
    }

    #[test]
    fn guide_keeps_image_orientation_with_bottom_up_coordinates() {
        let mosaic = fixture(2, 2, vec![0, 4, 3, 2]);
        let svg = guide_svg(&mosaic, 24);
        assert!(svg.contains("data-column=\"1\" data-row=\"2\" data-color=\"white\""));
        assert!(svg.contains("data-column=\"1\" data-row=\"1\" data-color=\"black\""));
        assert!(svg.contains("12 — Yellow — 1 tiles"));
        assert!(svg.contains("06 — Black — 1 tiles"));
        assert!(svg.contains("Total: 4 × 1×1 round tile (part 98138)"));
        assert!(svg.contains("NOT verified"));
        assert_eq!(svg.matches("data-column=").count(), 4);
        assert_eq!(build_rows(&mosaic)[0][0].0, "Black");
    }

    #[test]
    fn section_boundaries_are_bottom_anchored_on_partial_grids() {
        let mosaic = fixture(18, 20, vec![0; 18 * 20]);
        let svg = guide_svg(&mosaic, 24);
        // x=16*24; y=(20-16)*24, not 16*24 from the top.
        assert!(svg.contains("<path d=\"M384 0V480\"/>"));
        assert!(svg.contains("<path d=\"M0 96H432\"/>"));
        assert!(!svg.contains("<path d=\"M0 384H432\"/>"));
    }

    #[test]
    fn csv_counts_and_symbols_survive_palette_reordering() {
        let mosaic = Mosaic {
            width: 3,
            height: 1,
            grid: vec![0, 1, 0],
            palette: vec![MOSAIC_MAKER[4], MOSAIC_MAKER[0]],
        };
        let csv = parts_csv(&mosaic);
        assert_eq!(csv.lines().count(), 3);
        assert!(csv.contains("\"yellow\",\"12\",\"Yellow\",\"#f2cd37\",\"2\",\"98138\",\"Tile, Round 1 x 1\",\"Not verified\"\r\n"));
        assert!(csv.find("Yellow").unwrap() < csv.find("White").unwrap());
        assert!(guide_svg(&mosaic, 24).contains("12 — Yellow — 2 tiles"));
        assert_eq!(
            parts_list(&mosaic).iter().map(|p| p.count).sum::<usize>(),
            3
        );
    }

    #[test]
    fn exports_escape_color_names_and_handle_small_requested_cells() {
        let mosaic = Mosaic {
            width: 1,
            height: 1,
            grid: vec![0],
            palette: vec![crate::palette::TileColor {
                name: "A & <B>, \"C\"",
                rgb: crate::color::Srgb::new(0, 0, 0),
            }],
        };
        assert!(guide_svg(&mosaic, 0).contains("A &amp; &lt;B&gt;, &quot;C&quot;"));
        assert!(parts_csv(&mosaic).contains("\"A & <B>, \"\"C\"\"\""));
        assert!(stud_svg(&mosaic, 0).contains("viewBox=\"0 0 4 4\""));
        assert!(flat_svg(&mosaic, 0).contains("viewBox=\"0 0 1 1\""));
    }
}

// Copyright (c) 2026 Witalis Domitrz <witekdomitrz@gmail.com>
// AGPL License

//! The conversion pipeline: crop to the mosaic's aspect ratio, resize with
//! an area (box) filter, optional preprocessing (saturation, contrast,
//! brightness, background cleanup), then map each stud to a palette color.
//!
//! Input is a plain RGBA8 buffer — the browser decodes the uploaded file
//! (canvas `drawImage`) and hands the pixels over, so this crate has no
//! image-format dependencies and compiles to a tiny wasm module.
//!
//! Two matching orders are supported because they behave very differently:
//! - `Sharp`: quantize the hi-res image first, then take the majority color
//!   per stud cell. Preserves the edges of flat/cartoon art — the default.
//! - `Smooth`: average each stud cell first, then quantize the average.
//!   Softer; better for photographs.
//!
//! On top of either, optional Floyd–Steinberg dithering spreads
//! quantization error for gradients; for flat art it usually hurts.

// Pixel math mixes f64 arithmetic with 8-bit channels and small indices:
// every f64→int cast is clamped beforehand, grids are ≤ 192×192 and the
// hi-res working image is a fixed multiple of the grid, so the truncation,
// wrap and precision loss these casts could in principle suffer cannot
// occur here. Short names (w, h, r, g, b) are the conventional pixel-loop
// spelling.
#![allow(
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss
)]

use crate::color::{delta_e_2000, Lab, Srgb};

/// A decoded image: RGBA8, row-major, `width × height` pixels.
#[derive(Clone, Debug)]
pub(crate) struct Raster {
    pub(crate) width: usize,
    pub(crate) height: usize,
    /// Exactly `width * height * 4` bytes.
    pub(crate) pixels: Vec<u8>,
}

impl Raster {
    /// Pixel accessor with bounds guaranteed by construction.
    fn pixel(&self, x: usize, y: usize) -> [u8; 4] {
        let i = (y * self.width + x) * 4;
        [
            self.pixels[i],
            self.pixels[i + 1],
            self.pixels[i + 2],
            self.pixels[i + 3],
        ]
    }

    pub(crate) fn put(&mut self, x: usize, y: usize, rgba: [u8; 4]) {
        let i = (y * self.width + x) * 4;
        self.pixels[i..i + 4].copy_from_slice(&rgba);
    }
}

/// Conversion knobs; the UI form fields map 1:1 onto these.
#[derive(Clone, Debug)]
pub(crate) struct Options {
    /// Target grid: studs across / studs down.
    pub(crate) width: usize,
    pub(crate) height: usize,
    /// Saturation multiplier (1.0 = unchanged). A small boost keeps
    /// pastels from collapsing into white/gray with 5-color palettes.
    pub(crate) saturation: f64,
    /// Contrast multiplier around the mean luminance (1.0 = unchanged).
    pub(crate) contrast: f64,
    /// Brightness offset added to every channel (−100…100).
    pub(crate) brightness: i32,
    /// Matching order: quantize-then-pool (`Sharp`) vs pool-then-quantize
    /// (`Smooth`).
    pub(crate) order: Order,
    /// Error-diffusion dithering (Floyd–Steinberg, serpentine scan).
    pub(crate) dither: bool,
    /// Force near-white background pixels to White — keeps the cut-out
    /// look of flat illustrations and avoids gray-speckled backgrounds.
    pub(crate) white_background: bool,
    /// Clean up isolated single studs after mapping (a stud with no
    /// neighbor of its color becomes the neighborhood's majority color).
    pub(crate) despeckle: bool,
    /// Stud colors excluded from matching (by palette index).
    pub(crate) excluded: Vec<usize>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Order {
    Sharp,
    Smooth,
}

impl Order {
    pub(crate) fn from_str(s: &str) -> Self {
        if s == "smooth" {
            Self::Smooth
        } else {
            Self::Sharp
        }
    }
}

impl Default for Options {
    fn default() -> Self {
        Self {
            width: 48,
            height: 48,
            saturation: 1.15,
            contrast: 1.05,
            brightness: 0,
            order: Order::Sharp,
            dither: false,
            white_background: true,
            despeckle: true,
            excluded: Vec::new(),
        }
    }
}

/// The conversion result: the grid of palette indices plus the palette
/// actually used (post-exclusion).
pub(crate) struct Mosaic {
    /// Palette indices, row-major (`grid[y * width + x]`).
    pub(crate) grid: Vec<usize>,
    pub(crate) width: usize,
    pub(crate) height: usize,
    pub(crate) palette: Vec<crate::palette::TileColor>,
}

/// Working resolution per stud axis: quantize at 4× the grid, so the
/// majority vote in sharp mode has something to vote on.
const OVERSAMPLE: usize = 4;

/// Run the full pipeline on an RGBA8 image.
pub(crate) fn convert(
    image: &Raster,
    palette: &[crate::palette::TileColor],
    options: &Options,
) -> Mosaic {
    let palette = excluded_palette(palette, &options.excluded);
    let palette_lab: Vec<Lab> = palette.iter().map(|t| t.rgb.to_lab()).collect();

    let cropped = crop_to_aspect(image, options.width, options.height);
    let hi_w = options.width * OVERSAMPLE;
    let hi_h = options.height * OVERSAMPLE;
    let mut hi = area_resize(&cropped, hi_w, hi_h);
    adjust_colors(&mut hi, options);
    if options.white_background {
        force_background(&mut hi);
    }

    let labs: Vec<Lab> = (0..hi.width * hi.height)
        .map(|i| Srgb::new(hi.pixels[i * 4], hi.pixels[i * 4 + 1], hi.pixels[i * 4 + 2]).to_lab())
        .collect();

    let mut grid = match options.order {
        Order::Sharp => sharp_map(&labs, &palette_lab, options),
        Order::Smooth => smooth_map(&labs, &palette_lab, options),
    };
    if options.despeckle {
        despeckle(&mut grid, options.width, options.height);
    }

    Mosaic {
        grid,
        width: options.width,
        height: options.height,
        palette,
    }
}

/// Drop excluded colors from the palette; keep at least one.
fn excluded_palette(
    palette: &[crate::palette::TileColor],
    excluded: &[usize],
) -> Vec<crate::palette::TileColor> {
    let kept: Vec<_> = palette
        .iter()
        .enumerate()
        .filter(|(i, _)| !excluded.contains(i))
        .map(|(_, t)| *t)
        .collect();
    if kept.is_empty() {
        vec![palette[0]]
    } else {
        kept
    }
}

/// Crop the image to the target aspect ratio (centered), without scaling.
/// The mosaic's studs are square, so the source must be cropped, never
/// stretched.
fn crop_to_aspect(image: &Raster, width: usize, height: usize) -> Raster {
    let target = width as f64 / height as f64;
    let (w, h) = (image.width, image.height);
    let current = w as f64 / h as f64;
    let (crop_w, crop_h) = if current > target {
        ((h as f64 * target).round().max(1.0) as usize, h)
    } else {
        (w, ((w as f64) / target).round().max(1.0) as usize)
    };
    let x0 = w.saturating_sub(crop_w) / 2;
    let y0 = h.saturating_sub(crop_h) / 2;
    let mut out = Vec::with_capacity(crop_w * crop_h * 4);
    for y in y0..y0 + crop_h {
        let start = (y * w + x0) * 4;
        out.extend_from_slice(&image.pixels[start..start + crop_w * 4]);
    }
    Raster {
        width: crop_w,
        height: crop_h,
        pixels: out,
    }
}

/// Area-average downscale (box filter): the right choice for both flat art
/// (preserves color blocks) and photos. Transparent pixels composite over
/// white, so transparent-background PNGs behave like the sticker art they
/// usually are.
fn area_resize(image: &Raster, out_w: usize, out_h: usize) -> Raster {
    let src_w = image.width;
    let src_h = image.height;
    let mut out = Raster {
        width: out_w,
        height: out_h,
        pixels: vec![0_u8; out_w * out_h * 4],
    };
    for oy in 0..out_h {
        for ox in 0..out_w {
            let x0 = ox * src_w / out_w;
            let x1 = (((ox + 1) * src_w / out_w).max(x0 + 1)).min(src_w);
            let y0 = oy * src_h / out_h;
            let y1 = (((oy + 1) * src_h / out_h).max(y0 + 1)).min(src_h);
            let (mut red, mut green, mut blue, mut n) = (0_u64, 0_u64, 0_u64, 0_u64);
            for y in y0..y1 {
                for x in x0..x1 {
                    let [pr, pg, pb, pa] = image.pixel(x, y);
                    let a = u64::from(pa);
                    red += (u64::from(pr) * a + 255 * (255 - a)) / 255;
                    green += (u64::from(pg) * a + 255 * (255 - a)) / 255;
                    blue += (u64::from(pb) * a + 255 * (255 - a)) / 255;
                    n += 1;
                }
            }
            out.put(
                ox,
                oy,
                [(red / n) as u8, (green / n) as u8, (blue / n) as u8, 255],
            );
        }
    }
    out
}

/// In-place brightness / contrast / saturation adjustment (in that order,
/// contrast pivoting around the image's mean luminance).
fn adjust_colors(image: &mut Raster, options: &Options) {
    let neutral = |x: f64| (x - 1.0).abs() < f64::EPSILON;
    if neutral(options.saturation) && neutral(options.contrast) && options.brightness == 0 {
        return;
    }
    let mean = {
        let (mut sum, mut n) = (0.0_f64, 0.0_f64);
        for rgba in image.pixels.as_chunks::<4>().0 {
            sum += luminance(rgba[0], rgba[1], rgba[2]);
            n += 1.0;
        }
        sum / n.max(1.0)
    };
    for rgba in image.pixels.as_chunks_mut::<4>().0 {
        // Brightness → contrast (around the mean luminance) → saturation
        // (around the pixel's own luminance), computed per pixel so the
        // saturation pivot is the actual gray level of that pixel.
        let original_lum = luminance(rgba[0], rgba[1], rgba[2]);
        for c in &mut rgba[..3] {
            let x = (f64::from(*c) + f64::from(options.brightness)).clamp(0.0, 255.0);
            let x = (mean + (x - mean) * options.contrast).clamp(0.0, 255.0);
            let x = original_lum + (x - original_lum) * options.saturation;
            *c = x.round().clamp(0.0, 255.0) as u8;
        }
    }
}

fn luminance(r: u8, g: u8, b: u8) -> f64 {
    0.2126 * f64::from(r) + 0.7152 * f64::from(g) + 0.0722 * f64::from(b)
}

/// Force near-white background to pure white on the hi-res image.
/// Threshold ΔE ≈ 7 from white: softly-lit illustration paper counts as
/// background, the panda's white belly (shaded, near #f0ece4) mostly
/// survives. Runs only when requested.
fn force_background(image: &mut Raster) {
    let white = Lab {
        l: 100.0,
        a: 0.0,
        b: 0.0,
    };
    for rgba in image.pixels.as_chunks_mut::<4>().0 {
        if rgba[3] < 32 {
            *rgba = [255, 255, 255, 255];
            continue;
        }
        let lab = Srgb::new(rgba[0], rgba[1], rgba[2]).to_lab();
        if delta_e_2000(&lab, &white) < 7.0 {
            *rgba = [255, 255, 255, 255];
        }
    }
}

/// Remove isolated single studs: a stud whose 8 neighbors are all of
/// different colors (with `threshold` allowing near-misses) is likely
/// anti-aliasing residue; replace it with the majority color among its
/// neighbors. One pass, top-left to bottom-right — the sharp pipeline's
/// majority vote already does most of this work, so only stragglers remain.
fn despeckle(grid: &mut [usize], width: usize, height: usize) {
    // A few passes: absorbing one straggler can make its neighbor
    // isolated, so speckle collapses outward.
    for _ in 0..3 {
        let before = grid.to_vec();
        let at = |x: usize, y: usize| before[y * width + x];
        for gy in 0..height {
            for gx in 0..width {
                let index = at(gx, gy);
                let (mut same, mut different) = (0_usize, 0_usize);
                let mut neighbor_counts: std::collections::HashMap<usize, usize> =
                    std::collections::HashMap::new();
                for dy in -1_i32..=1 {
                    for dx in -1_i32..=1 {
                        if (dx == 0 && dy == 0)
                            || (gx == 0 && dx < 0)
                            || (gy == 0 && dy < 0)
                            || gx as i32 + dx >= width as i32
                            || gy as i32 + dy >= height as i32
                        {
                            continue;
                        }
                        let neighbor = at((gx as i32 + dx) as usize, (gy as i32 + dy) as usize);
                        if neighbor == index {
                            same += 1;
                        } else {
                            different += 1;
                            *neighbor_counts.entry(neighbor).or_default() += 1;
                        }
                    }
                }
                // A stud with at most one same-color neighbor is speckle
                // when the neighborhood has a clear majority (≥ 3) of one
                // other color: anti-aliasing residue, not detail.
                let Some((&majority, &majority_count)) =
                    neighbor_counts.iter().max_by_key(|(_, count)| *count)
                else {
                    continue;
                };
                if same <= 1 && different >= 4 && majority_count >= 3 {
                    grid[gy * width + gx] = majority;
                }
            }
        }
    }
}

/// Nearest palette color by CIEDE2000.
fn nearest(lab: &Lab, palette_lab: &[Lab]) -> usize {
    let (mut best, mut best_d) = (0, f64::INFINITY);
    for (i, plab) in palette_lab.iter().enumerate() {
        let d = delta_e_2000(lab, plab);
        if d < best_d {
            best_d = d;
            best = i;
        }
    }
    best
}

/// Sharp mode: quantize every hi-res pixel (with dithering if requested),
/// then take the majority vote per stud cell. Anti-aliased edges become
/// clean palette-colored borders instead of speckle.
fn sharp_map(labs: &[Lab], palette_lab: &[Lab], options: &Options) -> Vec<usize> {
    let hi_w = options.width * OVERSAMPLE;
    let (grid_w, grid_h) = (options.width, options.height);

    let quantized: Vec<usize> = if options.dither {
        dither_floyd_steinberg(labs, palette_lab, hi_w, options.height * OVERSAMPLE)
    } else {
        labs.iter().map(|lab| nearest(lab, palette_lab)).collect()
    };

    let mut grid = vec![0_usize; grid_w * grid_h];
    for gy in 0..grid_h {
        for gx in 0..grid_w {
            let mut counts = vec![0_usize; palette_lab.len()];
            for y in gy * OVERSAMPLE..(gy + 1) * OVERSAMPLE {
                for x in gx * OVERSAMPLE..(gx + 1) * OVERSAMPLE {
                    counts[quantized[y * hi_w + x]] += 1;
                }
            }
            grid[gy * grid_w + gx] = counts
                .iter()
                .enumerate()
                .max_by_key(|(_, count)| **count)
                .map_or(0, |(index, _)| index);
        }
    }
    grid
}

/// Smooth mode: average each stud cell in Lab space, then quantize the
/// average. With dithering on, the cell average is quantized per sub-pixel
/// position of a synthetic 1×1 dither pass — in practice we quantize the
/// cell average and let dithering act on a 1×1 grid (i.e. no dithering),
/// so dithering is a sharp-mode-only feature there.
fn smooth_map(labs: &[Lab], palette_lab: &[Lab], options: &Options) -> Vec<usize> {
    let hi_w = options.width * OVERSAMPLE;
    let (grid_w, grid_h) = (options.width, options.height);
    let mut grid = vec![0_usize; grid_w * grid_h];
    for gy in 0..grid_h {
        for gx in 0..grid_w {
            let (mut sum_l, mut sum_a, mut sum_b) = (0.0, 0.0, 0.0);
            for y in gy * OVERSAMPLE..(gy + 1) * OVERSAMPLE {
                for x in gx * OVERSAMPLE..(gx + 1) * OVERSAMPLE {
                    let lab = &labs[y * hi_w + x];
                    sum_l += lab.l;
                    sum_a += lab.a;
                    sum_b += lab.b;
                }
            }
            let n = (OVERSAMPLE * OVERSAMPLE) as f64;
            let avg = Lab {
                l: sum_l / n,
                a: sum_a / n,
                b: sum_b / n,
            };
            grid[gy * grid_w + gx] = nearest(&avg, palette_lab);
        }
    }
    grid
}

/// Floyd–Steinberg error diffusion over Lab values, serpentine scan (the
/// serpentine variant avoids the "worm" artifacts plain FS produces on
/// tiny palettes). Returns palette indices per hi-res pixel.
fn dither_floyd_steinberg(
    labs: &[Lab],
    palette_lab: &[Lab],
    width: usize,
    height: usize,
) -> Vec<usize> {
    // Working copy of the Lab values; errors accumulate into these.
    let mut work: Vec<(f64, f64, f64)> = labs.iter().map(|lab| (lab.l, lab.a, lab.b)).collect();
    let mut out = vec![0_usize; width * height];
    for row in 0..height {
        // Serpentine: alternate scan direction per row.
        let left_to_right = row % 2 == 0;
        for step in 0..width {
            let x = if left_to_right {
                step
            } else {
                width - 1 - step
            };
            let i = row * width + x;
            let (pix_l, pix_a, pix_b) = work[i];
            let index = nearest(
                &Lab {
                    l: pix_l,
                    a: pix_a,
                    b: pix_b,
                },
                palette_lab,
            );
            out[i] = index;
            let chosen = palette_lab[index];
            // Quantization error, to be pushed onto the neighbors.
            let (err_l, err_a, err_b) = (pix_l - chosen.l, pix_a - chosen.a, pix_b - chosen.b);
            let sign = if left_to_right { 1.0 } else { -1.0 };
            // Neighbor offsets in scan direction: (dx, dy, weight).
            let neighbors: [(i64, i64, f64); 4] = [
                (sign as i64, 0, 7.0 / 16.0),
                (-sign as i64, 1, 3.0 / 16.0),
                (0, 1, 5.0 / 16.0),
                (sign as i64, 1, 1.0 / 16.0),
            ];
            for (dx, dy, weight) in neighbors {
                let nx = x as i64 + dx;
                let ny = row as i64 + dy;
                if nx < 0 || nx >= width as i64 || ny >= height as i64 {
                    continue;
                }
                let j = (ny as usize) * width + nx as usize;
                let slot = &mut work[j];
                slot.0 += err_l * weight;
                slot.1 += err_a * weight;
                slot.2 += err_b * weight;
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::palette::MOSAIC_MAKER;

    fn options(width: usize, height: usize) -> Options {
        Options {
            width,
            height,
            ..Options::default()
        }
    }

    fn solid(w: usize, h: usize, rgba: [u8; 4]) -> Raster {
        Raster {
            width: w,
            height: h,
            pixels: rgba.iter().copied().cycle().take(w * h * 4).collect(),
        }
    }

    #[test]
    fn solid_image_maps_to_nearest_single_color() {
        // A 64×64 mid-gray image: L* ≈ 53.6 sits between Light Bluish
        // Gray (≈66) and Dark Bluish Gray (≈45), nearer the latter.
        let image = solid(64, 64, [128, 128, 128, 255]);
        let mosaic = convert(&image, MOSAIC_MAKER, &options(8, 8));
        assert!(
            mosaic.grid.iter().all(|&i| i == 2),
            "grid: {:?}",
            mosaic.grid
        );
    }

    #[test]
    fn white_background_stays_white() {
        let mut image = solid(64, 64, [253, 253, 251, 255]);
        // A dark blob in the middle.
        for y in 22..42 {
            for x in 22..42 {
                image.put(x, y, [20, 20, 20, 255]);
            }
        }
        let mosaic = convert(&image, MOSAIC_MAKER, &options(8, 8));
        // Corners (background) are White (index 0); center is dark.
        assert_eq!(mosaic.grid[0], 0);
        assert_eq!(mosaic.grid[8 * 8 - 1], 0);
        let center = mosaic.grid[3 * 8 + 3] + mosaic.grid[4 * 8 + 4];
        assert!(center >= 6, "center should be dark gray/black: {center}");
    }

    #[test]
    fn crop_to_aspect_never_stretches() {
        // 128×64 image into a square grid: a centered 64×64 crop.
        let image = solid(128, 64, [0, 0, 0, 255]);
        let mosaic = convert(&image, MOSAIC_MAKER, &options(16, 16));
        assert_eq!(mosaic.width, 16);
        assert_eq!(mosaic.grid.len(), 16 * 16);
    }

    #[test]
    fn excluded_palette_still_converts() {
        // Excluding everything falls back to the first color.
        let image = solid(16, 16, [90, 90, 90, 255]);
        let opts = Options {
            excluded: (0..MOSAIC_MAKER.len()).collect(),
            ..options(4, 4)
        };
        let mosaic = convert(&image, MOSAIC_MAKER, &opts);
        assert_eq!(mosaic.palette.len(), 1);
        assert!(mosaic.grid.iter().all(|&i| i == 0));
    }

    #[test]
    fn dither_spreads_error_on_gradient() {
        // A horizontal black→white gradient; dithering must mix indices
        // rather than produce a single flat band per row.
        let mut image = Raster {
            width: 128,
            height: 128,
            pixels: Vec::new(),
        };
        for _y in 0..128 {
            for x in 0..128 {
                let v = (x * 255 / 127).min(255) as u8;
                image.pixels.extend_from_slice(&[v, v, v, 255]);
            }
        }
        let opts = Options {
            dither: true,
            ..options(16, 16)
        };
        let mosaic = convert(&image, MOSAIC_MAKER, &opts);
        let distinct: std::collections::HashSet<usize> = mosaic.grid.iter().copied().collect();
        assert!(
            distinct.len() >= 3,
            "gradient should use several colors: {distinct:?}"
        );
    }

    #[test]
    fn saturation_boost_separates_pastel_from_white() {
        // A very pale pink: with the default saturation boost it should
        // land on White anyway (it is near-white), but a strong pink must
        // not map to White, and the boost must not change that direction.
        let image = solid(32, 32, [244, 200, 208, 255]);
        let boosted = Options {
            saturation: 2.0,
            white_background: false,
            ..options(8, 8)
        };
        let mosaic = convert(&image, crate::palette::EXTENDED, &boosted);
        let flat: std::collections::HashSet<usize> = mosaic.grid.iter().copied().collect();
        assert_eq!(flat.len(), 1, "solid input gives solid output");
        let pink_index = mosaic.grid[0];
        assert_ne!(
            mosaic.palette[pink_index].name, "White",
            "boosted pink must not collapse into white"
        );
    }

    #[test]
    fn transparent_pixels_composite_over_white() {
        let image = solid(32, 32, [0, 0, 0, 0]);
        let mosaic = convert(&image, MOSAIC_MAKER, &options(4, 4));
        assert!(mosaic
            .grid
            .iter()
            .all(|&i| mosaic.palette[i].name == "White"));
    }
}

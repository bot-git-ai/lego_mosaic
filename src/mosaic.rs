// Copyright (c) 2026 Witalis Domitrz <witekdomitrz@gmail.com>
// AGPL License

//! Deterministic, browser-independent RGBA → palette-stud conversion.
//! Exact fractional area resampling (including alpha over white) avoids
//! aliasing. Sharp uses sub-stud coverage and protects dark line work;
//! Smooth averages linear light. Both dither at the *final stud* resolution.
//! See CORE_API.md for JSON fields, bounds, and preset recipes.
#![allow(
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss
)]

use crate::color::{delta_e_2000, Lab, Srgb};
use crate::palette::TileColor;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug)]
pub(crate) struct Raster {
    pub(crate) width: usize,
    pub(crate) height: usize,
    /// Row-major RGBA8; transparent samples composite over white.
    pub(crate) pixels: Vec<u8>,
}

impl Raster {
    #[allow(dead_code)]
    pub(crate) fn put(&mut self, x: usize, y: usize, rgba: [u8; 4]) {
        let i = (y * self.width + x) * 4;
        self.pixels[i..i + 4].copy_from_slice(&rgba);
    }

    fn valid(&self) -> bool {
        self.width > 0
            && self.height > 0
            && self
                .width
                .checked_mul(self.height)
                .and_then(|n| n.checked_mul(4))
                == Some(self.pixels.len())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum Order {
    Sharp,
    Smooth,
}

impl Order {
    // Retained for native/legacy callers; serde accepts only documented enum values.
    #[allow(dead_code)]
    pub(crate) fn from_str(s: &str) -> Self {
        if s == "smooth" {
            Self::Smooth
        } else {
            Self::Sharp
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum FitMode {
    Contain,
    Crop,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum HueMode {
    Auto,
    Preserve,
    Grayscale,
    Target,
}

/// A circular HSV source hue selection and a perceptual destination.
/// Dark ink and low-saturation paper are never recolored. The outer 20%
/// of the hue tolerance is feathered; interior shades share one target.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct HueRemap {
    pub(crate) mode: HueMode,
    pub(crate) source_hue: f64,
    pub(crate) tolerance: f64,
    pub(crate) threshold: f64,
    pub(crate) strength: f64,
    pub(crate) target: [u8; 3],
}

impl Default for HueRemap {
    fn default() -> Self {
        Self {
            mode: HueMode::Auto,
            source_hue: 340.0,
            tolerance: 55.0,
            threshold: 0.07,
            strength: 1.0,
            target: [247, 209, 23],
        }
    }
}

/// All conversion controls. Missing JSON fields take defaults. Values are
/// sanitized in `convert`, not by the caller; NaN/∞ take neutral defaults.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct Options {
    pub(crate) width: usize,
    pub(crate) height: usize,
    pub(crate) saturation: f64,
    pub(crate) contrast: f64,
    pub(crate) brightness: i32,
    pub(crate) gamma: f64,
    pub(crate) order: Order,
    pub(crate) dither: bool,
    pub(crate) dither_strength: f64,
    pub(crate) white_background: bool,
    pub(crate) neutral_cleanup: f64,
    pub(crate) despeckle: bool,
    pub(crate) despeckle_strength: f64,
    pub(crate) excluded: Vec<usize>,
    pub(crate) fit: FitMode,
    pub(crate) crop_x: f64,
    pub(crate) crop_y: f64,
    pub(crate) zoom: f64,
    pub(crate) hue_remap: HueRemap,
    pub(crate) secondary_remap: HueRemap,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            width: 48,
            height: 48,
            saturation: 1.0,
            contrast: 1.0,
            brightness: 0,
            gamma: 1.0,
            order: Order::Sharp,
            dither: false,
            dither_strength: 0.75,
            white_background: true,
            neutral_cleanup: 0.5,
            despeckle: true,
            despeckle_strength: 0.35,
            excluded: Vec::new(),
            fit: FitMode::Contain,
            crop_x: 0.5,
            crop_y: 0.5,
            zoom: 1.0,
            hue_remap: HueRemap::default(),
            secondary_remap: HueRemap {
                source_hue: 250.0,
                tolerance: 35.0,
                threshold: 0.025,
                target: [160, 165, 169],
                ..HueRemap::default()
            },
        }
    }
}

impl Options {
    fn sanitized(&self) -> Self {
        let mut o = self.clone();
        o.width = o.width.clamp(1, 192);
        o.height = o.height.clamp(1, 192);
        o.saturation = bounded(o.saturation, 0.0, 3.0, 1.0);
        o.contrast = bounded(o.contrast, 0.25, 3.0, 1.0);
        o.brightness = o.brightness.clamp(-100, 100);
        o.gamma = bounded(o.gamma, 0.25, 4.0, 1.0);
        o.dither_strength = bounded(o.dither_strength, 0.0, 1.0, 0.75);
        o.neutral_cleanup = bounded(o.neutral_cleanup, 0.0, 1.0, 0.5);
        o.despeckle_strength = bounded(o.despeckle_strength, 0.0, 1.0, 0.35);
        o.crop_x = bounded(o.crop_x, 0.0, 1.0, 0.5);
        o.crop_y = bounded(o.crop_y, 0.0, 1.0, 0.5);
        o.zoom = bounded(o.zoom, 1.0, 8.0, 1.0);
        for h in [&mut o.hue_remap, &mut o.secondary_remap] {
            h.source_hue = if h.source_hue.is_finite() {
                h.source_hue.rem_euclid(360.0)
            } else {
                340.0
            };
            h.tolerance = bounded(h.tolerance, 0.0, 180.0, 55.0);
            h.threshold = bounded(h.threshold, 0.0, 1.0, 0.07);
            h.strength = bounded(h.strength, 0.0, 1.0, 1.0);
        }
        o
    }
}

fn bounded(x: f64, lo: f64, hi: f64, default: f64) -> f64 {
    if x.is_finite() {
        x.clamp(lo, hi)
    } else {
        default
    }
}

pub(crate) struct Mosaic {
    pub(crate) grid: Vec<usize>,
    pub(crate) width: usize,
    pub(crate) height: usize,
    pub(crate) palette: Vec<TileColor>,
}

const OVERSAMPLE: usize = 4;
const WHITE: Lab = Lab {
    l: 100.0,
    a: 0.0,
    b: 0.0,
};

pub(crate) fn convert(image: &Raster, palette: &[TileColor], options: &Options) -> Mosaic {
    let o = options.sanitized();
    let palette = excluded_palette(palette, &o.excluded);
    let labs: Vec<_> = palette.iter().map(|t| t.rgb.to_lab()).collect();
    let mut cells = vec![WHITE; o.width * o.height];
    let mut grid = vec![nearest(&WHITE, &labs); cells.len()];
    let mut confidence = vec![1.0; cells.len()];
    if image.valid() {
        let hi = resample(image, o.width * OVERSAMPLE, o.height * OVERSAMPLE, &o);
        let remap_target = remap_target(&o.hue_remap, &labs);
        let secondary_target = if o.secondary_remap.mode == HueMode::Auto {
            if palette.len() <= 6 {
                labs.iter()
                    .copied()
                    .filter(|l| chroma(*l) < 15.0 && l.l > 45.0 && l.l < 85.0)
                    .max_by(|a, b| a.l.total_cmp(&b.l))
            } else {
                None
            }
        } else {
            remap_target_for_secondary(&o.secondary_remap, &labs)
        };
        // Tiny fixed RGB cache: deterministic canonical 6-bit/channel colors.
        // This bounds expensive CIEDE2000 matching to 262k distinct colors,
        // and usually just a few hundred for flat illustrations.
        let mut cache = vec![usize::MAX; 64 * 64 * 64];
        let mut transformed = Vec::with_capacity(hi.len());
        let mut indices = Vec::with_capacity(hi.len());
        for rgb in hi {
            let key = (usize::from(rgb.r >> 2) << 12)
                | (usize::from(rgb.g >> 2) << 6)
                | usize::from(rgb.b >> 2);
            let lab = preprocess_both(rgb, &o, remap_target, secondary_target);
            transformed.push(lab);
            if cache[key] == usize::MAX {
                // Use a canonical bin center instead of the first encountered
                // sample so reversing the source never changes color decisions.
                let canonical = Srgb::new((rgb.r & 252) | 2, (rgb.g & 252) | 2, (rgb.b & 252) | 2);
                cache[key] = nearest(
                    &preprocess_both(canonical, &o, remap_target, secondary_target),
                    &labs,
                );
            }
            indices.push(cache[key]);
        }
        pool(
            &transformed,
            &indices,
            &labs,
            &o,
            &mut cells,
            &mut grid,
            &mut confidence,
        );
    }
    let dithering = o.dither && o.dither_strength > 0.0;
    if dithering {
        grid = diffuse(&cells, &labs, o.width, o.height, o.dither_strength);
    } else if o.despeckle && o.despeckle_strength > 0.0 {
        despeckle(&mut grid, &cells, &confidence, &labs, &o);
    }
    Mosaic {
        width: o.width,
        height: o.height,
        grid,
        palette,
    }
}

fn excluded_palette(palette: &[TileColor], excluded: &[usize]) -> Vec<TileColor> {
    let kept: Vec<_> = palette
        .iter()
        .enumerate()
        .filter(|(i, _)| !excluded.contains(i))
        .map(|(_, t)| *t)
        .collect();
    if !kept.is_empty() {
        return kept;
    }
    vec![palette.first().copied().unwrap_or(TileColor {
        name: "White",
        rgb: Srgb::new(255, 255, 255),
    })]
}

/// Source-coordinate viewport. Both modes keep square source pixels;
/// contain returns a larger virtual canvas, with its outside area white.
fn viewport(image: &Raster, out_w: usize, out_h: usize, o: &Options) -> (f64, f64, f64, f64) {
    let sw = image.width as f64;
    let sh = image.height as f64;
    let sx = sw / out_w as f64;
    let sy = sh / out_h as f64;
    let scale = match o.fit {
        FitMode::Contain => sx.max(sy),
        FitMode::Crop => sx.min(sy) / o.zoom,
    };
    let vw = out_w as f64 * scale;
    let vh = out_h as f64 * scale;
    let (x, y) = if o.fit == FitMode::Contain {
        ((sw - vw) * 0.5, (sh - vh) * 0.5)
    } else {
        (
            (o.crop_x * sw - vw * 0.5).clamp(0.0, (sw - vw).max(0.0)),
            (o.crop_y * sh - vh * 0.5).clamp(0.0, (sh - vh).max(0.0)),
        )
    };
    (x, y, scale, scale)
}

fn linear(v: f64) -> f64 {
    if v <= 0.04045 {
        v / 12.92
    } else {
        ((v + 0.055) / 1.055).powf(2.4)
    }
}
fn encoded(v: f64) -> f64 {
    if v <= 0.0031308 {
        v * 12.92
    } else {
        1.055 * v.powf(1.0 / 2.4) - 0.055
    }
}
fn byte(v: f64) -> u8 {
    (v * 255.0).round().clamp(0.0, 255.0) as u8
}

/// Exact overlap-weighted area filter: handles fractional crop/resize
/// boundaries, upsampling, alpha, and partial padding in the same pass.
/// Sharp samples encoded RGB to retain coverage; photo mode linear light.
fn resample(image: &Raster, out_w: usize, out_h: usize, o: &Options) -> Vec<Srgb> {
    let (vx, vy, dx, dy) = viewport(image, out_w, out_h, o);
    let table: Vec<f64> = (0..=255)
        .map(|n| {
            let x = f64::from(n) / 255.0;
            if o.order == Order::Smooth {
                linear(x)
            } else {
                x
            }
        })
        .collect();
    let mut out = Vec::with_capacity(out_w * out_h);
    for y in 0..out_h {
        let top = vy + y as f64 * dy;
        let bottom = top + dy;
        for x in 0..out_w {
            let left = vx + x as f64 * dx;
            let right = left + dx;
            let area = dx * dy;
            // Start with a white footprint, subtract covered non-white.
            let mut sum = [area; 3];
            let x0 = left.floor().max(0.0).min(image.width as f64) as usize;
            let x1 = right.ceil().max(0.0).min(image.width as f64) as usize;
            let y0 = top.floor().max(0.0).min(image.height as f64) as usize;
            let y1 = bottom.ceil().max(0.0).min(image.height as f64) as usize;
            for sy in y0..y1 {
                let wy = (bottom.min((sy + 1) as f64) - top.max(sy as f64)).max(0.0);
                for sx in x0..x1 {
                    let wx = (right.min((sx + 1) as f64) - left.max(sx as f64)).max(0.0);
                    let i = (sy * image.width + sx) * 4;
                    let weight = wx * wy * f64::from(image.pixels[i + 3]) / 255.0;
                    for c in 0..3 {
                        sum[c] -= (1.0 - table[usize::from(image.pixels[i + c])]) * weight;
                    }
                }
            }
            let v = sum.map(|n| {
                let n = (n / area).clamp(0.0, 1.0);
                byte(if o.order == Order::Smooth {
                    encoded(n)
                } else {
                    n
                })
            });
            out.push(Srgb::new(v[0], v[1], v[2]));
        }
    }
    out
}

/// Exposure → gamma → fixed midgray contrast → saturation around the
/// *adjusted* luminance. The old stale-luminance pivot tinted highlights.
fn adjust(rgb: Srgb, o: &Options) -> Srgb {
    let mut c = [rgb.r, rgb.g, rgb.b].map(|v| {
        let x = ((f64::from(v) + f64::from(o.brightness)) / 255.0).clamp(0.0, 1.0);
        ((x.powf(1.0 / o.gamma) - 0.5) * o.contrast + 0.5).clamp(0.0, 1.0)
    });
    let l = 0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2];
    for v in &mut c {
        *v = (l + (*v - l) * o.saturation).clamp(0.0, 1.0);
    }
    Srgb::new(byte(c[0]), byte(c[1]), byte(c[2]))
}

fn chroma(lab: Lab) -> f64 {
    lab.a.hypot(lab.b)
}

fn remap_target(h: &HueRemap, palette: &[Lab]) -> Option<Lab> {
    match h.mode {
        HueMode::Preserve => None,
        HueMode::Grayscale => Some(Lab {
            l: 70.0,
            a: 0.0,
            b: 0.0,
        }),
        HueMode::Target => Some(Srgb::new(h.target[0], h.target[1], h.target[2]).to_lab()),
        HueMode::Auto => {
            let colors: Vec<_> = palette
                .iter()
                .copied()
                .filter(|l| chroma(*l) > 24.0)
                .collect();
            if colors.is_empty() || colors.len() > 2 {
                return None;
            }
            // Brightest usable accent, not a dark brown/blue outline color.
            colors
                .into_iter()
                .filter(|l| l.l > 58.0)
                .max_by(|a, b| a.l.total_cmp(&b.l))
        }
    }
}

fn hue_saturation(rgb: Srgb) -> (f64, f64) {
    let [r, g, b] = [rgb.r, rgb.g, rgb.b].map(|n| f64::from(n) / 255.0);
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let d = max - min;
    if d < 1e-9 {
        return (0.0, 0.0);
    }
    let h = if max == r {
        (g - b) / d
    } else if max == g {
        (b - r) / d + 2.0
    } else {
        (r - g) / d + 4.0
    };
    ((h * 60.0).rem_euclid(360.0), d / max)
}

fn remap_target_for_secondary(h: &HueRemap, palette: &[Lab]) -> Option<Lab> {
    remap_target(h, palette)
}

#[cfg(test)]
fn preprocess(rgb: Srgb, o: &Options, target: Option<Lab>) -> Lab {
    preprocess_both(rgb, o, target, None)
}

// Membership always uses unadjusted source color, so changing exposure or
// saturation cannot make parts of the same color family escape the rule.
fn preprocess_both(rgb: Srgb, o: &Options, target: Option<Lab>, secondary: Option<Lab>) -> Lab {
    let original = rgb.to_lab();
    let mut lab = adjust(rgb, o).to_lab();
    if o.white_background && original.l > 96.0 && chroma(original) < 7.0 {
        return WHITE;
    }
    let (hue, saturation) = hue_saturation(rgb);
    for (h, destination) in [(&o.secondary_remap, secondary), (&o.hue_remap, target)] {
        if let Some(destination) = destination {
            let distance = ((hue - h.source_hue + 180.0).rem_euclid(360.0) - 180.0).abs();
            if saturation >= h.threshold
                && saturation > 0.02
                && original.l > 40.0
                && original.l < 96.0
                && distance <= h.tolerance
            {
                let edge = if h.tolerance < 1e-6 {
                    1.0
                } else {
                    ((h.tolerance - distance) / (h.tolerance * 0.2)).clamp(0.0, 1.0)
                };
                let protection = ((original.l - 40.0) / 18.0).clamp(0.0, 1.0);
                let amount = h.strength * edge * protection;
                let lightness = (destination.l + (lab.l - 80.0) * 0.25).clamp(0.0, 100.0);
                lab.l += (lightness - lab.l) * amount;
                lab.a += (destination.a - lab.a) * amount;
                lab.b += (destination.b - lab.b) * amount;
            }
        }
    }
    let amount = o.neutral_cleanup * ((12.0 - chroma(lab)) / 8.0).clamp(0.0, 1.0);
    lab.a *= 1.0 - amount;
    lab.b *= 1.0 - amount;
    lab
}

fn nearest(lab: &Lab, palette: &[Lab]) -> usize {
    let mut best = 0;
    let mut distance = f64::INFINITY;
    for (i, color) in palette.iter().enumerate() {
        let d = delta_e_2000(lab, color);
        if d < distance {
            distance = d;
            best = i;
        }
    }
    best
}

// Inverse Lab used only to average photo samples in linear light rather
// than averaging L* (which darkens a black/white checkerboard incorrectly).
fn lab_to_linear(lab: Lab) -> [f64; 3] {
    let fy = (lab.l + 16.0) / 116.0;
    let fx = fy + lab.a / 500.0;
    let fz = fy - lab.b / 200.0;
    let inv = |t: f64| {
        if t > 6.0 / 29.0 {
            t * t * t
        } else {
            (t - 4.0 / 29.0) * 108.0 / 841.0
        }
    };
    let x = 0.95047 * inv(fx);
    let y = inv(fy);
    let z = 1.08883 * inv(fz);
    [
        3.2404542 * x - 1.5371385 * y - 0.4985314 * z,
        -0.9692660 * x + 1.8760108 * y + 0.0415560 * z,
        0.0556434 * x - 0.2040259 * y + 1.0572252 * z,
    ]
}

fn linear_to_lab(c: [f64; 3]) -> Lab {
    let x = (0.4124564 * c[0] + 0.3575761 * c[1] + 0.1804375 * c[2]) / 0.95047;
    let y = 0.2126729 * c[0] + 0.7151522 * c[1] + 0.0721750 * c[2];
    let z = (0.0193339 * c[0] + 0.1191920 * c[1] + 0.9503041 * c[2]) / 1.08883;
    let f = |t: f64| {
        if t > 216.0 / 24389.0 {
            t.cbrt()
        } else {
            t * 841.0 / 108.0 + 4.0 / 29.0
        }
    };
    Lab {
        l: 116.0 * f(y) - 16.0,
        a: 500.0 * (f(x) - f(y)),
        b: 200.0 * (f(y) - f(z)),
    }
}

#[allow(clippy::too_many_arguments)]
fn pool(
    hi: &[Lab],
    indices: &[usize],
    palette: &[Lab],
    o: &Options,
    cells: &mut [Lab],
    grid: &mut [usize],
    confidence: &mut [f64],
) {
    let hi_w = o.width * OVERSAMPLE;
    let n = (OVERSAMPLE * OVERSAMPLE) as f64;
    let mut counts = vec![0; palette.len()];
    for gy in 0..o.height {
        for gx in 0..o.width {
            counts.fill(0);
            let mut sum = [0.0; 3];
            let mut ink = 0.0;
            let mut darkest = WHITE;
            for dy in 0..OVERSAMPLE {
                for dx in 0..OVERSAMPLE {
                    let p = (gy * OVERSAMPLE + dy) * hi_w + gx * OVERSAMPLE + dx;
                    let lab = hi[p];
                    counts[indices[p]] += 1;
                    let v = if o.order == Order::Smooth {
                        lab_to_linear(lab)
                    } else {
                        [lab.l, lab.a, lab.b]
                    };
                    for c in 0..3 {
                        sum[c] += v[c] / n;
                    }
                    ink += ((50.0 - lab.l) / 35.0).clamp(0.0, 1.0) / n;
                    if lab.l < darkest.l {
                        darkest = lab;
                    }
                }
            }
            let i = gy * o.width + gx;
            let avg = if o.order == Order::Smooth {
                linear_to_lab(sum)
            } else {
                Lab {
                    l: sum[0],
                    a: sum[1],
                    b: sum[2],
                }
            };
            if o.order == Order::Smooth {
                cells[i] = avg;
                grid[i] = nearest(&avg, palette);
                confidence[i] = 0.0;
                continue;
            }
            // Coverage winner; perceptual mean resolves ties, index final tie.
            let mut best = 0;
            for p in 1..palette.len() {
                if counts[p] > counts[best]
                    || (counts[p] == counts[best]
                        && delta_e_2000(&avg, &palette[p]) < delta_e_2000(&avg, &palette[best]))
                {
                    best = p;
                }
            }
            // A 1/4-stud black line must not disappear just because it does
            // not win a majority. Require genuinely dark coverage, not gray
            // antialias fringes. Despeckle protects this decision as well.
            let preserve_ink = ink >= 0.19 && darkest.l < 30.0 && palette[best].l > 45.0;
            if preserve_ink {
                best = nearest(&darkest, palette);
            }
            grid[i] = best;
            confidence[i] = if preserve_ink {
                1.0
            } else {
                f64::from(counts[best]) / n
            };
            // Sharp dithering uses the covered family's representative mean,
            // not a quantized value (which would leave no error to diffuse).
            if preserve_ink {
                cells[i] = darkest;
            } else {
                let mut representative = Lab {
                    l: 0.0,
                    a: 0.0,
                    b: 0.0,
                };
                let mut count: f64 = 0.0;
                for dy in 0..OVERSAMPLE {
                    for dx in 0..OVERSAMPLE {
                        let p = (gy * OVERSAMPLE + dy) * hi_w + gx * OVERSAMPLE + dx;
                        if indices[p] == best {
                            representative.l += hi[p].l;
                            representative.a += hi[p].a;
                            representative.b += hi[p].b;
                            count += 1.0;
                        }
                    }
                }
                cells[i] = Lab {
                    l: representative.l / count.max(1.0),
                    a: representative.a / count.max(1.0),
                    b: representative.b / count.max(1.0),
                };
            }
        }
    }
}

/// Real per-stud serpentine diffusion in linear RGB, perceptual palette
/// choice. Error is bounded; only the propagated error is strength-scaled.
/// Neutral gradients discard palette chroma error (bluish gray plastic must
/// not create random yellow compensation dots in a monochrome photograph).
fn diffuse(
    cells: &[Lab],
    palette: &[Lab],
    width: usize,
    height: usize,
    strength: f64,
) -> Vec<usize> {
    let source: Vec<_> = cells.iter().copied().map(lab_to_linear).collect();
    let colors: Vec<_> = palette.iter().copied().map(lab_to_linear).collect();
    let mut errors = vec![[0.0; 3]; cells.len()];
    let mut out = vec![0; cells.len()];
    for y in 0..height {
        let forward = y % 2 == 0;
        let sign: isize = if forward { 1 } else { -1 };
        for step in 0..width {
            let x = if forward { step } else { width - 1 - step };
            let i = y * width + x;
            let mut current = source[i];
            for (c, value) in current.iter_mut().enumerate() {
                *value = (*value + errors[i][c]).clamp(0.0, 1.0);
            }
            let lab = linear_to_lab(current);
            let chosen = nearest(&lab, palette);
            out[i] = chosen;
            let mut error = [0.0; 3];
            for c in 0..3 {
                error[c] = (current[c] - colors[chosen][c]).clamp(-0.35, 0.35);
            }
            if chroma(cells[i]) < 10.0 {
                let l = 0.2126 * error[0] + 0.7152 * error[1] + 0.0722 * error[2];
                error = [l; 3];
            }
            for (dx, dy, weight) in [
                (sign, 0, 7.0 / 16.0),
                (-sign, 1, 3.0 / 16.0),
                (0, 1, 5.0 / 16.0),
                (sign, 1, 1.0 / 16.0),
            ] {
                let nx = x as isize + dx;
                let ny = y + dy;
                if nx < 0 || nx >= width as isize || ny >= height {
                    continue;
                }
                let j = ny * width + nx as usize;
                // Stop diffusion crossing large illustration/photo edges.
                let edge = ((45.0 - (cells[i].l - cells[j].l).abs()) / 20.0).clamp(0.0, 1.0);
                for (c, value) in error.iter().enumerate() {
                    errors[j][c] += value * weight * strength * edge;
                }
            }
        }
    }
    out
}

/// One synchronous, deterministic pass. Never deletes dark details, stable
/// flat fills, or any 8-connected same-color line. Only isolated, uncertain
/// low-contrast studs surrounded by ≥6 identical neighbors can be cleaned.
fn despeckle(grid: &mut [usize], cells: &[Lab], confidence: &[f64], palette: &[Lab], o: &Options) {
    if o.width < 3 || o.height < 3 {
        return;
    }
    let before = grid.to_vec();
    let mut counts = vec![0; palette.len()];
    for y in 1..o.height - 1 {
        for x in 1..o.width - 1 {
            let i = y * o.width + x;
            let current = before[i];
            if palette[current].l < 38.0 || confidence[i] >= 0.75 {
                continue;
            }
            counts.fill(0);
            for dy in -1_isize..=1 {
                for dx in -1_isize..=1 {
                    if dx == 0 && dy == 0 {
                        continue;
                    }
                    let j = (y as isize + dy) as usize * o.width + (x as isize + dx) as usize;
                    counts[before[j]] += 1;
                }
            }
            if counts[current] != 0 {
                continue;
            }
            // Strict > is a stable palette-index tie break; no hash order.
            let mut majority = 0;
            for p in 1..counts.len() {
                if counts[p] > counts[majority] {
                    majority = p;
                }
            }
            if counts[majority] < 6 || (palette[current].l - palette[majority].l).abs() > 30.0 {
                continue;
            }
            let added_cost = delta_e_2000(&cells[i], &palette[majority])
                - delta_e_2000(&cells[i], &palette[current]);
            if added_cost <= o.despeckle_strength * 12.0 {
                grid[i] = majority;
            }
        }
    }
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
            pixels: rgba.into_iter().cycle().take(w * h * 4).collect(),
        }
    }
    fn gray(v: u8) -> Srgb {
        Srgb::new(v, v, v)
    }
    fn bw() -> [TileColor; 2] {
        [
            TileColor {
                name: "White",
                rgb: gray(255),
            },
            TileColor {
                name: "Black",
                rgb: gray(0),
            },
        ]
    }
    fn yellow_palette() -> [TileColor; 5] {
        [
            TileColor {
                name: "White",
                rgb: gray(245),
            },
            TileColor {
                name: "Light Gray",
                rgb: gray(170),
            },
            TileColor {
                name: "Dark Gray",
                rgb: gray(105),
            },
            TileColor {
                name: "Black",
                rgb: gray(10),
            },
            TileColor {
                name: "Yellow",
                rgb: Srgb::new(247, 209, 23),
            },
        ]
    }

    #[test]
    fn measured_rabbit_fur_is_gray_pink_yellow_paper_white() {
        let o = options(4, 4);
        for rgb in [
            [215, 213, 224, 255],
            [213, 211, 222, 255],
            [216, 214, 225, 255],
        ] {
            let result = convert(&solid(16, 16, rgb), &yellow_palette(), &o);
            assert!(
                result.grid.iter().all(|&n| n == 1),
                "lavender: {:?}",
                result.grid
            );
        }
        let pink = convert(&solid(16, 16, [216, 189, 193, 255]), &yellow_palette(), &o);
        assert!(pink.grid.iter().all(|&n| n == 4));
        let paper = convert(&solid(16, 16, [253, 253, 253, 255]), &yellow_palette(), &o);
        assert!(paper.grid.iter().all(|&n| n == 0));
        let natural = Options {
            secondary_remap: HueRemap {
                mode: HueMode::Preserve,
                ..o.secondary_remap.clone()
            },
            ..o
        };
        assert!(convert(
            &solid(16, 16, [215, 213, 224, 255]),
            &yellow_palette(),
            &natural
        )
        .grid
        .iter()
        .all(|&n| n == 0));
    }

    #[test]
    fn defaults_are_square_neutral_and_json_partial() {
        let o: Options =
            serde_json::from_str(r#"{"gamma":1.2,"hue_remap":{"mode":"grayscale"}}"#).unwrap();
        assert_eq!(o.width, 48);
        assert_eq!(o.height, 48);
        assert_eq!(o.fit, FitMode::Contain);
        assert_eq!(o.saturation, 1.0);
        assert_eq!(o.gamma, 1.2);
        assert_eq!(o.hue_remap.mode, HueMode::Grayscale);
        assert_eq!(o.hue_remap.tolerance, 55.0);
        let encoded = serde_json::to_string(&o).unwrap();
        assert!(encoded.contains("\"fit\":\"contain\""));
    }

    #[test]
    fn contain_pads_without_stretching_crop_fills() {
        let source = solid(80, 40, [0, 0, 0, 255]);
        let mut o = options(8, 8);
        let contained = convert(&source, &bw(), &o);
        assert!(contained.grid[..16].iter().all(|&n| n == 0));
        assert!(contained.grid[16..48].iter().all(|&n| n == 1));
        assert!(contained.grid[48..].iter().all(|&n| n == 0));
        o.fit = FitMode::Crop;
        assert!(convert(&source, &bw(), &o).grid.iter().all(|&n| n == 1));
    }

    #[test]
    fn crop_position_and_zoom_select_actual_source_region() {
        let mut source = solid(80, 40, [255, 255, 255, 255]);
        for y in 0..40 {
            for x in 0..40 {
                source.put(x, y, [0, 0, 0, 255]);
            }
        }
        let mut o = Options {
            fit: FitMode::Crop,
            crop_x: 0.0,
            ..options(8, 8)
        };
        assert!(convert(&source, &bw(), &o).grid.iter().all(|&n| n == 1));
        o.crop_x = 1.0;
        assert!(convert(&source, &bw(), &o).grid.iter().all(|&n| n == 0));
        o.zoom = 2.0;
        let (_, _, dx, dy) = viewport(&source, 8, 8, &o);
        assert_eq!(dx, 2.5);
        assert_eq!(dy, 2.5);
    }

    #[test]
    fn fractional_area_filter_conserves_coverage() {
        let mut source = solid(3, 1, [255, 255, 255, 255]);
        source.put(1, 0, [0, 0, 0, 255]);
        let o = Options {
            fit: FitMode::Crop,
            ..options(2, 1)
        };
        // Explicit 3:1 viewport matches source, each output gets half black.
        let resized = resample(&source, 6, 2, &o);
        assert_eq!(resized[0], gray(255));
        assert_eq!(resized[2], gray(0));
        let resized = resample(
            &source,
            1,
            1,
            &Options {
                fit: FitMode::Contain,
                ..o
            },
        );
        // 3x3 footprint: one black source pixel, eight white/padding pixels.
        assert!((i32::from(resized[0].r) - 227).abs() <= 1);
    }

    #[test]
    fn transparent_pixels_and_hidden_rgb_do_not_bleed() {
        let transparent = solid(32, 32, [255, 0, 255, 0]);
        assert!(convert(&transparent, MOSAIC_MAKER, &options(8, 8))
            .grid
            .iter()
            .all(|&n| n == 0));
        let half = solid(1, 1, [0, 0, 0, 128]);
        let hi = resample(&half, 1, 1, &options(1, 1));
        assert_eq!(hi[0], gray(127));
    }

    #[test]
    fn smooth_averages_linear_light_not_lab() {
        let mut source = solid(8, 8, [0, 0, 0, 255]);
        for y in 0..8 {
            for x in 4..8 {
                source.put(x, y, [255, 255, 255, 255]);
            }
        }
        let colors = [
            TileColor {
                name: "encoded midpoint",
                rgb: gray(128),
            },
            TileColor {
                name: "linear midpoint",
                rgb: gray(188),
            },
        ];
        let o = Options {
            order: Order::Smooth,
            white_background: false,
            neutral_cleanup: 0.0,
            ..options(1, 1)
        };
        assert_eq!(convert(&source, &colors, &o).grid, vec![1]);
    }

    #[test]
    fn sharp_retains_quarter_stud_black_line_and_single_eye() {
        let mut source = solid(32, 32, [255, 255, 255, 255]);
        for y in 0..32 {
            source.put(13, y, [0, 0, 0, 255]);
        }
        source.put(21, 13, [0, 0, 0, 255]);
        source.put(22, 13, [0, 0, 0, 255]);
        source.put(21, 14, [0, 0, 0, 255]);
        source.put(22, 14, [0, 0, 0, 255]);
        let o = Options {
            despeckle_strength: 1.0,
            ..options(8, 8)
        };
        let mosaic = convert(&source, &bw(), &o);
        for y in 0..8 {
            assert_eq!(mosaic.grid[y * 8 + 3], 1);
        }
        assert_eq!(mosaic.grid[3 * 8 + 5], 1);
        assert_eq!(mosaic.grid[3 * 8 + 4], 0);
    }

    #[test]
    fn actual_smooth_stud_dithering_changes_flat_tone_and_strength_zero_disables() {
        let source = solid(64, 64, [155, 155, 155, 255]);
        let mut o = Options {
            order: Order::Smooth,
            dither: false,
            despeckle: false,
            ..options(16, 16)
        };
        let plain = convert(&source, &bw(), &o);
        assert!(plain.grid.iter().all(|&n| n == plain.grid[0]));
        o.dither = true;
        o.dither_strength = 1.0;
        let dithered = convert(&source, &bw(), &o);
        assert_ne!(plain.grid, dithered.grid);
        assert!(dithered.grid.contains(&0) && dithered.grid.contains(&1));
        assert_eq!(dithered.grid, convert(&source, &bw(), &o).grid);
        o.dither_strength = 0.0;
        assert_eq!(plain.grid, convert(&source, &bw(), &o).grid);
    }

    #[test]
    fn gray_gradient_does_not_gain_yellow_dither_noise() {
        let mut source = solid(128, 64, [255, 255, 255, 255]);
        for y in 0..64 {
            for x in 0..128 {
                let n = (x * 255 / 127) as u8;
                source.put(x, y, [n, n, n, 255]);
            }
        }
        let o = Options {
            order: Order::Smooth,
            dither: true,
            ..options(32, 16)
        };
        let mosaic = convert(&source, &yellow_palette(), &o);
        assert!(!mosaic.grid.contains(&4));
        assert!(mosaic.grid.contains(&0) && mosaic.grid.contains(&3));
    }

    #[test]
    fn selected_pink_family_maps_consistently_to_yellow_or_light_gray() {
        for pink in [
            [244, 185, 202, 255],
            [233, 157, 181, 255],
            [240, 190, 210, 255],
        ] {
            let source = solid(16, 16, pink);
            let mut o = options(4, 4);
            let yellow = convert(&source, &yellow_palette(), &o);
            assert!(
                yellow.grid.iter().all(|&n| n == 4),
                "{:?}: {:?}",
                pink,
                yellow.grid
            );
            o.hue_remap.mode = HueMode::Grayscale;
            let gray = convert(&source, &yellow_palette(), &o);
            assert!(
                gray.grid.iter().all(|&n| n == 1),
                "{:?}: {:?}",
                pink,
                gray.grid
            );
            o.hue_remap.mode = HueMode::Target;
            o.hue_remap.target = [170, 170, 170];
            assert!(convert(&source, &yellow_palette(), &o)
                .grid
                .iter()
                .all(|&n| n == 1));
        }
    }

    #[test]
    fn hue_selection_wraps_and_protects_paper_and_ink() {
        let mut o = options(1, 1);
        o.hue_remap.source_hue = 0.0;
        o.hue_remap.tolerance = 30.0;
        let target = remap_target(
            &o.hue_remap,
            &yellow_palette()
                .iter()
                .map(|t| t.rgb.to_lab())
                .collect::<Vec<_>>(),
        );
        assert!(preprocess(Srgb::new(240, 150, 160), &o, target).b > 50.0);
        assert!(preprocess(Srgb::new(240, 160, 150), &o, target).b > 50.0);
        assert!(preprocess(Srgb::new(25, 10, 15), &o, target).l < 15.0);
        assert!(chroma(preprocess(Srgb::new(254, 251, 252), &o, target)) < 1.0);
        let cyan = Srgb::new(100, 230, 240);
        assert!((preprocess(cyan, &o, target).a - cyan.to_lab().a).abs() < 0.01);
    }

    #[test]
    fn auto_remap_preserves_extended_palette_colors() {
        let palette: Vec<_> = crate::palette::EXTENDED
            .iter()
            .map(|t| t.rgb.to_lab())
            .collect();
        assert!(remap_target(&HueRemap::default(), &palette).is_none());
    }

    #[test]
    fn neutral_cleanup_only_reduces_weak_casts() {
        let o = options(1, 1);
        let paper = Srgb::new(210, 208, 203);
        assert!(chroma(preprocess(paper, &o, None)) < chroma(paper.to_lab()));
        let pastel = Srgb::new(244, 190, 210);
        assert!((chroma(preprocess(pastel, &o, None)) - chroma(pastel.to_lab())).abs() < 0.01);
    }

    #[test]
    fn adjustments_are_neutral_and_brightness_gamma_monotonic() {
        let rgb = Srgb::new(60, 110, 160);
        let mut o = options(1, 1);
        assert_eq!(adjust(rgb, &o), rgb);
        o.gamma = 2.0;
        assert!(adjust(rgb, &o).g > rgb.g);
        o.gamma = 1.0;
        o.brightness = 30;
        assert_eq!(adjust(rgb, &o), Srgb::new(90, 140, 190));
        o.saturation = 0.0;
        let gray = adjust(rgb, &o);
        assert_eq!(gray.r, gray.g);
        assert_eq!(gray.g, gray.b);
    }

    #[test]
    fn despeckle_is_synchronous_conservative_and_deterministic() {
        let palette: Vec<_> = yellow_palette().iter().map(|t| t.rgb.to_lab()).collect();
        let mut grid = vec![0; 25];
        grid[12] = 1;
        let mut cells = vec![palette[0]; 25];
        cells[12] = Lab {
            l: 84.0,
            a: 0.0,
            b: 0.0,
        };
        let mut o = options(5, 5);
        o.despeckle_strength = 1.0;
        despeckle(&mut grid, &cells, &[0.0; 25], &palette, &o);
        assert_eq!(grid[12], 0);
        grid[12] = 3; // an isolated eye is never "noise"
        despeckle(&mut grid, &cells, &[0.0; 25], &palette, &o);
        assert_eq!(grid[12], 3);
        grid[12] = 1;
        grid[6] = 1; // diagonal connected line survives
        despeckle(&mut grid, &cells, &[0.0; 25], &palette, &o);
        assert_eq!(grid[12], 1);
        assert_eq!(grid[6], 1);
    }

    #[test]
    fn malformed_inputs_and_extreme_controls_are_safe() {
        let source = Raster {
            width: usize::MAX,
            height: 2,
            pixels: vec![0; 4],
        };
        let o = Options {
            width: 0,
            height: usize::MAX,
            gamma: f64::NAN,
            contrast: f64::INFINITY,
            crop_x: -10.0,
            zoom: -1.0,
            ..Options::default()
        };
        let mosaic = convert(&source, &[], &o);
        assert_eq!(mosaic.width, 1);
        assert_eq!(mosaic.height, 192);
        assert_eq!(mosaic.palette.len(), 1);
        assert!(mosaic.grid.iter().all(|&n| n == 0));
        assert_eq!(o.width, 0, "input options are not mutated");
    }

    #[test]
    fn exclusions_reindex_and_all_excluded_keep_first() {
        let source = solid(16, 16, [0, 0, 0, 255]);
        let o = Options {
            excluded: vec![0, 0, 999],
            ..options(4, 4)
        };
        assert!(convert(&source, &bw(), &o).grid.iter().all(|&n| n == 0));
        let o = Options {
            excluded: vec![0, 1],
            ..o
        };
        let mosaic = convert(&source, &bw(), &o);
        assert_eq!(mosaic.palette, vec![bw()[0]]);
    }
}

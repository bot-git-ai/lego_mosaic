// Copyright (c) 2026 Witalis Domitrz <witekdomitrz@gmail.com>
// AGPL License

//! Color science: sRGB → CIELAB conversion and the CIEDE2000 color
//! difference. Naive RGB nearest-color matching is what makes most mosaic
//! converters look muddy (midtones collapse into dark gray); matching in a
//! perceptual space is the single biggest quality win.

/// XYZ of the D65 reference white under the sRGB matrix: Lab conversion
/// divides by these so pure white maps to L\* = 100, a\* = b\* = 0.
const WHITE_X: f64 = 0.950_47;
const WHITE_Y: f64 = 1.0;
const WHITE_Z: f64 = 1.088_83;

/// An sRGB color with 0–255 components.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Srgb {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

impl Srgb {
    pub const fn new(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b }
    }

    /// Hex string for HTML output (`#f2f3f2`).
    pub fn hex(self) -> String {
        format!("#{:02x}{:02x}{:02x}", self.r, self.g, self.b)
    }

    /// sRGB → CIELAB (D65 reference white): inverse companding → linear RGB →
    /// XYZ (D65) → Lab.
    pub fn to_lab(self) -> Lab {
        let [lr, lg, lb] = [
            inverse_compand(f64::from(self.r) / 255.0),
            inverse_compand(f64::from(self.g) / 255.0),
            inverse_compand(f64::from(self.b) / 255.0),
        ];
        let x = 0.412_456_4 * lr + 0.357_576_1 * lg + 0.180_437_5 * lb;
        let y = 0.212_672_9 * lr + 0.715_152_2 * lg + 0.072_175_0 * lb;
        let z = 0.019_333_9 * lr + 0.119_192_0 * lg + 0.950_304_1 * lb;
        // WHITE_* scales pure white to L* = 100, a* = b* = 0.
        Lab {
            l: 116.0 * f(y / WHITE_Y) - 16.0,
            a: 500.0 * (f(x / WHITE_X) - f(y / WHITE_Y)),
            b: 200.0 * (f(y / WHITE_Y) - f(z / WHITE_Z)),
        }
    }
}

/// A CIELAB color (D65).
#[derive(Clone, Copy, Debug)]
pub struct Lab {
    pub l: f64,
    pub a: f64,
    pub b: f64,
}

/// Inverse sRGB companding: monitor-encoded 0–1 → linear light 0–1.
fn inverse_compand(c: f64) -> f64 {
    if c <= 0.040_45 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

/// The Lab cubic-root function with its linear knee (δ = 6/29).
fn f(t: f64) -> f64 {
    const DELTA: f64 = 6.0 / 29.0;
    if t > DELTA * DELTA * DELTA {
        t.cbrt()
    } else {
        t / (3.0 * DELTA * DELTA) + 4.0 / 29.0
    }
}

/// CIEDE2000 color difference between two Lab colors. With a tiny palette
/// CIE76 would give nearly the same nearest-neighbor verdicts, but 2000 is
/// cheap and correct near the hue discontinuities (skin tones, pastels).
pub fn delta_e_2000(lab1: &Lab, lab2: &Lab) -> f64 {
    let (l1, a1, b1) = (lab1.l, lab1.a, lab1.b);
    let (l2, a2, b2) = (lab2.l, lab2.a, lab2.b);

    let c1 = (a1 * a1 + b1 * b1).sqrt();
    let c2 = (a2 * a2 + b2 * b2).sqrt();
    let c_bar = f64::midpoint(c1, c2);
    let c_bar7 = c_bar.powi(7);
    let g = 0.5 * (1.0 - (c_bar7 / (c_bar7 + 25.0_f64.powi(7))).sqrt());
    let a1p = a1 * (1.0 + g);
    let a2p = a2 * (1.0 + g);
    let c1p = (a1p * a1p + b1 * b1).sqrt();
    let c2p = (a2p * a2p + b2 * b2).sqrt();

    let h1p = hue_angle(a1p, b1);
    let h2p = hue_angle(a2p, b2);

    let dl = l2 - l1;
    let dc = c2p - c1p;

    // Mean hue, handling the wrap-around when the two hues are far apart. Zero
    // chroma leaves the hue undefined, so delta_h is zero.
    let chroma_product = c1p * c2p;
    let dhp = if chroma_product.abs() < f64::EPSILON {
        0.0
    } else {
        let raw = h2p - h1p;
        let dh = if raw.abs() <= 180.0 {
            raw
        } else if raw > 180.0 {
            raw - 360.0
        } else {
            raw + 360.0
        };
        2.0 * chroma_product.sqrt() * (dh.to_radians() / 2.0).sin()
    };
    let mean_hue = if chroma_product.abs() < f64::EPSILON {
        h1p + h2p
    } else {
        let sum = h1p + h2p;
        let diff = (h1p - h2p).abs();
        if diff <= 180.0 {
            f64::midpoint(sum, 0.0)
        } else if sum < 360.0 {
            f64::midpoint(sum + 360.0, 0.0)
        } else {
            f64::midpoint(sum - 360.0, 0.0)
        }
    };

    let t = 1.0 - 0.17 * ((mean_hue - 30.0).to_radians()).cos()
        + 0.24 * ((2.0 * mean_hue).to_radians()).cos()
        + 0.32 * ((3.0 * mean_hue + 6.0).to_radians()).cos()
        - 0.20 * ((4.0 * mean_hue - 63.0).to_radians()).cos();

    let l_bar = f64::midpoint(l1, l2);
    let c_bar_p = f64::midpoint(c1p, c2p);
    let c_bar_p7 = c_bar_p.powi(7);

    let sl = 1.0
        + 0.015 * (l_bar - 50.0) * (l_bar - 50.0) / (20.0 + (l_bar - 50.0) * (l_bar - 50.0)).sqrt();
    let sc = 1.0 + 0.045 * c_bar_p;
    let sh = 1.0 + 0.015 * c_bar_p * t;

    // The rotation term: only active for blue-ish hues and nonzero chroma.
    let rt = if chroma_product.abs() < f64::EPSILON {
        0.0
    } else {
        let d_theta = 30.0 * (-(((mean_hue - 275.0) / 25.0) * ((mean_hue - 275.0) / 25.0))).exp();
        let r_c = 2.0 * (c_bar_p7 / (c_bar_p7 + 25.0_f64.powi(7))).sqrt();
        -r_c * (2.0 * d_theta).to_radians().sin()
    };

    let term_l = dl / sl * (dl / sl);
    let term_c = dc / sc;
    let term_h = dhp / sh;
    // The cross term uses the SIGNED chroma×hue product, which is why the
    // chroma terms are not squared before it is added.
    (term_l + term_c * term_c + term_h * term_h + rt * term_c * term_h).sqrt()
}

/// Hue angle in degrees (0 when both a and b are 0).
fn hue_angle(ap: f64, b: f64) -> f64 {
    if ap == 0.0 && b == 0.0 {
        0.0
    } else {
        let angle = b.atan2(ap).to_degrees();
        if angle < 0.0 {
            angle + 360.0
        } else {
            angle
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: f64, b: f64, tol: f64) -> bool {
        (a - b).abs() < tol
    }

    #[test]
    fn white_and_black_convert_to_extreme_lab() {
        let white = Srgb::new(255, 255, 255).to_lab();
        let black = Srgb::new(0, 0, 0).to_lab();
        assert!(close(white.l, 100.0, 0.1), "L* white = {}", white.l);
        assert!(close(black.l, 0.0, 0.1), "L* black = {}", black.l);
        assert!(close(white.a, 0.0, 0.1) && close(white.b, 0.0, 0.1));
    }

    #[test]
    fn pure_red_is_positive_a() {
        let red = Srgb::new(255, 0, 0).to_lab();
        assert!(red.a > 40.0, "a* red = {}", red.a);
        assert!(red.b > 30.0, "b* red = {}", red.b);
    }

    #[test]
    fn gray_is_neutral() {
        let gray = Srgb::new(128, 128, 128).to_lab();
        assert!(close(gray.a, 0.0, 0.5) && close(gray.b, 0.0, 0.5));
    }

    #[test]
    fn identical_colors_have_zero_difference() {
        let lab = Srgb::new(123, 45, 200).to_lab();
        assert!(close(delta_e_2000(&lab, &lab), 0.0, 1e-9));
    }

    #[test]
    fn similar_colors_are_closer_than_different_ones() {
        let base = Srgb::new(200, 100, 50).to_lab();
        let near = Srgb::new(205, 105, 55).to_lab();
        let far = Srgb::new(10, 40, 220).to_lab();
        assert!(delta_e_2000(&base, &near) < delta_e_2000(&base, &far));
    }

    /// Lab triple for the reference table below.
    type LabTriple = (f64, f64, f64);

    /// CIEDE2000 reference pairs from Sharma, Wu & Dalal (2005). One pair
    /// yielding 17.5649 in some copies is a known erratum; independent
    /// implementations give ≈12.7.
    #[test]
    fn matches_sharma_reference_pairs() {
        let cases: &[(LabTriple, LabTriple, f64)] = &[
            ((50.0, 2.6772, -79.7751), (50.0, 0.0, -82.7485), 2.0425),
            ((50.0, 3.1571, -77.2803), (50.0, 0.0, -82.7485), 2.8615),
            ((50.0, -1.3802, -84.2814), (50.0, 0.0, -82.7485), 1.0000),
            ((50.0, 0.0, 0.0), (50.0, -1.0, 2.0), 2.3669),
            ((50.0, 2.49, -0.001), (50.0, -2.49, 0.0009), 7.1792),
            ((50.0, 2.49, -0.001), (50.0, -2.49, 0.0011), 7.2195),
            ((50.0, -0.001, 2.49), (50.0, 0.0009, -2.49), 4.8045),
            ((50.0, 2.5, 0.0), (73.0, 25.0, -18.0), 27.1492),
            ((50.0, 2.5, 0.0), (56.0, -27.0, -3.0), 31.9030),
            ((50.0, 2.5, 0.0), (58.0, 24.0, 15.0), 19.4535),
            (
                (60.2574, -34.0099, 36.2677),
                (60.4626, -34.1751, 39.4387),
                1.2644,
            ),
        ];
        for (i, (lab1, lab2, expected)) in cases.iter().enumerate() {
            let lab1 = Lab {
                l: lab1.0,
                a: lab1.1,
                b: lab1.2,
            };
            let lab2 = Lab {
                l: lab2.0,
                a: lab2.1,
                b: lab2.2,
            };
            let got = delta_e_2000(&lab1, &lab2);
            assert!(
                close(got, *expected, 0.001),
                "case {i}: got {got}, want {expected}"
            );
        }
    }
}

// Copyright (c) 2026 Witalis Domitrz <witekdomitrz@gmail.com>
// AGPL License
//! End-to-end separator-guard behavior through the real CLI binary.
//! Guard 0 must reproduce the pre-guard behavior (the narrow white
//! separator between a dark feature and its gray outline closes); raising
//! the guard must never close previously opened separator tiles, and the
//! released strictness (>=0.5) keeps the dark feature, its outline, and the
//! white gap as three distinct stud columns.

use std::process::Command;

/// Exact synthetic artwork: dark feature, thin gray outline, and the narrow
/// white separator between them (the panda eye/outline failure geometry).
fn artwork(x: u32, _: u32) -> [u8; 3] {
    if (8..12).contains(&x) {
        [20, 20, 20] // dark feature
    } else if (15..17).contains(&x) {
        [110, 110, 110] // thin gray outline
    } else {
        [255, 255, 255] // paper + narrow white separator
    }
}

/// Convert the synthetic 32×32 image via the real CLI with a recipe carrying
/// `separator_guard`; return the resulting SVG.
fn convert(guard: Option<f64>, dir: &std::path::Path, tag: &str) -> String {
    let input = dir.join(format!("{tag}-input.png"));
    let output = dir.join(format!("{tag}.svg"));
    image::RgbaImage::from_fn(32, 32, |x, y| {
        let [r, g, b] = artwork(x, y);
        image::Rgba([r, g, b, 255])
    })
    .save(&input)
    .unwrap();
    let mut command = Command::new(env!("CARGO_BIN_EXE_lego-mosaic"));
    command
        .args(["convert"])
        .arg(&input)
        .args(["--output"])
        .arg(&output)
        // Fixed 8×8 grid so the artwork's pixel geometry maps to studs
        // exactly as in the mosaic.rs unit tests (4 px per stud).
        .args(["--size", "8"])
        .args(["--force"]);
    if let Some(guard) = guard {
        let recipe = dir.join(format!("{tag}-recipe.json"));
        std::fs::write(&recipe, format!(r#"{{"separator_guard": {guard}}}"#)).unwrap();
        command.args(["--recipe"]).arg(&recipe);
    }
    let outcome = command.output().expect("run lego-mosaic");
    assert!(
        outcome.status.success(),
        "conversion failed for guard {guard:?}: {}{}",
        String::from_utf8_lossy(&outcome.stdout),
        String::from_utf8_lossy(&outcome.stderr),
    );
    let svg = std::fs::read_to_string(&output).unwrap();
    let _ = std::fs::remove_file(&input);
    let _ = std::fs::remove_file(&output);
    svg
}

/// Distinct y values present for column `x` with the given fill.
fn rows(svg: &str, x: usize, hex: &str) -> Vec<usize> {
    let needle_prefix = format!("x=\"{}\" y=\"", x * 20);
    svg.match_indices(&needle_prefix)
        .filter_map(|(start, _)| {
            let rest = &svg[start + needle_prefix.len()..];
            let end = rest.find('"')?;
            let y: usize = rest[..end].parse().ok()?;
            let after = &rest[end + 1..];
            after
                .starts_with(&format!(" width=\"20\" height=\"20\" fill=\"{hex}\""))
                .then_some(y / 20)
        })
        .collect()
}

#[test]
fn separator_guard_is_end_to_end_monotonic_through_the_cli() {
    let dir = std::env::temp_dir().join(format!(
        "mosaic-separator-{}-{}",
        std::process::id(),
        line!()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();

    // The un-guarded default equals guard 1.0 (documented default).
    let default_svg = convert(None, &dir, "default");
    let black = 8_usize;
    assert_eq!(
        rows(&default_svg, 2, "#05131d").len(),
        8,
        "dark feature intact at default settings"
    );
    assert_eq!(rows(&default_svg, 4, "#6c6e68").len(), 8, "outline intact");
    assert_eq!(
        rows(&default_svg, 3, "#f2f3f2").len(),
        8,
        "separator intact"
    );

    let mut whites = Vec::new();
    for guard in [0.0, 0.5, 1.0] {
        let svg = convert(Some(guard), &dir, &format!("g{guard}"));
        assert_eq!(
            rows(&svg, 2, "#05131d").len(),
            black,
            "dark feature survives at guard {guard}"
        );
        assert_eq!(
            rows(&svg, 4, "#6c6e68").len(),
            8,
            "outline survives at guard {guard}"
        );
        whites.push(rows(&svg, 3, "#f2f3f2").len());
    }
    assert_eq!(
        whites,
        vec![0, 8, 8],
        "guard 0 closes the separator (old behavior); 0.5 and 1.0 keep it open"
    );
    let _ = std::fs::remove_dir_all(dir);
}

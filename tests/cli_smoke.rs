// Copyright (c) 2026 Witalis Domitrz <witekdomitrz@gmail.com>
// AGPL License
//! External binary regression through the real CLI: a solid 2×2 PNG must
//! convert to exactly the expected 48×48-stud SVG bytes (default palette,
//! exact-color mapping, 20 px studs). Ported from `tests/cli_smoke.py`.

use std::process::Command;

#[test]
fn cli_converts_a_solid_png_to_exact_expected_svg_bytes() {
    let dir = std::env::temp_dir().join(format!("mosaic-cli-{}-{}", std::process::id(), line!()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();

    let source = dir.join("solid.png");
    let output = dir.join("test.svg");
    image::RgbaImage::from_fn(2, 2, |_, _| image::Rgba([0xf2, 0xf3, 0xf2, 0xff]))
        .save(&source)
        .unwrap();

    let outcome = Command::new(env!("CARGO_BIN_EXE_lego-mosaic"))
        .args(["convert"])
        .arg(&source)
        .args(["--output"])
        .arg(&output)
        .output()
        .expect("run lego-mosaic");
    assert!(
        outcome.status.success(),
        "conversion failed: {}{}",
        String::from_utf8_lossy(&outcome.stdout),
        String::from_utf8_lossy(&outcome.stderr),
    );

    let mut expected = String::from(
        r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 960 960" width="960" height="960" role="img"><title>Flat exact-color mosaic preview</title><g shape-rendering="crispEdges">"#,
    );
    for y in 0..48 {
        for x in 0..48 {
            expected.push_str(&format!(
                r##"<rect x="{}" y="{}" width="20" height="20" fill="#f2f3f2"/>"##,
                x * 20,
                y * 20
            ));
        }
    }
    expected.push_str("</g></svg>");

    let svg = std::fs::read_to_string(&output).unwrap();
    assert_eq!(svg, expected, "CLI SVG differs from exact expected bytes");
    let _ = std::fs::remove_dir_all(&dir);
}

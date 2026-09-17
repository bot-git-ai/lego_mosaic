// Copyright (c) 2026 Witalis Domitrz <witekdomitrz@gmail.com>
// AGPL License
//! Native I/O and argument handling only. Pixel conversion and every SVG/CSV
//! byte are produced by the same library used by browser.rs.
use lego_mosaic::{HueMode, Options, Order, Raster};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

pub fn help() -> &'static str {
    "LEGO Mosaic Studio\n  lego-mosaic [serve]\n  lego-mosaic convert IMAGE --output mosaic.svg [options]\n\n  --size N | --width N --height N     studs (1–192, default 48)\n  --palette mosaic-maker|monochrome|extended\n  --preset artwork|photo|natural|grayscale|yellow-accent\n  --recipe FILE    load JSON Options; flags override recipe\n  --save-recipe FILE    save effective Options JSON\n  --parts FILE.csv --guide FILE.svg\n  --studs         round tile SVG (default flat SVG)\n  --brightness N --contrast N --gamma N --saturation N\n  --fit contain|crop|stretch --pad N (0–255)
  --color-limit N   cap tiles per color (e.g. 900 for set 40179; 0=off)\n  --stages-dir DIR  write fitted/adjusted/recolored/tiles PNGs for inspection\n  --outline-strength N    0–1, artwork only\n  --dither | --no-dither\n  --force         permit replacing existing output files\n\nPNG output is also supported: one pixel per stud, exact palette colors.\nPhoto/natural presets disable illustration recoloring. Recipes expose all\ncolor-family, crop and processing controls. Input PNG/JPEG/WebP/GIF; first frame."
}

#[derive(Debug)]
struct Request {
    input: PathBuf,
    outputs: BTreeMap<String, PathBuf>,
    palette: String,
    options: Options,
    studs: bool,
    force: bool,
    stages_dir: Option<PathBuf>,
}

fn parse(args: &[String]) -> Result<Request, String> {
    let mut values = BTreeMap::new();
    let mut input = None;
    let mut flags = Vec::new();
    let mut i = 0;
    while i < args.len() {
        let arg = &args[i];
        if ["--studs", "--flat", "--dither", "--no-dither", "--force"].contains(&arg.as_str()) {
            flags.push(arg.as_str());
        } else if arg.starts_with("--") {
            let key = arg.trim_start_matches("--");
            if ![
                "output",
                "parts",
                "guide",
                "save-recipe",
                "recipe",
                "size",
                "width",
                "height",
                "palette",
                "preset",
                "brightness",
                "contrast",
                "gamma",
                "saturation",
                "outline-strength",
                "fit",
                "pad",
                "color-limit",
                "stages-dir",
            ]
            .contains(&key)
            {
                return Err(format!("Unknown option {arg}"));
            }
            i += 1;
            let value = args
                .get(i)
                .ok_or_else(|| format!("Missing value for {arg}"))?;
            if values.insert(key.to_string(), value.clone()).is_some() {
                return Err(format!("Duplicate option {arg}"));
            }
        } else if input.replace(PathBuf::from(arg)).is_some() {
            return Err("Only one input image is accepted".into());
        }
        i += 1;
    }
    if flags.contains(&"--dither") && flags.contains(&"--no-dither") {
        return Err("Choose either --dither or --no-dither".into());
    }
    let mut options = Options::default();
    let mut palette = "mosaic-maker".to_string();
    if let Some(preset) = values.get("preset") {
        match preset.as_str() {
            "artwork" => {}
            "photo" => {
                options.order = Order::Smooth;
                options.dither = true;
                options.white_background = false;
                options.neutral_cleanup = 0.0;
                options.despeckle = false;
                options.outline_strength = 0.0;
                options.hue_remap.mode = HueMode::Preserve;
                options.secondary_remap.mode = HueMode::Preserve;
            }
            "natural" => {
                options.hue_remap.mode = HueMode::Preserve;
                options.secondary_remap.mode = HueMode::Preserve;
            }
            "grayscale" => {
                palette = "monochrome".into();
                options.order = Order::Smooth;
                options.saturation = 0.0;
                options.outline_strength = 0.0;
                options.hue_remap.mode = HueMode::Preserve;
                options.secondary_remap.mode = HueMode::Preserve;
            }
            "yellow-accent" => {
                options.hue_remap.mode = HueMode::Target;
            }
            _ => return Err(format!("Unknown preset: {preset}")),
        }
    }
    // Partial recipe overlays the preset; explicit flags always win.
    if let Some(file) = values.get("recipe") {
        let recipe: serde_json::Value =
            serde_json::from_slice(&std::fs::read(file).map_err(|e| format!("Read recipe: {e}"))?)
                .map_err(|e| format!("Recipe JSON: {e}"))?;
        if !recipe.is_object() {
            return Err("Recipe must be a JSON object of Options".into());
        }
        let mut merged = serde_json::to_value(&options).map_err(|e| e.to_string())?;
        fn overlay(base: &mut serde_json::Value, patch: serde_json::Value) {
            if let (Some(b), Some(p)) = (base.as_object_mut(), patch.as_object()) {
                for (k, v) in p {
                    if let Some(old) = b.get_mut(k) {
                        overlay(old, v.clone());
                    } else {
                        b.insert(k.clone(), v.clone());
                    }
                }
            } else {
                *base = patch;
            }
        }
        overlay(&mut merged, recipe);
        options = serde_json::from_value(merged).map_err(|e| format!("Recipe Options: {e}"))?;
    }
    if let Some(fit) = values.get("fit") {
        options.fit = match fit.as_str() {
            "contain" => lego_mosaic::FitMode::Contain,
            "crop" => lego_mosaic::FitMode::Crop,
            "stretch" => lego_mosaic::FitMode::Stretch,
            _ => return Err("Fit must be contain, crop or stretch".into()),
        };
    }
    if let Some(limit) = values.get("color-limit") {
        options.color_limit = limit
            .parse()
            .map_err(|_| "Color limit must be a non-negative integer")?;
    }
    if let Some(pad) = values.get("pad") {
        options.pad = pad.parse().map_err(|_| "Pad must be 0–255")?;
    }
    for key in ["size", "width", "height"] {
        if let Some(value) = values.get(key) {
            let n: usize = value.parse().map_err(|_| format!("Invalid --{key}"))?;
            if !(1..=192).contains(&n) {
                return Err("Dimensions must be 1–192".into());
            }
            if key != "height" {
                options.width = n;
            }
            if key != "width" {
                options.height = n;
            }
        }
    }
    for (key, dst, lo, hi) in [
        ("contrast", &mut options.contrast, 0.25, 3.0),
        ("gamma", &mut options.gamma, 0.25, 4.0),
        ("saturation", &mut options.saturation, 0.0, 3.0),
        ("outline-strength", &mut options.outline_strength, 0.0, 1.0),
    ] {
        if let Some(value) = values.get(key) {
            let n: f64 = value.parse().map_err(|_| format!("Invalid --{key}"))?;
            if !n.is_finite() || n < lo || n > hi {
                return Err(format!("--{key} must be {lo}–{hi}"));
            }
            *dst = n;
        }
    }
    if let Some(value) = values.get("brightness") {
        options.brightness = value.parse().map_err(|_| "Invalid brightness")?;
        if !(-100..=100).contains(&options.brightness) {
            return Err("Brightness must be -100–100".into());
        }
    }
    if flags.contains(&"--dither") {
        options.dither = true;
    }
    if flags.contains(&"--no-dither") {
        options.dither = false;
    }
    if let Some(id) = values.get("palette") {
        palette = id.clone();
    }
    if !lego_mosaic::palettes().iter().any(|p| p.0 == palette) {
        return Err(format!("Unknown palette: {palette}"));
    }
    let outputs: BTreeMap<_, _> = ["output", "parts", "guide", "save-recipe"]
        .into_iter()
        .filter_map(|key| values.get(key).map(|v| (key.to_string(), PathBuf::from(v))))
        .collect();
    let output = outputs.get("output").ok_or("--output is required")?;
    if !matches!(
        output.extension().and_then(|v| v.to_str()),
        Some("svg" | "png")
    ) {
        return Err("Output extension must be .svg or .png".into());
    }
    Ok(Request {
        input: input.ok_or("Input image is required")?,
        outputs,
        palette,
        options,
        studs: flags.contains(&"--studs"),
        force: flags.contains(&"--force"),
        stages_dir: values.get("stages-dir").map(PathBuf::from),
    })
}

fn identity(path: &Path) -> Result<PathBuf, String> {
    if path.exists() {
        return path.canonicalize().map_err(|e| e.to_string());
    }
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    Ok(parent
        .canonicalize()
        .map_err(|e| format!("Output parent: {e}"))?
        .join(path.file_name().ok_or("Invalid output filename")?))
}
fn validate_paths(request: &Request) -> Result<(), String> {
    let source = identity(&request.input)?;
    let mut seen = vec![source];
    for path in request.outputs.values() {
        let id = identity(path)?;
        if seen.contains(&id) {
            return Err("Input/output paths collide".into());
        }
        seen.push(id);
        if path.exists() && !request.force {
            return Err(format!("{} exists; use --force", path.display()));
        }
        if path.is_dir() {
            return Err("Output path is a directory".into());
        }
    }
    if let Some(dir) = &request.stages_dir {
        if dir.exists() && !request.force {
            return Err(format!("{} exists; use --force", dir.display()));
        }
    }
    Ok(())
}

pub fn run(args: &[String]) -> Result<(), String> {
    if args == ["--help"] || args == ["-h"] {
        println!("{}", help());
        return Ok(());
    }
    let req = parse(args)?;
    validate_paths(&req)?;
    let mut reader = image::ImageReader::open(&req.input)
        .map_err(|e| format!("Open image: {e}"))?
        .with_guessed_format()
        .map_err(|e| e.to_string())?;
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(16384);
    limits.max_image_height = Some(16384);
    limits.max_alloc = Some(256 * 1024 * 1024);
    reader.limits(limits);
    let image = reader
        .decode()
        .map_err(|e| format!("Decode image: {e}"))?
        .to_rgba8();
    let raster = Raster {
        width: image.width() as usize,
        height: image.height() as usize,
        pixels: image.into_raw(),
    };
    let mosaic = lego_mosaic::convert(&raster, &req.palette, &req.options)?;
    let stage_rasters: Vec<(String, Raster)> = if req.stages_dir.is_some() {
        let tiles = &lego_mosaic::palettes()
            .iter()
            .find(|p| p.0 == req.palette)
            .ok_or("Unknown palette")?
            .2;
        let s = lego_mosaic::stages(&raster, tiles, &req.options);
        vec![
            ("fitted".into(), s.fitted),
            ("adjusted".into(), s.adjusted),
            ("recolored".into(), s.recolored),
            ("tiles".into(), s.tiles),
        ]
    } else {
        Vec::new()
    };
    let svg = if req.studs {
        lego_mosaic::stud_svg(&mosaic, 20)
    } else {
        lego_mosaic::flat_svg(&mosaic, 20)
    };
    let mut artifacts = Vec::new();
    for (kind, path) in &req.outputs {
        let bytes = match kind.as_str() {
            "output" if path.extension().is_some_and(|e| e == "png") => {
                let mut img = image::RgbaImage::new(mosaic.width as u32, mosaic.height as u32);
                for (i, &index) in mosaic.grid.iter().enumerate() {
                    let c = mosaic.palette[index].rgb;
                    img.put_pixel(
                        (i % mosaic.width) as u32,
                        (i / mosaic.width) as u32,
                        image::Rgba([c.r, c.g, c.b, 255]),
                    );
                }
                let mut buf = std::io::Cursor::new(Vec::new());
                img.write_to(&mut buf, image::ImageFormat::Png)
                    .map_err(|e| e.to_string())?;
                buf.into_inner()
            }
            "output" => svg.as_bytes().to_vec(),
            "parts" => lego_mosaic::parts_csv(&mosaic).into_bytes(),
            "guide" => lego_mosaic::guide_svg(&mosaic, 24).into_bytes(),
            "save-recipe" => serde_json::to_vec_pretty(&req.options).map_err(|e| e.to_string())?,
            _ => unreachable!(),
        };
        artifacts.push((path, bytes));
    }
    // Generate and validate before opening any destination; create_new protects
    // existing work even if another process creates it after validation.
    for (path, bytes) in artifacts {
        use std::io::Write;
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create(req.force)
            .create_new(!req.force)
            .truncate(req.force)
            .open(path)
            .map_err(|e| format!("Write {}: {e}", path.display()))?;
        file.write_all(&bytes).map_err(|e| e.to_string())?;
    }
    if let Some(dir) = &req.stages_dir {
        std::fs::create_dir_all(dir).map_err(|e| format!("Create {}: {e}", dir.display()))?;
        let stem = req
            .input
            .file_stem()
            .and_then(|v| v.to_str())
            .unwrap_or("image");
        for (name, raster) in &stage_rasters {
            let mut img = image::RgbaImage::new(raster.width as u32, raster.height as u32);
            for (i, px) in raster.pixels.as_chunks::<4>().0.iter().enumerate() {
                img.put_pixel(
                    (i % raster.width) as u32,
                    (i / raster.width) as u32,
                    image::Rgba([px[0], px[1], px[2], 255]),
                );
            }
            let path = dir.join(format!("{stem}-{name}.png"));
            img.save(&path)
                .map_err(|e| format!("Write {}: {e}", path.display()))?;
        }
    }
    eprintln!(
        "{} × {} mosaic, {} tiles; palette {}",
        mosaic.width,
        mosaic.height,
        mosaic.grid.len(),
        req.palette
    );
    if !mosaic.overflow.is_empty() {
        for (index, excess) in &mosaic.overflow {
            eprintln!(
                "warning: {} exceeds the {}-tile color cap by {} tiles",
                mosaic.palette[*index].name, req.options.color_limit, excess
            );
        }
    } else if req.options.color_limit > 0 {
        eprintln!(
            "color cap of {} tiles per color satisfied",
            req.options.color_limit
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn args(words: &[&str]) -> Vec<String> {
        words.iter().map(|s| s.to_string()).collect()
    }
    #[test]
    fn parser_dimensions_and_presets() {
        let r = parse(&args(&[
            "x.png", "--output", "x.svg", "--size", "64", "--height", "48", "--preset", "photo",
        ]))
        .unwrap();
        assert_eq!((r.options.width, r.options.height), (64, 48));
        assert_eq!(r.options.secondary_remap.mode, HueMode::Preserve);
        assert!(parse(&args(&["x.png", "--output", "x.svg", "--size", "0"])).is_err());
    }
    #[test]
    fn cli_uses_exact_shared_svg() {
        let dir = std::env::temp_dir().join(format!("mosaic-cli-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let input = dir.join("input.png");
        let output = dir.join("out.svg");
        let _ = std::fs::remove_file(&output);
        image::RgbaImage::from_pixel(8, 8, image::Rgba([255, 255, 255, 255]))
            .save(&input)
            .unwrap();
        run(&args(&[
            input.to_str().unwrap(),
            "--output",
            output.to_str().unwrap(),
            "--size",
            "1",
        ]))
        .unwrap();
        let expected="<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 20 20\" width=\"20\" height=\"20\" role=\"img\"><title>Flat exact-color mosaic preview</title><g shape-rendering=\"crispEdges\"><rect x=\"0\" y=\"0\" width=\"20\" height=\"20\" fill=\"#f2f3f2\"/></g></svg>";
        assert_eq!(std::fs::read_to_string(&output).unwrap(), expected);
        assert!(run(&args(&[
            input.to_str().unwrap(),
            "--output",
            input.to_str().unwrap(),
            "--force"
        ]))
        .is_err());
        assert!(run(&args(&[
            input.to_str().unwrap(),
            "--output",
            output.to_str().unwrap()
        ]))
        .is_err());
        std::fs::remove_dir_all(dir).unwrap();
    }
}

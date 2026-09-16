// Copyright (c) 2026 Witalis Domitrz <witekdomitrz@gmail.com>
// AGPL License

//! LEGO Mosaic Maker: a picture → round-plate mosaic web app in pure
//! Rust. Serves one page plus a `POST /api/convert` endpoint. Designed to
//! sit behind the `/lego-mosaic` prefix (bot-web reverse-proxies it), so
//! paths here are prefix-free.

mod color;
mod mosaic;
mod multipart;
mod palette;
mod render;
mod server;
mod ui;

/// Offline conversion: `lego-mosaic preview <in> <out.png> [palette] [size]`
/// writes an upscaled PNG of the stud grid (flat colors), so a conversion
/// can be judged without a browser.
mod preview {
    // Same small-dimension pixel math as `mosaic` (grid ≤ 192 studs): the
    // usize→u32 casts cannot truncate.
    #![allow(clippy::cast_possible_truncation)]

    use std::path::PathBuf;

    use crate::mosaic::{self, Options};
    use crate::palette;

    pub(super) fn run(args: &[String]) {
        let Some(input) = args.first().map(PathBuf::from) else {
            eprintln!("usage: lego-mosaic preview <in> <out.png> [palette] [size]");
            std::process::exit(2);
        };
        let output = args
            .get(1)
            .map_or_else(|| PathBuf::from("preview.png"), PathBuf::from);
        let palette_id = args.get(2).map_or("mosaic-maker", String::as_str);
        let size = args
            .get(3)
            .and_then(|s| s.parse::<usize>().ok())
            .unwrap_or(48);
        let image = image::open(&input).unwrap_or_else(|error| {
            eprintln!("cannot open {}: {error}", input.display());
            std::process::exit(1);
        });
        let options = Options {
            width: size,
            height: size,
            ..Options::default()
        };
        let mosaic = mosaic::convert(&image.to_rgba8(), palette::by_id(palette_id), &options);
        // Each stud as an 8×8 block of its color.
        let scale = 8_u32;
        let mut out =
            image::RgbaImage::new(mosaic.width as u32 * scale, mosaic.height as u32 * scale);
        for (i, &index) in mosaic.grid.iter().enumerate() {
            let rgb = mosaic.palette[index].rgb;
            let x0 = (i % mosaic.width) as u32 * scale;
            let y0 = (i / mosaic.width) as u32 * scale;
            for dy in 0..scale {
                for dx in 0..scale {
                    out.put_pixel(x0 + dx, y0 + dy, image::Rgba([rgb.r, rgb.g, rgb.b, 255]));
                }
            }
        }
        out.save(&output).unwrap_or_else(|error| {
            eprintln!("cannot write {}: {error}", output.display());
            std::process::exit(1);
        });
        println!(
            "wrote {} ({}×{} studs)",
            output.display(),
            mosaic.width,
            mosaic.height
        );
    }
}

use std::fmt::Write as _;
use std::net::TcpListener;
use std::thread;

use mosaic::Options;
use server::{Request, Response};

fn main() {
    let mut args = std::env::args().skip(1);
    // `lego-mosaic serve` (the default) runs the web app; `lego-mosaic
    // preview <in> <out> [palette] [size]` converts a file offline so
    // results can be inspected without a browser.
    if args.next().as_deref() == Some("preview") {
        let rest: Vec<String> = args.collect();
        preview::run(&rest);
        return;
    }
    serve();
}

fn serve() {
    let addr = std::env::var("LEGO_MOSAIC_ADDR").unwrap_or_else(|_| "127.0.0.1:3210".into());
    let listener = TcpListener::bind(&addr).unwrap_or_else(|error| {
        eprintln!("lego-mosaic: cannot bind {addr}: {error}");
        std::process::exit(1);
    });
    eprintln!("lego-mosaic: listening on http://{addr}");
    for stream in listener.incoming() {
        match stream {
            Ok(mut stream) => {
                thread::spawn(move || {
                    if let Ok(Some(request)) = server::read_request(&mut stream) {
                        let response = route(&request);
                        if let Err(error) = server::respond(&mut stream, &response) {
                            eprintln!("lego-mosaic: respond failed: {error}");
                        }
                    }
                });
            }
            Err(error) => eprintln!("lego-mosaic: accept failed: {error}"),
        }
    }
}

fn route(request: &Request) -> Response {
    match (request.method.as_str(), request.path.as_str()) {
        ("GET", "/" | "") => Response::html(200, ui::page()),
        ("GET", "/healthz") => Response::text(200, "ok\n"),
        ("POST", "/api/convert") => convert_handler(request),
        ("GET", _) => Response::text(404, "not found\n"),
        (_, _) => Response::text(405, "method not allowed\n"),
    }
}

/// Parse one form field as f64, falling back to a default.
fn field_f64(parts: &[multipart::Part], name: &str, default: f64) -> f64 {
    parts
        .iter()
        .find(|part| part.name == name)
        .and_then(|part| part.text().trim().parse::<f64>().ok())
        .filter(|value| value.is_finite())
        .unwrap_or(default)
}

fn convert_handler(request: &Request) -> Response {
    let content_type = request.content_type.as_deref().unwrap_or_default();
    let Some(parts) = multipart::parse(content_type, &request.body) else {
        return Response::json(400, r#"{"error": "expected multipart/form-data"}"#);
    };
    let Some(image_part) = parts.iter().find(|part| part.name == "image") else {
        return Response::json(400, r#"{"error": "missing image"}"#);
    };
    let image = match image::load_from_memory(&image_part.body) {
        Ok(image) => image.to_rgba8(),
        Err(error) => {
            return Response::json(
                400,
                format!(r#"{{"error": "cannot decode image: {error}"}}"#),
            )
        }
    };

    let palette_id =
        parts
            .iter()
            .find(|part| part.name == "palette")
            .map_or("mosaic-maker", |part| match part.text().as_str() {
                "monochrome" => "monochrome",
                "extended" => "extended",
                _ => "mosaic-maker",
            });
    let size = parts
        .iter()
        .find(|part| part.name == "size")
        .and_then(|part| part.text().trim().parse::<usize>().ok())
        .filter(|size| (8..=192).contains(size))
        .unwrap_or(48);
    let excluded: Vec<usize> = parts
        .iter()
        .filter(|part| part.name == "exclude")
        .filter_map(|part| part.text().trim().parse::<usize>().ok())
        .collect();
    let dither = parts
        .iter()
        .any(|part| part.name == "dither" && !part.body.is_empty());
    let options = Options {
        width: size,
        height: size,
        saturation: field_f64(&parts, "saturation", 1.0).clamp(0.5, 4.0),
        contrast: field_f64(&parts, "contrast", 1.0).clamp(0.5, 3.0),
        brightness: 0,
        order: mosaic::Order::from_str(
            &parts
                .iter()
                .find(|part| part.name == "order")
                .map_or("sharp".to_string(), |part| part.text().trim().to_string()),
        ),
        dither,
        white_background: parts
            .iter()
            .any(|part| part.name == "white_background" && !part.body.is_empty()),
        // The despeckle pass only fixes anti-aliasing stragglers; dithering
        // intentionally produces speckle, so never undo it.
        despeckle: !dither,
        excluded,
    };

    let mosaic = mosaic::convert(&image, palette::by_id(palette_id), &options);
    let svg = render::stud_svg(&mosaic, 20);
    let parts_list = render::parts_list(&mosaic);
    let rows = render::build_rows(&mosaic);
    Response::json(
        200,
        format!(
            r#"{{"svg": {}, "width": {}, "height": {}, "tiles": {}, "parts": [{}], "rows": [[{}]]}}"#,
            json_string(&svg),
            mosaic.width,
            mosaic.height,
            mosaic.grid.len(),
            parts_list
                .iter()
                .map(|part| format!(
                    r#"{{"name": {}, "hex": "{}", "count": {}}}"#,
                    json_string(&part.name),
                    part.hex,
                    part.count
                ))
                .collect::<Vec<_>>()
                .join(","),
            rows.iter()
                .map(|row| row
                    .iter()
                    .map(|(name, hex)| format!("[{}, \"{}\"]", json_string(name), hex))
                    .collect::<Vec<_>>()
                    .join(","))
                .collect::<Vec<_>>()
                .join("],[")
        ),
    )
}

/// Escape a string as a JSON string literal (no external serde: the
/// payloads are generated here, so hand-rolled escaping is enough).
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

    #[test]
    fn field_parsing_falls_back() {
        let parts = vec![
            multipart::Part {
                name: "saturation".into(),
                filename: None,
                body: b"1.5".to_vec(),
            },
            multipart::Part {
                name: "contrast".into(),
                filename: None,
                body: b"NaN".to_vec(),
            },
        ];
        assert!((field_f64(&parts, "saturation", 1.0) - 1.5).abs() < 1e-12);
        assert!((field_f64(&parts, "contrast", 1.0) - 1.0).abs() < 1e-12);
        assert!((field_f64(&parts, "missing", 2.0) - 2.0).abs() < 1e-12);
    }

    #[test]
    fn routes_resolve() {
        let get = |path: &str| Request {
            method: "GET".into(),
            path: path.into(),
            body: Vec::new(),
            content_type: None,
        };
        assert_eq!(route(&get("/")).status, 200);
        assert_eq!(route(&get("/healthz")).status, 200);
        assert_eq!(route(&get("/nope")).status, 404);
        assert_eq!(route(&get("/")).content_type, "text/html; charset=utf-8");
    }

    /// The real end-to-end path: multipart body in, JSON mosaic out.
    #[test]
    fn convert_handler_end_to_end() {
        let png: Vec<u8> = {
            let image = image::RgbaImage::from_pixel(64, 64, image::Rgba([180, 40, 40, 255]));
            let mut bytes = std::io::Cursor::new(Vec::new());
            image.write_to(&mut bytes, image::ImageFormat::Png).unwrap();
            bytes.into_inner()
        };
        let boundary = "BOUND7";
        let mut body = format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"image\"; \
             filename=\"x.png\"\r\nContent-Type: image/png\r\n\r\n"
        )
        .into_bytes();
        body.extend_from_slice(&png);
        body.extend_from_slice(
            format!(
                "\r\n--{boundary}\r\nContent-Disposition: form-data; \
                 name=\"palette\"\r\n\r\nmosaic-maker\r\n--{boundary}--\r\n"
            )
            .as_bytes(),
        );
        let request = Request {
            method: "POST".into(),
            path: "/api/convert".into(),
            body,
            content_type: Some(format!("multipart/form-data; boundary={boundary}")),
        };
        let response = route(&request);
        assert_eq!(response.status, 200);
        let text = String::from_utf8(response.body).unwrap();
        assert!(text.contains("\"width\": 48"));
        assert!(text.contains("\"tiles\": 2304"));
        assert!(text.contains("Red"));
        assert!(text.contains("<svg"));
    }

    #[test]
    fn convert_handler_rejects_garbage() {
        let request = Request {
            method: "POST".into(),
            path: "/api/convert".into(),
            body: b"not multipart".to_vec(),
            content_type: Some("application/json".into()),
        };
        assert_eq!(route(&request).status, 400);
    }
}

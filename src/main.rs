// Copyright (c) 2026 Witalis Domitrz <witekdomitrz@gmail.com>
// AGPL License

//! LEGO Mosaic Maker: a picture → round-plate mosaic web app. The whole
//! conversion runs in the browser: this binary only serves the page and
//! the wasm module (built from this same crate). No POST endpoints, no
//! multipart, no server-side conversion — the bot-web reverse proxy only
//! ever relays two GETs.
//!
//! Designed to sit behind the `/lego-mosaic` prefix, so paths here are
//! prefix-free.

mod server;
mod ui;

use std::net::TcpListener;
use std::thread;

/// The wasm module, built with
/// `cargo build --target wasm32-unknown-unknown --release` into `assets/`
/// (see `build-wasm.sh`) and committed, so the released binary is
/// self-contained.
const WASM: &[u8] = include_bytes!("../assets/lego_mosaic.wasm");

fn main() {
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

fn route(request: &server::Request) -> server::Response {
    match (request.method.as_str(), request.path.as_str()) {
        ("GET", "/" | "") => server::Response::html(200, ui::page()),
        ("GET", "/mosaic.wasm") => server::Response {
            status: 200,
            content_type: "application/wasm",
            body: WASM.to_vec(),
        },
        ("GET", "/healthz") => server::Response::text(200, "ok\n"),
        ("GET", _) => server::Response::text(404, "not found\n"),
        (_, _) => server::Response::text(405, "method not allowed\n"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn get(path: &str) -> server::Request {
        server::Request {
            method: "GET".into(),
            path: path.into(),
        }
    }

    #[test]
    fn routes_resolve() {
        assert_eq!(route(&get("/")).status, 200);
        assert_eq!(route(&get("/")).content_type, "text/html; charset=utf-8");
        assert_eq!(route(&get("/healthz")).status, 200);
        let wasm = route(&get("/mosaic.wasm"));
        assert_eq!(wasm.status, 200);
        assert_eq!(wasm.content_type, "application/wasm");
        // Real wasm modules start with the `\0asm` magic.
        assert_eq!(&wasm.body[..4], b"\0asm");
        assert_eq!(route(&get("/nope")).status, 404);
    }
}

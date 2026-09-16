// Copyright (c) 2026 Witalis Domitrz <witekdomitrz@gmail.com>
// AGPL License

//! LEGO Mosaic Studio: a static host for a Rust/WebAssembly browser app.
//! Images never leave the browser. Routes here are prefix-free; gateway
//! mounts the app at `/lego-mosaic/` and strips that prefix.

mod server;
mod ui;

use std::net::TcpListener;
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};
use std::thread;
use std::time::Duration;

struct Connection(Arc<AtomicUsize>);
impl Drop for Connection {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::Relaxed);
    }
}

// Generated together by build-wasm.sh; embedded for self-contained releases.
const WASM: &[u8] = include_bytes!("../assets/mosaic_bg.wasm");
const BINDINGS: &str = include_str!("../assets/mosaic.js");

fn main() {
    let addr = std::env::var("LEGO_MOSAIC_ADDR").unwrap_or_else(|_| "127.0.0.1:3210".into());
    let listener = TcpListener::bind(&addr).unwrap_or_else(|error| {
        eprintln!("lego-mosaic: cannot bind {addr}: {error}");
        std::process::exit(1);
    });
    eprintln!("lego-mosaic: listening on http://{addr}");
    let active = Arc::new(AtomicUsize::new(0));
    for stream in listener.incoming() {
        match stream {
            Ok(mut stream) => {
                if active.load(Ordering::Relaxed) >= 32 {
                    continue;
                }
                if stream
                    .set_read_timeout(Some(Duration::from_secs(10)))
                    .is_err()
                    || stream
                        .set_write_timeout(Some(Duration::from_secs(10)))
                        .is_err()
                {
                    continue;
                }
                active.fetch_add(1, Ordering::Relaxed);
                let guard = Connection(Arc::clone(&active));
                let _ = thread::Builder::new()
                    .name("mosaic-http".into())
                    .spawn(move || {
                        let _guard = guard;
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
        ("GET", "/mosaic_bg.wasm") => server::Response {
            status: 200,
            content_type: "application/wasm",
            body: WASM.to_vec(),
        },
        ("GET", "/mosaic.js") => server::Response {
            status: 200,
            content_type: "text/javascript; charset=utf-8",
            body: BINDINGS.as_bytes().to_vec(),
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
        let wasm = route(&get("/mosaic_bg.wasm"));
        assert_eq!(wasm.status, 200);
        assert_eq!(wasm.content_type, "application/wasm");
        // Real wasm modules start with the `\0asm` magic.
        assert_eq!(&wasm.body[..4], b"\0asm");
        let bindings = route(&get("/mosaic.js"));
        assert_eq!(bindings.status, 200);
        assert!(bindings.content_type.starts_with("text/javascript"));
        assert!(BINDINGS.contains("mosaic_bg.wasm"));
        assert_eq!(route(&get("/nope")).status, 404);
        assert_eq!(
            route(&server::Request {
                method: "POST".into(),
                path: "/".into()
            })
            .status,
            405
        );
    }
}

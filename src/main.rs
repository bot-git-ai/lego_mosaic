// Copyright (c) 2026 Witalis Domitrz <witekdomitrz@gmail.com>
// AGPL License

//! LEGO Mosaic Studio: a browser app. Images never leave the browser.
//! The default action — `dist` — writes the app shell to a directory as a
//! static, front-end-only PWA that any file host can publish. `serve` remains
//! only as a convenience for local development. There is no image API in
//! either, and nothing in the generated output needs this binary at runtime.

mod assets;
mod cli;
mod dist;
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

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    // The bare binary keeps serving: `serve` is the default action, and the
    // browser tests spawn the binary with no arguments and poll `/healthz`.
    // The static generator is the explicit `dist`.
    if !args.is_empty() && args[0] != "serve" {
        if args == ["--help"] || args == ["-h"] {
            println!("{}", cli::help());
            return;
        }
        let result = if args[0] == "convert" {
            cli::run(&args[1..])
        } else if args[0] == "dist" {
            dist_command(&args[1..])
        } else {
            Err(format!("Unknown command: {}\n{}", args[0], cli::help()))
        };
        if let Err(error) = result {
            eprintln!("lego-mosaic: {error}");
            std::process::exit(2);
        }
        return;
    }
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

/// `lego-mosaic dist [DIR]`: emit the static PWA. The default directory is
/// `dist` beside the current one, so the documented build line needs no
/// argument.
fn dist_command(args: &[String]) -> Result<(), String> {
    let mut directory = None;
    for arg in args {
        if arg == "--help" || arg == "-h" {
            println!("{}", cli::help());
            return Ok(());
        }
        if directory.replace(std::path::PathBuf::from(arg)).is_some() {
            return Err("dist takes a single output directory".into());
        }
    }
    let directory = directory.unwrap_or_else(|| std::path::PathBuf::from("dist"));
    let written = dist::write(&directory)?;
    // Name the files, so the output is self-describing in a build log.
    eprintln!(
        "lego-mosaic: wrote {} files to {} ({})",
        written.len(),
        directory.display(),
        written.join(" ")
    );
    Ok(())
}

fn route(request: &server::Request) -> server::Response {
    if request.method != "GET" {
        return server::Response::text(405, "method not allowed\n");
    }
    if request.path == "/healthz" {
        return server::Response::text(200, "ok\n");
    }
    // One asset list serves both front ends, so the host can never drift from
    // what `dist` writes. The host keeps its prefix-free routes; the static
    // build names the same bytes by file.
    match assets::find(&request.path) {
        Some(asset) => server::Response {
            status: 200,
            content_type: asset.content_type,
            body: asset.body.to_vec(),
        },
        None => server::Response::text(404, "not found\n"),
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
        assert!(String::from_utf8_lossy(&bindings.body).contains("mosaic_bg.wasm"));
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

    /// The host and the static build must publish identical bytes. A silent
    /// divergence would let `serve` and a deployed `dist` behave differently
    /// for the same release.
    #[test]
    fn every_served_asset_is_exactly_what_dist_writes() {
        for asset in assets::all() {
            let served = route(&get(asset.route));
            assert_eq!(served.status, 200, "{} is not served", asset.name);
            assert_eq!(
                served.body, asset.body,
                "{} differs between the host and dist",
                asset.name
            );
            assert_eq!(served.content_type, asset.content_type);
        }
        // A fresh install must get a versioned worker, not the template.
        let worker = route(&get("/service-worker.js"));
        assert!(!worker.body.windows(11).any(|w| w == b"__VERSION__"));
    }
}

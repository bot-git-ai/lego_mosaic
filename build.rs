// Copyright (c) 2026 Witalis Domitrz <witekdomitrz@gmail.com>
// AGPL License

//! Write the static app shell to `dist/` while the crate compiles.
//!
//! The studio is a browser application, so publishing it is a file copy, not a
//! program run. Seven of the eight shell files are committed as they are; the
//! other two are derived:
//!
//! * `service-worker.js` carries a `__VERSION__` placeholder standing for a
//!   cache name derived from the bytes of every *other* shell file. Deriving
//!   it here rather than per request is what makes the cache name change
//!   exactly when the shell does.
//! * `manifest.webmanifest` is plain data and lives as a file of its own.
//!
//! Both are written to `OUT_DIR` as well as `dist/`: `dist/` is what gets
//! published, and the copy under `OUT_DIR` is what the tests read, so nothing
//! has to depend on the top-level directory being present.
//!
//! `cargo build --release` therefore leaves a publishable site behind, and no
//! `cargo run` step exists.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};

/// The shell, in the order it is written: the version hash depends on it, and
/// a stable order keeps that hash stable for identical content.
const SHELL: &[(&str, &str)] = &[
    ("index.html", "src/ui.html"),
    ("mosaic.js", "assets/mosaic.js"),
    ("mosaic_bg.wasm", "assets/mosaic_bg.wasm"),
    ("worker.js", "src/worker.js"),
    ("icon-192.png", "assets/icon-192.png"),
    ("icon-512.png", "assets/icon-512.png"),
    ("manifest.webmanifest", "src/manifest.webmanifest"),
];

fn main() {
    let root = PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").unwrap());
    let out = PathBuf::from(std::env::var_os("OUT_DIR").unwrap());

    // Watch the SOURCE paths, not the output names: cargo compares these
    // against real files, so `service-worker.js` and `ui.html` have to be
    // named as the files they are. Watching the destination names watches
    // files that never change, and the script then never re-runs.
    for (_, source) in SHELL {
        println!("cargo:rerun-if-changed={source}");
    }
    println!("cargo:rerun-if-changed=src/service-worker.js");
    println!("cargo:rerun-if-changed=build.rs");

    let mut built = Vec::with_capacity(SHELL.len() + 1);
    for (name, source) in SHELL {
        let bytes = std::fs::read(root.join(source))
            .unwrap_or_else(|error| panic!("reading {source}: {error}"));
        built.push((*name, bytes));
    }

    // The worker's own template is hashed too, and it is deliberately kept
    // out of `built` so it is hashed exactly once, in template form. A change
    // to the caching logic must invalidate the cache: clients holding the old
    // worker would otherwise keep running stale logic against new assets.
    let template = std::fs::read_to_string(root.join("src/service-worker.js"))
        .unwrap_or_else(|error| panic!("reading src/service-worker.js: {error}"));
    let version = cache_version(&built, &template);
    let worker = template.replace("__VERSION__", &version);
    assert!(
        !worker.contains("__VERSION__"),
        "the service worker still contains the version placeholder"
    );

    built.push(("service-worker.js", worker.into_bytes()));

    // Both destinations, written the same way, so they cannot disagree.
    for dir in [root.join("dist"), out.join("dist")] {
        write_tree(&dir, &built);
    }
}

/// A cache name derived from the bytes of every shell file except the worker.
///
/// It deliberately covers the worker's own source: a change to the caching
/// logic must invalidate the cache too, or clients keep running the old logic
/// against new assets.
fn cache_version(built: &[(&str, Vec<u8>)], worker_template: &str) -> String {
    let mut hasher = DefaultHasher::new();
    for (name, bytes) in built {
        name.hash(&mut hasher);
        bytes.hash(&mut hasher);
    }
    worker_template.hash(&mut hasher);
    format!("{:x}", hasher.finish())
}

/// Write every file into `dir`, replacing the directory wholesale.
///
/// A partial tree would be worse than none: a dropped asset left beside a
/// newer one is exactly the stale shell a service worker cannot notice. The
/// swap is a directory rename, so a failure mid-write leaves the previous tree
/// untouched rather than half-overwritten.
fn write_tree(dir: &Path, built: &[(&str, Vec<u8>)]) {
    let staging = dir.with_extension("new");
    let _ = std::fs::remove_dir_all(&staging);
    std::fs::create_dir_all(&staging)
        .unwrap_or_else(|error| panic!("{}: {error}", staging.display()));

    for (name, bytes) in built {
        let path = staging.join(name);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("create shell directory");
        }
        std::fs::write(&path, bytes)
            .unwrap_or_else(|error| panic!("writing {}: {error}", path.display()));
    }

    let previous = dir.with_extension("old");
    let _ = std::fs::remove_dir_all(&previous);
    // `rename` onto an existing directory fails on Linux, so move the old one
    // aside first and only then put the new one in place.
    if dir.exists() {
        std::fs::rename(dir, &previous)
            .unwrap_or_else(|error| panic!("moving aside {}: {error}", dir.display()));
    }
    if let Err(error) = std::fs::rename(&staging, dir) {
        // Put the previous tree back rather than leaving nothing behind.
        let _ = std::fs::rename(&previous, dir);
        panic!("publishing {}: {error}", dir.display());
    }
    let _ = std::fs::remove_dir_all(&previous);
}

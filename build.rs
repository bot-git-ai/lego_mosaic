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
//! Everything is written to `dist/`, which is the whole site and the only copy.
//! An earlier version mirrored the files into `OUT_DIR` so the tests could read
//! them there, but that copy never held the wasm artefacts -- they belong to the
//! `wasm-bindgen` step, which writes only to `dist/`. The tests were therefore
//! asserting against a six-file subset that was not the site, and passed only
//! where a previous build had left the real files lying around. One directory,
//! one truth.
//!
//! `cargo build --release` therefore leaves a publishable site behind, and no
//! `cargo run` step exists.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};

/// The files this script owns, and the committed file each is copied from.
///
/// The two wasm artefacts are deliberately absent: they are written into
/// `dist/` by the `wasm-bindgen` step, which runs *after* a wasm build and
/// *before* this one. They are build output and are never committed, so they
/// have no committed source to copy from — and this script must not delete
/// them, because they are the studio itself.
const SHELL: &[(&str, &str)] = &[
    ("index.html", "src/ui.html"),
    ("worker.js", "src/worker.js"),
    ("icon-192.png", "assets/icon-192.png"),
    ("icon-512.png", "assets/icon-512.png"),
    ("manifest.webmanifest", "src/manifest.webmanifest"),
];

fn main() {
    let root = PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").unwrap());

    // The site is built only for the host target. This script also runs during
    // `cargo build --lib --target wasm32-unknown-unknown`, and at that moment
    // `assets/mosaic_bg.wasm` and `assets/mosaic.js` are the *output* of that
    // build: they do not exist yet, so writing the site there would fail on
    // the very step that produces them. The host build that follows the
    // wasm-bindgen pass is the one that publishes.
    let target = std::env::var("TARGET").unwrap_or_default();
    if target != "wasm32-unknown-unknown" && target.contains("wasm") {
        return;
    }
    if target.starts_with("wasm") {
        return;
    }

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

    write_tree(&root.join("dist"), &built);
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

/// Write every file this script owns into `dir`, in place.
///
/// An earlier version built a staging tree and renamed it over `dist/`, which
/// is right when the script owns the whole directory. It does not any more:
/// the wasm artefacts are written into `dist/` by the `wasm-bindgen` step,
/// which runs before this one, and a wholesale swap deleted them -- leaving a
/// publishable-looking `dist/` with no studio in it and no error. So only the
/// files named above are written, and the rest of the directory is untouched.
///
/// Each is written under a scratch name and renamed over its target, so a host
/// serving the directory never observes a half-written file.
fn write_tree(dir: &Path, built: &[(&str, Vec<u8>)]) {
    std::fs::create_dir_all(dir).unwrap_or_else(|error| panic!("{}: {error}", dir.display()));

    for (name, bytes) in built {
        let path = dir.join(name);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("create shell directory");
        }
        let scratch = dir.join(format!(".{name}.new"));
        std::fs::write(&scratch, bytes)
            .unwrap_or_else(|error| panic!("writing {}: {error}", scratch.display()));
        std::fs::rename(&scratch, &path)
            .unwrap_or_else(|error| panic!("publishing {}: {error}", path.display()));
    }
}

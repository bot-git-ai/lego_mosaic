// Copyright (c) 2026 Witalis Domitrz <witekdomitrz@gmail.com>
// AGPL License

//! Write the static app shell to `dist/` while the crate compiles.
//!
//! The studio is a browser application, so publishing it is a file copy, not a
//! program run. Four of the nine shell files are copied from committed
//! sources and three are derived here:
//!
//! * `icon-192.png` and `icon-512.png` are rasterized from `assets/icon.svg`,
//!   which is the committed source of truth for the icon and is never replaced
//!   by a PNG. The two sizes are the same drawing at two resolutions, so
//!   generating them is what keeps them from drifting into two different icons.
//! * `service-worker.js` carries a `__VERSION__` placeholder standing for a
//!   cache name derived from the bytes of every *other* shell file. Deriving
//!   it here rather than per request is what makes the cache name change
//!   exactly when the shell does.
//!
//! Everything is written to `dist/`, which is the whole site and the only copy.
//! An earlier version also mirrored the files into `OUT_DIR` for the tests, but
//! that copy never held the two wasm artefacts -- they belong to the
//! `wasm-bindgen` step, which writes only to `dist/` -- so the tests were
//! asserting against a subset that was not the site.
//!
//! `cargo build --release` therefore leaves a publishable site behind, and no
//! `cargo run` step exists.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};

/// The files this script owns, and the committed file each is copied from.
///
/// `icon.svg` is copied unchanged: the page links it, a browser falls back to
/// it, and the install PNGs are rasterized from it, so the site can never link
/// a different drawing than the one it shipped a PNG of.
///
/// The two wasm artefacts are deliberately absent: the `wasm-bindgen` step
/// writes them into `dist/` and they are never committed, so they have no
/// committed source to copy from — and this script must not delete them,
/// because they are the studio itself.
const SHELL: &[(&str, &str)] = &[
    ("index.html", "src/ui.html"),
    ("worker.js", "src/worker.js"),
    ("icon.svg", "assets/icon.svg"),
    ("manifest.webmanifest", "src/manifest.webmanifest"),
];

/// The icon sizes the manifest declares, and the only two PNGs the site has.
///
/// One list, so the manifest and the rasterized files cannot disagree about
/// what was built. `tests/shell.rs` reads the same two sizes out of this file
/// to check the committed manifest against it.
const ICON_SIZES: [u32; 2] = [192, 512];

fn main() {
    let root = PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").unwrap());

    // Build the site only for the host target: this script also runs during
    // `cargo build --lib --target wasm32-unknown-unknown`, where
    // `mosaic_bg.wasm` and `mosaic.js` are the *output* of that build and do
    // not exist yet. The host build after the wasm-bindgen pass is the one that
    // publishes.
    let target = std::env::var("TARGET").unwrap_or_default();
    if target != "wasm32-unknown-unknown" && target.contains("wasm") {
        return;
    }
    if target.starts_with("wasm") {
        return;
    }

    // Watch the SOURCE paths, not the output names: cargo compares these
    // against real files, so `ui.html` and `service-worker.js` must be named
    // as the files they are. Watching the destination names watches files that
    // never change, and the script then never re-runs.
    for (_, source) in SHELL {
        println!("cargo:rerun-if-changed={source}");
    }
    println!("cargo:rerun-if-changed=src/service-worker.js");
    println!("cargo:rerun-if-changed=build.rs");

    let mut built = Vec::with_capacity(SHELL.len() + ICON_SIZES.len() + 1);
    for (name, source) in SHELL {
        let bytes = std::fs::read(root.join(source))
            .unwrap_or_else(|error| panic!("reading {source}: {error}"));
        built.push((*name, bytes));
    }
    built.extend(rasterize_icons(&root));

    // Hash the worker's own template in template form, keeping it out of
    // `built` so its version placeholder is not hashed twice.
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

/// Rasterize `assets/icon.svg` at each install size.
///
/// These PNGs are build output derived from `assets/icon.svg`, which is why
/// neither exists in the tree between builds. Committing them was what let the
/// two sizes drift into two different icons — a hand-made pair of PNGs is two
/// hand-made icons, and only one of them is ever looked at.
///
/// Renders straight to each size rather than rasterizing at 512 and
/// downscaling: the source is a 512-unit viewBox, so a 192 render and a 512
/// render differ only in output resolution and both are exact.
fn rasterize_icons(root: &Path) -> Vec<(&'static str, Vec<u8>)> {
    let svg = std::fs::read(root.join("assets/icon.svg"))
        .unwrap_or_else(|error| panic!("reading assets/icon.svg: {error}"));
    let options = usvg::Options::default();
    let tree = usvg::Tree::from_data(&svg, &options)
        .unwrap_or_else(|error| panic!("parsing assets/icon.svg: {error}"));

    let mut icons = Vec::with_capacity(ICON_SIZES.len());
    // `resvg` takes a `Transform`, whose fields are f32, so the scale factor
    // has to end up as one. The width is already an f32 from `usvg`; the size
    // is brought across through a `u16` because that widening is exact at
    // every value f32 can hold, so no lint has to be silenced to say so.
    let source_width = tree.size().width();
    for size in ICON_SIZES {
        let mut pixmap = tiny_skia::Pixmap::new(size, size)
            .unwrap_or_else(|| panic!("a {size}x{size} pixmap: out of memory"));
        let exact = u16::try_from(size)
            .unwrap_or_else(|_| panic!("icon-{size}.png: sizes above 65535 make no sense"));
        let scale = f32::from(exact) / source_width;
        let transform = tiny_skia::Transform::from_scale(scale, scale);
        resvg::render(&tree, transform, &mut pixmap.as_mut());
        let png = pixmap
            .encode_png()
            .unwrap_or_else(|error| panic!("encoding icon-{size}.png: {error}"));
        // A blank install icon is only ever noticed on a home screen, so fail
        // the build instead.
        assert!(
            !png.is_empty() && png.len() > 100,
            "icon-{size}.png rasterized to nothing; assets/icon.svg is empty or unrenderable"
        );
        icons.push((leak(format!("icon-{size}.png")), png));
    }
    icons
}

/// Hand a formatted name out with a `'static` lifetime.
///
/// The icons are built once and only borrowed for the length of `write_tree`,
/// so this keeps `built` a flat list of pairs instead of threading an
/// owned-name type through three functions.
fn leak(name: String) -> &'static str {
    Box::leak(name.into_boxed_str())
}

/// A cache name derived from the bytes of every shell file, plus the worker's
/// own template.
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

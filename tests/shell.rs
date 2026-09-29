// Copyright (c) 2026 Witalis Domitrz <witekdomitrz@gmail.com>
// AGPL License

//! Invariants of the generated static shell.
//!
//! The shell is the product, and it is written by `build.rs` rather than by
//! this crate, so nothing else in the test suite would notice if it drifted:
//! the binary no longer serves it, so these assertions are the only thing
//! standing between a broken `index.html` and a published site.
//!
//! They read the tree `build.rs` wrote, which is also the tree that gets
//! published, so what is asserted here is exactly what ships.

use std::path::{Path, PathBuf};

/// The generated shell. `build.rs` writes the same bytes to `OUT_DIR`, so this
/// resolves wherever the build ran from.
fn shell() -> PathBuf {
    Path::new(env!("OUT_DIR")).join("dist")
}

fn read(name: &str) -> String {
    let path = shell().join(name);
    std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("reading {}: {error}", path.display()))
}

fn index() -> String {
    read("index.html")
}

/// The shell is the app, and the app is wasm. A hand-written ABI would mean the
/// conversion no longer shares the library the CLI uses, which is the property
/// the parity test exists to protect.
#[test]
fn the_page_loads_generated_bindings_not_a_manual_wasm_abi() {
    let page = index();
    assert!(
        page.contains("<!DOCTYPE html>"),
        "the shell must be a document"
    );
    assert!(page.contains("<script type=\"module\">"), "a module script");
    assert!(
        page.contains("import('./mosaic.js')"),
        "the generated bindings"
    );
    assert_eq!(
        page.matches("<script").count(),
        1,
        "exactly one script:\n{page}"
    );
    for obsolete in [
        "instantiateStreaming",
        "alloc_buf",
        "wasm.exports",
        "api/convert",
        "fetch(",
    ] {
        assert!(!page.contains(obsolete), "{obsolete} in the static shell");
    }
}

/// The controls have to exist and be labelled: this is an accessible browser
/// app, not a canvas demo.
#[test]
fn the_page_has_accessible_local_input_and_build_outputs() {
    let page = index();
    for id in [
        "image",
        "drop",
        "original",
        "mosaic-image",
        "status",
        "error",
        "exclusions",
        "parts",
        "rows",
        "export-svg",
        "export-csv",
        "print",
        "zoom",
        "reset",
    ] {
        assert!(page.contains(&format!("id=\"{id}\"")), "missing #{id}");
    }
    assert!(
        page.contains("<label") || page.contains("aria-label"),
        "controls must be labelled"
    );
    assert!(
        page.contains("local only") || page.contains("local") && page.contains("browser"),
        "the page must tell the user images stay on the machine"
    );
    assert!(
        page.contains("never leaves"),
        "the page must state plainly that images are not uploaded"
    );
}

/// Every file the shell references has to be in the tree, or the published
/// site 404s on load. This is the check that actually catches a dropped asset.
#[test]
fn every_referenced_file_is_generated() {
    for name in [
        "mosaic.js",
        "mosaic_bg.wasm",
        "worker.js",
        "service-worker.js",
        "manifest.webmanifest",
        "icon-192.png",
        "icon-512.png",
    ] {
        let path = shell().join(name);
        assert!(path.is_file(), "{name} missing from the generated shell");
        let size = std::fs::metadata(&path).expect("stat").len();
        assert!(size > 0, "{name} is empty");
    }
    assert!(
        index().contains("manifest.webmanifest"),
        "the page registers a manifest"
    );
}

/// `dist/` is build output and must never be committed.
///
/// This asks git about the repository rather than about the generated tree,
/// because the release gate exports the candidate as a bare directory that is
/// not a checkout: `git check-ignore` there has no `.gitignore` to consult and
/// reports failure for a reason that has nothing to do with this invariant.
/// Ignorability is a property of the repository; what is being published is a
/// property of the tree, asserted by the tests above.
#[test]
fn dist_is_never_committed_to_the_repository() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let inside_a_checkout = std::process::Command::new("git")
        .args(["rev-parse", "--git-dir"])
        .current_dir(root)
        .output()
        .map(|out| out.status.success())
        .unwrap_or(false);
    if !inside_a_checkout {
        // The release gate exports the candidate as a bare directory with no
        // `.git`, so there is no index and no ignore file to ask about.
        return;
    }
    let ignored = std::process::Command::new("git")
        .args(["check-ignore", "-q", "dist/"])
        .current_dir(root)
        .status()
        .expect("git check-ignore")
        .success();
    assert!(ignored, "dist/ must be in .gitignore");

    let tracked = std::process::Command::new("git")
        .args(["ls-files", "dist/"])
        .current_dir(root)
        .output()
        .expect("git ls-files");
    assert!(
        tracked.stdout.is_empty(),
        "dist/ is tracked:\n{}",
        String::from_utf8_lossy(&tracked.stdout)
    );
}

/// The service worker's cache name must be pinned to a real version, or every
/// client keeps serving a stale shell forever.
#[test]
fn the_service_worker_cache_is_pinned_to_a_content_version() {
    let worker = read("service-worker.js");
    assert!(
        !worker.contains("__VERSION__"),
        "the version placeholder was never substituted"
    );
    let template = std::fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("src/service-worker.js"),
    )
    .expect("the committed worker template");
    assert!(
        template.contains("__VERSION__"),
        "the committed template should keep the placeholder, not a stale version"
    );
    assert!(
        worker.len() != template.len(),
        "the generated worker must differ from the template"
    );
}

/// Relative URLs only: the site is mounted under an arbitrary prefix, which is
/// what lets the same build be published to GitHub Pages or a subdirectory.
#[test]
fn the_shell_is_mountable_anywhere() {
    let page = index();
    assert!(
        !page.contains("http://") && !page.contains("https://"),
        "an absolute URL would break the site outside its own origin"
    );
    assert!(
        page.contains("./mosaic.js"),
        "bindings must be referenced relatively"
    );
    let worker = read("service-worker.js");
    assert!(
        worker.contains("new URL('./', self.location.href)"),
        "the worker must resolve its cache from its own location"
    );
    let manifest: serde_json::Value =
        serde_json::from_str(&read("manifest.webmanifest")).expect("the manifest is valid JSON");
    assert_eq!(manifest["start_url"], "./", "start_url must be relative");
    assert_eq!(manifest["scope"], "./", "scope must be relative");
    assert_eq!(manifest["id"], "./", "id must be relative");
}

/// Publishing is a copy, so the wasm the page loads must be the committed
/// artifact rather than something rebuilt from a different toolchain.
#[test]
fn the_generated_wasm_is_the_committed_artifact() {
    let generated = std::fs::read(shell().join("mosaic_bg.wasm")).expect("generated wasm");
    let committed =
        std::fs::read(Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/mosaic_bg.wasm"))
            .expect("committed wasm");
    assert_eq!(
        generated, committed,
        "the shipped wasm must be the committed one, byte for byte"
    );
}

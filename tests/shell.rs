// Copyright (c) 2026 Witalis Domitrz <witekdomitrz@gmail.com>
// AGPL License

//! Invariants of the app shell source, and of what is and is not committed.
//!
//! Everything here reads committed files. `dist/` is build output and is
//! gitignored, so the release gate — which exports the candidate tree — never
//! has it, and cannot build it either: that needs the wasm target and a pinned
//! `wasm-bindgen` CLI. A test asserting on `dist/` would therefore run only in
//! a developer's checkout, which is exactly where it is least likely to catch
//! anything, so those assertions are gone rather than skipped.
//!
//! What covers the built output is running the two build steps, in the order
//! AGENTS.md gives them. A test on a leftover directory cannot do that job.
//!
//! One thing these tests do keep: the property that a committed build never
//! rots. That was the reason the wasm artefacts were committed, and it is the
//! failure mode most worth a test here — a stale `assets/mosaic_bg.wasm` ships
//! a converter that predates the code that would produce it, silently.

use std::path::Path;

/// The app shell, as committed.
///
/// `build.rs` copies this into `dist/index.html` byte for byte, so asserting
/// on it asserts on exactly what gets published.
fn shell() -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/ui.html");
    std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("reading {}: {error}", path.display()))
}

/// The service worker template, as committed.
///
/// `build.rs` hashes this file's bytes into the cache name and writes the
/// result into `dist/`, so asserting on it asserts on exactly what gets
/// published.
fn worker_template() -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/service-worker.js");
    std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("reading {}: {error}", path.display()))
}

/// Tracked file names, or `None` outside a checkout.
///
/// The release gate exports the candidate as a bare directory with no `.git`,
/// so there is no index to ask. Callers decide what that means.
fn tracked_files() -> Option<String> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let inside = std::process::Command::new("git")
        .args(["rev-parse", "--git-dir"])
        .current_dir(root)
        .output()
        .map(|out| out.status.success())
        .unwrap_or(false);
    if !inside {
        return None;
    }
    let output = std::process::Command::new("git")
        .args(["ls-files"])
        .current_dir(root)
        .output()
        .expect("git ls-files");
    Some(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// The shell is the app, and the app is wasm. A hand-written ABI would mean the
/// conversion no longer shares the library the CLI uses — which is the
/// property the CLI/browser parity checks used to protect.
#[test]
fn the_page_loads_generated_bindings_not_a_manual_wasm_abi() {
    let page = shell();
    assert!(
        page.contains("<!DOCTYPE html>"),
        "the shell must be a document"
    );
    assert!(page.contains("<script type=\"module\">"), "a module script");
    assert!(
        page.contains("import('./mosaic.js')"),
        "the page must load the generated bindings"
    );
    assert_eq!(
        page.matches("<script").count(),
        1,
        "exactly one script tag:\n{page}"
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

/// An accessible browser app, not a canvas demo: the controls have to exist and
/// be labelled, and the page has to say plainly that images stay local.
#[test]
fn the_page_has_accessible_local_input_and_build_outputs() {
    let page = shell();
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
        page.contains("never leaves"),
        "the page must state that images are not uploaded"
    );
}

/// The site is mounted under an arbitrary prefix, so every URL in it is
/// relative. One build, any subdirectory.
#[test]
fn the_shell_is_mountable_anywhere() {
    let page = shell();
    assert!(
        !page.contains("http://") && !page.contains("https://"),
        "an absolute URL would break the site outside its own origin"
    );
    assert!(
        page.contains("./mosaic.js"),
        "bindings must be referenced relatively"
    );
    assert!(
        page.contains("manifest.webmanifest"),
        "the page must register a manifest"
    );
    let manifest: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(
            Path::new(env!("CARGO_MANIFEST_DIR")).join("src/manifest.webmanifest"),
        )
        .expect("the committed manifest"),
    )
    .expect("the manifest is valid JSON");
    for key in ["start_url", "scope", "id"] {
        assert_eq!(manifest[key], "./", "{key} must be relative");
    }
}

/// The service worker is a committed template with exactly one placeholder, and
/// `build.rs` substitutes it. A template with no placeholder would mean the
/// cache never invalidates; a second one would mean the substitution is not
/// the only edit.
#[test]
fn the_service_worker_template_has_exactly_one_placeholder() {
    let worker = worker_template();
    assert_eq!(
        worker.matches("__VERSION__").count(),
        1,
        "the template must carry exactly one version placeholder"
    );
    assert!(
        worker.contains("new URL('./', self.location.href)"),
        "the worker must resolve its cache from its own location"
    );
}

/// Nothing generated may be tracked — not the wasm, not the bindings, not the
/// site. This is the test that would have caught them being committed.
#[test]
fn no_build_artifact_is_committed() {
    let Some(tracked) = tracked_files() else {
        return; // not a checkout: the gate's exported tree
    };
    for artefact in [
        "assets/mosaic.js",
        "assets/mosaic_bg.wasm",
        "dist/index.html",
        "dist/mosaic.js",
        "dist/mosaic_bg.wasm",
    ] {
        assert!(
            !tracked.lines().any(|line| line == artefact),
            "{artefact} is tracked; generated artefacts must never be committed"
        );
    }
}

/// `dist/` has to be ignored, or a build would leave the next commit dirty.
#[test]
fn dist_is_ignored() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    if tracked_files().is_none() {
        return; // not a checkout
    }
    let ignored = std::process::Command::new("git")
        .args(["check-ignore", "-q", "dist/"])
        .current_dir(root)
        .status()
        .expect("git check-ignore")
        .success();
    assert!(ignored, "dist/ must be in .gitignore");
}

/// The worker only ever answers for a URL inside its own app's directory.
///
/// This is the guard that stops one of these apps from taking over the pages it
/// shares an origin with. A service worker registered for a scope is consulted
/// for every URL under that scope, and every app here is served from the same
/// origin as pages that are not apps at all — so "the scope is small" is a
/// promise, and this test is what keeps it one.
#[test]
fn the_worker_never_answers_outside_its_own_directory() {
    let worker = worker_template();
    assert!(
        worker.contains("new URL('./', self.location.href)"),
        "the worker must resolve its own directory from its location"
    );
    // The guard is a prefix test against that directory, on the request URL,
    // applied before the allowlist decides anything.
    assert!(
        worker.contains("IS_OWN(url)"),
        "the fetch handler must check the request is inside this app's directory; \
         without it a mis-scoped registration serves whatever it cached"
    );
    assert!(
        worker.contains("const IS_OWN = url => url.startsWith(ROOT.href)"),
        "the directory guard must be a prefix test against the worker's own root"
    );
}

/// The page states the worker's scope instead of inheriting it, and cleans up a
/// wider registration left behind by an earlier version.
///
/// A registration outlives the page that created it, and nothing short of an
/// explicit `unregister` takes one away. So the second half is what makes this
/// recoverable without the user clearing their browser: a stale registration
/// is not fixed by a reload, and the newer worker cannot take control of a
/// scope it does not own.
#[test]
fn the_page_states_the_scope_and_releases_a_wider_one() {
    let source = std::fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("src/browser.rs"),
    )
    .expect("reading src/browser.rs");
    // Comments go first. The prose in `src/browser.rs` names these calls while
    // explaining them, so an assertion over raw text can be satisfied by the
    // explanation while the call it is about is gone: green, and proving
    // nothing. A test about what the code does has to read the code.
    let browser = strip_rust_comments(&source);
    assert!(
        browser.contains("register_with_options"),
        "the worker must be registered with an explicit scope; left to default, \
         the scope is whatever directory the registering page sits in, which on \
         a shared origin is every other page's problem too"
    );
    assert!(
        browser.contains("RegistrationOptions::new()") && browser.contains("set_scope(SCOPE)"),
        "the scope has to be actually stated, not merely a named constant"
    );
    assert!(
        browser.contains("get_registrations") && browser.contains("unregister"),
        "a stale wider registration survives a reload, a version bump and a \
         reinstall; only an explicit unregister clears it"
    );
}

/// A worker's script is compared by suffix, not by `trim_end_matches`.
///
/// `trim_end_matches` strips a *set of characters*, so a directory whose name
/// ends in those letters is silently treated as ours — and a registration
/// belonging to a sibling app would be torn down. This is a regression test for
/// a real bug in the first version of this code.
#[test]
fn the_script_comparison_strips_a_suffix_rather_than_a_character_set() {
    let source = std::fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("src/browser.rs"),
    )
    .expect("reading src/browser.rs");
    // Comments go first, for the reason above: the prose names the method to
    // explain why it is not used, so the assertion is about code, not a word.
    let browser = strip_rust_comments(&source);
    let calls: Vec<&str> = browser
        .lines()
        .filter(|line| {
            let code = line.split("//").next().unwrap_or(line);
            code.contains("trim_end_matches(")
        })
        .collect();
    assert!(
        calls.is_empty(),
        "`trim_end_matches` strips a character set, not a filename: it would eat \
         any directory ending in those letters and tear down a sibling's worker. \
         Found: {calls:?}"
    );
    assert!(
        browser.contains("strip_suffix(\"service-worker.js\")"),
        "the comparison must strip the one filename it expects"
    );
}

/// The scope is named once, and the page and the worker agree on the directory.
///
/// Two independent resolutions of "where am I" — the page's `./` and the
/// worker's `new URL('./', self.location.href)`. They have to describe the same
/// directory, or the page registers a scope the worker's guard does not match.
#[test]
fn the_scope_is_a_relative_directory_shared_with_the_worker() {
    let source = std::fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("src/browser.rs"),
    )
    .expect("reading src/browser.rs");
    let browser = strip_rust_comments(&source);
    assert!(
        browser.contains("const SCOPE: &str = \"./\";"),
        "the scope must be the app's own directory, relative — so one build works \
         from any subdirectory"
    );
    assert!(
        worker_template().contains("new URL('./', self.location.href)"),
        "the worker must resolve the same directory the page registered"
    );
}

/// Drop `//` line comments and `/* ... */` blocks, so an assertion about what
/// the code *does* cannot be satisfied by a comment saying what it does.
fn strip_rust_comments(source: &str) -> String {
    let mut out = String::with_capacity(source.len());
    let mut rest = source;
    // A line comment runs to the end of the line, and a block comment to its
    // closer. A `//` inside a string literal is not a comment, but none of the
    // strings these tests look for contain one, and a full Rust tokenizer is
    // not worth the complexity to guard against it.
    while let Some(start) = rest.find('/') {
        let after = &rest[start + 1..];
        if let Some(tail) = after.strip_prefix("//") {
            out.push_str(&rest[..start]);
            rest = tail.split_once('\n').map_or("", |(_, line)| line);
        } else if let Some(tail) = after.strip_prefix("/*") {
            out.push_str(&rest[..start]);
            rest = tail.split_once("*/").map_or("", |(_, line)| line);
        } else {
            out.push_str(&rest[..=start]);
            rest = after;
        }
    }
    out.push_str(rest);
    out
}

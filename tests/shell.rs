// Copyright (c) 2026 Witalis Domitrz <witekdomitrz@gmail.com>
// AGPL License

//! Invariants of the app shell source, of what is and is not committed, and of
//! the workflow that publishes the site.
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
//!
//! The publishing workflow is asserted here too, and for the same reason
//! `dist/` is not: the file that can overwrite the live site is a hundred lines
//! of text that nothing else in the repository exercises, and its
//! silent-by-construction failures (a variable that expands to nothing, an
//! artifact named on one side only, a line commented out rather than deleted)
//! are precisely the ones a green CI run cannot report. Read with the comments
//! stripped, or a `#` line that explains a setting satisfies the assertion
//! that the setting is there.
//!
//! This is the only place a YAML parser is reachable, and it is a test-only
//! dependency: nothing here needs one, because a source-text invariant can be
//! checked by reading the line. See `the_pages_workflow_is_the_only_thing_that
//! _can_publish`.

use std::path::Path;

/// The repository root.
fn root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
}

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
/// site, and not the icons. This is the test that would have caught them being
/// committed.
///
/// The two PNGs are the sharpest case. They used to be committed, and because
/// nothing generated them they were two hand-drawn icons rather than one
/// drawing at two resolutions: the 192 was a different pitch and a different
/// radius from the 512, so the app showed a different icon on an iPhone than
/// on an Android home screen. `assets/icon.svg` is now the icon, and these are
/// its build output.
#[test]
fn no_build_artifact_is_committed() {
    let Some(tracked) = tracked_files() else {
        return; // not a checkout: the gate's exported tree
    };
    for artefact in [
        "assets/mosaic.js",
        "assets/mosaic_bg.wasm",
        "assets/icon-192.png",
        "assets/icon-512.png",
        "dist/index.html",
        "dist/mosaic.js",
        "dist/mosaic_bg.wasm",
    ] {
        assert!(
            !tracked.lines().any(|line| line == artefact),
            "{artefact} is tracked; generated artefacts must never be committed"
        );
    }
    assert!(
        tracked.lines().any(|line| line == "assets/icon.svg"),
        "the icon must have exactly one committed source of truth: assets/icon.svg"
    );
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
    let source =
        std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("src/browser.rs"))
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
    let source =
        std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("src/browser.rs"))
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
    let source =
        std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("src/browser.rs"))
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

/// The icon has exactly one source of truth, and the page and the worker both
/// reach for the file that is it.
///
/// A browser tab and an installed app get their icons by three different
/// routes — the page's `<link rel="icon">`, the manifest, and the worker's
/// precache — and each can be given a different answer. `icon.svg` is the
/// drawing; the PNGs are rasterized from it during the build. If the page ever
/// links a PNG while the manifest declares another, the installed icon and the
/// tab icon drift apart, and nothing in a static site notices.
#[test]
fn the_svg_is_the_icon_and_the_page_links_it() {
    let icon =
        std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/icon.svg"))
            .expect("reading assets/icon.svg");
    assert!(
        icon.contains("<svg"),
        "assets/icon.svg must be an SVG, not a renamed PNG"
    );
    assert!(
        icon.contains("viewBox"),
        "an icon with no viewBox cannot be scaled to two sizes, which is the \
         entire reason the PNGs are generated"
    );

    let page = shell();
    assert!(
        page.contains("href=\"./icon.svg\"") && page.contains("image/svg+xml"),
        "the page must link the SVG as its icon"
    );
    // Relative, like every other URL in the shell: one build, any subdirectory.
    assert!(
        !page.contains("href=\"/icon.svg\""),
        "an absolute icon URL breaks the site outside its own origin"
    );
    assert!(
        worker_template().contains("'icon.svg'"),
        "the worker must precache the SVG it publishes; an uncached icon is a \
         blank tab icon for every offline launch"
    );
}

/// The manifest declares exactly the icons `build.rs` rasterizes.
///
/// The list is written twice — once in `build.rs` and once in the committed
/// manifest — so the two can disagree, and a manifest promising an icon that
/// was never built is a manifest that fails only on a real home screen. This
/// reads the sizes straight out of `build.rs` rather than restating them, so
/// adding a size to the build is a change this test follows.
#[test]
fn the_manifest_declares_exactly_the_icons_the_build_rasterizes() {
    let build = std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("build.rs"))
        .expect("reading build.rs");
    let sizes: Vec<u32> = build
        .lines()
        .find_map(|line| line.trim().strip_prefix("const ICON_SIZES: [u32; 2] = ["))
        .and_then(|rest| rest.strip_suffix("];"))
        .expect("build.rs must declare ICON_SIZES, the list of rasterized sizes")
        .split(',')
        .filter_map(|size| size.trim().parse().ok())
        .collect();
    assert!(
        !sizes.is_empty(),
        "ICON_SIZES must list the sizes to rasterize"
    );

    let manifest: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(
            Path::new(env!("CARGO_MANIFEST_DIR")).join("src/manifest.webmanifest"),
        )
        .expect("the committed manifest"),
    )
    .expect("the manifest is valid JSON");
    let declared: Vec<String> = manifest["icons"]
        .as_array()
        .expect("the manifest must declare an icons array")
        .iter()
        .map(|icon| {
            assert_eq!(
                icon["type"], "image/png",
                "only the generated install sizes belong in the manifest"
            );
            icon["sizes"]
                .as_str()
                .expect("an icon needs sizes")
                .to_owned()
        })
        .collect();

    let expected: Vec<String> = sizes.iter().map(|s| format!("{s}x{s}")).collect();
    assert_eq!(
        declared, expected,
        "the manifest must declare exactly the sizes build.rs rasterizes"
    );
    // And the page's apple-touch-icon must be one of them, at a size iOS will
    // not upscale into mush.
    assert!(
        shell().contains("href=\"./icon-192.png\""),
        "the apple-touch-icon must be the 192 install icon"
    );
}

/// The icon drawing, and only the icon drawing, is committed as source.
///
/// `assets/icon.svg` is hand-editable, so the failure worth guarding is the
/// whole design being duplicated: once a PNG is committed alongside it, the
/// next person to change the icon edits whichever file they find first, and
/// the site quietly ships the old drawing. An empty or unparseable SVG is the
/// other way this breaks, and it breaks as a blank home-screen tile.
#[test]
fn the_committed_icon_is_a_real_drawing() {
    let icon =
        std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/icon.svg"))
            .expect("reading assets/icon.svg");
    for (what, needle) in [("studs", "fill=\"#f4cb46\""), ("a viewBox", "viewBox=")] {
        assert!(icon.contains(needle), "the icon must keep {what}: {needle}");
    }
    // No backdrop. The launcher already has a colour — its own, or the one the
    // user picked — and the icon is the nine studs alone, so it shows on it
    // rather than carrying a rectangle the user never asked for. Guarded here
    // because it is the one edit that makes the icon worse while looking
    // harmless: a background rect renders fine, on a home screen, until the
    // day someone whose launcher is dark notices the dark square.
    assert!(
        !icon.contains("<rect"),
        "the icon must stay the nine studs alone, with no background rect: the \
         place it is shown supplies the colour behind it"
    );
    // Three by three, because a mosaic that is nine studs is the app's whole
    // idea, and a dropped circle is invisible in review but obvious on a
    // home screen.
    assert_eq!(
        icon.matches("<circle").count(),
        9,
        "the icon is a 3x3 grid of studs; found {} circles",
        icon.matches("<circle").count()
    );
}

/// A workflow file, as committed.
///
/// Nonexistent is not a reason to fail. A partial export of the tree — which is
/// what a consumer gets who takes a snapshot rather than a clone — has no
/// `.github` at all, and a test that hard-failed on its absence would make that
/// red for a reason that says nothing about the studio. A workflow that is
/// present but broken fails its own assertions, loudly.
fn workflow(name: &str) -> Option<String> {
    std::fs::read_to_string(root().join(".github/workflows").join(name)).ok()
}

/// The workflow with its comments removed.
///
/// A `#` inside a quoted string is not a comment, and a `#` in a value is not
/// one either — but neither workflow quotes anything in the keys these tests
/// read, and neither carries a `#` in a value they depend on, so a line-wise cut
/// at the first `#` is enough and a YAML parser is not worth a dependency.
fn strip_yaml_comments(source: &str) -> String {
    source
        .lines()
        .map(|line| match line.find('#') {
            Some(index) => &line[..index],
            None => line,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// The workflow's shell with line continuations folded.
///
/// `wasm-bindgen … \` followed by an indented `target/…wasm` is one command to
/// the shell, so it is one line here. Without folding, a check that the
/// generator is handed the library's artefact cannot see the argument, because
/// the argument is on the next line.
fn fold_continuations(source: &str) -> String {
    let mut folded = String::with_capacity(source.len());
    for line in source.lines() {
        if let Some(head) = line.strip_suffix('\\') {
            folded.push_str(head);
            folded.push(' ');
        } else {
            folded.push_str(line);
            folded.push('\n');
        }
    }
    folded
}

/// The exact `wasm-bindgen` requirement `Cargo.toml` states, or `None`.
///
/// Read the way Cargo writes it: `wasm-bindgen = "=0.2.128"` is an *exact*
/// requirement. A looser form ("0.2.128", "^0.2") resolves to whatever is newest
/// in the lockfile, and a workflow's literal would then be naming one arbitrary
/// version of several, so the loose form is not accepted — it fails here rather
/// than quietly producing two generators.
fn pinned_wasm_bindgen() -> Option<String> {
    let manifest = std::fs::read_to_string(root().join("Cargo.toml")).ok()?;
    let requirement = manifest.lines().find_map(|line| {
        let rest = line.trim().strip_prefix("wasm-bindgen")?;
        let rest = rest.trim_start().strip_prefix('=')?;
        Some(rest.trim().trim_matches('"').to_owned())
    })?;
    requirement.starts_with('=').then_some(requirement)
}

/// The studio is named `mosaic` everywhere the site is concerned, and the whole
/// deploy turns on that: the bindings are generated as `mosaic.js` and
/// `mosaic_bg.wasm`, and `src/ui.html` loads `./mosaic.js` with a dynamic
/// import.
///
/// A sibling repo's workflow names the same pair `app`, and a copy that kept the
/// name would generate a module nothing loads — a site that renders perfectly
/// and never starts, with the only symptom being the catch handler's "Studio
/// could not start" message. So the agreement is asserted on both sides, and
/// against the committed page rather than a remembered string.
#[test]
fn the_pages_workflow_builds_the_bindings_under_the_name_the_page_imports() {
    let Some(pages) = workflow("pages.yml") else {
        return;
    };
    let live = fold_continuations(&strip_yaml_comments(&pages));

    let bindings = live
        .lines()
        .find(|line| line.trim_start().starts_with("wasm-bindgen "))
        .expect("pages.yml must run wasm-bindgen");
    assert!(
        bindings.contains("--out-name mosaic"),
        "the generator must be invoked as --out-name mosaic, or the site ships bindings under a \
         name the page does not import: {bindings:?}"
    );
    // `dist/` and the generator's own target argument, so a change to either is
    // a visible diff rather than a build that writes somewhere nobody looks.
    assert!(
        bindings.contains("--out-dir dist"),
        "the bindings belong in the published directory: {bindings:?}"
    );
    assert!(
        bindings.contains("target/wasm32-unknown-unknown/release/lego_mosaic.wasm"),
        "the generator must read the lib's wasm: the crate's package name is `lego-mosaic`, and \
         its library artefact is `lego_mosaic.wasm`. A sibling repo's path, or the `[[bin]]` \
         target's own artefact, names a file this build never produces: {bindings:?}"
    );

    // And the page must import exactly that module.
    assert!(
        shell().contains("import('./mosaic.js')"),
        "src/ui.html must import the generated bindings; the workflow generates them under the \
         name it asks for"
    );

    // The site check has to inspect the files that were actually generated, or
    // it passes by never looking at them.
    for check in [
        "dist/mosaic_bg.wasm",
        "dist/mosaic.js",
        "grep -q 'export' dist/mosaic.js",
        "grep -q '__wbindgen_start' dist/mosaic.js",
    ] {
        assert!(
            live.contains(check),
            "the built-site check must assert on {check}; the bindings are the studio, and a \
             check that names the wrong file is green without having looked at anything"
        );
    }
    for wrong in ["dist/app.js", "dist/app_bg.wasm", "--out-name app"] {
        assert!(
            !live.contains(wrong),
            "pages.yml names {wrong}; this crate's bindings are the `mosaic` pair, and the page \
             imports ./mosaic.js"
        );
    }
}

/// The exact file list the workflow expects in `dist/`.
///
/// It is the site's whole form, and every entry is load-bearing: the two wasm
/// artefacts are the studio itself, the worker's cache version is derived from
/// the bytes of everything else, and the two PNGs are the install icon. The
/// check is a membership test in both directions, so a file that stops being
/// built and one that starts being built are both caught.
///
/// The list is also the test that the native CLI stays out of the published
/// site: this crate has a `[[bin]]` target, the host build in the workflow
/// builds it, and anything it produced under `dist/` would land here as an
/// unexpected entry.
#[test]
fn the_pages_workflow_expects_exactly_the_files_the_site_is() {
    let Some(pages) = workflow("pages.yml") else {
        return;
    };
    let live = strip_yaml_comments(&pages);
    let expected = live
        .lines()
        .find_map(|line| line.trim().strip_prefix("expected=\"")?.strip_suffix('"'))
        .expect("pages.yml must list the files the site is, for the built-site check to read");

    let expected: Vec<&str> = expected.split_whitespace().collect();
    assert_eq!(
        expected.len(),
        9,
        "the site is nine files: seven written by `build.rs` and the two wasm-bindgen artefacts. \
         Found: {expected:?}"
    );
    // A hand-maintained list, so a new site file has to be added to it — and
    // the `build.rs` one has to be added to `build.rs` too, which is the point
    // of a list rather than a directory listing.
    for file in [
        "icon-192.png",
        "icon-512.png",
        "icon.svg",
        "index.html",
        "manifest.webmanifest",
        "mosaic_bg.wasm",
        "mosaic.js",
        "service-worker.js",
        "worker.js",
    ] {
        assert!(
            expected.contains(&file),
            "the published site contains {file}; the workflow's expected list is {expected:?}"
        );
    }
    // The two wasm artefacts are the studio itself. Nothing else in this list
    // is JavaScript *code*: `worker.js` is the 125-byte bootstrap the bindings
    // start, and `service-worker.js` is the caching shell, both committed and
    // copied. So the name worth guarding is the one that must not appear.
    assert!(
        !expected
            .iter()
            .any(|file| *file == "app.js" || *file == "app_bg.wasm"),
        "the bindings are the `mosaic` pair, as the page's ./mosaic.js import requires; the \
         list has {expected:?}"
    );
}

/// The built site is checked, not assumed green.
///
/// No test in this suite can assert on `dist/`: it is gitignored, so the tree
/// the release gate exports never has it, and building it needs the wasm target
/// and a pinned generator. The workflow is therefore the only place the built
/// output is ever inspected, which makes each of these checks load-bearing. Four
/// failures are silent by construction, and each is one line in the workflow:
///
/// * a `__VERSION__` left unsubstituted ships a service worker whose cache
///   never invalidates;
/// * bindings with no `export` fail the page's dynamic import, in the browser,
///   long after the build looked fine;
/// * bindings with no `__wbindgen_start` load and then do nothing at all, with
///   no error message anywhere;
/// * a page that never calls the initializer renders and does nothing.
///
/// So the check is asserted to exist, rather than left to review — and a
/// commented-out `# grep` must not be able to stand in for it.
#[test]
fn the_pages_workflow_checks_the_built_site_instead_of_trusting_the_build() {
    let Some(pages) = workflow("pages.yml") else {
        return;
    };
    let live = strip_yaml_comments(&pages);

    for check in [
        "grep -q '__VERSION__' dist/service-worker.js",
        "grep -q 'm.default()' dist/index.html",
    ] {
        assert!(
            live.contains(check),
            "the built-site check must include `{check}`. The failure it prevents has no other \
             signal: the job is green, the page is served, and the studio never runs."
        );
    }
    // The icons are rasterized from the committed SVG by `build.rs`, so their
    // being PNGs is a real fact about the build rather than a formatting
    // convention. The magic number is read from the file, so the check has to
    // name the file too — and `dist/icon-${size}.png` names both sizes at
    // once, which is why this is one assertion and not a loop.
    assert!(
        live.contains("dist/icon-${size}.png") && live.contains("89504e470d0a1a0a"),
        "the built-site check must read the magic number of dist/icon-192.png and \
         dist/icon-512.png; the icons are generated from assets/icon.svg, and a truncated write \
         is a blank home-screen tile"
    );
    // Membership in both directions, or "all nine files are present" is only
    // half the statement — the other half is "and nothing else is".
    assert!(
        live.contains("grep -x -F -v -f /tmp/expected-files"),
        "the built-site check must reject files that are not on the expected list; a green build \
         that produced eight of the nine is the half-built site this project has already had"
    );
}

/// The Pages deployment cannot drift from the crate it publishes.
///
/// The deploy job installs its own `wasm-bindgen`, pinned to a literal in the
/// YAML, and compares its digest against a second literal in the same file.
/// `Cargo.toml` pins the same version the crate compiles against. Move the
/// dependency and the workflow keeps building happily: it generates bindings for
/// a runtime the page does not have, and the only symptom is a live site that
/// fails at startup with "Studio could not start" — for every visitor, and only
/// in the browser.
///
/// The version has to be *declared*, not merely mentioned. `$WASM_BINDGEN_VERSION`
/// with nothing to expand to installs no generator at all, and every other check
/// in the job still passes — so the declaration is asserted, not trusted.
#[test]
fn the_pages_workflow_installs_the_generator_the_crate_is_pinned_to() {
    let Some(pages) = workflow("pages.yml") else {
        return;
    };
    let expected = pinned_wasm_bindgen()
        .expect("Cargo.toml must pin wasm-bindgen exactly, e.g. wasm-bindgen = \"=0.2.128\"");
    let version = expected.trim_start_matches('=');

    // Read the declaration out of the live text, and require it at the
    // top level, so a step-scoped copy of the same version does not satisfy it
    // by accident: a value that only one job can see is not the single source
    // of truth the `env:` block exists to be.
    let line = pages
        .lines()
        .find(|line| line.trim_start().starts_with("WASM_BINDGEN_VERSION:"))
        .expect("pages.yml must declare WASM_BINDGEN_VERSION");
    assert!(
        line.starts_with("  WASM_BINDGEN_VERSION:") && !line.starts_with("   "),
        "WASM_BINDGEN_VERSION belongs in the workflow's top-level `env:`, so every step sees one \
         value: {line:?}"
    );
    assert_eq!(
        line.trim(),
        format!("WASM_BINDGEN_VERSION: {version}"),
        "the generator pages.yml installs must be the version Cargo.toml pins ({version})"
    );

    // And the step must read that variable rather than naming a version of its
    // own, which is how two literals start disagreeing.
    let live = strip_yaml_comments(&pages);
    assert!(
        live.contains("version=\"$WASM_BINDGEN_VERSION\""),
        "the generator step must install the version from the env block, not a second literal"
    );
}

/// The build this repository can actually do must not carry a sibling's
/// compiler flag.
///
/// `chess_clock`'s Pages workflow sets `RUSTFLAGS: --cfg=web_sys_unstable_apis`
/// because its Screen Wake Lock use is behind that cfg in web-sys 0.3.105 —
/// every item carries it *in addition to* its feature gate, so enabling the four
/// `Navigator`/`WakeLock`/`WakeLockSentinel`/`WakeLockType` cargo features is
/// necessary and not sufficient, and without the flag the wasm build dies with
/// five errors naming `WakeLockSentinel`.
///
/// The studio uses no such API. `grep -ri wake src/ Cargo.toml` finds nothing,
/// and a clean `cargo build --locked --lib --target wasm32-unknown-unknown
/// --release` on an empty target directory succeeds with `RUSTFLAGS` unset — so
/// the flag would be dead configuration, and a `.cargo/config.toml` carrying it
/// would be worse: a file whose only content is a claim about this crate that
/// nothing checks, in a repository where the test below says it is untrue.
///
/// So the absence is asserted. The day the studio grows a wake lock, this goes
/// red, and that is the intended moment to set the flag: the red test is a
/// better signal than a flag that was copied and happens to be unnecessary.
#[test]
fn the_pages_workflow_does_not_carry_a_compiler_flag_this_crate_does_not_need() {
    let Some(pages) = workflow("pages.yml") else {
        return;
    };
    let live = strip_yaml_comments(&pages);
    assert!(
        !live.contains("web_sys_unstable_apis"),
        "pages.yml sets --cfg=web_sys_unstable_apis, which is chess_clock's wake-lock flag. This \
         crate uses no unstable web-sys API and its wasm build succeeds without it (verified on a \
         clean target directory), so the flag is dead configuration. If the studio has grown a \
         Wake Lock, set the flag deliberately and say so in AGENTS.md -- and drop this test."
    );
    assert!(
        !root().join(".cargo/config.toml").exists(),
        ".cargo/config.toml must not carry --cfg=web_sys_unstable_apis either: this crate's wasm \
         build needs no unstable web-sys cfg, and a config file is the one mechanism that applies \
         to every build and every clone without anyone deciding to set it"
    );
}

/// A deploy that can run from any branch is a deploy a stranger can run.
///
/// `pages: write` and `id-token: write` are the two permissions that make a
/// GitHub Actions job able to overwrite the live site, and the token behind them
/// is minted for the repository however the workflow was reached. The project
/// *wants* an automatic deploy on every merge to master — that is the point,
/// and it is why nobody has to remember to publish. What it does not want is
/// that same power on every other ref, so the invariant asserted here is the
/// narrow one that survives the convenience: master is the only ref that can
/// reach the live site, and the publishing permissions live in the one job that
/// is gated on it.
#[test]
fn only_master_can_reach_the_live_site() {
    let Some(pages) = workflow("pages.yml") else {
        return;
    };
    let live = strip_yaml_comments(&pages);

    // The trigger must be the named branch, not a bare `push:`. A bare `push:`
    // deploys from every branch that exists, including a contributor's feature
    // branch, and it also changes what `on:` means for pull requests — which is
    // the opposite of the intent.
    assert!(
        live.contains("branches: [master]"),
        "pages.yml must trigger on `branches: [master]`, not a bare `push:`; a bare push deploys \
         from every branch, including other people's"
    );
    // A tag trigger alongside the branch trigger would publish a version that
    // was never on master.
    assert!(
        !live.contains("tags:"),
        "pages.yml must not also deploy on tags; a tagged commit that never reached master would \
         be published to the live site"
    );
    // No manual trigger either: it is one more ref-shaped way in, and this
    // account is not an admin of the repository, so it can only ever be
    // something the owner has to notice is missing.
    assert!(
        !live.contains("workflow_dispatch:"),
        "pages.yml must not offer workflow_dispatch: the deploy is automatic on master, and a \
         manual trigger is a second, unaudited way to reach the live site"
    );

    // The deploy job's own gate, named so the assertion cannot be satisfied by
    // a gate on some other job. This is the check that still holds when the
    // trigger is widened by accident.
    let deploy_job = live
        .split("\n  deploy:")
        .nth(1)
        .expect("pages.yml must have a `deploy:` job");

    // The two halves of the gate, read out of the workflow with its comments
    // stripped. Read this way, or the comment block above the `if:` -- which
    // names both halves while explaining why they are there -- satisfies the
    // assertion on its own. That is not hypothetical: it is how the `RUSTFLAGS`
    // assertion in the sibling repository's equivalent file shipped, green,
    // with the line commented out rather than deleted.
    let live_gate = live
        .split("\n  deploy:")
        .nth(1)
        .and_then(|job| job.split_once("if:").map(|(_, after)| after))
        .expect("the `deploy` job must have an `if:` gate");

    // Not a fork. This workflow is byte-identical in `wdomitrz/lego_mosaic` and
    // in its fork `bot-git-ai/lego_mosaic` -- a fork exists precisely so its
    // files can be copied -- so a gate that tests only the branch name cannot
    // tell the two repositories apart. Both have a `master`, and the push that
    // happens on every merge would try to publish from the fork: red at
    // "Creating Pages deployment failed ... Ensure GitHub Pages has been
    // enabled", because a fork has no Pages site until someone enables one by
    // hand. And were it enabled there, the fork would serve its own copy,
    // diverging from the published site as soon as the two masters diverge.
    //
    // `github.event.repository.fork` is the discriminator because it needs no
    // configuration on either side. The obvious alternative, a repository
    // Actions variable, has the failure mode this assertion exists to prevent:
    // it would have to be set on the *owning* repository to publish, and no
    // account but the user's can do that, so the gate would ship as silently
    // off on the one repository where it matters.
    assert!(
        live_gate.contains("!github.event.repository.fork"),
        "the `deploy` job must be gated on `!github.event.repository.fork`; this workflow is \
         byte-identical in the fork `bot-git-ai/lego_mosaic`, so a branch-name-only gate \
         publishes from the fork too -- failing with 'Ensure GitHub Pages has been enabled' \
         until Pages is enabled there, and serving a divergent copy afterwards",
    );
    // Both halves are one condition, not two gates. An `if:` per job would be
    // an AND across two independent gates, and a `build`-job gate would stop
    // the *build* from running on the fork rather than just its publish -- the
    // opposite of what this is for, since the build is what keeps the fork's
    // copy of the site honest.
    assert!(
        live_gate.contains("github.ref == 'refs/heads/master'"),
        "the fork rule must extend the master gate, not replace it: `deploy` must be one `if:` \
         testing both `github.ref` and `github.event.repository.fork`",
    );
    assert!(
        deploy_job.contains("needs: build"),
        "the `deploy` job must depend on the build, or it publishes whatever a failed build left"
    );

    // And the permissions that can actually publish must be scoped to that job
    // rather than granted workflow-wide, so a build step or a third-party
    // action added later cannot spend them.
    let build_job = live
        .split("\n  build:")
        .nth(1)
        .and_then(|after| after.split("\n  deploy:").next())
        .expect("pages.yml must have a `build:` job");
    for permission in ["pages: write", "id-token: write"] {
        assert!(
            !build_job.contains(permission),
            "the `build` job must not hold `{permission}`; both publishing permissions belong to \
             `deploy` alone"
        );
    }
    for permission in ["pages: write", "id-token: write"] {
        assert!(
            deploy_job.contains(permission),
            "the `deploy` job must hold `{permission}`, or the deploy has no way to publish"
        );
    }
    // The environment is what makes the repository's own Pages approval rules
    // apply, so a protected branch or a reviewer-gated environment can still
    // hold the live site back.
    assert!(
        deploy_job.contains("name: github-pages"),
        "the `deploy` job must name the `github-pages` environment, so the repository's Pages \
         settings can gate it"
    );
}

/// The deploy has to be handed something the upload actually produced.
///
/// `deploy-pages` v5 takes `artifact_name`. There is no `artifact_id` input:
/// passing one is reported as `Unexpected input(s) 'artifact_id'` and the action
/// falls back to its own default, which is only the right answer while the
/// upload side also defaults to the same string. Change one side and the deploy
/// finds no artifact and fails with a bare `HttpError: Not Found` — which says
/// nothing about the artifact, so the cause has to be read out of the workflow,
/// not out of the error.
#[test]
fn the_deploy_is_handed_the_artifact_the_build_uploaded() {
    let Some(pages) = workflow("pages.yml") else {
        return;
    };
    let live = strip_yaml_comments(&pages);

    // The input that v5 does not have. Its presence is a warning at run time
    // and never an error, so nothing else would ever report it.
    assert!(
        !live.contains("artifact_id:"),
        "pages.yml passes `artifact_id` to deploy-pages v5, which has no such input; it is warned \
         about and ignored, leaving the deploy to guess the artifact name"
    );

    // Both sides name the artifact the same way. Read the two keys out of the
    // live text rather than asserting a fixed string, so the invariant is the
    // agreement and not the particular name.
    //
    // Only the two are read, and only where they are the artifact's own keys:
    // the file also carries the workflow's `name:` and every step's `name:`, and
    // a plain prefix match would pick up whichever of those comes first.
    // `artifact_name:` is unique, and the upload's `name:` is the one indented
    // ten spaces — a step's own `name:` is eight.
    let name_of = |key: &str, indent: usize| {
        live.lines().find_map(|line| {
            let prefix = format!("{}{key}: ", " ".repeat(indent));
            let rest = line.strip_prefix(prefix.as_str())?;
            Some(rest.trim().trim_matches('"').to_owned())
        })
    };
    let uploaded = name_of("name", 10).unwrap_or_else(|| {
        panic!("pages.yml must state the upload step's artifact `name:` so the deploy can match it")
    });
    let deployed = name_of("artifact_name", 10).unwrap_or_else(|| {
        panic!(
            "pages.yml must pass `artifact_name:` to deploy-pages, or it uses a default that can \
             drift from the upload"
        )
    });
    assert_eq!(
        uploaded, deployed,
        "the artifact the build uploads ({uploaded:?}) and the one the deploy asks for \
         ({deployed:?}) must be the same name"
    );
    // And it must be the Pages upload, not the run-artifact upload: the Pages
    // artifact is a single tarball the deploy finds by name, while a plain
    // upload produces a green build and then a deploy that cannot find what it
    // was given.
    assert!(
        live.contains("actions/upload-pages-artifact@"),
        "pages.yml must use actions/upload-pages-artifact, not actions/upload-artifact: the \
         deploy looks for a Pages tarball by name"
    );
}

/// The published site is `dist/`, and only `dist/`.
///
/// The workflow builds this crate's `[[bin]]` target on purpose — it is the
/// native conversion CLI, and `build.yml` compiles it on every push, so it is
/// not something the site deploy may quietly stop covering. What the site may
/// not do is put any of it on the web, and the two are kept apart by the upload
/// path rather than by a list of exclusions that someone can add to.
#[test]
fn the_pages_workflow_publishes_dist_and_never_the_binary() {
    let Some(pages) = workflow("pages.yml") else {
        return;
    };
    let live = strip_yaml_comments(&pages);

    // A path of `.`, `./*` or the repository root would sweep in whatever the
    // build happened to leave lying around.
    let paths: Vec<&str> = live
        .lines()
        .filter_map(|line| {
            let rest = line.trim().strip_prefix("path: ")?;
            Some(rest.trim().trim_matches('"'))
        })
        .collect();
    assert_eq!(
        paths,
        vec!["dist/"],
        "the only published path must be dist/: the static site is the whole form, and the \
         `[[bin]]` build lands in target/. Found: {paths:?}"
    );
    // And nothing may copy the binary, or any of target/, into the site.
    for wrong in ["cp target/", "lego-mosaic", "target/release/"] {
        assert!(
            !live.contains(wrong),
            "pages.yml names {wrong}; the native CLI is not part of the published site"
        );
    }
    // The two builds, in the order AGENTS.md gives: the wasm library for the
    // studio, the host build for the shell (and, deliberately, the CLI).
    assert!(
        live.contains("cargo build --locked --lib --target wasm32-unknown-unknown --release"),
        "the site is the library compiled to wasm"
    );
    assert!(
        live.contains("touch build.rs") && live.contains("cargo build --release --locked"),
        "the second build must force `build.rs` to re-run: it writes into the source tree, so \
         Cargo cannot see its own output change and the site would be left half-built"
    );
    // `build.rs` derives the worker's cache version from the bytes in `dist/`,
    // so the order is a correctness requirement, not a habit.
    let bindings_at = live.find("--out-name mosaic").expect("the bindings step");
    let shell_at = live.find("touch build.rs").expect("the shell build step");
    assert!(
        bindings_at < shell_at,
        "the bindings must be generated before the host build: build.rs hashes every file in \
         dist/, the wasm included, to derive the service worker's cache version"
    );
}

/// The Pages workflow is the only thing in this repository that can publish, and
/// the CI build stays outside it.
///
/// `build.yml` runs on every pull request, including from forks. A deploy needs
/// `pages: write`, `id-token: write` and the `github-pages` environment, none of
/// which a fork PR has — so the two are separate files, and a step added to
/// `build.yml` is a step that cannot publish even if someone gets its logic
/// wrong. The two costs of splitting them are real and worth naming: the site is
/// built twice per push to master, and a test that the deploy exists cannot see
/// whether the *build* workflow is the one that runs on a fork. What that leaves
/// covered is that the deploy exists, is gated, and is not reachable from a pull
/// request — which is the part that can silently overreach.
#[test]
fn the_pages_workflow_is_the_only_thing_that_can_publish() {
    let Some(pages) = workflow("pages.yml") else {
        return;
    };
    let build = workflow("build.yml").expect("this repository has a CI build workflow");
    let live_build = strip_yaml_comments(&build);

    for permission in ["pages: write", "id-token: write"] {
        assert!(
            !live_build.contains(permission),
            "build.yml holds `{permission}`; the permissions that can overwrite the live site \
             belong to pages.yml's deploy job alone, and build.yml runs on fork pull requests \
             where the github-pages environment does not exist"
        );
    }
    for action in ["upload-pages-artifact", "deploy-pages"] {
        assert!(
            !live_build.contains(action),
            "build.yml must not use {action}: it runs on every pull request, including from \
             forks. The build is a check; publishing is pages.yml's job."
        );
    }
    // ...and the trigger has to match the claim. A `build.yml` that had grown a
    // bare `push:` would not publish anything, but it would no longer be the
    // read-only-for-everyone build the split is for.
    assert!(
        live_build.contains("pull_request:"),
        "build.yml must run on pull requests; that is the half of the reason the deploy is a \
         separate file"
    );
    // The deploy's own workflow must not be triggered by a pull request at all.
    assert!(
        !strip_yaml_comments(&pages).contains("pull_request:"),
        "pages.yml must not trigger on pull_request: a fork's pull request runs with a read-only \
         token, and a deploy step that is attempted and cannot succeed is one that a later \
         trigger change would make succeed"
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

// Copyright (c) 2026 Witalis Domitrz <witekdomitrz@gmail.com>
// AGPL License

//! `lego-mosaic dist`: write the app shell to a directory as a static,
//! front-end-only PWA.
//!
//! The studio was always a browser application — the conversion runs in wasm
//! and images never leave the machine. The built-in host exists only to
//! deliver the shell, so publishing the same bytes as plain files removes the
//! server from the picture without changing what the app does.
//!
//! Everything the shell needs is relative (`./mosaic.js`, `new URL('./', …)`,
//! `start_url: "./"`), so the output runs unchanged from a subdirectory, a
//! CDN or a plain file host. Two files are generated rather than copied: the
//! manifest and the service worker, whose cache name is pinned to a
//! content-derived version here instead of per request.

use std::path::{Path, PathBuf};

/// Write every asset to `dir`, creating it if needed.
///
/// The directory is built beside itself and swapped into place, so a failed
/// or interrupted run cannot leave a half-written shell in the location a
/// host is serving from.
pub(crate) fn write(dir: &Path) -> Result<Vec<String>, String> {
    // A bare relative name like `dist` has an empty parent: that means the
    // current directory, which is the documented default, not an error.
    let parent = match dir.parent() {
        Some(path) if !path.as_os_str().is_empty() => path.to_path_buf(),
        _ => PathBuf::from("."),
    };
    std::fs::create_dir_all(&parent)
        .map_err(|error| format!("Create {}: {error}", parent.display()))?;

    // A sibling staging directory keeps the swap on one filesystem, which a
    // plain rename requires.
    let staging = dir.with_extension("incoming");
    // A previous failed run can leave one behind.
    let _ = std::fs::remove_dir_all(&staging);
    std::fs::create_dir_all(&staging)
        .map_err(|error| format!("Create {}: {error}", staging.display()))?;

    let mut written = Vec::new();
    for asset in crate::assets::all() {
        let path = staging.join(asset.name);
        std::fs::write(&path, asset.body)
            .map_err(|error| format!("Write {}: {error}", path.display()))?;
        written.push(asset.name.to_string());
    }

    // Replace any previous shell wholesale, so files dropped from the app
    // cannot linger in the published directory.
    let previous = dir.with_extension("previous");
    let _ = std::fs::remove_dir_all(&previous);
    if dir.exists() {
        std::fs::rename(dir, &previous)
            .map_err(|error| format!("Replace {}: {error}", dir.display()))?;
    }
    std::fs::rename(&staging, dir)
        .map_err(|error| format!("Publish {}: {error}", dir.display()))?;
    let _ = std::fs::remove_dir_all(&previous);
    Ok(written)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A unique scratch directory, removed on drop even if a test fails.
    struct Scratch(std::path::PathBuf);
    impl Scratch {
        fn new(tag: &str) -> Self {
            let dir = std::env::temp_dir().join(format!(
                "mosaic-dist-{}-{}-{tag}",
                std::process::id(),
                stamp()
            ));
            let _ = std::fs::remove_dir_all(&dir);
            Self(dir)
        }
        /// Create the directory itself, for tests that `chdir` into it.
        fn created(tag: &str) -> Self {
            let scratch = Self::new(tag);
            std::fs::create_dir_all(&scratch.0).expect("create scratch dir");
            scratch
        }
    }
    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// Monotonic per-run counter: tests in one binary run concurrently and
    /// would otherwise share a directory name.
    fn stamp() -> u64 {
        use std::sync::atomic::{AtomicU64, Ordering};
        static NEXT: AtomicU64 = AtomicU64::new(0);
        NEXT.fetch_add(1, Ordering::Relaxed)
    }

    #[test]
    fn writes_every_asset_and_leaves_no_staging_directory() {
        let scratch = Scratch::new("all");
        let dir = scratch.0.join("dist");
        let written = write(&dir).expect("write dist");

        assert_eq!(written.len(), crate::assets::all().len());
        for name in &written {
            let path = dir.join(name);
            assert!(path.is_file(), "{name} missing from dist");
            assert!(
                std::fs::metadata(&path).expect("stat").len() > 0,
                "{name} is empty"
            );
        }
        // The staging and previous directories must not survive: a host
        // serving the parent would otherwise publish two half-shells.
        assert!(
            !dir.with_extension("incoming").exists(),
            "staging left behind"
        );
        assert!(
            !dir.with_extension("previous").exists(),
            "backup left behind"
        );
    }

    #[test]
    fn output_bytes_match_the_assets_the_server_would_serve() {
        let scratch = Scratch::new("bytes");
        let dir = scratch.0.join("dist");
        write(&dir).expect("write dist");

        for asset in crate::assets::all() {
            let bytes = std::fs::read(dir.join(asset.name)).expect("read asset");
            assert_eq!(
                bytes, asset.body,
                "{} differs between dist and the embedded shell",
                asset.name
            );
        }
    }

    #[test]
    fn the_shell_is_self_contained_and_mounts_anywhere() {
        let scratch = Scratch::new("relative");
        let dir = scratch.0.join("dist");
        write(&dir).expect("write dist");

        let page = std::fs::read_to_string(dir.join("index.html")).expect("read page");
        // The shell must reference its neighbours by relative path only, so
        // the directory can be published under any prefix.
        assert!(
            page.contains("import('./mosaic.js')"),
            "loader is not relative"
        );
        assert!(
            page.contains(r#"href="./manifest.webmanifest""#),
            "manifest link"
        );
        for absolute in ["http://", "https://", "src=\"/", "href=\"/"] {
            assert!(
                !page.contains(absolute),
                "page is not prefix-free: {absolute}"
            );
        }
        // The wasm module is fetched by the generated bindings, so it has to
        // sit next to them.
        let bindings = std::fs::read_to_string(dir.join("mosaic.js")).expect("read bindings");
        assert!(
            bindings.contains("mosaic_bg.wasm"),
            "bindings miss the module"
        );
    }

    #[test]
    fn republishing_replaces_a_stale_shell_instead_of_merging_into_it() {
        let scratch = Scratch::new("replace");
        let dir = scratch.0.join("dist");
        write(&dir).expect("first dist");

        // A file left over from an earlier build, no longer part of the shell.
        let orphan = dir.join("old-worker.js");
        std::fs::write(&orphan, b"// removed from the app").expect("write orphan");
        write(&dir).expect("second dist");

        assert!(
            !orphan.exists(),
            "a dropped asset survived a rebuild; the published shell is stale"
        );
        assert!(
            dir.join("service-worker.js").is_file(),
            "rebuild lost a file"
        );
    }

    /// The documented default is a bare `./dist`, so a bare relative name has
    /// to be accepted rather than rejected for having no parent component.
    #[test]
    fn a_bare_relative_name_is_the_current_directory() {
        let scratch = Scratch::created("bare-name");
        let saved = std::env::current_dir().ok();
        assert!(saved.is_some(), "need a cwd to chdir");
        std::env::set_current_dir(&scratch.0).expect("chdir to scratch");
        let written = write(Path::new("dist"));
        std::env::set_current_dir(saved.expect("saved cwd")).expect("restore cwd");
        let written = written.expect("a bare name must not be refused");
        assert!(!written.is_empty());
        assert!(scratch.0.join("dist/index.html").is_file());
    }
}

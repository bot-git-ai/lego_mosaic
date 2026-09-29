// Copyright (c) 2026 Witalis Domitrz <witekdomitrz@gmail.com>
// AGPL License

//! The complete application shell, defined once for every front end.
//!
//! The studio has no dynamic content: every byte it serves is already
//! embedded in the binary. So the built-in HTTP host and `lego-mosaic dist`
//! are not two implementations to keep in step — they are two deliveries of
//! the same asset list defined here. A file added, renamed or retyped in one
//! place can therefore never silently go missing from the other.

use std::sync::OnceLock;

use crate::ui;

/// The app-shell manifest, served verbatim as a real file so a static host
/// needs no special-casing. Relative `start_url`/`scope` keep it mountable at
/// any prefix. Kept as one literal: splitting it would splice Rust source
/// fragments into the middle of JSON string values. The doubled `##` is
/// required by the hex colors, which contain `"#`.
pub(crate) const MANIFEST: &str = r##"{"id":"./","name":"Mosaic Studio","short_name":"Mosaic","start_url":"./","scope":"./","display":"standalone","background_color":"#f4f6f8","theme_color":"#17243a","icons":[{"src":"icon-192.png","sizes":"192x192","type":"image/png","purpose":"any maskable"},{"src":"icon-512.png","sizes":"512x512","type":"image/png","purpose":"any maskable"}]}"##;

/// The service-worker source, before its cache name is pinned to a version.
const SERVICE_WORKER: &str = include_str!("service-worker.js");
/// The placeholder standing in for the content-derived cache version.
const VERSION_PLACEHOLDER: &str = "__VERSION__";

/// One file of the app shell, byte for byte as it is delivered.
#[derive(Clone)]
pub(crate) struct Asset {
    /// Name inside the dist directory, and the name the service worker
    /// caches. `index.html` is the app shell itself.
    pub(crate) name: &'static str,
    /// Path the built-in host serves it on. Equal to `name` for everything
    /// except the shell, which the host answers at the root.
    pub(crate) route: &'static str,
    pub(crate) content_type: &'static str,
    pub(crate) body: &'static [u8],
}

/// The shell, built once and shared by every request and by `dist`.
pub(crate) fn all() -> &'static [Asset] {
    static SHELL: OnceLock<Vec<Asset>> = OnceLock::new();
    SHELL.get_or_init(|| {
        // Every file except the service worker, whose cache name is derived
        // from the bytes of all the others. Hashing it here is what makes a
        // release install as a fresh, atomic cache instead of leaving
        // returning users on the previous wasm.
        let shell = vec![
            Asset {
                name: "index.html",
                route: "/",
                content_type: "text/html; charset=utf-8",
                body: page(),
            },
            Asset {
                name: "mosaic.js",
                route: "/mosaic.js",
                content_type: "text/javascript; charset=utf-8",
                body: include_bytes!("../assets/mosaic.js").as_slice(),
            },
            Asset {
                name: "mosaic_bg.wasm",
                route: "/mosaic_bg.wasm",
                content_type: "application/wasm",
                body: include_bytes!("../assets/mosaic_bg.wasm").as_slice(),
            },
            Asset {
                name: "worker.js",
                route: "/worker.js",
                content_type: "text/javascript; charset=utf-8",
                body: include_bytes!("worker.js").as_slice(),
            },
            Asset {
                name: "manifest.webmanifest",
                route: "/manifest.webmanifest",
                content_type: "application/manifest+json",
                body: MANIFEST.as_bytes(),
            },
            Asset {
                name: "icon-192.png",
                route: "/icon-192.png",
                content_type: "image/png",
                body: include_bytes!("../assets/icon-192.png").as_slice(),
            },
            Asset {
                name: "icon-512.png",
                route: "/icon-512.png",
                content_type: "image/png",
                body: include_bytes!("../assets/icon-512.png").as_slice(),
            },
        ];
        let version = cache_version(&shell);
        let mut assets = Vec::with_capacity(shell.len() + 1);
        assets.extend(shell);
        assets.push(Asset {
            name: "service-worker.js",
            route: "/service-worker.js",
            content_type: "text/javascript; charset=utf-8",
            body: versioned_service_worker(&version),
        });
        assets
    })
}

/// Find the asset the built-in host should answer a request with. An empty
/// path is the root the request line carries for a bare `GET /`.
pub(crate) fn find(route: &str) -> Option<&'static Asset> {
    all()
        .iter()
        .find(|asset| asset.route == route || (route.is_empty() && asset.route == "/"))
}

/// The app shell, owned by `ui` and interned so the list can hand out
/// `&'static` bytes without copying the page on every request.
fn page() -> &'static [u8] {
    static PAGE: OnceLock<Vec<u8>> = OnceLock::new();
    PAGE.get_or_init(|| ui::page().into_bytes())
}

/// A content-derived cache version over the shell and the worker template.
///
/// Deliberately ignores the cache names themselves and the `ROOT.pathname`
/// scope segment, which the service worker adds at runtime: two builds of
/// identical content must agree, and the same build mounted at two prefixes
/// must share one version.
fn cache_version(shell: &[Asset]) -> String {
    use std::hash::{Hash, Hasher};
    let mut version = std::collections::hash_map::DefaultHasher::new();
    for asset in shell {
        asset.name.hash(&mut version);
        asset.body.hash(&mut version);
    }
    SERVICE_WORKER.hash(&mut version);
    format!("{:x}", version.finish())
}

/// The worker source with its cache name pinned to `version`. Every other
/// reference in the file is left untouched, so the copy on disk stays a
/// readable template with exactly one placeholder.
fn versioned_service_worker(version: &str) -> &'static [u8] {
    static WORKER: OnceLock<Vec<u8>> = OnceLock::new();
    WORKER.get_or_init(|| {
        SERVICE_WORKER
            .replace(VERSION_PLACEHOLDER, version)
            .into_bytes()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shell_is_exactly_the_service_worker_allowlist_plus_the_shell_page() {
        let names: Vec<&str> = all().iter().map(|asset| asset.name).collect();
        assert_eq!(
            names,
            vec![
                "index.html",
                "mosaic.js",
                "mosaic_bg.wasm",
                "worker.js",
                "manifest.webmanifest",
                "icon-192.png",
                "icon-512.png",
                "service-worker.js",
            ]
        );
        // Every file the worker pre-caches must exist in the shell, or the
        // browser install fails and the app silently loses offline support.
        let service_worker = find("/service-worker.js").expect("service worker");
        let text = std::str::from_utf8(service_worker.body).expect("worker is utf-8");
        for cached in [
            "mosaic.js",
            "mosaic_bg.wasm",
            "worker.js",
            "manifest.webmanifest",
            "icon-192.png",
            "icon-512.png",
        ] {
            assert!(
                text.contains(&format!("'{cached}'")),
                "service worker no longer caches {cached}"
            );
            assert!(
                find(&format!("/{cached}")).is_some(),
                "{cached} not in shell"
            );
        }
    }

    #[test]
    fn service_worker_carries_a_version_and_no_leftover_placeholder() {
        let worker = find("/service-worker.js").expect("service worker");
        let text = std::str::from_utf8(worker.body).expect("worker is utf-8");
        assert!(
            !text.contains(VERSION_PLACEHOLDER),
            "placeholder left in worker"
        );
        // The version is content-derived, so it must be a bare hex digest
        // between the cache prefix and the closing quote.
        let line = text
            .lines()
            .find(|line| line.contains("const CACHE"))
            .expect("cache line");
        let version = line
            .rsplit('-')
            .next()
            .expect("cache name ends in a version")
            .trim_end_matches("';");
        assert!(
            !version.is_empty() && version.chars().all(|c| c.is_ascii_hexdigit()),
            "cache version is not a hex digest: {version:?}"
        );
    }

    #[test]
    fn versions_are_stable_for_identical_content() {
        // The whole point of deriving the version from the bytes: rebuilding
        // without changing anything must not orphan returning users' caches.
        let shell: Vec<Asset> = all()
            .iter()
            .filter(|asset| asset.name != "service-worker.js")
            .cloned()
            .collect();
        assert_eq!(cache_version(&shell), cache_version(&shell));
        // A one-byte change anywhere in the shell must move the version.
        let mut changed = shell.clone();
        changed[0].body = b"<html>different</html>";
        assert_ne!(cache_version(&shell), cache_version(&changed));
    }

    #[test]
    fn wasm_module_carries_its_magic_and_bindings_are_javascript() {
        let wasm = find("/mosaic_bg.wasm").expect("wasm");
        assert_eq!(&wasm.body[..4], b"\0asm", "not a wasm module");
        let bindings = find("/mosaic.js").expect("bindings");
        let text = std::str::from_utf8(bindings.body).expect("bindings are utf-8");
        assert!(text.contains("mosaic_bg.wasm"), "bindings miss the module");
    }

    #[test]
    fn manifest_is_valid_json_with_relative_scope() {
        let manifest: serde_json::Value =
            serde_json::from_str(MANIFEST).expect("manifest is valid JSON");
        assert_eq!(manifest["start_url"], "./");
        assert_eq!(manifest["scope"], "./");
        assert_eq!(manifest["display"], "standalone");
        // Relative icons are what make the app mountable at any prefix.
        for icon in manifest["icons"].as_array().expect("icons") {
            let src = icon["src"].as_str().expect("icon src");
            assert!(!src.starts_with('/'), "icon {src} is not relative");
            assert!(
                find(&format!("/{src}")).is_some(),
                "icon {src} not in shell"
            );
        }
    }
}

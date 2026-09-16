// Copyright (c) 2026 Witalis Domitrz <witekdomitrz@gmail.com>
// AGPL License

//! Static, responsive shell for the Rust browser application. The only
//! handwritten JavaScript imports wasm-bindgen's generated module loader.

pub(crate) fn page() -> String {
    include_str!("ui.html").to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn page_loads_generated_bindings_not_a_manual_wasm_abi() {
        let page = page();
        assert!(page.contains("<!DOCTYPE html>"));
        assert!(page.contains("<script type=\"module\">"));
        assert!(page.contains("import('./mosaic.js')"));
        assert_eq!(page.matches("<script").count(), 1);
        for obsolete in [
            "instantiateStreaming",
            "alloc_buf",
            "wasm.exports",
            "api/convert",
            "fetch(",
        ] {
            assert!(!page.contains(obsolete), "no {obsolete} in static shell");
        }
    }

    #[test]
    fn page_has_accessible_local_input_and_build_outputs() {
        let page = page();
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
            assert!(page.contains(&format!("id=\"{id}\"")), "missing {id}");
        }
        assert!(page.contains("aria-live=\"polite\""));
        assert!(page.contains("@media print"));
    }
}

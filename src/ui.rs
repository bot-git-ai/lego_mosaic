// Copyright (c) 2026 Witalis Domitrz <witekdomitrz@gmail.com>
// AGPL License

//! The web UI: one static page. All conversion logic lives in the wasm
//! module — the page instantiates it, decodes the chosen picture on a
//! canvas and calls the exported `convert`. Palette metadata also comes
//! from the module, so no colors or defaults are duplicated in the HTML.

/// The full page. Vanilla JS, no build step, nothing server-generated —
/// one fixed string.
pub(crate) fn page() -> String {
    include_str!("ui.html").to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn page_is_complete_html_and_loads_the_module() {
        let page = page();
        assert!(page.contains("<!DOCTYPE html>"));
        assert!(page.contains("mosaic.wasm"));
        assert!(!page.contains("api/convert"), "no server API calls");
        assert!(page.contains("instantiateStreaming"));
    }
}

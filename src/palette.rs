// Copyright (c) 2026 Witalis Domitrz <witekdomitrz@gmail.com>
// AGPL License

//! Tile palettes. Mosaic Maker uses white, two grays, black and yellow.
//! Part 98138 is a 1×1 round TILE (smooth top), not a round plate.
//! Extended colors are an approximate design palette, not a verified
//! inventory: check the part/color combination and current stock before
//! ordering. RGB values are LDraw-style screen approximations, not measured
//! plastic colors; lighting, displays and production batches vary.

use crate::color::Srgb;

/// One tile color: what the UI shows and the parts list counts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct TileColor {
    pub(crate) name: &'static str,
    pub(crate) rgb: Srgb,
}

impl TileColor {
    const fn new(name: &'static str, r: u8, g: u8, b: u8) -> Self {
        Self {
            name,
            rgb: Srgb::new(r, g, b),
        }
    }

    /// Stable app identifier, not an official LEGO or BrickLink color ID.
    pub(crate) fn id(&self) -> String {
        self.name.to_ascii_lowercase().replace(' ', "-")
    }

    /// Printable app color number, stable across palettes and exclusions.
    /// Canonical extended-palette order must not be changed without migrating
    /// these symbols. New colors should be appended. Unknown colors use `?`.
    pub(crate) fn symbol(&self) -> String {
        EXTENDED
            .iter()
            .position(|tile| tile.name == self.name)
            .map_or_else(|| "?".to_string(), |index| format!("{:02}", index + 1))
    }

    pub(crate) fn hex(&self) -> String {
        self.rgb.hex()
    }
}

/// The five colors of the official Mosaic Maker personalized sets.
pub(crate) const MOSAIC_MAKER: &[TileColor] = &[
    TileColor::new("White", 0xF2, 0xF3, 0xF2),
    TileColor::new("Light Bluish Gray", 0xA0, 0xA5, 0xA9),
    TileColor::new("Dark Bluish Gray", 0x6C, 0x6E, 0x68),
    TileColor::new("Black", 0x05, 0x13, 0x1D),
    TileColor::new("Yellow", 0xF2, 0xCD, 0x37),
];

/// Just the neutral ramp: white, grays, black. Good for the pixel-art look.
pub(crate) const MONOCHROME: &[TileColor] = &[
    TileColor::new("White", 0xF2, 0xF3, 0xF2),
    TileColor::new("Very Light Gray", 0xE0, 0xE0, 0xDD),
    TileColor::new("Light Bluish Gray", 0xA0, 0xA5, 0xA9),
    TileColor::new("Light Gray", 0x9B, 0xA1, 0x9D),
    TileColor::new("Dark Bluish Gray", 0x6C, 0x6E, 0x68),
    TileColor::new("Black", 0x05, 0x13, 0x1D),
];

/// Extended design palette: lets accents (pink, blue, yellow…) survive.
/// Part 98138 color/stock availability is NOT verified. Legacy Light Gray
/// is distinct from Light Bluish Gray (LEGO's Medium Stone Grey); do not
/// invent a second modern gray under the latter's alternate name.
pub(crate) const EXTENDED: &[TileColor] = &[
    TileColor::new("White", 0xF2, 0xF3, 0xF2),
    TileColor::new("Very Light Gray", 0xE0, 0xE0, 0xDD),
    TileColor::new("Light Bluish Gray", 0xA0, 0xA5, 0xA9),
    TileColor::new("Light Gray", 0x9B, 0xA1, 0x9D),
    TileColor::new("Dark Bluish Gray", 0x6C, 0x6E, 0x68),
    TileColor::new("Black", 0x05, 0x13, 0x1D),
    TileColor::new("Reddish Brown", 0x58, 0x2A, 0x12),
    TileColor::new("Nougat", 0xD0, 0x8B, 0x6C),
    TileColor::new("Light Nougat", 0xF6, 0xD7, 0xB3),
    TileColor::new("Dark Tan", 0x92, 0x82, 0x62),
    TileColor::new("Tan", 0xE4, 0xCD, 0x9E),
    TileColor::new("Yellow", 0xF2, 0xCD, 0x37),
    TileColor::new("Bright Light Orange", 0xF8, 0xBB, 0x56),
    TileColor::new("Orange", 0xFE, 0x8A, 0x18),
    TileColor::new("Red", 0xB4, 0x00, 0x0F),
    TileColor::new("Dark Red", 0x72, 0x10, 0x0C),
    TileColor::new("Pink", 0xFC, 0x97, 0xAC),
    TileColor::new("Bright Pink", 0xE4, 0xAD, 0xC8),
    TileColor::new("Magenta", 0x92, 0x33, 0x8F),
    TileColor::new("Purple", 0x81, 0x03, 0x8D),
    TileColor::new("Lavender", 0xC9, 0xA9, 0xD9),
    TileColor::new("Medium Lavender", 0xA0, 0x6E, 0xB5),
    TileColor::new("Dark Blue", 0x0A, 0x34, 0x63),
    TileColor::new("Blue", 0x00, 0x55, 0xBF),
    TileColor::new("Medium Blue", 0x5A, 0x93, 0xDB),
    TileColor::new("Dark Azure", 0x07, 0x8B, 0xC9),
    TileColor::new("Medium Azure", 0x36, 0xA2, 0xEB),
    TileColor::new("Light Aqua", 0xAD, 0xE2, 0xEA),
    TileColor::new("Dark Green", 0x18, 0x46, 0x32),
    TileColor::new("Green", 0x23, 0x78, 0x2D),
    TileColor::new("Bright Green", 0x4B, 0x9F, 0x4A),
    TileColor::new("Sand Green", 0xA0, 0xBC, 0xA8),
    TileColor::new("Lime", 0xBB, 0xE9, 0x0D),
    TileColor::new("Yellowish Green", 0xE2, 0xF9, 0x92),
];

/// All selectable palettes, with the ids used by the API/UI.
pub(crate) fn all() -> &'static [(&'static str, &'static str, &'static [TileColor])] {
    &[
        ("mosaic-maker", "Mosaic Maker (5)", MOSAIC_MAKER),
        ("monochrome", "Grayscale (6)", MONOCHROME),
        ("extended", "Extended (34)", EXTENDED),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn palettes_are_nonempty_and_unique_named() {
        for (id, label, tiles) in all() {
            assert!(!id.is_empty() && !label.is_empty());
            assert!(!tiles.is_empty(), "{id} is empty");
            for (i, tile) in tiles.iter().enumerate() {
                assert!(!tiles[..i].iter().any(|t| t.name == tile.name));
            }
        }
    }

    #[test]
    fn ids_are_unique() {
        let ids: std::collections::HashSet<_> = all().iter().map(|(id, _, _)| *id).collect();
        assert_eq!(ids.len(), all().len());
    }
    #[test]
    fn official_five_contains_yellow_not_brown() {
        let names: Vec<_> = MOSAIC_MAKER.iter().map(|tile| tile.name).collect();
        assert_eq!(
            names,
            [
                "White",
                "Light Bluish Gray",
                "Dark Bluish Gray",
                "Black",
                "Yellow"
            ]
        );
        assert_eq!(MOSAIC_MAKER[4].hex(), "#f2cd37");
    }

    #[test]
    fn color_ids_symbols_and_rgb_are_distinct_and_consistent() {
        for (_, _, tiles) in all() {
            for (i, tile) in tiles.iter().enumerate() {
                assert!(!tiles[..i].iter().any(|t| t.rgb == tile.rgb));
                assert!(!tiles[..i].iter().any(|t| t.id() == tile.id()));
                assert!(!tiles[..i].iter().any(|t| t.symbol() == tile.symbol()));
                let canonical = EXTENDED.iter().find(|t| t.name == tile.name).unwrap();
                assert_eq!(tile.rgb, canonical.rgb);
                assert_eq!(tile.symbol(), canonical.symbol());
                assert_ne!(tile.symbol(), "?");
            }
        }
        assert_eq!(MOSAIC_MAKER[4].id(), "yellow");
        assert_eq!(MOSAIC_MAKER[4].symbol(), "12");
        let dark = EXTENDED.iter().find(|t| t.name == "Dark Green").unwrap();
        let green = EXTENDED.iter().find(|t| t.name == "Green").unwrap();
        assert!(dark.rgb.to_lab().l < green.rgb.to_lab().l);
        assert_eq!(dark.hex(), "#184632");
    }
}

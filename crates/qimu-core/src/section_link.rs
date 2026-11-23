//! Section <-> content-block link mode (PLAN §3.3, §8 decision 5).
//!
//! The core concept mapping of the DeckCraft integration is
//! "content block = PPT section". Whether an assembled deck may carry
//! real OOXML sections (`p14:sectionLst` in `ppt/presentation.xml`)
//! depends on the DeckCraft write path surviving a save -> reopen
//! round trip with section names AND slide membership intact. That
//! is exactly what the P0 probe checks:
//! `crates/qimu-core/tests/deckcraft_section_probe.rs`, executed in
//! CI by `.github/workflows/p0-deckcraft.yml`.
//!
//! Until the probe passes, this crate builds WITHOUT the
//! `deckcraft-sections` feature and [`section_mode`] reports
//! [`SectionMode::ManifestOnly`] — the degradation the user accepted
//! in §8 decision 5:
//!
//! * import: sections found in a source deck are used only as split
//!   *hints* for block proposals (see `propose.rs`);
//! * assembly: no sections are written into the exported package
//!   (see `assemble.rs`); block boundaries live in the manifest
//!   alone, exactly as before the integration.
//!
//! When CI shows the probe green, enabling native mode is a
//! one-line change (add `deckcraft-sections` to the default features
//! or pass `--features deckcraft-sections`); the two call sites
//! above are the only places that consult this switch, so flipping
//! it needs no further rewiring.

/// How content-block boundaries are carried in exported decks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SectionMode {
    /// Blocks are written as real PPT sections and read back from
    /// them; the manifest keeps the block-id <-> section mapping.
    Native,
    /// Sections are import hints only; assembly writes none and the
    /// manifest is the sole record of block boundaries (§8-5
    /// degradation).
    ManifestOnly,
}

/// The section mode this build was compiled with.
pub const fn section_mode() -> SectionMode {
    if cfg!(feature = "deckcraft-sections") {
        SectionMode::Native
    } else {
        SectionMode::ManifestOnly
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mode_matches_feature_flag() {
        // The probe-gated default: without the feature the accepted
        // degradation must be in force.
        if cfg!(feature = "deckcraft-sections") {
            assert_eq!(section_mode(), SectionMode::Native);
        } else {
            assert_eq!(section_mode(), SectionMode::ManifestOnly);
        }
    }
}

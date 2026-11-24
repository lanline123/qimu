//! Import block proposals (§5.8): given a deck, propose how to split it
//! into content blocks for the importer to confirm. The built-in
//! proposer is a deterministic heuristic (section-opener detection);
//! the interface is the seam where an AI proposer plugs in later —
//! proposals are always confirmed by a human before import.
//!
//! SECTION WIRING (PLAN §3.3 item 1, §8 decision 5; implemented
//! via `crate::pptx::deck_sections`): in BOTH
//! section modes a source deck's real sections are the preferred
//! split boundaries here (one section = one proposed block, named
//! after the section) and this heuristic only arbitrates decks
//! without sections. The modes differ only in what assembly writes
//! back — see `crate::section_link`.

use crate::pptx::Package;
use crate::text::{extract_texts, slide_title};

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct BlockProposal {
    pub name: String,
    pub pages: Vec<usize>,
    pub confidence: f64,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ProposalReport {
    pub proposer: String,
    pub slide_count: usize,
    pub blocks: Vec<BlockProposal>,
}

struct PageFeat {
    title: String,
    runs: usize,
    text_len: usize,
}

/// A page looks like a section opener when it carries a fresh title but
/// little body content (few runs / short text).
fn is_section_opener(f: &PageFeat, prev: &PageFeat) -> bool {
    !f.title.is_empty()
        && f.title != prev.title
        && f.title != "(无标题)"
        && (f.runs <= 4 || f.text_len < 60)
}

pub fn propose_blocks(pkg: &Package) -> Result<ProposalReport, String> {
    // Real sections are the preferred boundaries (§8 decision 5):
    // one section = one proposed block, named after the section.
    // The heuristic below only arbitrates decks without sections.
    if let Some(spans) = crate::pptx::deck_sections(pkg)? {
        let slide_count = spans.iter().map(|sp| sp.pages.len()).sum();
        let blocks = spans
            .iter()
            .enumerate()
            .map(|(i, sp)| BlockProposal {
                name: if sp.name.trim().is_empty() {
                    format!("第{}部分", i + 1)
                } else {
                    sp.name.clone()
                },
                pages: sp.pages.clone(),
                confidence: 1.0,
            })
            .collect();
        return Ok(ProposalReport {
            proposer: "section-v1".to_string(),
            slide_count,
            blocks,
        });
    }
    let slide_refs = crate::pptx::slides(pkg)?;
    let mut feats = Vec::new();
    for s in &slide_refs {
        let xml = pkg.get_text(&s.part)?;
        let texts = extract_texts(&xml);
        feats.push(PageFeat {
            title: slide_title(&xml),
            runs: texts.len(),
            text_len: texts.iter().map(|t| t.chars().count()).sum(),
        });
    }
    let mut blocks: Vec<BlockProposal> = Vec::new();
    for (i, f) in feats.iter().enumerate() {
        let boundary = i == 0 || is_section_opener(f, &feats[i - 1]);
        if boundary {
            blocks.push(BlockProposal {
                name: if f.title.is_empty() || f.title == "(无标题)" {
                    format!("第{}部分", blocks.len() + 1)
                } else {
                    f.title.clone()
                },
                pages: vec![i],
                confidence: if i == 0 { 0.6 } else { 0.9 },
            });
        } else if let Some(last) = blocks.last_mut() {
            last.pages.push(i);
        }
    }
    if blocks.len() <= 1 {
        for b in &mut blocks {
            b.confidence = 0.5;
        }
    }
    Ok(ProposalReport {
        proposer: "heuristic-v1".to_string(),
        slide_count: feats.len(),
        blocks,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pptx::Package;

    #[test]
    fn case2_proposal_groups_content_under_openers() {
        let Some(bytes) = crate::test_fixtures::case2_deck() else {
            eprintln!("{}", crate::test_fixtures::skip_msg("Case2 deck"));
            return;
        };
        let pkg = Package::load(&bytes).unwrap();
        let report = propose_blocks(&pkg).unwrap();
        assert_eq!(report.slide_count, 7);
        let total: usize = report.blocks.iter().map(|b| b.pages.len()).sum();
        assert_eq!(total, 7, "every page in exactly one block");
        // pages are contiguous and ordered
        let flat: Vec<usize> = report.blocks.iter().flat_map(|b| b.pages.clone()).collect();
        assert_eq!(flat, (0..7).collect::<Vec<_>>());
    }
}

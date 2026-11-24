//! Text extraction from slide XML: all <a:t> runs, plus a best-effort title
//! (the text of the first shape). Used for search and card labels.
//!
//! Phase 3: extraction parses with craft-xml instead of scanning for
//! `<a:t>` substrings — text inside comments or CDATA is no longer
//! picked up, entity and numeric character references are decoded by
//! the parser, and malformed XML yields no text instead of partial
//! matches.

pub fn extract_texts(slide_xml: &str) -> Vec<String> {
    let Ok(root) = crate::pptx::parse_xml(slide_xml, "slide part") else {
        return Vec::new();
    };
    root.find_all(craft_xml::Local("t"))
        .map(|el| el.text().to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

pub fn slide_title(slide_xml: &str) -> String {
    // The title placeholder usually comes first in document order; take the
    // first text run as the display title, falling back to "(无标题)".
    extract_texts(slide_xml)
        .into_iter()
        .next()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| "(无标题)".to_string())
}

pub fn slide_full_text(slide_xml: &str) -> String {
    extract_texts(slide_xml).join(" ")
}

/// The speaker-notes text of one slide (its 讲稿): the text of the notes
/// page's body placeholder, paragraphs joined by newlines. Empty when the
/// slide has no notes or the body is empty.
pub fn notes_text(pkg: &crate::pptx::Package, slide_part: &str) -> String {
    let rels_part = craft_opc::rels_path(slide_part);
    let rels_xml = match pkg.get_text(&rels_part) {
        Ok(x) => x,
        Err(_) => return String::new(),
    };
    let rels = crate::pptx::parse_rels(&rels_xml);
    let notes_rel = match rels
        .iter()
        .find(|r| craft_opc::type_is(&r.ty, "notesSlide"))
    {
        Some(r) => r,
        None => return String::new(),
    };
    let notes_part = crate::pptx::resolve_target(slide_part, &notes_rel.target);
    let xml = match pkg.get_text(&notes_part) {
        Ok(x) => x,
        Err(_) => return String::new(),
    };
    let Ok(root) = crate::pptx::parse_xml(&xml, &notes_part) else {
        return String::new();
    };
    // The notes page's body placeholder: a shape whose placeholder is
    // type="body"; its paragraphs' runs, paragraphs joined by newlines.
    for sp in root.find_all(craft_xml::Local("sp")) {
        let is_body = sp
            .descendants()
            .any(|d| d.is(craft_xml::Local("ph")) && d.attr("type") == Some("body"));
        if !is_body {
            continue;
        }
        let Some(txbody) = sp.find(craft_xml::Local("txBody")) else {
            continue;
        };
        let mut paras = Vec::new();
        for p in txbody.children(craft_xml::Local("p")) {
            let runs: Vec<String> = p
                .find_all(craft_xml::Local("t"))
                .map(|t| t.text().to_string())
                .filter(|s| !s.is_empty())
                .collect();
            if !runs.is_empty() {
                paras.push(runs.join(""));
            }
        }
        return paras.join("\n");
    }
    String::new()
}

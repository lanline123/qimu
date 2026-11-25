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

/// The page's one-line content summary — the per-page `desc`
/// (prototype page field `d`: 「架构图 · 分层与关键组件」, the
/// summary line shown under the title in #406, in the #101
/// thumbnail cell and on bind candidates).
///
/// Source choice (weighed against the prototype's semantics):
/// `d` is a summary of the page's VISIBLE content — in the
/// prototype's page editor it is the page's editable body text —
/// so it is NOT the speaker notes (those are the page's 讲稿,
/// already carried separately as the version's scripts). The
/// on-slide source is the first paragraph of visible text after
/// the title paragraph: on a laid-out deck that is the subtitle
/// placeholder when one exists, and the body's first paragraph
/// otherwise. Whitespace is collapsed, the result is capped at
/// 60 chars (a summary line, not the full body), and a page with
/// nothing after its title yields an empty desc — never a
/// fabricated one.
pub fn slide_desc(slide_xml: &str) -> String {
    let Ok(root) = crate::pptx::parse_xml(slide_xml, "slide part") else {
        return String::new();
    };
    let title = slide_title(slide_xml);
    let mut paras: Vec<String> = Vec::new();
    for p in root.find_all(craft_xml::Local("p")) {
        let raw: String = p
            .find_all(craft_xml::Local("t"))
            .map(|t| t.text().to_string())
            .collect();
        let text = raw.split_whitespace().collect::<Vec<_>>().join(" ");
        if !text.is_empty() {
            paras.push(text);
        }
    }
    // The first paragraph carries the title (the title is the
    // first text run on the slide); the desc is the first later
    // paragraph that is not just the title repeated (footers and
    // master furniture repeat it).
    for (i, para) in paras.iter().enumerate() {
        if i == 0 || para == &title || para == &paras[0] {
            continue;
        }
        let capped: String = para.chars().take(60).collect();
        return capped
            .trim_end_matches([' ', '·', '、', '，', ',', '；', ';', '：', ':'])
            .to_string();
    }
    String::new()
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

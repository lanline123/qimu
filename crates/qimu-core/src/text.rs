//! Text extraction from slide XML: all <a:t> runs, plus a best-effort title
//! (the text of the first shape). Used for search and card labels.

pub fn extract_texts(slide_xml: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = slide_xml;
    while let Some(start) = rest.find("<a:t>") {
        let after = &rest[start + 5..];
        match after.find("</a:t>") {
            Some(end) => {
                let raw = &after[..end];
                out.push(unescape(raw));
                rest = &after[end + 6..];
            }
            None => break,
        }
    }
    out
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
    let rels_part = {
        let (dir, file) = match slide_part.rfind('/') {
            Some(i) => (&slide_part[..i], &slide_part[i + 1..]),
            None => ("", slide_part),
        };
        format!("{dir}/_rels/{file}.rels")
    };
    let rels_xml = match pkg.get_text(&rels_part) {
        Ok(x) => x,
        Err(_) => return String::new(),
    };
    let rels = crate::pptx::parse_rels(&rels_xml);
    let notes_rel = match rels.iter().find(|r| r.raw.contains("/notesSlide")) {
        Some(r) => r,
        None => return String::new(),
    };
    let notes_part = crate::pptx::resolve_target(slide_part, &notes_rel.target);
    let xml = match pkg.get_text(&notes_part) {
        Ok(x) => x,
        Err(_) => return String::new(),
    };
    // find the <p:sp> block carrying the body placeholder
    for block in split_sp_blocks(&xml) {
        if block.contains("type=\"body\"") {
            let mut paras = Vec::new();
            for p in split_paragraphs(block) {
                let runs = extract_texts(p);
                if !runs.is_empty() {
                    paras.push(runs.join(""));
                }
            }
            return paras.join("\n");
        }
    }
    String::new()
}

fn split_sp_blocks(xml: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut rest = xml;
    while let Some(s) = rest.find("<p:sp>") {
        let after = &rest[s..];
        match after.find("</p:sp>") {
            Some(e) => {
                out.push(&after[..e + 7]);
                rest = &after[e + 7..];
            }
            None => break,
        }
    }
    out
}

fn split_paragraphs(block: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut rest = block;
    while let Some(s) = rest.find("<a:p>") {
        let after = &rest[s..];
        match after.find("</a:p>") {
            Some(e) => {
                out.push(&after[..e + 6]);
                rest = &after[e + 6..];
            }
            None => break,
        }
    }
    if out.is_empty() && block.contains("<a:p/>") {
        // single empty paragraph — no text
    }
    out
}

fn unescape(s: &str) -> String {
    s.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&amp;", "&")
}

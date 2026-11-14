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

fn unescape(s: &str) -> String {
    s.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&amp;", "&")
}

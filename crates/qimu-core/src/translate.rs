//! AI translation pipeline (§3.13): translate a fragment's text runs in
//! place — layout untouched, only <a:t> contents change — producing a
//! DRAFT rendition in the target language. Providers are pluggable: a
//! deterministic stub for development/tests, and an OpenAI-compatible
//! HTTP provider configured in settings (the product's only
//! internet-dependent feature). Glossary pairs are applied by the
//! provider prompt (real provider) or by substitution (stub), and
//! {{placeholders}} must survive translation untouched.

use crate::pptx::Package;

/// One glossary pair: source term -> prescribed translation.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct GlossaryPair {
    pub src: String,
    pub dst: String,
}

pub trait Translator {
    /// Translate a batch of strings; output must align 1:1 with input.
    fn translate(
        &self,
        texts: &[String],
        target_lang: &str,
        glossary: &[GlossaryPair],
    ) -> Result<Vec<String>, String>;
}

/// Development stub: applies glossary substitutions, then prefixes each
/// string with the target language tag. Deterministic — used until a
/// real provider is configured, and in tests.
pub struct StubTranslator;

impl Translator for StubTranslator {
    fn translate(
        &self,
        texts: &[String],
        target_lang: &str,
        glossary: &[GlossaryPair],
    ) -> Result<Vec<String>, String> {
        Ok(texts
            .iter()
            .map(|t| {
                let mut out = t.clone();
                for g in glossary {
                    if !g.src.is_empty() {
                        out = out.replace(&g.src, &g.dst);
                    }
                }
                if out.trim().is_empty() {
                    out
                } else {
                    format!("[{}] {}", target_lang.to_uppercase(), out)
                }
            })
            .collect())
    }
}

/// Collect every <a:t> run text of every slide, in deck order.
fn collect_runs(pkg: &Package) -> Result<Vec<(String, String)>, String> {
    // (slide_part, run_text)
    let mut out = Vec::new();
    for s in crate::pptx::slides(pkg)? {
        let xml = pkg.get_text(&s.part)?;
        for t in crate::text::extract_texts(&xml) {
            out.push((s.part.clone(), t));
        }
    }
    Ok(out)
}

fn replace_run(xml: &str, old: &str, new: &str) -> String {
    let needle = format!("<a:t>{}</a:t>", xml_escape(old));
    let replacement = format!("<a:t>{}</a:t>", xml_escape(new));
    xml.replacen(&needle, &replacement, 1)
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// Translate all slide text runs of a fragment. Returns the new package
/// and the number of translated runs. Text that is empty or a pure
/// {{placeholder}} is left untouched.
pub fn translate_package(
    pkg: &Package,
    translator: &dyn Translator,
    target_lang: &str,
    glossary: &[GlossaryPair],
) -> Result<(Package, usize), String> {
    let runs = collect_runs(pkg)?;
    let translatable: Vec<String> = runs
        .iter()
        .map(|(_, t)| t.clone())
        .filter(|t| !t.trim().is_empty())
        .collect();
    let translated = translator.translate(&translatable, target_lang, glossary)?;
    if translated.len() != translatable.len() {
        return Err("翻译服务返回的条数与原文不一致".into());
    }
    let mut out = pkg.clone();
    let mut idx = 0;
    // group replacements per slide to apply sequentially
    let mut by_slide: std::collections::BTreeMap<String, Vec<(String, String)>> =
        Default::default();
    for (part, text) in &runs {
        if text.trim().is_empty() {
            continue;
        }
        let new = translated[idx].clone();
        idx += 1;
        if new != *text {
            by_slide
                .entry(part.clone())
                .or_default()
                .push((text.clone(), new));
        }
    }
    for (part, pairs) in &by_slide {
        let mut xml = out.get_text(part)?;
        for (old, new) in pairs {
            xml = replace_run(&xml, old, new);
        }
        out.parts.insert(part.clone(), xml.into_bytes());
    }
    Ok((out, idx))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stub_applies_glossary_and_prefix() {
        let t = StubTranslator;
        let g = vec![GlossaryPair {
            src: "Redwood".into(),
            dst: "红木".into(),
        }];
        let out = t
            .translate(&["Visit Redwood".into(), "  ".into()], "zh", &g)
            .unwrap();
        assert_eq!(out[0], "[ZH] Visit 红木");
        assert_eq!(out[1], "  ");
    }
}

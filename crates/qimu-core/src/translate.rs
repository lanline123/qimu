//! AI translation pipeline (§3.13): translate a fragment's text runs in
//! place — layout untouched, only <a:t> contents change — producing a
//! DRAFT rendition in the target language. Providers are pluggable: a
//! deterministic stub for development/tests, and an OpenAI-compatible
//! HTTP provider configured in settings (the product's only
//! internet-dependent feature). Glossary pairs are applied by the
//! provider prompt (real provider) or by substitution (stub), and
//! {{placeholders}} must survive translation untouched.
//!
//! Long decks are translated in chunks of at most
//! [`TRANSLATE_CHUNK_RUNS`] strings per provider call (one provider
//! request per chunk, progress reported after each), so a single
//! request never carries a whole deck's text. After every chunk the
//! glossary is *enforced* deterministically (see [`enforce_glossary`]):
//! a prescribed rendering the model dropped is repaired when the term
//! survives untranslated, and counted as unresolved — never silently
//! ignored — when it does not.

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

/// Most strings sent to a provider in one call. Slide runs and speaker
/// scripts are chunked to this size so long decks stay well inside
/// provider context / output limits and progress stays meaningful.
pub const TRANSLATE_CHUNK_RUNS: usize = 40;

/// Outcome of glossary enforcement over a set of translated strings.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct GlossaryReport {
    /// Strings whose source contained a glossary term.
    pub checked: usize,
    /// Translations where the prescribed rendering was missing but the
    /// term survived untranslated and was substituted deterministically.
    pub repaired: usize,
    /// Translations where neither the prescribed rendering nor the
    /// source term survives (the model paraphrased the term away):
    /// counted so callers can surface them for human proofreading.
    pub unresolved: usize,
}

impl GlossaryReport {
    pub fn merge(&mut self, other: GlossaryReport) {
        self.checked += other.checked;
        self.repaired += other.repaired;
        self.unresolved += other.unresolved;
    }
}

/// Enforce the glossary on one translated string. For every pair whose
/// source term appears in the source string, the prescribed rendering
/// must appear in the translation; if the term instead survived
/// untranslated it is substituted in place (deterministic repair),
/// otherwise the miss is counted as unresolved.
pub fn enforce_glossary(
    source: &str,
    translated: &str,
    glossary: &[GlossaryPair],
) -> (String, GlossaryReport) {
    let mut out = translated.to_string();
    let mut rep = GlossaryReport::default();
    for g in glossary {
        if g.src.is_empty() || g.dst.is_empty() || !source.contains(&g.src) {
            continue;
        }
        rep.checked += 1;
        if out.contains(&g.dst) {
            continue;
        }
        if out.contains(&g.src) {
            out = out.replace(&g.src, &g.dst);
            rep.repaired += 1;
        } else {
            rep.unresolved += 1;
        }
    }
    (out, rep)
}

/// Translate a list of strings in chunks of [`TRANSLATE_CHUNK_RUNS`],
/// enforcing the glossary after each chunk and reporting progress as
/// (done, total) after each chunk. Output aligns 1:1 with input.
pub fn translate_texts_chunked(
    translator: &dyn Translator,
    texts: &[String],
    target_lang: &str,
    glossary: &[GlossaryPair],
    on_progress: &mut dyn FnMut(usize, usize),
) -> Result<(Vec<String>, GlossaryReport), String> {
    let total = texts.len();
    let mut out: Vec<String> = Vec::with_capacity(total);
    let mut report = GlossaryReport::default();
    for chunk in texts.chunks(TRANSLATE_CHUNK_RUNS) {
        let part = translator.translate(chunk, target_lang, glossary)?;
        if part.len() != chunk.len() {
            return Err("翻译服务返回的条数与原文不一致".into());
        }
        for (src, dst) in chunk.iter().zip(part.iter()) {
            let (fixed, rep) = enforce_glossary(src, dst, glossary);
            report.merge(rep);
            out.push(fixed);
        }
        on_progress(out.len(), total);
    }
    if total == 0 {
        on_progress(0, 0);
    }
    Ok((out, report))
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

/// Decode the XML entities a text node may carry (named,
/// decimal and hex character references). Collection decodes
/// run text through the XML parser, so replacement must
/// compare against the decoded form too (audit L6).
fn xml_unescape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(pos) = rest.find('&') {
        out.push_str(&rest[..pos]);
        let tail = &rest[pos..];
        let Some(end) = tail.find(';') else {
            out.push_str(tail);
            return out;
        };
        let ent = &tail[1..end];
        let decoded = match ent {
            "amp" => Some('&'),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "quot" => Some('"'),
            "apos" => Some('\''),
            _ => ent
                .strip_prefix("#x")
                .and_then(|h| u32::from_str_radix(h, 16).ok())
                .or_else(|| {
                    ent.strip_prefix('#')
                        .and_then(|d| d.parse::<u32>().ok())
                })
                .and_then(char::from_u32),
        };
        match decoded {
            Some(c) => {
                out.push(c);
                rest = &tail[end + 1..];
            }
            None => {
                out.push('&');
                rest = &tail[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

/// Replace the `nth` (0-based) `<a:t>` element whose DECODED
/// text equals `old`, writing `new` escaped. The old code
/// string-replaced the first raw `<a:t>{escaped old}</a:t>`
/// occurrence (audit L6): runs stored with character
/// references or CDATA never matched (the run was silently
/// counted as translated but left in the source language),
/// and two identical runs on one slide — or a translation
/// equal to a sibling run's source — collided on the first
/// hit. Occurrence-exact, decode-aware matching fixes both.
/// Returns the new xml and whether a replacement happened.
fn replace_run_nth(xml: &str, old: &str, new: &str, nth: usize) -> (String, bool) {
    let mut seen = 0usize;
    let mut pos = 0usize;
    while let Some(off) = xml[pos..].find("<a:t") {
        let tag_start = pos + off;
        let after = &xml[tag_start + 4..];
        // Only `<a:t>` / `<a:t ...attrs>` is a text run: the
        // next char must end or space the tag name, otherwise
        // this is `<a:tbl>`, `<a:tc>`, … and not a run at all.
        match after.chars().next() {
            Some('>') | Some('/') => {}
            Some(c) if c.is_whitespace() => {}
            _ => {
                pos = tag_start + 4;
                continue;
            }
        }
        // A self-closed `<a:t/>` carries no text and is skipped.
        let Some(gt) = after.find('>') else { break };
        if after.as_bytes().get(gt.saturating_sub(1)) == Some(&b'/') {
            pos = tag_start + 4 + gt + 1;
            continue;
        }
        let inner_start = tag_start + 4 + gt + 1;
        let Some(close) = xml[inner_start..].find("</a:t>") else { break };
        let inner_end = inner_start + close;
        if xml_unescape(&xml[inner_start..inner_end]) == old {
            if seen == nth {
                let mut out = String::with_capacity(xml.len() + new.len());
                out.push_str(&xml[..inner_start]);
                out.push_str(&xml_escape(new));
                out.push_str(&xml[inner_end..]);
                return (out, true);
            }
            seen += 1;
        }
        pos = inner_end + 6;
    }
    (xml.to_string(), false)
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// Number of non-empty slide text runs a translation of this package
/// will process (the denominator for progress reporting).
pub fn count_translatable_runs(pkg: &Package) -> Result<usize, String> {
    Ok(collect_runs(pkg)?
        .iter()
        .filter(|(_, t)| !t.trim().is_empty())
        .count())
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
    let (out, n, _report) =
        translate_package_chunked(pkg, translator, target_lang, glossary, &mut |_, _| {})?;
    Ok((out, n))
}

/// Chunked variant of [`translate_package`]: provider calls are split
/// by [`translate_texts_chunked`], progress is reported per chunk, and
/// the glossary enforcement outcome is returned alongside.
pub fn translate_package_chunked(
    pkg: &Package,
    translator: &dyn Translator,
    target_lang: &str,
    glossary: &[GlossaryPair],
    on_progress: &mut dyn FnMut(usize, usize),
) -> Result<(Package, usize, GlossaryReport), String> {
    let runs = collect_runs(pkg)?;
    let translatable: Vec<String> = runs
        .iter()
        .map(|(_, t)| t.clone())
        .filter(|t| !t.trim().is_empty())
        .collect();
    let (translated, report) = translate_texts_chunked(
        translator,
        &translatable,
        target_lang,
        glossary,
        on_progress,
    )?;
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
    let mut failed = 0usize;
    // Occurrence counters per (slide, source text): the runs
    // were collected in deck order, so the k-th pair for a
    // given text targets the k-th matching element.
    let mut occurrence: std::collections::HashMap<(String, String), usize> =
        Default::default();
    for (part, pairs) in &by_slide {
        let mut xml = out.get_text(part)?;
        for (old, new) in pairs {
            let key = (part.clone(), old.clone());
            let nth = {
                let e = occurrence.entry(key).or_default();
                let v = *e;
                *e += 1;
                v
            };
            let (next, applied) = replace_run_nth(&xml, old, new, nth);
            xml = next;
            if !applied {
                failed += 1;
            }
        }
        out.parts.insert(part.clone(), xml.into_bytes());
    }
    // The returned count is runs actually translated: a run
    // whose replacement could not be applied is not counted
    // (and is visible on stderr) instead of being silently
    // reported as translated (audit L6).
    if failed > 0 {
        eprintln!("translate: {failed} run replacement(s) could not be applied");
    }
    Ok((out, idx - failed, report))
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

    #[test]
    fn enforce_repairs_untranslated_term_and_counts_unresolved() {
        let g = vec![
            GlossaryPair {
                src: "华东".into(),
                dst: "EastChina".into(),
            },
            GlossaryPair {
                src: "渠道".into(),
                dst: "Channel".into(),
            },
        ];
        // term survived untranslated -> deterministic repair
        let (out, rep) = enforce_glossary("华东市场", "华东 market", &g);
        assert_eq!(out, "EastChina market");
        assert_eq!(rep.checked, 1);
        assert_eq!(rep.repaired, 1);
        assert_eq!(rep.unresolved, 0);
        // prescribed rendering already present -> clean
        let (out, rep) = enforce_glossary("华东市场", "EastChina market", &g);
        assert_eq!(out, "EastChina market");
        assert_eq!(rep.checked, 1);
        assert_eq!(rep.repaired + rep.unresolved, 0);
        // term paraphrased away entirely -> unresolved, text untouched
        let (out, rep) = enforce_glossary("销售渠道", "sales network", &g);
        assert_eq!(out, "sales network");
        assert_eq!(rep.checked, 1);
        assert_eq!(rep.unresolved, 1);
    }

    #[test]
    fn chunked_translation_reports_progress_per_chunk() {
        struct Echo;
        impl Translator for Echo {
            fn translate(
                &self,
                texts: &[String],
                target_lang: &str,
                _glossary: &[GlossaryPair],
            ) -> Result<Vec<String>, String> {
                Ok(texts
                    .iter()
                    .map(|t| format!("{}:{}", target_lang, t))
                    .collect())
            }
        }
        let texts: Vec<String> = (0..(TRANSLATE_CHUNK_RUNS + 5))
            .map(|n| format!("t{n}"))
            .collect();
        let mut seen: Vec<(usize, usize)> = Vec::new();
        let (out, rep) =
            translate_texts_chunked(&Echo, &texts, "en", &[], &mut |d, t| seen.push((d, t)))
                .unwrap();
        assert_eq!(out.len(), texts.len());
        assert_eq!(out[0], "en:t0");
        assert_eq!(
            seen,
            vec![
                (TRANSLATE_CHUNK_RUNS, texts.len()),
                (texts.len(), texts.len())
            ]
        );
        assert_eq!(rep, GlossaryReport::default());
    }
}

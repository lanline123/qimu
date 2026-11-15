//! PPTX package surgery (Phase-0 POC, fragment Route B: direct OOXML
//! package manipulation, using DeckCraft's code as the relationship-
//! handling reference — spec §8.6).
//!
//! Imported decks are stored ONLY in split form (spec §3.1): each version
//! is a small PPTX fragment, the deck is manifest + fragment references.
//! The safety net implemented here: after splitting, reassemble the deck
//! from the fragments and verify it page-by-page against the source
//! (page count, per-page fingerprints, structural integrity). A failed
//! check must roll back the whole import.

use serde::Serialize;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::io::{Cursor, Read, Write};

pub type PptxResult<T> = Result<T, String>;

const CONTENT_TYPES: &str = "[Content_Types].xml";
const PRESENTATION: &str = "ppt/presentation.xml";
const PRESENTATION_RELS: &str = "ppt/_rels/presentation.xml.rels";

/// An OOXML package in memory: part name -> bytes (directory entries dropped).
#[derive(Debug, Clone)]
pub struct Package {
    pub parts: BTreeMap<String, Vec<u8>>,
}

impl Package {
    pub fn load(bytes: &[u8]) -> PptxResult<Package> {
        let mut zip = zip::ZipArchive::new(Cursor::new(bytes))
            .map_err(|e| format!("not a valid zip/pptx: {e}"))?;
        let mut parts = BTreeMap::new();
        for i in 0..zip.len() {
            let mut f = zip.by_index(i).map_err(|e| e.to_string())?;
            if f.is_dir() {
                continue;
            }
            let name = f.name().to_string();
            let mut buf = Vec::new();
            f.read_to_end(&mut buf).map_err(|e| e.to_string())?;
            parts.insert(name, buf);
        }
        Ok(Package { parts })
    }

    pub fn to_bytes(&self) -> PptxResult<Vec<u8>> {
        let mut out = Cursor::new(Vec::new());
        {
            let mut zw = zip::ZipWriter::new(&mut out);
            let opts = zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Stored);
            for (name, bytes) in &self.parts {
                zw.start_file(name, opts).map_err(|e| e.to_string())?;
                zw.write_all(bytes).map_err(|e| e.to_string())?;
            }
            zw.finish().map_err(|e| e.to_string())?;
        }
        Ok(out.into_inner())
    }

    pub fn get(&self, name: &str) -> PptxResult<&[u8]> {
        self.parts
            .get(name)
            .map(|v| v.as_slice())
            .ok_or_else(|| format!("missing part {name}"))
    }

    pub fn get_text(&self, name: &str) -> PptxResult<String> {
        String::from_utf8(self.get(name)?.to_vec())
            .map_err(|_| format!("part {name} is not valid UTF-8"))
    }
}

/// One slide of a deck, in presentation order.
#[derive(Debug, Clone, Serialize)]
pub struct SlideRef {
    /// 0-based position in the deck.
    pub index: usize,
    /// Part name, e.g. "ppt/slides/slide3.xml".
    pub part: String,
    /// Relationship ID in presentation.xml (r:id).
    pub rid: String,
    /// SHA-256 over slide XML + its .rels (hex).
    pub fingerprint: String,
}

/// Extract `<tag ... />` (or `<tag ...>...</tag>` start-tag) spans containing
/// `marker`, returning each element's raw text. POC-grade string scanning;
/// attribute order inside an element is not assumed.
pub(crate) fn scan_elements<'a>(xml: &'a str, open: &str) -> Vec<&'a str> {
    let mut out = Vec::new();
    let mut rest = xml;
    while let Some(pos) = rest.find(open) {
        // Guard against prefix collisions: "<Relationship" must not match
        // the "<Relationships" root, "<p:sldId" must not match
        // "<p:sldIdLst". A tag name continuing with a letter/digit means
        // this is a different, longer element — skip the occurrence.
        let after_open = &rest[pos + open.len()..];
        if after_open
            .chars()
            .next()
            .map(|c| c.is_ascii_alphanumeric())
            .unwrap_or(false)
        {
            rest = after_open;
            continue;
        }
        let start = pos;
        let after = &rest[start..];
        // Element ends at "/>" for empty elements (Relationship, Override,
        // sldId are all empty elements in the parts we scan).
        if let Some(end) = after.find("/>") {
            out.push(&after[..end + 2]);
            rest = &after[end + 2..];
        } else {
            break;
        }
    }
    out
}

pub(crate) fn attr<'a>(element: &'a str, name: &str) -> Option<&'a str> {
    let pat = format!("{name}=\"");
    let start = element.find(&pat)? + pat.len();
    let end = element[start..].find('"')? + start;
    Some(&element[start..end])
}

/// Resolve a relationship target against the directory of its source part.
pub(crate) fn resolve_target(source_part: &str, target: &str) -> String {
    if let Some(stripped) = target.strip_prefix('/') {
        return normalize_path(stripped);
    }
    let base = match source_part.rfind('/') {
        Some(i) => &source_part[..i],
        None => "",
    };
    // For a .rels file, the source part is the part it describes:
    // "ppt/slides/_rels/slide1.xml.rels" describes "ppt/slides/slide1.xml".
    let base = base.strip_suffix("_rels").map(|b| b.trim_end_matches('/')).unwrap_or(base);
    if base.is_empty() {
        normalize_path(target)
    } else {
        normalize_path(&format!("{base}/{target}"))
    }
}

fn normalize_path(p: &str) -> String {
    let mut out: Vec<&str> = Vec::new();
    for seg in p.split('/') {
        match seg {
            "" | "." => {}
            ".." => {
                out.pop();
            }
            s => out.push(s),
        }
    }
    out.join("/")
}

pub(crate) struct Rel {
    pub id: String,
    pub target: String,
    pub external: bool,
    pub raw: String,
}

pub(crate) fn parse_rels(xml: &str) -> Vec<Rel> {
    scan_elements(xml, "<Relationship")
        .into_iter()
        .filter_map(|el| {
            Some(Rel {
                id: attr(el, "Id")?.to_string(),
                target: attr(el, "Target")?.to_string(),
                external: attr(el, "TargetMode") == Some("External"),
                raw: el.to_string(),
            })
        })
        .collect()
}

/// Slides of the deck in presentation order, with per-page fingerprints
/// (identity by part + content, never by printed folio — spec §3.1).
pub fn slides(pkg: &Package) -> PptxResult<Vec<SlideRef>> {
    let pres = pkg.get_text(PRESENTATION)?;
    let rels = parse_rels(&pkg.get_text(PRESENTATION_RELS)?);
    let by_id: BTreeMap<&str, &Rel> = rels.iter().map(|r| (r.id.as_str(), r)).collect();

    let mut out = Vec::new();
    for el in scan_elements(&pres, "<p:sldId") {
        let rid = attr(el, "r:id").ok_or("sldId without r:id")?;
        let rel = by_id.get(rid).ok_or_else(|| format!("sldId references unknown {rid}"))?;
        let part = resolve_target(PRESENTATION, &rel.target);
        let fp = page_fingerprint(pkg, &part);
        out.push(SlideRef { index: out.len(), part, rid: rid.to_string(), fingerprint: fp });
    }
    Ok(out)
}

/// Per-page fingerprint: slide XML plus its relationship table, so a page
/// whose media/layout links change is a different page.
pub fn page_fingerprint(pkg: &Package, slide_part: &str) -> String {
    // Fingerprint = slide XML + the CONTENT of everything the slide
    // directly references (by relationship type). It deliberately does
    // not hash relationship target NAMES: assembly renames parts when
    // merging fragments, and a rename must not make a page look
    // "modified". Changing a referenced part's content (media, layout,
    // chart) does change the fingerprint.
    let mut h = Sha256::new();
    if let Some(bytes) = pkg.parts.get(slide_part) {
        h.update(bytes);
    }
    let rels_name = rels_part_name(slide_part);
    if let Some(bytes) = pkg.parts.get(&rels_name) {
        let text = String::from_utf8_lossy(bytes);
        let mut entries: Vec<String> = parse_rels(&text)
            .iter()
            .map(|r| {
                let ty = attr(&r.raw, "Type").unwrap_or("");
                if r.external {
                    format!("{ty}|ext|{}", r.target)
                } else {
                    let abs = resolve_target(slide_part, &r.target);
                    let content_hash = pkg
                        .parts
                        .get(&abs)
                        .map(|b| {
                            let mut hh = Sha256::new();
                            hh.update(b);
                            format!("{:x}", hh.finalize())
                        })
                        .unwrap_or_default();
                    format!("{ty}|{content_hash}")
                }
            })
            .collect();
        entries.sort();
        for e in entries {
            h.update(e.as_bytes());
            h.update(b"\n");
        }
    }
    format!("{:x}", h.finalize())
}

fn rels_part_name(part: &str) -> String {
    match part.rfind('/') {
        Some(i) => format!("{}/_rels/{}.rels", &part[..i], &part[i + 1..]),
        None => format!("_rels/{part}.rels"),
    }
}

fn remove_elements_containing(xml: &str, open: &str, keep: impl Fn(&str) -> bool) -> String {
    let mut out = String::with_capacity(xml.len());
    let mut rest = xml;
    while let Some(pos) = rest.find(open) {
        out.push_str(&rest[..pos]);
        let after = &rest[pos..];
        match after.find("/>") {
            Some(end) => {
                let el = &after[..end + 2];
                if keep(el) {
                    out.push_str(el);
                }
                rest = &after[end + 2..];
            }
            None => {
                out.push_str(after);
                rest = "";
                break;
            }
        }
    }
    out.push_str(rest);
    out
}

/// Split a deck into one single-slide fragment per slide (Route B pruning:
/// shared parts are copied wholesale at POC stage; slide/notes parts of
/// other slides are dropped and the three bookkeeping parts —
/// presentation.xml, its .rels, [Content_Types].xml — are pruned).
pub fn split(pkg: &Package) -> PptxResult<Vec<Package>> {
    let deck_slides = slides(pkg)?;
    let pres_xml = pkg.get_text(PRESENTATION)?;
    let sld_elements = scan_elements(&pres_xml, "<p:sldId");
    if sld_elements.len() != deck_slides.len() {
        return Err("sldId count mismatch while parsing presentation.xml".into());
    }

    // Notes part owned by each slide (via its .rels), if any.
    let mut notes_of: BTreeMap<String, Option<String>> = BTreeMap::new();
    for s in &deck_slides {
        let rels_name = rels_part_name(&s.part);
        let note = pkg.parts.get(&rels_name).and_then(|bytes| {
            let text = String::from_utf8_lossy(bytes);
            parse_rels(&text)
                .into_iter()
                .find(|r| r.target.contains("notesSlide"))
                .map(|r| resolve_target(&s.part, &r.target))
        });
        notes_of.insert(s.part.clone(), note);
    }

    let mut fragments = Vec::new();
    for (i, s) in deck_slides.iter().enumerate() {
        // Keep only what this slide actually reaches through its
        // relationship closure (layout/master/theme chain, media,
        // charts, notes) plus package scaffolding. Everything else —
        // above all other slides' media — is pruned, so a fragment is
        // proportional to its own page, not to the whole deck.
        let mut keep: BTreeSet<String> = BTreeSet::new();
        let mut stack = vec![s.part.clone()];
        // The notes master is scaffolding (kept below), but its own
        // relationship closure (its theme) must come with it.
        for name in pkg.parts.keys() {
            if name.starts_with("ppt/notesMasters/") && name.ends_with(".xml") {
                stack.push(name.clone());
            }
        }
        while let Some(p) = stack.pop() {
            if !keep.insert(p.clone()) {
                continue;
            }
            let rels_name = rels_part_name(&p);
            if let Some(bytes) = pkg.parts.get(&rels_name) {
                keep.insert(rels_name.clone());
                let text = String::from_utf8_lossy(bytes);
                for r in parse_rels(&text) {
                    if !r.external {
                        let t = resolve_target(&p, &r.target);
                        if pkg.parts.contains_key(&t) {
                            stack.push(t);
                        }
                    }
                }
            }
        }
        for name in pkg.parts.keys() {
            if name == CONTENT_TYPES
                || name == "_rels/.rels"
                || name.starts_with("docProps/")
                || name == PRESENTATION
                || name == PRESENTATION_RELS
                || name == "ppt/presProps.xml"
                || name == "ppt/viewProps.xml"
                || name == "ppt/tableStyles.xml"
                || name.starts_with("ppt/notesMasters/")
                || name.starts_with("ppt/fonts/")
            {
                keep.insert(name.clone());
            }
        }
        let _ = &notes_of;
        let removed: BTreeSet<String> = pkg
            .parts
            .keys()
            .filter(|k| !keep.contains(*k))
            .cloned()
            .collect();

        let mut parts: BTreeMap<String, Vec<u8>> = BTreeMap::new();
        for (name, bytes) in &pkg.parts {
            if removed.contains(name) {
                continue;
            }
            parts.insert(name.clone(), bytes.clone());
        }

        // Prune presentation.xml: keep only this slide's sldId element.
        let kept_el = sld_elements[i];
        let pruned_pres = {
            let begin = pres_xml.find("<p:sldIdLst").ok_or("no sldIdLst")?;
            let open_end = pres_xml[begin..].find('>').ok_or("bad sldIdLst")? + begin + 1;
            let close = pres_xml.find("</p:sldIdLst>").ok_or("no /sldIdLst")?;
            format!("{}{}{}", &pres_xml[..open_end], kept_el, &pres_xml[close..])
        };
        parts.insert(PRESENTATION.to_string(), pruned_pres.into_bytes());

        // Prune presentation rels: drop relationships to removed parts.
        let pres_rels = pkg.get_text(PRESENTATION_RELS)?;
        let removed_targets: BTreeSet<String> = removed.iter().cloned().collect();
        let pruned_rels = remove_elements_containing(&pres_rels, "<Relationship", |el| {
            match attr(el, "Target") {
                Some(t) => !removed_targets.contains(&resolve_target(PRESENTATION, t)),
                None => true,
            }
        });
        parts.insert(PRESENTATION_RELS.to_string(), pruned_rels.into_bytes());

        // Prune [Content_Types].xml overrides for removed parts.
        let ct = pkg.get_text(CONTENT_TYPES)?;
        let pruned_ct = remove_elements_containing(&ct, "<Override", |el| {
            match attr(el, "PartName") {
                Some(p) => !removed.contains(p.trim_start_matches('/')),
                None => true,
            }
        });
        parts.insert(CONTENT_TYPES.to_string(), pruned_ct.into_bytes());

        fragments.push(Package { parts });
    }
    Ok(fragments)
}

/// Reassemble a full deck from single-slide fragments produced by `split`.
/// Original relationship IDs and part names are preserved by construction,
/// so the bookkeeping parts are unions of the fragments' pruned versions.
pub fn reassemble(fragments: &[Package]) -> PptxResult<Package> {
    if fragments.is_empty() {
        return Err("no fragments to reassemble".into());
    }
    let mut parts: BTreeMap<String, Vec<u8>> = BTreeMap::new();
    let mut sld_elements: Vec<String> = Vec::new();
    let mut pres_rel_by_id: BTreeMap<String, String> = BTreeMap::new();
    let mut overrides: BTreeMap<String, String> = BTreeMap::new();
    let mut defaults: BTreeMap<String, String> = BTreeMap::new();
    let mut pres_template: Option<String> = None;
    let mut rels_template: Option<String> = None;
    let mut ct_template: Option<String> = None;

    for frag in fragments {
        let fslides = slides(frag)?;
        if fslides.len() != 1 {
            return Err("fragment does not contain exactly one slide".into());
        }
        let pres = frag.get_text(PRESENTATION)?;
        let els = scan_elements(&pres, "<p:sldId");
        sld_elements.push(els[0].to_string());
        if pres_template.is_none() {
            pres_template = Some(pres);
            rels_template = Some(frag.get_text(PRESENTATION_RELS)?);
            ct_template = Some(frag.get_text(CONTENT_TYPES)?);
        }
        for rel in parse_rels(&frag.get_text(PRESENTATION_RELS)?) {
            pres_rel_by_id.entry(rel.id).or_insert(rel.raw);
        }
        let ct = frag.get_text(CONTENT_TYPES)?;
        for el in scan_elements(&ct, "<Override") {
            if let Some(p) = attr(el, "PartName") {
                overrides.entry(p.to_string()).or_insert(el.to_string());
            }
        }
        for el in scan_elements(&ct, "<Default") {
            if let Some(e) = attr(el, "Extension") {
                defaults.entry(e.to_string()).or_insert(el.to_string());
            }
        }
        for (name, bytes) in &frag.parts {
            if name == PRESENTATION || name == PRESENTATION_RELS || name == CONTENT_TYPES {
                continue;
            }
            match parts.get(name) {
                Some(existing) if existing != bytes => {
                    return Err(format!("fragments disagree on shared part {name}"));
                }
                _ => {
                    parts.insert(name.clone(), bytes.clone());
                }
            }
        }
    }

    // presentation.xml: template with the full sldIdLst restored in order.
    let pres = pres_template.unwrap();
    let begin = pres.find("<p:sldIdLst").ok_or("no sldIdLst")?;
    let open_end = pres[begin..].find('>').ok_or("bad sldIdLst")? + begin + 1;
    let close = pres.find("</p:sldIdLst>").ok_or("no /sldIdLst")?;
    let full_pres = format!("{}{}{}", &pres[..open_end], sld_elements.concat(), &pres[close..]);
    parts.insert(PRESENTATION.to_string(), full_pres.into_bytes());

    // presentation rels: template shell with the union of relationships.
    let rels_t = rels_template.unwrap();
    let rels = rebuild_container(&rels_t, "<Relationships", "</Relationships>", &pres_rel_by_id.values().cloned().collect::<Vec<_>>().concat());
    parts.insert(PRESENTATION_RELS.to_string(), rels.into_bytes());

    // [Content_Types].xml: defaults + union of overrides.
    let ct_t = ct_template.unwrap();
    let mut ct_body = String::new();
    for d in defaults.values() {
        ct_body.push_str(d);
    }
    for o in overrides.values() {
        ct_body.push_str(o);
    }
    let ct = rebuild_container(&ct_t, "<Types", "</Types>", &ct_body);
    parts.insert(CONTENT_TYPES.to_string(), ct.into_bytes());

    Ok(Package { parts })
}

fn rebuild_container(template: &str, open_marker: &str, close_marker: &str, inner: &str) -> String {
    let Some(begin) = template.find(open_marker) else { return template.to_string() };
    let Some(open_end) = template[begin..].find('>').map(|i| begin + i + 1) else {
        return template.to_string();
    };
    let Some(close) = template.find(close_marker) else { return template.to_string() };
    format!("{}{}{}", &template[..open_end], inner, &template[close..])
}

/// Structural integrity: every non-external relationship target exists,
/// and every part is covered by [Content_Types].xml.
pub fn structural_errors(pkg: &Package) -> Vec<String> {
    let mut errors = Vec::new();
    // Content-type coverage.
    let mut covered: BTreeSet<String> = BTreeSet::new();
    let mut default_exts: BTreeSet<String> = BTreeSet::new();
    if let Ok(ct) = pkg.get_text(CONTENT_TYPES) {
        for el in scan_elements(&ct, "<Override") {
            if let Some(p) = attr(el, "PartName") {
                covered.insert(p.trim_start_matches('/').to_string());
            }
        }
        for el in scan_elements(&ct, "<Default") {
            if let Some(e) = attr(el, "Extension") {
                default_exts.insert(e.to_lowercase());
            }
        }
    }
    for name in pkg.parts.keys() {
        if name == CONTENT_TYPES {
            continue;
        }
        let ext = name.rsplit('.').next().unwrap_or("").to_lowercase();
        if !covered.contains(name) && !default_exts.contains(&ext) {
            errors.push(format!("part {name} not covered by [Content_Types].xml"));
        }
    }
    // Relationship targets exist.
    for (name, bytes) in &pkg.parts {
        if !name.ends_with(".rels") {
            continue;
        }
        // The described part: strip "_rels/" and ".rels".
        let described = name
            .replace("/_rels/", "/")
            .trim_end_matches(".rels")
            .to_string();
        let text = String::from_utf8_lossy(bytes);
        for rel in parse_rels(&text) {
            if rel.external {
                continue;
            }
            let target = resolve_target(&described, &rel.target);
            if !pkg.parts.contains_key(&target) {
                errors.push(format!("{name}: relationship {} -> missing part {target}", rel.id));
            }
        }
    }
    errors
}

#[derive(Debug, Serialize)]
pub struct VerifyReport {
    pub slide_count: usize,
    pub source_fingerprints: Vec<String>,
    pub reassembled_fingerprints: Vec<String>,
    pub fingerprints_match: bool,
    pub structural_errors: Vec<String>,
    pub ok: bool,
}

/// The §3.1 import safety net: split the deck, reassemble it from the
/// fragments alone, and compare page-by-page against the source.
pub fn split_reassemble_verify(source: &Package) -> PptxResult<VerifyReport> {
    let src_slides = slides(source)?;
    let fragments = split(source)?;
    let reassembled = reassemble(&fragments)?;
    let re_slides = slides(&reassembled)?;

    let source_fps: Vec<String> = src_slides.iter().map(|s| s.fingerprint.clone()).collect();
    let re_fps: Vec<String> = re_slides.iter().map(|s| s.fingerprint.clone()).collect();
    let fingerprints_match = source_fps == re_fps;
    let mut errs = structural_errors(&reassembled);
    if src_slides.len() != re_slides.len() {
        errs.push(format!(
            "slide count changed: {} -> {}",
            src_slides.len(),
            re_slides.len()
        ));
    }
    Ok(VerifyReport {
        slide_count: src_slides.len(),
        source_fingerprints: source_fps,
        reassembled_fingerprints: re_fps,
        fingerprints_match,
        ok: fingerprints_match && errs.is_empty(),
        structural_errors: errs,
    })
}

/// Whole-package fingerprint (used for dedup / identical-paste detection).
pub fn package_fingerprint(pkg: &Package) -> String {
    let mut h = Sha256::new();
    for (name, bytes) in &pkg.parts {
        h.update(name.as_bytes());
        h.update(bytes);
    }
    format!("{:x}", h.finalize())
}

fn xml_escape_text(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// Write speaker notes (讲稿) into the notes page of the slide at
/// `slide_idx` (0-based, deck order). Only works when the slide already
/// has a notes page with a body placeholder — returns false otherwise
/// (callers treat notes as best-effort on export).
pub fn set_notes_text(pkg: &mut Package, slide_idx: usize, text: &str) -> bool {
    let slide_refs = match slides(pkg) {
        Ok(s) => s,
        Err(_) => return false,
    };
    let slide_part = match slide_refs.get(slide_idx) {
        Some(s) => s.part.clone(),
        None => return false,
    };
    let (dir, file) = match slide_part.rfind('/') {
        Some(i) => (&slide_part[..i], &slide_part[i + 1..]),
        None => return false,
    };
    let rels_part = format!("{dir}/_rels/{file}.rels");
    let rels_xml = match pkg.get_text(&rels_part) {
        Ok(x) => x,
        Err(_) => return false,
    };
    let rels = parse_rels(&rels_xml);
    let notes_rel = match rels.iter().find(|r| r.raw.contains("/notesSlide")) {
        Some(r) => r,
        None => return false,
    };
    let notes_part = resolve_target(&slide_part, &notes_rel.target);
    let xml = match pkg.get_text(&notes_part) {
        Ok(x) => x,
        Err(_) => return false,
    };
    // locate the body placeholder's <p:sp> block
    let mut search_from = 0;
    let (sp_start, sp_end) = loop {
        let Some(rel_s) = xml[search_from..].find("<p:sp>") else {
            return false;
        };
        let s = search_from + rel_s;
        let Some(rel_e) = xml[s..].find("</p:sp>") else {
            return false;
        };
        let e = s + rel_e + "</p:sp>".len();
        if xml[s..e].contains("type=\"body\"") {
            break (s, e);
        }
        search_from = e;
    };
    let block = &xml[sp_start..sp_end];
    let Some(tb_rel) = block.find("<p:txBody>") else {
        return false;
    };
    let tb_start = sp_start + tb_rel;
    let Some(tb_close_rel) = xml[tb_start..].find("</p:txBody>") else {
        return false;
    };
    let tb_close = tb_start + tb_close_rel;
    // keep everything inside txBody before the first paragraph
    // (a:bodyPr, a:lstStyle), replace the paragraphs
    let inner = &xml[tb_start + "<p:txBody>".len()..tb_close];
    let keep_len = inner
        .find("<a:p>")
        .or_else(|| inner.find("<a:p/>"))
        .unwrap_or(inner.len());
    let mut paras = String::new();
    for line in text.split('\n') {
        if line.is_empty() {
            paras.push_str("<a:p/>");
        } else {
            paras.push_str(&format!(
                "<a:p><a:r><a:rPr lang=\"zh-CN\" dirty=\"0\"/><a:t>{}</a:t></a:r><a:endParaRPr lang=\"zh-CN\" dirty=\"0\"/></a:p>",
                xml_escape_text(line)
            ));
        }
    }
    let mut out = String::with_capacity(xml.len() + paras.len());
    out.push_str(&xml[..tb_start + "<p:txBody>".len() + keep_len]);
    out.push_str(&paras);
    out.push_str(&xml[tb_close..]);
    pkg.parts.insert(notes_part, out.into_bytes());
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_paths() {
        assert_eq!(normalize_path("ppt/slides/../media/a.png"), "ppt/media/a.png");
        assert_eq!(resolve_target("ppt/slides/slide1.xml", "../media/a.png"), "ppt/media/a.png");
        assert_eq!(
            resolve_target("ppt/slides/_rels/slide1.xml.rels", "../media/a.png"),
            "ppt/media/a.png"
        );
    }
}

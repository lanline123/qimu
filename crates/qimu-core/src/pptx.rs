//! PPTX package surgery (fragment Route B: direct OOXML package
//! manipulation, using DeckCraft's code as the relationship-handling
//! reference — spec §8.6).
//!
//! Phase 3: the file layer under this module is craft-opc / craft-xml
//! / craft-zip (storytold/craft-libs) — archive reads and writes go
//! through craft-opc (bounded, preflighted), relationship and
//! content-type semantics through craft-opc's parsers, and every XML
//! part that is parsed or rewritten goes through craft-xml's owned
//! tree and writer. Slide XML parts themselves are never
//! reserialized: they move between packages as opaque bytes, which is
//! what keeps the page fingerprints below meaningful.
//!
//! Imported decks are stored ONLY in split form (spec §3.1): each version
//! is a small PPTX fragment, the deck is manifest + fragment references.
//! The safety net implemented here: after splitting, reassemble the deck
//! from the fragments and verify it page-by-page against the source
//! (page count, per-page fingerprints, structural integrity). A failed
//! check must roll back the whole import.

mod editor_extended;
use editor_extended::{
    add_element, add_picture_relationship, change_preset, group_inner_span, line_color,
    replace_element_fill, reset_run_style, set_line_color, span_at, valid_rgb,
};
pub use editor_extended::{append_blank_slides, editor_preset_allowed, NewSlideElement};

use craft_xml::{El, Local, Node};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};

pub type PptxResult<T> = Result<T, String>;

/// Bounded reads for imported packages (craft-opc limits): a hostile
/// or corrupt archive is refused, never partially trusted. Sized for
/// real decks — tens of thousands of parts, video-sized single parts,
/// a gigabyte inflated in total.
const PACKAGE_LIMITS: craft_opc::Limits = craft_opc::Limits::new(50_000, 256 << 20, 1 << 30);

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
        let pkg = craft_opc::Package::open(bytes, &PACKAGE_LIMITS)
            .map_err(|e| format!("not a valid zip/pptx: {e}"))?;
        // craft-opc leaves a damaged entry out with a warning and
        // reads the rest; the import safety net keeps the old
        // fail-closed behaviour — any unreadable entry refuses the
        // whole package.
        if !pkg.skipped().is_empty() {
            return Err(format!(
                "unreadable zip entries: {}",
                pkg.skipped().join(", ")
            ));
        }
        let mut parts = BTreeMap::new();
        for name in pkg.names() {
            if let Some(bytes) = pkg.get(name) {
                parts.insert(name.to_string(), bytes.to_vec());
            }
        }
        Ok(Package { parts })
    }

    pub fn to_bytes(&self) -> PptxResult<Vec<u8>> {
        // The same observable shape as the old zip-crate writer —
        // entries in name order, stored uncompressed — but written by
        // craft-zip (through craft-opc's ZipOut): deterministic, with
        // DOS-epoch timestamps on every entry.
        let mut out = craft_opc::ZipOut::new();
        for (name, bytes) in &self.parts {
            out.add(name, bytes, craft_opc::Compression::Store)
                .map_err(|e| e.to_string())?;
        }
        out.finish().map_err(|e| e.to_string())
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

/// Parse an XML part into craft-xml's owned tree (names as written,
/// namespace declarations kept as attributes). craft-xml's defaults
/// are lenient (varied real-world generators parse), tightened here
/// where leniency is a hazard: a DOCTYPE with an internal subset —
/// where entity definitions live (XXE, expansion) — is refused, and
/// an undefined entity is an error instead of silently dropped text.
/// This is the single parse entry for the whole crate, so the posture
/// cannot drift between call sites.
pub(crate) fn parse_xml(xml: &str, part: &str) -> PptxResult<El> {
    let mut options = craft_xml::Options::default();
    options.doctype = craft_xml::Doctype::RejectInternalSubset;
    options.unknown_entity = craft_xml::UnknownEntity::Error;
    options
        .parse(xml.as_bytes())
        .map_err(|e| format!("part {part} is not readable XML: {e}"))
}

/// Serialize a parsed part back to bytes: the standalone declaration
/// plus the root element, in craft-xml's writer form.
pub(crate) fn serialize_xml(root: &El) -> Vec<u8> {
    let mut out = craft_xml::DECL_STANDALONE.to_string();
    out.push_str(&root.to_xml());
    out.into_bytes()
}

/// The part a .rels part describes: "ppt/slides/_rels/slide1.xml.rels"
/// describes "ppt/slides/slide1.xml", and "_rels/.rels" describes the
/// package root (""). A plain part name describes itself.
pub(crate) fn described_part(part: &str) -> String {
    if let Some(pos) = part.rfind("/_rels/") {
        let file = part[pos + "/_rels/".len()..].trim_end_matches(".rels");
        format!("{}/{file}", &part[..pos])
    } else if let Some(file) = part.strip_prefix("_rels/") {
        file.trim_end_matches(".rels").to_string()
    } else {
        part.to_string()
    }
}

/// Resolve a relationship target against the directory of its source
/// part (the source may be named by the part itself or by its .rels
/// part). craft-opc name resolution: `..` cannot climb out of the
/// package, a leading `/` is package-absolute, and a `#fragment`
/// suffix is dropped.
pub(crate) fn resolve_target(source_part: &str, target: &str) -> String {
    let described = described_part(source_part);
    craft_opc::resolve(craft_opc::dir_of(&described), target)
}

/// One relationship of a part, as parsed by craft-opc. `target` is the
/// target as written in the .rels part (a relative reference or a
/// URL); resolve it with [`resolve_target`] against the described
/// part.
#[derive(Debug, Clone)]
pub(crate) struct Rel {
    pub id: String,
    /// The full relationship type URI.
    pub ty: String,
    pub target: String,
    pub external: bool,
}

// NOTE (merge repair): the two string-scanning helpers below are the
// element-model section's own tools (slide XML is edited as text by
// slide_elements / apply_slide_ops). They were lost when the phase-3
// craft-xml rewrite of this file's parsing half replaced the file the
// editor package was based on; restored verbatim from that package.
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

pub(crate) fn parse_rels(xml: &str) -> Vec<Rel> {
    craft_opc::Rels::parse(xml.as_bytes(), "")
        .list
        .into_iter()
        .map(|r| Rel {
            id: r.id,
            ty: r.ty,
            target: r.target_raw,
            external: r.external,
        })
        .collect()
}

/// Serialize relationships with their original ids, in the order
/// given. craft-opc's RelsOut renumbers ids (rId1, rId2, ...), which
/// package surgery cannot accept — sldId elements reference the
/// existing ids — so the container is written with craft-xml
/// directly. Items are (Id, Type, Target, external).
pub(crate) fn rels_xml(items: &[(String, String, String, bool)]) -> String {
    let mut w = craft_xml::Writer::document();
    w.open(
        "Relationships",
        &[(
            "xmlns",
            "http://schemas.openxmlformats.org/package/2006/relationships",
        )],
    );
    for (id, ty, target, external) in items {
        w.tag("Relationship")
            .attr("Id", id)
            .attr("Type", ty)
            .attr("Target", target)
            .attr_opt("TargetMode", external.then_some("External"))
            .empty();
    }
    w.finish()
}

/// Slides of the deck in presentation order, with per-page fingerprints
/// (identity by part + content, never by printed folio — spec §3.1).
pub fn slides(pkg: &Package) -> PptxResult<Vec<SlideRef>> {
    let pres = pkg.get_text(PRESENTATION)?;
    let root = parse_xml(&pres, PRESENTATION)?;
    let rels = parse_rels(&pkg.get_text(PRESENTATION_RELS)?);
    let by_id: BTreeMap<&str, &Rel> = rels.iter().map(|r| (r.id.as_str(), r)).collect();

    let mut out = Vec::new();
    let sld_ids: Vec<&El> = root
        .find(Local("sldIdLst"))
        .map(|lst| lst.children(Local("sldId")).collect())
        .unwrap_or_default();
    for el in sld_ids {
        let rid = el.attr_local("r:id").ok_or("sldId without r:id")?;
        let rel = by_id
            .get(rid)
            .ok_or_else(|| format!("sldId references unknown {rid}"))?;
        let part = resolve_target(PRESENTATION, &rel.target);
        let fp = page_fingerprint(pkg, &part);
        out.push(SlideRef {
            index: out.len(),
            part,
            rid: rid.to_string(),
            fingerprint: fp,
        });
    }
    Ok(out)
}

/// One PowerPoint section of a deck: its name and the 0-based
/// slide indices that belong to it, in presentation order.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SectionSpan {
    pub name: String,
    pub pages: Vec<usize>,
}

/// The deck's real sections (`p14:sectionLst` in
/// `ppt/presentation.xml`), resolved to slide indices in
/// presentation order. `Ok(None)` means the deck carries no usable
/// section list — the common case for decks written by tools that
/// never create sections — and callers fall back to their
/// sectionless behaviour.
///
/// Slides belonging to no section are grouped into a single span
/// with an empty name, positioned where its first slide appears;
/// naming that span is the caller's existing fallback concern (the
/// file stem in `import_blocks`, the numbered-part fallback in
/// `propose_blocks`). Structural oddities (unparseable slide ids,
/// sections referencing slides the deck does not have, a section
/// list naming no deck slide at all) also degrade to `Ok(None)`
/// rather than an error: sections are a hint layer, and a deck
/// that imports fine without them must not start failing over its
/// section list.
pub fn deck_sections(pkg: &Package) -> PptxResult<Option<Vec<SectionSpan>>> {
    let pres = pkg.get_text(PRESENTATION)?;
    let root = parse_xml(&pres, PRESENTATION)?;
    // Deck order: the numeric ids of the main sldIdLst.
    let mut deck_ids: Vec<u32> = Vec::new();
    if let Some(lst) = root.children(Local("sldIdLst")).next() {
        for el in lst.children(Local("sldId")) {
            match el.attr("id").and_then(|v| v.parse::<u32>().ok()) {
                Some(id) => deck_ids.push(id),
                None => return Ok(None),
            }
        }
    }
    if deck_ids.is_empty() {
        return Ok(None);
    }
    // Section definitions, in document order: (name, member ids).
    let mut defs: Vec<(String, Vec<u32>)> = Vec::new();
    for ext_lst in root.children(Local("extLst")) {
        for ext in ext_lst.children(Local("ext")) {
            for lst in ext.children(Local("sectionLst")) {
                for sec in lst.children(Local("section")) {
                    let name = sec.attr("name").unwrap_or_default().to_string();
                    let mut ids = Vec::new();
                    if let Some(id_lst) = sec.children(Local("sldIdLst")).next() {
                        for el in id_lst.children(Local("sldId")) {
                            if let Some(id) = el.attr("id").and_then(|v| v.parse::<u32>().ok())
                            {
                                ids.push(id);
                            }
                        }
                    }
                    defs.push((name, ids));
                }
            }
        }
    }
    if defs.is_empty() {
        return Ok(None);
    }
    // Walk the deck in order, opening each span at its first
    // slide: one span per section (first definition wins when an
    // id is listed twice), plus one unnamed span collecting the
    // slides no section claims.
    let mut spans: Vec<SectionSpan> = Vec::new();
    let mut span_of_def: Vec<Option<usize>> = vec![None; defs.len()];
    let mut loose: Option<usize> = None;
    let mut any_sectioned = false;
    for (idx, id) in deck_ids.iter().enumerate() {
        let owner = defs.iter().position(|(_, ids)| ids.contains(id));
        match owner {
            Some(di) => {
                any_sectioned = true;
                let si = match span_of_def[di] {
                    Some(si) => si,
                    None => {
                        spans.push(SectionSpan {
                            name: defs[di].0.clone(),
                            pages: Vec::new(),
                        });
                        let si = spans.len() - 1;
                        span_of_def[di] = Some(si);
                        si
                    }
                };
                spans[si].pages.push(idx);
            }
            None => {
                let si = match loose {
                    Some(si) => si,
                    None => {
                        spans.push(SectionSpan {
                            name: String::new(),
                            pages: Vec::new(),
                        });
                        let si = spans.len() - 1;
                        loose = Some(si);
                        si
                    }
                };
                spans[si].pages.push(idx);
            }
        }
    }
    if !any_sectioned {
        return Ok(None);
    }
    Ok(Some(spans))
}

/// The fingerprint entries of one part's relationships: for every
/// relationship, its type plus the content hash of its target (or the
/// target itself for external relationships). Target NAMES are
/// deliberately not hashed: assembly renames parts when merging
/// fragments, and a rename must not make a page look "modified".
fn rel_content_entries(pkg: &Package, part: &str) -> Vec<String> {
    let rels_name = rels_part_name(part);
    let Some(bytes) = pkg.parts.get(&rels_name) else {
        return Vec::new();
    };
    let text = String::from_utf8_lossy(bytes);
    parse_rels(&text)
        .iter()
        .map(|r| {
            let ty = r.ty.as_str();
            if r.external {
                format!("{ty}|ext|{}", r.target)
            } else {
                let abs = resolve_target(part, &r.target);
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
        .collect()
}

/// The internal targets of `part`'s relationships whose type ends
/// with `suffix` (e.g. "/slideLayout", "/slideMaster"), resolved to
/// absolute part names.
fn rel_targets_of_type(pkg: &Package, part: &str, suffix: &str) -> Vec<String> {
    let rels_name = rels_part_name(part);
    let Some(bytes) = pkg.parts.get(&rels_name) else {
        return Vec::new();
    };
    let text = String::from_utf8_lossy(bytes);
    parse_rels(&text)
        .iter()
        .filter(|r| !r.external && r.ty.ends_with(suffix))
        .map(|r| resolve_target(part, &r.target))
        .collect()
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
    //
    // Audit2 E-N1: the walk used to stop at the slide's direct
    // targets. The layout's own content was hashed, but the chain
    // above it — layout -> slideMaster -> theme — was not, so two
    // pages with identical slide+layout content under different
    // masters/themes (different backgrounds, theme colors/fonts,
    // master placeholders) collapsed into one page entity and shared
    // one thumbnail and one assembled scaffold. The walk now extends
    // two more levels: the layout's relationships (which reach the
    // master) and the master's relationships (which reach the
    // theme), each entry namespaced by its level so a hash cannot
    // migrate between levels unnoticed.
    let mut h = Sha256::new();
    if let Some(bytes) = pkg.parts.get(slide_part) {
        h.update(bytes);
    }
    let mut entries = rel_content_entries(pkg, slide_part);
    for layout in rel_targets_of_type(pkg, slide_part, "/slideLayout") {
        for e in rel_content_entries(pkg, &layout) {
            entries.push(format!("layout>{e}"));
        }
        for master in rel_targets_of_type(pkg, &layout, "/slideMaster") {
            for e in rel_content_entries(pkg, &master) {
                entries.push(format!("master>{e}"));
            }
        }
    }
    entries.sort();
    for e in entries {
        h.update(e.as_bytes());
        h.update(b"\n");
    }
    format!("{:x}", h.finalize())
}

fn rels_part_name(part: &str) -> String {
    craft_opc::rels_path(part)
}

/// Split a deck into one single-slide fragment per slide (Route B pruning:
/// shared parts are copied wholesale at POC stage; slide/notes parts of
/// other slides are dropped and the three bookkeeping parts —
/// presentation.xml, its .rels, [Content_Types].xml — are pruned).
pub fn split(pkg: &Package) -> PptxResult<Vec<Package>> {
    let deck_slides = slides(pkg)?;
    let pres_xml = pkg.get_text(PRESENTATION)?;
    let pres_root = parse_xml(&pres_xml, PRESENTATION)?;
    let sld_elements: Vec<El> = pres_root
        .find(Local("sldIdLst"))
        .map(|lst| lst.children(Local("sldId")).cloned().collect())
        .unwrap_or_default();
    if sld_elements.len() != deck_slides.len() {
        return Err("sldId count mismatch while parsing presentation.xml".into());
    }
    let ct_root = parse_xml(&pkg.get_text(CONTENT_TYPES)?, CONTENT_TYPES)?;

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
    let _ = &notes_of;

    let mut fragments = Vec::new();
    for (i, s) in deck_slides.iter().enumerate() {
        fragments.push(split_slide_at(
            pkg,
            &pres_root,
            &ct_root,
            &sld_elements[i],
            &s.part,
        )?);
    }
    Ok(fragments)
}

/// Build the single-slide fragment for slide `index` of a deck —
/// exactly the fragment `split` produces at that position, but
/// computed without building any other slide's fragment (lazy
/// import, user ruling 2026-11-25: after the deck is opened, its
/// pages split one at a time in priority order, so the split cost
/// of a page is paid only when that page's turn comes).
pub fn split_one(pkg: &Package, index: usize) -> PptxResult<Package> {
    let deck_slides = slides(pkg)?;
    let slide = deck_slides.get(index).ok_or("拆分目标页位越界")?;
    let pres_xml = pkg.get_text(PRESENTATION)?;
    let pres_root = parse_xml(&pres_xml, PRESENTATION)?;
    let sld_elements: Vec<El> = pres_root
        .find(Local("sldIdLst"))
        .map(|lst| lst.children(Local("sldId")).cloned().collect())
        .unwrap_or_default();
    if sld_elements.len() != deck_slides.len() {
        return Err("sldId count mismatch while parsing presentation.xml".into());
    }
    let ct_root = parse_xml(&pkg.get_text(CONTENT_TYPES)?, CONTENT_TYPES)?;
    split_slide_at(pkg, &pres_root, &ct_root, &sld_elements[index], &slide.part)
}

/// One slide's fragment construction — the per-slide body of
/// `split`, shared with `split_one` so both produce the identical
/// fragment for the same slide.
fn split_slide_at(
    pkg: &Package,
    pres_root: &El,
    ct_root: &El,
    sld_el: &El,
    slide_part: &str,
) -> PptxResult<Package> {
        // Keep only what this slide actually reaches through its
        // relationship closure (layout/master/theme chain, media,
        // charts, notes) plus package scaffolding. Everything else —
        // above all other slides' media — is pruned, so a fragment is
        // proportional to its own page, not to the whole deck.
        let mut keep: BTreeSet<String> = BTreeSet::new();
        let mut stack = vec![slide_part.to_string()];
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
        let mut frag_pres = pres_root.clone();
        if let Some(lst) = frag_pres.child_mut(Local("sldIdLst")) {
            lst.children = vec![Node::El(sld_el.clone())];
        }
        parts.insert(PRESENTATION.to_string(), serialize_xml(&frag_pres));

        // Prune presentation rels: drop relationships to removed parts.
        let pres_rels = parse_rels(&pkg.get_text(PRESENTATION_RELS)?);
        let kept_rels: Vec<(String, String, String, bool)> = pres_rels
            .iter()
            .filter(|r| !removed.contains(&resolve_target(PRESENTATION, &r.target)))
            .map(|r| (r.id.clone(), r.ty.clone(), r.target.clone(), r.external))
            .collect();
        parts.insert(
            PRESENTATION_RELS.to_string(),
            rels_xml(&kept_rels).into_bytes(),
        );

        // Prune [Content_Types].xml overrides for removed parts.
        let mut frag_ct = ct_root.clone();
        frag_ct.children.retain(|node| match node {
            Node::El(el) if el.is(Local("Override")) => match el.attr("PartName") {
                Some(p) => !removed.contains(p.trim_start_matches('/')),
                None => true,
            },
            _ => true,
        });
        parts.insert(CONTENT_TYPES.to_string(), serialize_xml(&frag_ct));

        return Ok(Package { parts });
}

/// Reassemble a full deck from single-slide fragments produced by `split`.
/// Original relationship IDs and part names are preserved by construction,
/// so the bookkeeping parts are unions of the fragments' pruned versions.
pub fn reassemble(fragments: &[Package]) -> PptxResult<Package> {
    if fragments.is_empty() {
        return Err("no fragments to reassemble".into());
    }
    let mut parts: BTreeMap<String, Vec<u8>> = BTreeMap::new();
    let mut sld_elements: Vec<El> = Vec::new();
    let mut pres_rel_by_id: BTreeMap<String, (String, String, String, bool)> = BTreeMap::new();
    let mut overrides: BTreeMap<String, String> = BTreeMap::new();
    let mut defaults: BTreeMap<String, String> = BTreeMap::new();
    let mut pres_template: Option<El> = None;

    for frag in fragments {
        let fslides = slides(frag)?;
        if fslides.len() != 1 {
            return Err("fragment does not contain exactly one slide".into());
        }
        let pres_root = parse_xml(&frag.get_text(PRESENTATION)?, PRESENTATION)?;
        let first_sld = pres_root
            .find(Local("sldIdLst"))
            .and_then(|lst| lst.children(Local("sldId")).next().cloned())
            .ok_or("fragment presentation has no sldId")?;
        sld_elements.push(first_sld);
        if pres_template.is_none() {
            pres_template = Some(pres_root);
        }
        for rel in parse_rels(&frag.get_text(PRESENTATION_RELS)?) {
            pres_rel_by_id.entry(rel.id.clone()).or_insert((
                rel.id,
                rel.ty,
                rel.target,
                rel.external,
            ));
        }
        let ct_root = parse_xml(&frag.get_text(CONTENT_TYPES)?, CONTENT_TYPES)?;
        for el in ct_root.elements() {
            if el.is(Local("Override")) {
                if let (Some(p), Some(c)) = (el.attr("PartName"), el.attr("ContentType")) {
                    overrides.entry(p.to_string()).or_insert(c.to_string());
                }
            } else if el.is(Local("Default")) {
                if let (Some(e), Some(c)) = (el.attr("Extension"), el.attr("ContentType")) {
                    defaults.entry(e.to_string()).or_insert(c.to_string());
                }
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
    let mut pres_root = pres_template.unwrap();
    if let Some(lst) = pres_root.child_mut(Local("sldIdLst")) {
        lst.children = sld_elements.into_iter().map(Node::El).collect();
    }
    parts.insert(PRESENTATION.to_string(), serialize_xml(&pres_root));

    // presentation rels: the union of the fragments' relationships,
    // in relationship-id order (the old BTreeMap ordering).
    let rel_items: Vec<(String, String, String, bool)> = pres_rel_by_id.into_values().collect();
    parts.insert(
        PRESENTATION_RELS.to_string(),
        rels_xml(&rel_items).into_bytes(),
    );

    // [Content_Types].xml: defaults + union of overrides.
    let mut w = craft_xml::Writer::document();
    w.open(
        "Types",
        &[(
            "xmlns",
            "http://schemas.openxmlformats.org/package/2006/content-types",
        )],
    );
    for (ext, ctype) in &defaults {
        w.empty("Default", &[("Extension", ext), ("ContentType", ctype)]);
    }
    for (part, ctype) in &overrides {
        w.empty("Override", &[("PartName", part), ("ContentType", ctype)]);
    }
    parts.insert(CONTENT_TYPES.to_string(), w.into_bytes());

    Ok(Package { parts })
}

/// Structural integrity: every non-external relationship target exists,
/// and every part is covered by [Content_Types].xml.
pub fn structural_errors(pkg: &Package) -> Vec<String> {
    let mut errors = Vec::new();
    // Content-type coverage (craft-opc: Override by part name, then
    // Default by extension, both case-insensitive).
    let content_types = pkg
        .parts
        .get(CONTENT_TYPES)
        .map(|bytes| craft_opc::ContentTypes::parse(bytes))
        .unwrap_or_default();
    for name in pkg.parts.keys() {
        if name == CONTENT_TYPES {
            continue;
        }
        if content_types.of(name).is_none() {
            errors.push(format!("part {name} not covered by [Content_Types].xml"));
        }
    }
    // Relationship targets exist.
    for (name, bytes) in &pkg.parts {
        if !name.ends_with(".rels") {
            continue;
        }
        let described = described_part(name);
        let text = String::from_utf8_lossy(bytes);
        for rel in parse_rels(&text) {
            if rel.external {
                continue;
            }
            let target = resolve_target(&described, &rel.target);
            if !pkg.parts.contains_key(&target) {
                errors.push(format!(
                    "{name}: relationship {} -> missing part {target}",
                    rel.id
                ));
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
    let fragments = split(source)?;
    verify_from_singles(source, &fragments)
}

/// The §3.1 check itself, run over fragments the caller already
/// split: reassemble them and compare page-by-page against the
/// source. The staged import (line ⑤) splits the source exactly
/// once and feeds those very fragments here — the check must never
/// split a second time on its own, or the single-pass guarantee is
/// fiction (analysis line ⑦, point S6a).
pub fn verify_from_singles(
    source: &Package,
    fragments: &[Package],
) -> PptxResult<VerifyReport> {
    let src_slides = slides(source)?;
    let reassembled = reassemble(fragments)?;
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

/// The `<p:txBody>` of the shape carrying the body placeholder
/// (`<p:ph type="body"/>`), if the tree has one.
fn body_txbody_mut(el: &mut El) -> Option<&mut El> {
    if el.is(Local("sp")) {
        let is_body = el
            .descendants()
            .any(|d| d.is(Local("ph")) && d.attr("type") == Some("body"));
        if is_body {
            return el.child_mut(Local("txBody"));
        }
    }
    for child in el.elements_mut() {
        if let Some(found) = body_txbody_mut(child) {
            return Some(found);
        }
    }
    None
}

/// One notes paragraph in the form the old string writer emitted:
/// a run with zh-CN run properties, or an empty `<a:p/>` for a blank
/// line. Text escaping is the serializer's job (craft-xml).
fn notes_paragraph(line: &str) -> El {
    let mut p = El::new("a:p");
    if line.is_empty() {
        return p;
    }
    let mut rpr = El::new("a:rPr");
    rpr.set_attr("lang", "zh-CN");
    rpr.set_attr("dirty", "0");
    let mut t = El::new("a:t");
    t.push(line);
    let mut r = El::new("a:r");
    r.push(rpr);
    r.push(t);
    let mut end = El::new("a:endParaRPr");
    end.set_attr("lang", "zh-CN");
    end.set_attr("dirty", "0");
    p.push(r);
    p.push(end);
    p
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
    let rels_part = rels_part_name(&slide_part);
    let rels_xml = match pkg.get_text(&rels_part) {
        Ok(x) => x,
        Err(_) => return false,
    };
    let rels = parse_rels(&rels_xml);
    let notes_rel = match rels
        .iter()
        .find(|r| craft_opc::type_is(&r.ty, "notesSlide"))
    {
        Some(r) => r,
        None => return false,
    };
    let notes_part = resolve_target(&slide_part, &notes_rel.target);
    let xml = match pkg.get_text(&notes_part) {
        Ok(x) => x,
        Err(_) => return false,
    };
    let mut root = match parse_xml(&xml, &notes_part) {
        Ok(r) => r,
        Err(_) => return false,
    };
    let Some(txbody) = body_txbody_mut(&mut root) else {
        return false;
    };
    // Keep everything inside txBody before the first paragraph
    // (a:bodyPr, a:lstStyle); replace the paragraphs.
    let cut = txbody
        .children
        .iter()
        .position(|n| matches!(n, Node::El(e) if e.is(Local("p"))))
        .unwrap_or(txbody.children.len());
    txbody.children.truncate(cut);
    for line in text.split('\n') {
        txbody.children.push(Node::El(notes_paragraph(line)));
    }
    pkg.parts.insert(notes_part, serialize_xml(&root));
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pkg_with_presentation(xml: &str) -> Package {
        let mut parts = BTreeMap::new();
        parts.insert(PRESENTATION.to_string(), xml.as_bytes().to_vec());
        Package { parts }
    }

    #[test]
    fn deck_sections_groups_by_section_with_loose_head() {
        let xml = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<p:presentation xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships" xmlns:p14="http://schemas.microsoft.com/office/powerpoint/2010/main"><p:sldIdLst><p:sldId id="256" r:id="rId2"/><p:sldId id="257" r:id="rId3"/><p:sldId id="258" r:id="rId4"/><p:sldId id="259" r:id="rId5"/></p:sldIdLst><p:extLst><p:ext uri="{521415D9-36F0-4FBD-ABE6-6ED785790B25}"><p14:sectionLst><p14:section name="Alpha" id="{11111111-1111-1111-1111-111111111111}"><p14:sldIdLst><p14:sldId id="257"/><p14:sldId id="258"/></p14:sldIdLst></p14:section><p14:section name="Beta" id="{22222222-2222-2222-2222-222222222222}"><p14:sldIdLst><p14:sldId id="259"/></p14:sldIdLst></p14:section></p14:sectionLst></p:ext></p:extLst></p:presentation>"#;
        let pkg = pkg_with_presentation(xml);
        let spans = deck_sections(&pkg).unwrap().expect("sections present");
        assert_eq!(spans.len(), 3);
        assert_eq!(spans[0].name, "");
        assert_eq!(spans[0].pages, vec![0]);
        assert_eq!(spans[1].name, "Alpha");
        assert_eq!(spans[1].pages, vec![1, 2]);
        assert_eq!(spans[2].name, "Beta");
        assert_eq!(spans[2].pages, vec![3]);
    }

    #[test]
    fn deck_sections_none_without_section_list() {
        let xml = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<p:presentation xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"><p:sldIdLst><p:sldId id="256" r:id="rId2"/></p:sldIdLst></p:presentation>"#;
        let pkg = pkg_with_presentation(xml);
        assert_eq!(deck_sections(&pkg).unwrap(), None);
    }

    #[test]
    fn resolve_targets() {
        assert_eq!(
            resolve_target("ppt/presentation.xml", "slides/slide1.xml"),
            "ppt/slides/slide1.xml"
        );
        assert_eq!(
            resolve_target("ppt/slides/slide1.xml", "../media/a.png"),
            "ppt/media/a.png"
        );
        assert_eq!(
            resolve_target("ppt/slides/_rels/slide1.xml.rels", "../media/a.png"),
            "ppt/media/a.png"
        );
        // `..` cannot climb out of the package.
        assert_eq!(
            resolve_target("ppt/slides/slide1.xml", "../../../x.xml"),
            "x.xml"
        );
        assert_eq!(
            rels_part_name("ppt/slides/slide1.xml"),
            "ppt/slides/_rels/slide1.xml.rels"
        );
        assert_eq!(
            described_part("ppt/slides/_rels/slide1.xml.rels"),
            "ppt/slides/slide1.xml"
        );
        assert_eq!(described_part("_rels/.rels"), "");
    }
}

// ---------- page-level text-run access (visual page editor) ----------
/// Text escaping for the run-surgery splice below (its replacements
/// are written back into the slide XML byte stream, which is never
/// reserialized). Backed by craft-xml's escaper — the same three
/// replacements the old hand-written helper made.
fn xml_escape_text(s: &str) -> String {
    craft_xml::esc(s)
}

fn xml_unescape_text(s: &str) -> String {
    s.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&amp;", "&")
}

/// The text runs (`<a:t>` contents, XML-unescaped, in document order)
/// of one slide of the package. `slide_idx` is 0-based deck order.
/// This is the read side of the page-level visual editor: the client
/// shows one editable field per run.
pub fn slide_text_runs(pkg: &Package, slide_idx: usize) -> PptxResult<Vec<String>> {
    let refs = slides(pkg)?;
    let sref = refs
        .get(slide_idx)
        .ok_or_else(|| format!("slide index {slide_idx} out of range"))?;
    let xml = pkg.get_text(&sref.part)?;
    let mut out = Vec::new();
    let mut rest = xml.as_str();
    while let Some(s) = rest.find("<a:t>") {
        let after = &rest[s + 5..];
        match after.find("</a:t>") {
            Some(e) => {
                out.push(xml_unescape_text(&after[..e]));
                rest = &after[e + 6..];
            }
            None => break,
        }
    }
    Ok(out)
}

/// Replace the contents of individual text runs of one slide, in
/// place. `edits` are (run_index, new_text); run indices follow
/// `slide_text_runs`. New text is XML-escaped on write. Returns the
/// number of runs actually replaced. Runs are plain `<a:t>` elements
/// (the same authoring assumption `personalize` makes: a placeholder
/// or editable string sits inside a single run).
pub fn replace_slide_text_runs(
    pkg: &mut Package,
    slide_idx: usize,
    edits: &[(usize, String)],
) -> PptxResult<usize> {
    if edits.is_empty() {
        return Ok(0);
    }
    let refs = slides(pkg)?;
    let sref = refs
        .get(slide_idx)
        .ok_or_else(|| format!("slide index {slide_idx} out of range"))?;
    let part = sref.part.clone();
    let xml = pkg.get_text(&part)?;
    // Split the slide XML at every <a:t>...</a:t> run, keeping the
    // separators, so replacements can be spliced by run index.
    let mut segments: Vec<String> = Vec::new(); // even: literal, odd: run content
    let mut rest = xml.as_str();
    loop {
        match rest.find("<a:t>") {
            Some(s) => {
                let after = &rest[s + 5..];
                match after.find("</a:t>") {
                    Some(e) => {
                        segments.push(format!("{}<a:t>", &rest[..s]));
                        segments.push(after[..e].to_string());
                        rest = &after[e + 6..];
                    }
                    None => {
                        segments.push(rest.to_string());
                        rest = "";
                        break;
                    }
                }
            }
            None => {
                segments.push(rest.to_string());
                break;
            }
        }
        if rest.is_empty() {
            break;
        }
    }
    let run_count = segments.len() / 2;
    let mut applied = 0usize;
    let mut new_segments = segments.clone();
    for (run, text) in edits {
        if *run < run_count {
            new_segments[run * 2 + 1] = xml_escape_text(text);
            applied += 1;
        }
    }
    if applied == 0 {
        return Ok(0);
    }
    let mut out = String::with_capacity(xml.len());
    for (idx, seg) in new_segments.iter().enumerate() {
        out.push_str(seg);
        if idx % 2 == 1 {
            out.push_str("</a:t>");
        }
    }
    pkg.parts.insert(part, out.into_bytes());
    Ok(applied)
}

// ---------- slide element model (visual page editor v2) ----------
//
// The v1 editor treated a slide as a flat list of `<a:t>` runs. The v2
// editor needs the slide's actual element tree: which shapes, pictures
// and lines sit where, and how their text is styled. This section is
// the read/write pair for that model, in the same string-scanning
// style as the rest of this file (no XML DOM in the workspace).
// Group members retain their ordering and coordinate space.

/// Direct formatting of one text run, as the editor exposes it: font
/// size in points, bold, and an "RRGGBB" color. `None` = the run
/// carries no explicit value (inherits from layout/master).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct RunStyle {
    pub sz: Option<f32>,
    pub b: Option<bool>,
    pub color: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ElRun {
    pub text: String,
    pub style: RunStyle,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ElPara {
    pub runs: Vec<ElRun>,
}

/// One top-level element of a slide's `<p:spTree>`, in document order.
/// Geometry is EMU (914400 per inch); `rot` is in 60000ths of a
/// degree. `has_xfrm` is false when the element inherits its transform
/// from the layout (no `<a:off>` of its own) — such elements can be
/// restyled but not moved until they gain an explicit transform.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SlideElement {
    pub kind: String, // text | shape | picture | line | graphic | group
    pub name: String,
    pub x: i64,
    pub y: i64,
    pub cx: i64,
    pub cy: i64,
    pub rot: i64,
    pub flip_h: bool,
    pub flip_v: bool,
    pub has_xfrm: bool,
    pub geom: Option<String>,
    pub line_color: Option<String>,
    pub group_view: Option<[i64; 4]>,
    pub children: Vec<SlideElement>,
    pub media: Option<String>,
    pub paras: Vec<ElPara>,
}

/// Minimal tag tokenizer: returns (name, start, end, closing,
/// self_closing) for the first tag at or after `from`. Comments and
/// processing instructions are skipped; quoted attribute values may
/// contain `>`.
fn next_tag(xml: &str, from: usize) -> Option<(String, usize, usize, bool, bool)> {
    let bytes = xml.as_bytes();
    let mut i = from;
    while i < bytes.len() {
        if bytes[i] != b'<' {
            i += 1;
            continue;
        }
        if xml[i..].starts_with("<!--") {
            i = xml[i..].find("-->")? + i + 3;
            continue;
        }
        if xml[i..].starts_with("<?") {
            i = xml[i..].find("?>")? + i + 2;
            continue;
        }
        if xml[i..].starts_with("<!") {
            i = xml[i..].find('>')? + i + 1;
            continue;
        }
        let closing = xml[i..].starts_with("</");
        let name_start = if closing { i + 2 } else { i + 1 };
        let mut j = name_start;
        while j < bytes.len()
            && (bytes[j].is_ascii_alphanumeric() || matches!(bytes[j], b':' | b'_' | b'-' | b'.'))
        {
            j += 1;
        }
        if j == name_start {
            i += 1;
            continue;
        }
        let name = xml[name_start..j].to_string();
        let mut k = j;
        let mut quote: Option<u8> = None;
        while k < bytes.len() {
            match quote {
                Some(q) => {
                    if bytes[k] == q {
                        quote = None;
                    }
                }
                None => match bytes[k] {
                    b'"' | b'\'' => quote = Some(bytes[k]),
                    b'>' => break,
                    _ => {}
                },
            }
            k += 1;
        }
        if k >= bytes.len() {
            return None;
        }
        let self_closing = !closing && k > i && bytes[k - 1] == b'/';
        return Some((name, i, k + 1, closing, self_closing));
    }
    None
}

/// Spans of the direct children of a fragment (whose own root tag has
/// already been stripped) whose tag is in `names`. Returns (tag,
/// start, end), end just past the matching close tag.
fn child_spans(xml: &str, names: &[&str]) -> Vec<(String, usize, usize)> {
    let mut out = Vec::new();
    let mut depth = 0usize;
    let mut cur: Option<(String, usize, usize)> = None; // (tag, start, depth at open)
    let mut pos = 0usize;
    while let Some((name, s, e, closing, self_closing)) = next_tag(xml, pos) {
        pos = e;
        if closing {
            depth = depth.saturating_sub(1);
            if let Some((tag, start, d)) = &cur {
                if *d == depth && *tag == name {
                    out.push((tag.clone(), *start, e));
                    cur = None;
                }
            }
            continue;
        }
        if depth == 0 && cur.is_none() && names.contains(&name.as_str()) {
            if self_closing {
                out.push((name.clone(), s, e));
                continue;
            }
            cur = Some((name.clone(), s, depth));
        }
        if !self_closing {
            depth += 1;
        }
    }
    out
}

/// The content between a block's first `>` and its final `<`.
fn tag_inner(block: &str) -> &str {
    let s = block.find('>').map(|i| i + 1).unwrap_or(0);
    let e = block.rfind('<').unwrap_or(block.len());
    if e >= s {
        &block[s..e]
    } else {
        ""
    }
}

/// The raw text of the first `<open...>` tag inside `block`.
fn find_tag_text<'a>(block: &'a str, open: &str) -> Option<&'a str> {
    let s = block.find(open)?;
    // Guard against prefix collisions, same as scan_elements.
    let after = &block[s + open.len()..];
    if after
        .chars()
        .next()
        .map(|c| c.is_ascii_alphanumeric())
        .unwrap_or(false)
    {
        return None;
    }
    let (_, _, e, _, _) = next_tag(block, s)?;
    Some(&block[s..e])
}

fn find_tag_attr(block: &str, open: &str, name: &str) -> Option<String> {
    let tag = find_tag_text(block, open)?;
    attr(tag, name).map(str::to_string)
}

fn parse_i64(s: Option<String>) -> i64 {
    s.and_then(|v| v.parse().ok()).unwrap_or(0)
}

/// (x, y, cx, cy, has_xfrm) from an element block: the first
/// `<a:off>` / `<a:ext>` in document order are the element's own
/// transform (they precede any text body).
fn geom_of(block: &str) -> (i64, i64, i64, i64, bool) {
    let has = block.contains("<a:off");
    (
        parse_i64(find_tag_attr(block, "<a:off", "x")),
        parse_i64(find_tag_attr(block, "<a:off", "y")),
        parse_i64(find_tag_attr(block, "<a:ext", "cx")),
        parse_i64(find_tag_attr(block, "<a:ext", "cy")),
        has,
    )
}

fn parse_run_style(rblock: &str) -> RunStyle {
    let mut st = RunStyle {
        sz: None,
        b: None,
        color: None,
    };
    if let Some(rp) = find_tag_text(rblock, "<a:rPr") {
        if let Some(v) = attr(rp, "sz") {
            st.sz = v.parse::<f32>().ok().map(|h| h / 100.0);
        }
        if let Some(v) = attr(rp, "b") {
            st.b = Some(v == "1" || v.eq_ignore_ascii_case("true"));
        }
    }
    // The fill lives inside the (non-self-closing) rPr element.
    if let Some(s) = rblock.find("<a:solidFill>") {
        if let Some(v) = find_tag_attr(&rblock[s..], "<a:srgbClr", "val") {
            st.color = Some(v.to_uppercase());
        }
    }
    st
}

fn parse_paras(block: &str) -> Vec<ElPara> {
    let Some(ts) = block.find("<p:txBody>") else {
        return Vec::new();
    };
    let inner = tag_inner(&block[ts..]);
    let mut paras = Vec::new();
    for (_, ps, pe) in child_spans(inner, &["a:p"]) {
        let pblock = &inner[ps..pe];
        let pinner = tag_inner(pblock);
        let mut runs = Vec::new();
        for (_, rs, re) in child_spans(pinner, &["a:r", "a:fld"]) {
            let rblock = &pinner[rs..re];
            let text = match (rblock.find("<a:t>"), rblock.find("</a:t>")) {
                (Some(a), Some(b)) if b > a => xml_unescape_text(&rblock[a + "<a:t>".len()..b]),
                _ => String::new(),
            };
            runs.push(ElRun {
                text,
                style: parse_run_style(rblock),
            });
        }
        paras.push(ElPara { runs });
    }
    paras
}

fn picture_media(pkg: &Package, slide_part: &str, block: &str) -> Option<String> {
    let s = block.find("r:embed=\"")? + "r:embed=\"".len();
    let e = block[s..].find('"')? + s;
    let rid = &block[s..e];
    let (dir, file) = slide_part.rsplit_once('/')?;
    let rels_part = format!("{dir}/_rels/{file}.rels");
    let rels_xml = pkg.get_text(&rels_part).ok()?;
    let rel = parse_rels(&rels_xml).into_iter().find(|r| r.id == rid)?;
    if rel.external {
        return None;
    }
    Some(resolve_target(slide_part, &rel.target))
}

fn parse_element(
    pkg: &Package,
    slide_part: &str,
    tag: &str,
    block: &str,
    depth: usize,
) -> SlideElement {
    let paras = if tag == "p:grpSp" {
        Vec::new()
    } else {
        parse_paras(block)
    };
    let geom = find_tag_attr(block, "<a:prstGeom", "prst");
    let kind = match tag {
        "p:pic" => "picture",
        "p:cxnSp" => "line",
        "p:graphicFrame" => "graphic",
        "p:grpSp" => "group",
        _ => {
            if paras.iter().any(|p| !p.runs.is_empty()) {
                "text"
            } else if geom.as_deref() == Some("line") {
                "line"
            } else {
                "shape"
            }
        }
    }
    .to_string();
    let (x, y, cx, cy, has_xfrm) = geom_of(block);
    let rot = find_tag_text(block, "<a:xfrm")
        .or_else(|| find_tag_text(block, "<p:xfrm"))
        .and_then(|t| attr(t, "rot").map(str::to_string))
        .map(|v| v.parse().unwrap_or(0))
        .unwrap_or(0);
    let media = if kind == "picture" {
        picture_media(pkg, slide_part, block)
    } else {
        None
    };
    let children = if kind == "group" && depth < 32 {
        group_inner_span(block)
            .map(|(inside, end)| {
                let inner = &block[inside..end];
                child_spans(inner, &EL_TAGS)
                    .into_iter()
                    .map(|(tag, a, b)| {
                        parse_element(pkg, slide_part, &tag, &inner[a..b], depth + 1)
                    })
                    .collect()
            })
            .unwrap_or_default()
    } else {
        Vec::new()
    };
    let group_view = (kind == "group").then(|| {
        [
            parse_i64(find_tag_attr(block, "<a:chOff", "x")),
            parse_i64(find_tag_attr(block, "<a:chOff", "y")),
            parse_i64(find_tag_attr(block, "<a:chExt", "cx")),
            parse_i64(find_tag_attr(block, "<a:chExt", "cy")),
        ]
    });
    let xfrm = find_tag_text(block, "<a:xfrm")
        .or_else(|| find_tag_text(block, "<p:xfrm"))
        .unwrap_or("");
    let flag = |name| matches!(attr(xfrm, name), Some("1" | "true"));
    SlideElement {
        flip_h: flag("flipH"),
        flip_v: flag("flipV"),
        line_color: (kind == "line").then(|| line_color(block)).flatten(),
        group_view,
        children,
        kind,
        name: find_tag_attr(block, "<p:cNvPr", "name").unwrap_or_default(),
        x,
        y,
        cx,
        cy,
        rot,
        has_xfrm,
        geom,
        media,
        paras,
    }
}

fn sp_tree_inner(xml: &str) -> Option<(usize, usize)> {
    let s = xml.find("<p:spTree>")? + "<p:spTree>".len();
    let e = xml[s..].find("</p:spTree>")? + s;
    Some((s, e))
}

const EL_TAGS: [&str; 5] = ["p:sp", "p:pic", "p:cxnSp", "p:graphicFrame", "p:grpSp"];

/// Slide size in EMU from `<p:sldSz>` (16:9 default when absent).
pub fn slide_size(pkg: &Package) -> (i64, i64) {
    let pres = pkg.get_text(PRESENTATION).unwrap_or_default();
    for el in scan_elements(&pres, "<p:sldSz") {
        let cx = attr(el, "cx")
            .and_then(|v| v.parse().ok())
            .unwrap_or(12192000);
        let cy = attr(el, "cy")
            .and_then(|v| v.parse().ok())
            .unwrap_or(6858000);
        return (cx, cy);
    }
    (12192000, 6858000)
}

/// The top-level elements of one slide, in document order — the v2
/// editor's read side (companion to `slide_text_runs`).
pub fn slide_elements(pkg: &Package, slide_idx: usize) -> PptxResult<Vec<SlideElement>> {
    let refs = slides(pkg)?;
    let sref = refs
        .get(slide_idx)
        .ok_or_else(|| format!("slide index {slide_idx} out of range"))?;
    let xml = pkg.get_text(&sref.part)?;
    let (ts, te) = sp_tree_inner(&xml).ok_or("slide has no spTree")?;
    let inner = &xml[ts..te];
    let mut out = Vec::new();
    for (tag, s, e) in child_spans(inner, &EL_TAGS) {
        out.push(parse_element(pkg, &sref.part, &tag, &inner[s..e], 0));
    }
    Ok(out)
}

// ---------- element write side ----------

/// One edit operation against one slide, applied in list order.
/// Indices address the document as it evolves: structural ops are
/// emitted by the caller in an order that keeps indices valid
/// (updates first, deletions descending, insertions ascending).
pub enum SlideOp {
    SetText {
        el: usize,
        para: usize,
        run: usize,
        text: String,
    },
    SetStyle {
        el: usize,
        para: usize,
        run: usize,
        sz: Option<f32>,
        b: Option<bool>,
        color: Option<String>,
    },
    AddPara {
        el: usize,
        after: Option<usize>,
        text: String,
    },
    DeletePara {
        el: usize,
        para: usize,
    },
    AddRun {
        el: usize,
        para: usize,
        text: String,
    },
    DeleteRun {
        el: usize,
        para: usize,
        run: usize,
    },
    Move {
        el: usize,
        x: i64,
        y: i64,
    },
    Resize {
        el: usize,
        cx: i64,
        cy: i64,
    },
    ReplacePic {
        el: usize,
        bytes: Vec<u8>,
        ext: String,
    },
    ResetStyle {
        el: usize,
        para: usize,
        run: usize,
    },
    LineColor {
        el: usize,
        color: String,
    },
    Preset {
        el: usize,
        preset: String,
    },
    AddElement(NewSlideElement),
    InGroup {
        parents: Vec<usize>,
        op: Box<SlideOp>,
    },
}

/// Rewrite (or insert) attribute `name` on the tag starting at
/// `tag_start` within `xml`.
fn set_tag_attr(xml: &str, tag_start: usize, name: &str, value: &str) -> String {
    let Some((_, _, tag_end, _, self_closing)) = next_tag(xml, tag_start) else {
        return xml.to_string();
    };
    let tag = &xml[tag_start..tag_end];
    let pat = format!(" {name}=\"");
    let new_tag = if let Some(p) = tag.find(&pat) {
        let vs = p + pat.len();
        let ve = tag[vs..].find('"').map(|i| vs + i).unwrap_or(vs);
        format!("{}{}{}", &tag[..vs], value, &tag[ve..])
    } else if self_closing {
        format!("{} {}=\"{}\"/>", &tag[..tag.len() - 2], name, value)
    } else {
        format!("{} {}=\"{}\">", &tag[..tag.len() - 1], name, value)
    };
    format!("{}{}{}", &xml[..tag_start], new_tag, &xml[tag_end..])
}

/// Paragraph spans (absolute, within the element block).
fn para_spans(block: &str) -> Vec<(usize, usize)> {
    let Some(ts) = block.find("<p:txBody>") else {
        return Vec::new();
    };
    let tbody = &block[ts..];
    let inner = tag_inner(tbody);
    let inner_start = ts + tbody.find('>').map(|i| i + 1).unwrap_or(0);
    child_spans(inner, &["a:p"])
        .into_iter()
        .map(|(_, s, e)| (inner_start + s, inner_start + e))
        .collect()
}

/// Run spans (absolute, within the element block) of one paragraph.
fn run_spans(block: &str, para: (usize, usize)) -> Vec<(usize, usize)> {
    let pblock = &block[para.0..para.1];
    let inner_start = para.0 + pblock.find('>').map(|i| i + 1).unwrap_or(0);
    let inner = tag_inner(pblock);
    child_spans(inner, &["a:r", "a:fld"])
        .into_iter()
        .map(|(_, s, e)| (inner_start + s, inner_start + e))
        .collect()
}

/// The `<a:pPr.../>` / `<a:pPr>...</a:pPr>` text of a paragraph, if any.
fn para_ppr(pblock: &str) -> String {
    let Some(s) = pblock.find("<a:pPr") else {
        return String::new();
    };
    let rest = &pblock[s..];
    if let Some((_, _, e, _, sc)) = next_tag(rest, 0) {
        if sc {
            return rest[..e].to_string();
        }
        if let Some(c) = rest.find("</a:pPr>") {
            return rest[..c + "</a:pPr>".len()].to_string();
        }
    }
    String::new()
}

/// The first run's `<a:rPr...>` tag text within a block (style
/// template for cloned runs/paragraphs).
fn first_rpr(block: &str) -> String {
    find_tag_text(block, "<a:rPr")
        .map(str::to_string)
        .unwrap_or_else(|| "<a:rPr lang=\"zh-CN\" dirty=\"0\"/>".to_string())
}

fn splice(xml: &str, span: (usize, usize), replacement: &str) -> String {
    format!("{}{}{}", &xml[..span.0], replacement, &xml[span.1..])
}

/// Apply a style change to one run block (`<a:r>...</a:r>`).
fn style_run_block(rb: &str, sz: Option<f32>, b: Option<bool>, color: Option<String>) -> String {
    let mut out = rb.to_string();
    let attrs: Vec<(&str, String)> = {
        let mut v: Vec<(&str, String)> = Vec::new();
        if let Some(s) = sz {
            v.push(("sz", format!("{}", (s * 100.0).round() as i64)));
        }
        if let Some(bv) = b {
            v.push(("b", if bv { "1" } else { "0" }.to_string()));
        }
        v
    };
    match out.find("<a:rPr") {
        Some(rp_start) => {
            for (name, value) in &attrs {
                out = set_tag_attr(&out, rp_start, name, value);
            }
            if let Some(c) = &color {
                let fill = format!("<a:solidFill><a:srgbClr val=\"{c}\"/></a:solidFill>");
                if let Some(updated) = replace_element_fill(&out, rp_start, "a:rPr", &fill) {
                    out = updated;
                }
            }
        }
        None => {
            // No rPr: insert one right after the run's open tag.
            let mut rpr = String::from("<a:rPr lang=\"zh-CN\" dirty=\"0\"");
            for (name, value) in &attrs {
                rpr.push_str(&format!(" {name}=\"{value}\""));
            }
            if let Some(c) = &color {
                rpr.push_str(&format!(
                    "><a:solidFill><a:srgbClr val=\"{c}\"/></a:solidFill></a:rPr>"
                ));
            } else {
                rpr.push_str("/>");
            }
            let open_end = out.find('>').map(|i| i + 1).unwrap_or(0);
            out = format!("{}{}{}", &out[..open_end], rpr, &out[open_end..]);
        }
    }
    out
}

fn op_set_text(block: &str, para: usize, run: usize, text: &str) -> Option<String> {
    let p = *para_spans(block).get(para)?;
    let r = *run_spans(block, p).get(run)?;
    let rb = &block[r.0..r.1];
    let esc = xml_escape_text(text);
    let new_rb = if let (Some(a), Some(b)) = (rb.find("<a:t>"), rb.find("</a:t>")) {
        format!("{}{}{}", &rb[..a + 5], esc, &rb[b..])
    } else if rb.contains("<a:t/>") {
        rb.replace("<a:t/>", &format!("<a:t>{esc}</a:t>"))
    } else {
        // Append a text node before the run's close tag.
        let close = rb.rfind("</a:")?;
        format!("{}<a:t>{esc}</a:t>{}", &rb[..close], &rb[close..])
    };
    Some(splice(block, r, &new_rb))
}

fn op_set_style(
    block: &str,
    para: usize,
    run: usize,
    sz: Option<f32>,
    b: Option<bool>,
    color: Option<String>,
) -> Option<String> {
    let p = *para_spans(block).get(para)?;
    let r = *run_spans(block, p).get(run)?;
    let new_rb = style_run_block(&block[r.0..r.1], sz, b, color);
    Some(splice(block, r, &new_rb))
}

fn op_add_para(block: &str, after: Option<usize>, text: &str) -> Option<String> {
    let spans = para_spans(block);
    let (insert_at, template) = match after {
        Some(i) => {
            let sp = *spans.get(i)?;
            (sp.1, block[sp.0..sp.1].to_string())
        }
        None => match spans.last() {
            Some(sp) => (sp.1, block[sp.0..sp.1].to_string()),
            None => {
                // No paragraphs yet: insert before </p:txBody>.
                let te = block.find("</p:txBody>")?;
                (te, String::new())
            }
        },
    };
    let ppr = para_ppr(&template);
    let rpr = first_rpr(&template);
    let new_para = format!(
        "<a:p>{}<a:r>{}<a:t>{}</a:t></a:r></a:p>",
        ppr,
        rpr,
        xml_escape_text(text)
    );
    Some(format!(
        "{}{}{}",
        &block[..insert_at],
        new_para,
        &block[insert_at..]
    ))
}

fn op_delete_para(block: &str, para: usize) -> Option<String> {
    let spans = para_spans(block);
    if spans.len() <= 1 {
        return None; // never leave a text body paragraph-less
    }
    let sp = *spans.get(para)?;
    Some(splice(block, sp, ""))
}

fn op_add_run(block: &str, para: usize, text: &str) -> Option<String> {
    let p = *para_spans(block).get(para)?;
    let pblock = &block[p.0..p.1];
    let rpr = first_rpr(pblock);
    let new_run = format!("<a:r>{}<a:t>{}</a:t></a:r>", rpr, xml_escape_text(text));
    if pblock.ends_with("/>") {
        // A self-closing empty paragraph: expand it around the run.
        return Some(splice(block, p, &format!("<a:p>{new_run}</a:p>")));
    }
    // Insert before <a:endParaRPr> when present, else before </a:p>.
    let insert_at = match pblock.find("<a:endParaRPr") {
        Some(i) => p.0 + i,
        None => p.1 - "</a:p>".len(),
    };
    Some(format!(
        "{}{}{}",
        &block[..insert_at],
        new_run,
        &block[insert_at..]
    ))
}

fn op_delete_run(block: &str, para: usize, run: usize) -> Option<String> {
    let p = *para_spans(block).get(para)?;
    let r = *run_spans(block, p).get(run)?;
    Some(splice(block, r, ""))
}

fn op_move_resize(
    block: &str,
    new_off: Option<(i64, i64)>,
    new_ext: Option<(i64, i64)>,
) -> Option<String> {
    let mut out = block.to_string();
    if !out.contains("<a:off") {
        // No explicit transform: insert one into the property bag so
        // the element stops inheriting its position from the layout.
        let (x, y) = new_off.unwrap_or((0, 0));
        let (cx, cy) = new_ext.unwrap_or_else(|| {
            let g = geom_of(&out);
            (g.2, g.3)
        });
        let xfrm = format!(
            "<a:xfrm><a:off x=\"{x}\" y=\"{y}\"/><a:ext cx=\"{cx}\" cy=\"{cy}\"/></a:xfrm>"
        );
        let anchor = ["<p:spPr>", "<p:spPr ", "<p:grpSpPr>"]
            .iter()
            .filter_map(|a| out.find(a).map(|i| (a, i)))
            .next()?;
        let tag_end = out[anchor.1..].find('>')? + anchor.1 + 1;
        out = format!("{}{}{}", &out[..tag_end], xfrm, &out[tag_end..]);
        return Some(out);
    }
    if let Some((x, y)) = new_off {
        let s = out.find("<a:off")?;
        out = set_tag_attr(&out, s, "x", &x.to_string());
        let s = out.find("<a:off")?;
        out = set_tag_attr(&out, s, "y", &y.to_string());
    }
    if let Some((cx, cy)) = new_ext {
        match out.find("<a:ext") {
            Some(s) => {
                out = set_tag_attr(&out, s, "cx", &cx.to_string());
                let s = out.find("<a:ext")?;
                out = set_tag_attr(&out, s, "cy", &cy.to_string());
            }
            None => {
                // off without ext: add the ext right after the off tag.
                let s = out.find("<a:off")?;
                let (_, _, e, _, _) = next_tag(&out, s)?;
                out = format!("{}<a:ext cx=\"{cx}\" cy=\"{cy}\"/>{}", &out[..e], &out[e..]);
            }
        }
    }
    Some(out)
}

fn content_type_for(ext: &str) -> &'static str {
    match ext {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "bmp" => "image/bmp",
        "svg" => "image/svg+xml",
        "emf" => "image/x-emf",
        "wmf" => "image/x-wmf",
        "tif" | "tiff" => "image/tiff",
        _ => "application/octet-stream",
    }
}

fn ensure_content_type(pkg: &mut Package, ext: &str) {
    let Ok(ct) = pkg.get_text(CONTENT_TYPES) else {
        return;
    };
    if ct.contains(&format!("Extension=\"{ext}\"")) {
        return;
    }
    let Some(pos) = ct.find("</Types>") else {
        return;
    };
    let add = format!(
        "<Default Extension=\"{ext}\" ContentType=\"{}\"/>",
        content_type_for(ext)
    );
    let out = format!("{}{}{}", &ct[..pos], add, &ct[pos..]);
    pkg.parts
        .insert(CONTENT_TYPES.to_string(), out.into_bytes());
}

/// Replace only this picture's embed reference; other uses of the old relationship remain intact.
fn op_replace_pic(
    pkg: &mut Package,
    slide_part: &str,
    block: &str,
    bytes: &[u8],
    ext: &str,
) -> PptxResult<Option<String>> {
    let Some(blip) = find_tag_text(block, "<a:blip") else {
        return Ok(None);
    };
    if attr(blip, "r:embed").is_none() {
        return Ok(None);
    }
    let start = block.find(blip).ok_or("找不到图片节点")?;
    let id = add_picture_relationship(pkg, slide_part, bytes, ext)?;
    Ok(Some(set_tag_attr(block, start, "r:embed", &id)))
}

/// Apply a batch of element operations to one slide of the package.
/// Returns the number of operations that actually changed something.
pub fn apply_slide_ops(pkg: &mut Package, slide_idx: usize, ops: &[SlideOp]) -> PptxResult<usize> {
    if ops.is_empty() {
        return Ok(0);
    }
    let refs = slides(pkg)?;
    let sref = refs
        .get(slide_idx)
        .ok_or_else(|| format!("slide index {slide_idx} out of range"))?;
    let part = sref.part.clone();
    let mut xml = pkg.get_text(&part)?;
    let mut applied = 0usize;
    for command in ops {
        let (parents, op) = match command {
            SlideOp::InGroup { parents, op } => (parents.as_slice(), op.as_ref()),
            op => (&[][..], op),
        };
        let span = |el| span_at(&xml, parents, el);
        let edit_element = |el, edit: &dyn Fn(&str) -> Option<String>| {
            span(el).and_then(|(a, b)| edit(&xml[a..b]).map(|new| splice(&xml, (a, b), &new)))
        };
        let next: Option<String> = match op {
            SlideOp::InGroup { .. } => return Err("不要嵌套组合操作封装".into()),
            SlideOp::AddElement(el) => {
                if !parents.is_empty() {
                    return Err("新增元素只能放在页面根层".into());
                }
                Some(add_element(pkg, &part, &xml, el)?)
            }
            SlideOp::ResetStyle { el, para, run } => {
                edit_element(*el, &|block| reset_run_style(block, *para, *run))
            }
            SlideOp::Preset { el, preset } => {
                edit_element(*el, &|block| change_preset(block, preset))
            }
            SlideOp::LineColor { el, color } => {
                if !valid_rgb(color) {
                    return Err("线条颜色必须为 RRGGBB".into());
                }
                edit_element(*el, &|block| set_line_color(block, color))
            }
            SlideOp::ReplacePic { el, bytes, ext } => {
                if let Some((a, b)) = span(*el) {
                    op_replace_pic(pkg, &part, &xml[a..b], bytes, ext)?
                        .map(|block| splice(&xml, (a, b), &block))
                } else {
                    None
                }
            }
            SlideOp::SetText {
                el,
                para,
                run,
                text,
            } => span(*el).and_then(|(s, e)| {
                op_set_text(&xml[s..e], *para, *run, text).map(|nb| splice(&xml, (s, e), &nb))
            }),
            SlideOp::SetStyle {
                el,
                para,
                run,
                sz,
                b,
                color,
            } => {
                if color.as_deref().is_some_and(|color| !valid_rgb(color)) {
                    return Err("文本颜色必须为 RRGGBB".into());
                }
                span(*el).and_then(|(s, e)| {
                    op_set_style(&xml[s..e], *para, *run, *sz, *b, color.clone())
                        .map(|nb| splice(&xml, (s, e), &nb))
                })
            }
            SlideOp::AddPara { el, after, text } => span(*el).and_then(|(s, e)| {
                op_add_para(&xml[s..e], *after, text).map(|nb| splice(&xml, (s, e), &nb))
            }),
            SlideOp::DeletePara { el, para } => span(*el).and_then(|(s, e)| {
                op_delete_para(&xml[s..e], *para).map(|nb| splice(&xml, (s, e), &nb))
            }),
            SlideOp::AddRun { el, para, text } => span(*el).and_then(|(s, e)| {
                op_add_run(&xml[s..e], *para, text).map(|nb| splice(&xml, (s, e), &nb))
            }),
            SlideOp::DeleteRun { el, para, run } => span(*el).and_then(|(s, e)| {
                op_delete_run(&xml[s..e], *para, *run).map(|nb| splice(&xml, (s, e), &nb))
            }),
            SlideOp::Move { el, x, y } => span(*el).and_then(|(s, e)| {
                op_move_resize(&xml[s..e], Some((*x, *y)), None).map(|nb| splice(&xml, (s, e), &nb))
            }),
            SlideOp::Resize { el, cx, cy } => span(*el).and_then(|(s, e)| {
                op_move_resize(&xml[s..e], None, Some((*cx, *cy)))
                    .map(|nb| splice(&xml, (s, e), &nb))
            }),
        };
        if let Some(nx) = next {
            if nx != xml {
                xml = nx;
                applied += 1;
            }
        }
    }
    if applied > 0 {
        pkg.parts.insert(part, xml.into_bytes());
    }
    Ok(applied)
}

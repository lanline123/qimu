//! Assembly: merge an ordered list of fragments (single-slide or
//! multi-slide) into one presentation. This is the export engine.
//!
//! Strategy: the first fragment is the base (its masters, theme, slide
//! size and package scaffolding win). Every other fragment's parts are
//! imported under a per-fragment filename prefix (`q{i}_`), keeping
//! directories — so relationship targets only ever change their final
//! filename segment, and each slide keeps its own layout/master/theme
//! chain (faithful rendering per source deck). Notes slides are imported;
//! their notes-master reference is repointed at the base deck's notes
//! master. Correctness over size: shared parts are duplicated per
//! fragment, content-hash dedup is a later optimization.
//!
//! SECTION WIRING POINT (PLAN §3.3 item 2, §8 decision 5): when
//! `crate::section_link::section_mode()` is `Native`, this is where
//! each assembled content block is additionally written as a
//! same-named PPT section (`p14:sectionLst`) and the manifest
//! records the block-id <-> section mapping. While the mode is
//! `ManifestOnly` (the P0 default), assembly stays byte-for-byte
//! the pre-integration behaviour: no sections written, block
//! boundaries live in the manifest alone.

use crate::pptx::{attr, parse_rels, resolve_target, scan_elements, slides, Package};
use std::collections::BTreeMap;

const PRESENTATION: &str = "ppt/presentation.xml";
const PRESENTATION_RELS: &str = "ppt/_rels/presentation.xml.rels";
const CONTENT_TYPES: &str = "[Content_Types].xml";
const RELS_HEADER: &str = "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n<Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\">";
const SLIDE_TYPE: &str =
    "http://schemas.openxmlformats.org/officeDocument/2006/relationships/slide";
const MASTER_TYPE: &str =
    "http://schemas.openxmlformats.org/officeDocument/2006/relationships/slideMaster";

fn dir_of(part: &str) -> &str {
    match part.rfind('/') {
        Some(i) => &part[..i],
        None => "",
    }
}

fn file_of(part: &str) -> &str {
    match part.rfind('/') {
        Some(i) => &part[i + 1..],
        None => part,
    }
}

fn is_shared(part: &str) -> bool {
    part == CONTENT_TYPES
        || part == "_rels/.rels"
        || part == PRESENTATION
        || part == PRESENTATION_RELS
        || part.starts_with("docProps/")
        || part == "ppt/presProps.xml"
        || part == "ppt/viewProps.xml"
        || part == "ppt/tableStyles.xml"
        || part.starts_with("ppt/notesMasters/")
}

fn renamed(part: &str, prefix: &str) -> String {
    let dir = dir_of(part);
    if dir.is_empty() {
        format!("{prefix}{}", file_of(part))
    } else {
        format!("{dir}/{prefix}{}", file_of(part))
    }
}

fn relativize(from_dir: &str, target: &str) -> String {
    if from_dir.is_empty() {
        return target.to_string();
    }
    let from: Vec<&str> = from_dir.split('/').collect();
    let tgt: Vec<&str> = target.split('/').collect();
    let mut common = 0;
    while common < from.len() && common < tgt.len() && from[common] == tgt[common] {
        common += 1;
    }
    let ups = from.len() - common;
    let mut out = String::new();
    for _ in 0..ups {
        out.push_str("../");
    }
    out.push_str(&tgt[common..].join("/"));
    out
}

fn rels_owner(rels_part: &str) -> String {
    // "ppt/slides/_rels/slide1.xml.rels" -> "ppt/slides/slide1.xml"
    let stripped = rels_part.replace("/_rels/", "/");
    stripped
        .strip_suffix(".rels")
        .unwrap_or(&stripped)
        .to_string()
}

fn replace_target(element: &str, new_target: &str) -> String {
    if let Some(pos) = element.find("Target=\"") {
        let start = pos + "Target=\"".len();
        if let Some(endrel) = element[start..].find('"') {
            let mut out = element.to_string();
            out.replace_range(start..start + endrel, new_target);
            return out;
        }
    }
    element.to_string()
}

/// Rewrite one imported .rels part: targets are repointed at renamed
/// parts; references to shared parts are dropped, except notes-master
/// references which are repointed at the base deck's notes master.
fn rewrite_rels(
    rels_xml: &str,
    owner_old: &str,
    rename_map: &BTreeMap<String, String>,
    base_notes_master: &Option<String>,
) -> String {
    let owner_dir = dir_of(owner_old).to_string();
    let mut out = String::from(RELS_HEADER);
    for el in scan_elements(rels_xml, "<Relationship") {
        let target = match attr(el, "Target") {
            Some(t) => t,
            None => continue,
        };
        if attr(el, "TargetMode") == Some("External") {
            out.push_str(el);
            continue;
        }
        let abs = resolve_target(owner_old, target);
        let new_abs = if let Some(n) = rename_map.get(&abs) {
            n.clone()
        } else if abs.starts_with("ppt/notesMasters/") {
            match base_notes_master {
                Some(bnm) => bnm.clone(),
                None => continue,
            }
        } else if is_shared(&abs) {
            continue;
        } else {
            continue;
        };
        let rel_target = relativize(&owner_dir, &new_abs);
        out.push_str(&replace_target(el, &rel_target));
    }
    out.push_str("</Relationships>");
    out
}

fn lst_inner(xml: &str, tag: &str) -> String {
    let open = format!("<p:{tag}>");
    let close = format!("</p:{tag}>");
    if let Some(s) = xml.find(&open) {
        if let Some(e) = xml[s..].find(&close) {
            return xml[s + open.len()..s + e].to_string();
        }
    }
    String::new()
}

fn replace_lst(xml: &str, tag: &str, new_inner: &str) -> String {
    let open = format!("<p:{tag}>");
    let close = format!("</p:{tag}>");
    let self_closed = format!("<p:{tag}/>");
    if let Some(s) = xml.find(&open) {
        if let Some(e) = xml[s..].find(&close) {
            let mut out = xml.to_string();
            out.replace_range(s..s + e + close.len(), &format!("{open}{new_inner}{close}"));
            return out;
        }
    }
    if let Some(s) = xml.find(&self_closed) {
        let mut out = xml.to_string();
        out.replace_range(s..s + self_closed.len(), &format!("{open}{new_inner}{close}"));
        return out;
    }
    xml.to_string()
}

pub fn assemble(fragments: &[Package]) -> Result<Package, String> {
    if fragments.is_empty() {
        return Err("没有可组装的片段".into());
    }
    let base = &fragments[0];
    let mut out = base.clone();
    let base_notes_master = base
        .parts
        .keys()
        .find(|k| k.starts_with("ppt/notesMasters/") && k.ends_with(".xml"))
        .cloned();

    // Final slide order: (part name in `out`, presentation rId).
    let mut final_slides: Vec<(String, String)> = slides(base)?
        .iter()
        .map(|s| (s.part.clone(), s.rid.clone()))
        .collect();

    // Fresh rIds for presentation.xml.rels, above the base's maximum.
    let base_pres_rels = base.get_text(PRESENTATION_RELS)?;
    let mut next_rid: u32 = parse_rels(&base_pres_rels)
        .iter()
        .filter_map(|r| r.id.strip_prefix("rId").and_then(|n| n.parse::<u32>().ok()))
        .max()
        .unwrap_or(0)
        + 1;
    let mut fresh_rid = || {
        let id = format!("rId{next_rid}");
        next_rid += 1;
        id
    };

    let mut new_pres_rels = String::new();
    let mut new_master_entries = String::new();
    let mut master_id: u64 = scan_elements(&lst_inner(&base.get_text(PRESENTATION)?, "sldMasterIdLst"), "<p:sldMasterId")
        .iter()
        .filter_map(|el| attr(el, "id").and_then(|v| v.parse::<u64>().ok()))
        .max()
        .unwrap_or(2147483648)
        + 1;

    let mut ct = out.get_text(CONTENT_TYPES)?.to_string();

    for (i, frag) in fragments.iter().enumerate().skip(1) {
        let prefix = format!("q{i}_");
        // rename map for this fragment
        let mut map: BTreeMap<String, String> = BTreeMap::new();
        for name in frag.parts.keys() {
            if !is_shared(name) {
                map.insert(name.clone(), renamed(name, &prefix));
            }
        }
        // import parts (rewriting .rels on the way)
        for (old, new) in &map {
            let bytes = frag.get(old)?;
            if old.ends_with(".rels") {
                let owner = rels_owner(old);
                let text = String::from_utf8(bytes.to_vec())
                    .map_err(|_| format!("{old} is not UTF-8"))?;
                let rewritten = rewrite_rels(&text, &owner, &map, &base_notes_master);
                out.parts.insert(new.clone(), rewritten.into_bytes());
            } else {
                out.parts.insert(new.clone(), bytes.to_vec());
            }
        }
        // content types: adopt this fragment's overrides for imported parts
        if let Ok(frag_ct) = frag.get_text(CONTENT_TYPES) {
            for el in scan_elements(&frag_ct, "<Override") {
                if let (Some(pn), Some(_)) = (attr(el, "PartName"), attr(el, "ContentType")) {
                    let part_name = pn.strip_prefix('/').unwrap_or(pn);
                    if let Some(new_name) = map.get(part_name) {
                        let needle = format!("PartName=\"/{new_name}\"");
                        if !ct.contains(&needle) {
                            let new_el = manual_partname(el, &format!("/{new_name}"));
                            if let Some(pos) = ct.rfind("</Types>") {
                                ct.insert_str(pos, &new_el);
                            }
                        }
                    }
                }
            }
        }
        // masters of this fragment → presentation entries + rels
        let frag_pres = frag.get_text(PRESENTATION)?;
        let frag_pres_rels = parse_rels(&frag.get_text(PRESENTATION_RELS)?);
        let rel_by_id: BTreeMap<&str, &crate::pptx::Rel> =
            frag_pres_rels.iter().map(|r| (r.id.as_str(), r)).collect();
        for el in scan_elements(&lst_inner(&frag_pres, "sldMasterIdLst"), "<p:sldMasterId") {
            if let Some(rid) = attr(el, "r:id") {
                if let Some(rel) = rel_by_id.get(rid) {
                    let abs = resolve_target(PRESENTATION, &rel.target);
                    if let Some(new_master) = map.get(&abs) {
                        let new_rid = fresh_rid();
                        new_master_entries.push_str(&format!(
                            "<p:sldMasterId id=\"{master_id}\" r:id=\"{new_rid}\"/>"
                        ));
                        master_id += 1;
                        new_pres_rels.push_str(&format!(
                            "<Relationship Id=\"{new_rid}\" Type=\"{MASTER_TYPE}\" Target=\"{}\"/>",
                            relativize("ppt", new_master)
                        ));
                    }
                }
            }
        }
        // slides of this fragment
        for s in slides(frag)? {
            if let Some(new_part) = map.get(&s.part) {
                let new_rid = fresh_rid();
                new_pres_rels.push_str(&format!(
                    "<Relationship Id=\"{new_rid}\" Type=\"{SLIDE_TYPE}\" Target=\"{}\"/>",
                    relativize("ppt", new_part)
                ));
                final_slides.push((new_part.clone(), new_rid));
            }
        }
    }

    // presentation.xml: masters = base + imported; slides = final order
    let pres = out.get_text(PRESENTATION)?;
    let masters_inner = format!(
        "{}{}",
        lst_inner(&pres, "sldMasterIdLst"),
        new_master_entries
    );
    let pres = replace_lst(&pres, "sldMasterIdLst", &masters_inner);
    let mut sld_inner = String::new();
    for (idx, (_, rid)) in final_slides.iter().enumerate() {
        sld_inner.push_str(&format!("<p:sldId id=\"{}\" r:id=\"{rid}\"/>", 256 + idx));
    }
    let pres = replace_lst(&pres, "sldIdLst", &sld_inner);
    out.parts.insert(PRESENTATION.to_string(), pres.into_bytes());

    // presentation rels: base rels + new entries
    let mut pres_rels = out.get_text(PRESENTATION_RELS)?.to_string();
    if let Some(pos) = pres_rels.rfind("</Relationships>") {
        pres_rels.insert_str(pos, &new_pres_rels);
    }
    out.parts
        .insert(PRESENTATION_RELS.to_string(), pres_rels.into_bytes());

    out.parts.insert(CONTENT_TYPES.to_string(), ct.into_bytes());
    Ok(out)
}

fn manual_partname(element: &str, new_partname: &str) -> String {
    if let Some(pos) = element.find("PartName=\"") {
        let start = pos + "PartName=\"".len();
        if let Some(endrel) = element[start..].find('"') {
            let mut out = element.to_string();
            out.replace_range(start..start + endrel, new_partname);
            return out;
        }
    }
    element.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pptx::{split, structural_errors};
    use sha2::{Digest, Sha256};

    fn xml_hash(pkg: &Package, part: &str) -> String {
        let mut h = Sha256::new();
        h.update(pkg.get(part).unwrap());
        format!("{:x}", h.finalize())
    }

    #[test]
    fn assemble_mixed_order_preserves_slide_xml() {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../user/files/Redwood_Visit_Revised_4tph_0_6vcr.pptx"
        );
        let bytes = std::fs::read(path).expect("redwood deck present");
        let pkg = Package::load(&bytes).unwrap();
        let frags = split(&pkg).unwrap();
        assert_eq!(frags.len(), 17);
        // pick slides 5, 2, 16 (0-based 4, 1, 15) in that order
        let picked = vec![frags[4].clone(), frags[1].clone(), frags[15].clone()];
        let merged = assemble(&picked).unwrap();
        let errs = structural_errors(&merged);
        assert!(errs.is_empty(), "structural errors: {errs:?}");
        let merged_slides = slides(&merged).unwrap();
        assert_eq!(merged_slides.len(), 3);
        // slide XML bytes must equal the originals' (identity by content)
        let orig = slides(&pkg).unwrap();
        for (pos, src_idx) in [4usize, 1, 15].iter().enumerate() {
            assert_eq!(
                xml_hash(&merged, &merged_slides[pos].part),
                xml_hash(&pkg, &orig[*src_idx].part),
                "slide at position {pos} differs from source slide {src_idx}"
            );
        }
        // and the merged deck itself splits/reassembles cleanly
        let report = crate::pptx::split_reassemble_verify(&merged).unwrap();
        assert!(report.ok);
    }
}

#[cfg(test)]
mod debug_tests {
    use super::*;
    use crate::pptx::{split, split_reassemble_verify};

    #[test]
    fn nested_assembly_like_server_export() {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../user/files/Redwood_Visit_Revised_4tph_0_6vcr.pptx"
        );
        let bytes = std::fs::read(path).unwrap();
        let pkg = Package::load(&bytes).unwrap();
        let singles = split(&pkg).unwrap();
        // blocks like the server import
        let mut block_frags = Vec::new();
        for range in [0..6usize, 6..12, 12..17] {
            let picked: Vec<Package> = singles[range].to_vec();
            block_frags.push(assemble(&picked).unwrap());
        }
        // export: split each block, assemble all
        let mut all: Vec<Package> = Vec::new();
        for bf in &block_frags {
            all.extend(split(bf).unwrap());
        }
        assert_eq!(all.len(), 17);
        let merged = assemble(&all).unwrap();
        // Regression: rewritten .rels parts must be well-formed — exactly
        // one <Relationships root (a prefix-collision in scan_elements
        // once doubled it, which strict readers reject).
        for (name, bytes) in &merged.parts {
            if name.ends_with(".rels") {
                let text = String::from_utf8_lossy(bytes);
                assert_eq!(
                    text.matches("<Relationships").count(),
                    1,
                    "doubled Relationships root in {name}"
                );
                assert!(text.trim_end().ends_with("</Relationships>"), "unterminated rels in {name}");
            }
        }
        let report = split_reassemble_verify(&merged).unwrap();
        println!("verify: slides={} fp_match={} errors={:?}", report.slide_count, report.fingerprints_match, report.structural_errors);
        let errs = crate::pptx::structural_errors(&merged);
        println!("structural on merged: {} {:?}", errs.len(), &errs[..errs.len().min(5)]);
        assert!(report.ok, "nested assembly failed verification");
    }
}

#[cfg(test)]
mod debug_charts {
    use super::*;
    use crate::pptx::split;

    #[test]
    fn case2_charts_survive_split_and_assemble() {
        let bytes = std::fs::read("/home/hatch/workspace/user/files/Case2_Charts_Animation.pptx")
            .expect("case2 deck");
        let pkg = Package::load(&bytes).unwrap();
        let singles = split(&pkg).unwrap();
        assert_eq!(singles.len(), 7);
        for (i, s) in singles.iter().enumerate() {
            let charts: Vec<&String> = s
                .parts
                .keys()
                .filter(|k| k.contains("charts/") && k.ends_with(".xml"))
                .collect();
            println!("single {i}: {} parts, charts: {charts:?}", s.parts.len());
        }
        let merged = assemble(&singles).unwrap();
        let charts: Vec<&String> = merged
            .parts
            .keys()
            .filter(|k| k.contains("chart"))
            .collect();
        println!("merged charts: {charts:?}");
        assert!(charts.iter().any(|c| c.contains("chart1")), "charts lost in assemble");
    }
}


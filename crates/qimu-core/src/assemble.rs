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

use crate::pptx::{
    described_part, parse_rels, parse_xml, rels_xml, resolve_target, serialize_xml, slides, Package,
};
use craft_xml::{El, Local, Node};
use std::collections::BTreeMap;

const PRESENTATION: &str = "ppt/presentation.xml";
const PRESENTATION_RELS: &str = "ppt/_rels/presentation.xml.rels";
const CONTENT_TYPES: &str = "[Content_Types].xml";
const SLIDE_TYPE: &str =
    "http://schemas.openxmlformats.org/officeDocument/2006/relationships/slide";
const MASTER_TYPE: &str =
    "http://schemas.openxmlformats.org/officeDocument/2006/relationships/slideMaster";

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
    let dir = craft_opc::dir_of(part);
    if dir.is_empty() {
        format!("{prefix}{}", craft_opc::file_of(part))
    } else {
        format!("{dir}/{prefix}{}", craft_opc::file_of(part))
    }
}

/// Rewrite one imported .rels part: targets are repointed at renamed
/// parts; references to shared parts are dropped, except notes-master
/// references which are repointed at the base deck's notes master.
/// Relationships are parsed and re-serialized (craft-xml via
/// `pptx::rels_xml`), ids preserved; only the target values change
/// semantically.
fn rewrite_rels(
    rels_src: &str,
    owner_old: &str,
    rename_map: &BTreeMap<String, String>,
    base_notes_master: &Option<String>,
) -> String {
    let items: Vec<(String, String, String, bool)> = parse_rels(rels_src)
        .into_iter()
        .filter_map(|r| {
            if r.external {
                return Some((r.id, r.ty, r.target, true));
            }
            let abs = resolve_target(owner_old, &r.target);
            let new_abs = if let Some(n) = rename_map.get(&abs) {
                n.clone()
            } else if abs.starts_with("ppt/notesMasters/") {
                base_notes_master.clone()?
            } else {
                // Shared scaffolding (is_shared) and anything not
                // imported with this fragment: dropped, as before.
                return None;
            };
            Some((r.id, r.ty, craft_opc::relative(owner_old, &new_abs), false))
        })
        .collect();
    rels_xml(&items)
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

    let mut new_pres_rel_items: Vec<(String, String, String, bool)> = Vec::new();
    let mut new_master_items: Vec<El> = Vec::new();
    // The base presentation tree, edited in place at the end (imports
    // never touch the shared presentation part itself).
    let mut pres_root = parse_xml(&base.get_text(PRESENTATION)?, PRESENTATION)?;
    let mut master_id: u64 = pres_root
        .find(Local("sldMasterIdLst"))
        .and_then(|lst| {
            lst.children(Local("sldMasterId"))
                .filter_map(|el| el.attr("id").and_then(|v| v.parse::<u64>().ok()))
                .max()
        })
        .unwrap_or(2147483648)
        + 1;

    // The base [Content_Types].xml tree; fragment overrides for
    // imported parts are appended to it.
    let mut ct_root = parse_xml(&out.get_text(CONTENT_TYPES)?, CONTENT_TYPES)?;
    let mut ct_known: std::collections::BTreeSet<String> = ct_root
        .elements()
        .filter(|el| el.is(Local("Override")))
        .filter_map(|el| el.attr("PartName").map(str::to_string))
        .collect();

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
                let owner = described_part(old);
                let text =
                    String::from_utf8(bytes.to_vec()).map_err(|_| format!("{old} is not UTF-8"))?;
                let rewritten = rewrite_rels(&text, &owner, &map, &base_notes_master);
                out.parts.insert(new.clone(), rewritten.into_bytes());
            } else {
                out.parts.insert(new.clone(), bytes.to_vec());
            }
        }
        // content types: adopt this fragment's overrides for imported parts
        if let Ok(frag_ct) = frag.get_text(CONTENT_TYPES) {
            if let Ok(frag_ct_root) = parse_xml(&frag_ct, CONTENT_TYPES) {
                for el in frag_ct_root.elements() {
                    if !el.is(Local("Override")) {
                        continue;
                    }
                    let (Some(pn), Some(cty)) = (el.attr("PartName"), el.attr("ContentType"))
                    else {
                        continue;
                    };
                    let part_name = pn.strip_prefix('/').unwrap_or(pn);
                    if let Some(new_name) = map.get(part_name) {
                        let new_pn = format!("/{new_name}");
                        if ct_known.insert(new_pn.clone()) {
                            let mut o = El::new("Override");
                            o.set_attr("PartName", &new_pn);
                            o.set_attr("ContentType", cty);
                            ct_root.push(o);
                        }
                    }
                }
            }
        }
        // masters of this fragment → presentation entries + rels
        let frag_pres_root = parse_xml(&frag.get_text(PRESENTATION)?, PRESENTATION)?;
        let frag_pres_rels = parse_rels(&frag.get_text(PRESENTATION_RELS)?);
        let rel_by_id: BTreeMap<&str, &crate::pptx::Rel> =
            frag_pres_rels.iter().map(|r| (r.id.as_str(), r)).collect();
        let master_items: Vec<El> = frag_pres_root
            .find(Local("sldMasterIdLst"))
            .map(|lst| lst.children(Local("sldMasterId")).cloned().collect())
            .unwrap_or_default();
        for el in &master_items {
            if let Some(rid) = el.attr_local("r:id") {
                if let Some(rel) = rel_by_id.get(rid) {
                    let abs = resolve_target(PRESENTATION, &rel.target);
                    if let Some(new_master) = map.get(&abs) {
                        let new_rid = fresh_rid();
                        let mut item = El::new("p:sldMasterId");
                        item.set_attr("id", master_id.to_string());
                        item.set_attr("r:id", &new_rid);
                        master_id += 1;
                        new_master_items.push(item);
                        new_pres_rel_items.push((
                            new_rid,
                            MASTER_TYPE.to_string(),
                            craft_opc::relative(PRESENTATION, new_master),
                            false,
                        ));
                    }
                }
            }
        }
        // slides of this fragment
        for s in slides(frag)? {
            if let Some(new_part) = map.get(&s.part) {
                let new_rid = fresh_rid();
                new_pres_rel_items.push((
                    new_rid.clone(),
                    SLIDE_TYPE.to_string(),
                    craft_opc::relative(PRESENTATION, new_part),
                    false,
                ));
                final_slides.push((new_part.clone(), new_rid));
            }
        }
    }

    // presentation.xml: masters = base + imported; slides = final order
    if let Some(lst) = pres_root.child_mut(Local("sldMasterIdLst")) {
        for item in new_master_items {
            lst.push(item);
        }
    }
    if let Some(lst) = pres_root.child_mut(Local("sldIdLst")) {
        lst.children = final_slides
            .iter()
            .enumerate()
            .map(|(idx, (_, rid))| {
                let mut el = El::new("p:sldId");
                el.set_attr("id", (256 + idx).to_string());
                el.set_attr("r:id", rid);
                Node::El(el)
            })
            .collect();
    }
    out.parts
        .insert(PRESENTATION.to_string(), serialize_xml(&pres_root));

    // presentation rels: base rels (document order) + new entries
    let mut rel_items: Vec<(String, String, String, bool)> =
        parse_rels(&out.get_text(PRESENTATION_RELS)?)
            .into_iter()
            .map(|r| (r.id, r.ty, r.target, r.external))
            .collect();
    rel_items.extend(new_pres_rel_items);
    out.parts.insert(
        PRESENTATION_RELS.to_string(),
        rels_xml(&rel_items).into_bytes(),
    );

    out.parts
        .insert(CONTENT_TYPES.to_string(), serialize_xml(&ct_root));
    Ok(out)
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
        let Some(bytes) = crate::test_fixtures::redwood_deck() else {
            eprintln!("{}", crate::test_fixtures::skip_msg("Redwood deck"));
            return;
        };
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
        let Some(bytes) = crate::test_fixtures::redwood_deck() else {
            eprintln!("{}", crate::test_fixtures::skip_msg("Redwood deck"));
            return;
        };
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
                assert!(
                    text.trim_end().ends_with("</Relationships>"),
                    "unterminated rels in {name}"
                );
            }
        }
        let report = split_reassemble_verify(&merged).unwrap();
        println!(
            "verify: slides={} fp_match={} errors={:?}",
            report.slide_count, report.fingerprints_match, report.structural_errors
        );
        let errs = crate::pptx::structural_errors(&merged);
        println!(
            "structural on merged: {} {:?}",
            errs.len(),
            &errs[..errs.len().min(5)]
        );
        assert!(report.ok, "nested assembly failed verification");
    }
}

#[cfg(test)]
mod debug_charts {
    use super::*;
    use crate::pptx::split;

    #[test]
    fn case2_charts_survive_split_and_assemble() {
        let Some(bytes) = crate::test_fixtures::case2_deck() else {
            eprintln!("{}", crate::test_fixtures::skip_msg("Case2 deck"));
            return;
        };
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
        assert!(
            charts.iter().any(|c| c.contains("chart1")),
            "charts lost in assemble"
        );
    }
}

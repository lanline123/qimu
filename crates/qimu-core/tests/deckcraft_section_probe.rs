//! P0 PROBE — DeckCraft section write/read round trip.
//!
//! Question (PLAN §3.3, §6 P0, §8 decision 5): if a presentation
//! whose slides are grouped into sections (qimu content blocks) is
//! saved through DeckCraft's PPTX writer, do the sections really
//! land in the package as `p14:sectionLst`, with the right slides
//! in each section — and does a re-import restore names AND slide
//! membership?
//!
//! Method (mirrors the tasking, stricter than DeckCraft's own
//! corpus test, which asserts section names only):
//!   1. Build a 3-slide deck in memory: section "Alpha" = slides
//!      1-2, section "导入块 Beta" = slide 3 (CJK name on purpose:
//!      UTF-8 names must survive too).
//!   2. Export via `deckcraft_pptx::export`.
//!   3. Unzip and inspect `ppt/presentation.xml` directly: a
//!      `p14:sectionLst` must exist, carry both sections in order,
//!      and its `p14:sldId` references must resolve — through
//!      `p:sldIdLst` and the presentation rels — to the slide parts
//!      whose titles are the expected ones, in the expected
//!      grouping. The union of section slide ids must equal the
//!      deck's slide id list (no slide lost or double-filed).
//!   4. Re-import via `deckcraft_pptx::import` and assert the model
//!      sections match the originals by name and by member-slide
//!      titles.
//!   5. Export -> import once more and repeat the model assertions
//!      (the mapping must be stable, not a one-off).
//!
//! VERDICT RULES (read by the P0 report, enforced by CI red/green):
//!   * This test passes  => native section mode may be enabled:
//!     flip `deckcraft-sections` on (see `qimu_core::section_link`).
//!   * This test fails   => §8 decision 5 degradation stays in
//!     force (ManifestOnly); the failing assertion names the exact
//!     layer that broke (no sectionLst / wrong grouping / lost
//!     membership / unstable second cycle).
//!
//! Runs in CI only (`.github/workflows/p0-deckcraft.yml`); local
//! builds are out of policy for this project.

use std::io::Read;
use std::sync::Arc;

use deckcraft_model::text::TextBody;
use deckcraft_model::{defaults, LayoutType, Presentation, Section, Slide};

const SEC_A: &str = "Alpha";
const SEC_B: &str = "导入块 Beta";
const T1: &str = "P0 Slide One Alpha";
const T2: &str = "P0 Slide Two Alpha";
const T3: &str = "P0 Slide Three Beta";

fn titled_slide(p: &mut Presentation, kind: LayoutType, title: &str) -> Slide {
    let layout = defaults::layout_of_kind(p, kind).unwrap_or_default();
    let mut s = defaults::new_slide(p, layout).expect("probe: new_slide");
    if let Some(t) = s.shapes.first_mut() {
        t.text = Some(TextBody::from_text(title));
    }
    s
}

fn slide_title(s: &Slide) -> String {
    for shape in &s.shapes {
        if let Some(tb) = &shape.text {
            let t = tb.text();
            if !t.trim().is_empty() {
                return t.trim().to_string();
            }
        }
    }
    String::new()
}

fn build_probe_deck() -> Presentation {
    let mut p = defaults::new_presentation(None);
    let s1 = titled_slide(&mut p, LayoutType::Title, T1);
    let s2 = titled_slide(&mut p, LayoutType::TitleOnly, T2);
    let s3 = titled_slide(&mut p, LayoutType::TitleOnly, T3);
    let (id1, id2, id3) = (s1.id, s2.id, s3.id);
    p.slides = vec![Arc::new(s1), Arc::new(s2), Arc::new(s3)];
    p.sections = vec![
        Section {
            name: SEC_A.into(),
            slides: vec![id1, id2],
        },
        Section {
            name: SEC_B.into(),
            slides: vec![id3],
        },
    ];
    p
}

/// Extract (name, [sldId, ...]) per `<p14:section>` in document order.
fn parse_section_lst(xml: &str) -> Vec<(String, Vec<u32>)> {
    let mut out = Vec::new();
    let mut rest = xml;
    while let Some(i) = rest.find("<p14:section ") {
        let tag_start = i + "<p14:section ".len();
        let tag_end = rest[tag_start..]
            .find('>')
            .map(|j| tag_start + j)
            .expect("probe: section tag");
        let tag = &rest[tag_start..tag_end];
        let name = attr(tag, "name").expect("probe: section name attr");
        let close = rest[tag_end..]
            .find("</p14:section>")
            .map(|j| tag_end + j)
            .expect("probe: section close");
        let body = &rest[tag_end..close];
        let mut ids = Vec::new();
        let mut b = body;
        while let Some(k) = b.find("<p14:sldId ") {
            let t0 = k + "<p14:sldId ".len();
            let t1 = b[t0..].find('>').map(|j| t0 + j).expect("probe: sldId tag");
            ids.push(
                attr(&b[t0..t1], "id")
                    .expect("probe: sldId id")
                    .parse::<u32>()
                    .expect("probe: sldId id num"),
            );
            b = &b[t1..];
        }
        out.push((name, ids));
        rest = &rest[close..];
    }
    out
}

fn attr(tag: &str, key: &str) -> Option<String> {
    let pat = format!("{key}=\"");
    let i = tag.find(&pat)? + pat.len();
    let j = tag[i..].find('"')? + i;
    Some(tag[i..j].to_string())
}

/// (sldId, rId) pairs of the main `<p:sldIdLst>`, in document order.
fn parse_sld_id_lst(xml: &str) -> Vec<(u32, String)> {
    let start = xml.find("<p:sldIdLst>").expect("probe: sldIdLst") + "<p:sldIdLst>".len();
    let end = xml[start..]
        .find("</p:sldIdLst>")
        .map(|j| start + j)
        .expect("probe: sldIdLst close");
    let block = &xml[start..end];
    let mut out = Vec::new();
    let mut b = block;
    while let Some(k) = b.find("<p:sldId ") {
        let t0 = k + "<p:sldId ".len();
        let t1 = b[t0..].find('>').map(|j| t0 + j).expect("probe: sldId tag");
        let tag = &b[t0..t1];
        out.push((
            attr(tag, "id")
                .expect("probe: sldId id")
                .parse::<u32>()
                .expect("probe: sldId id num"),
            attr(tag, "r:id").expect("probe: sldId r:id"),
        ));
        b = &b[t1..];
    }
    out
}

fn rels_targets(rels: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut b = rels;
    while let Some(k) = b.find("<Relationship ") {
        let t0 = k + "<Relationship ".len();
        let t1 = b[t0..].find('>').map(|j| t0 + j).expect("probe: rel tag");
        let tag = &b[t0..t1];
        if let (Some(id), Some(target)) = (attr(tag, "Id"), attr(tag, "Target")) {
            out.push((id, target));
        }
        b = &b[t1..];
    }
    out
}

fn part_text(bytes: &[u8], name: &str) -> String {
    let mut zip = zip::ZipArchive::new(std::io::Cursor::new(bytes)).expect("probe: zip open");
    let mut f = zip
        .by_name(name)
        .unwrap_or_else(|_| panic!("probe: missing part {name}"));
    let mut s = String::new();
    f.read_to_string(&mut s).expect("probe: part utf8");
    s
}

fn first_a_t(xml: &str) -> String {
    let Some(i) = xml.find("<a:t>") else {
        return String::new();
    };
    let rest = &xml[i + 5..];
    let j = rest.find("</a:t>").expect("probe: a:t close");
    rest[..j].to_string()
}

fn assert_model_sections(p: &Presentation, cycle: &str) {
    let names: Vec<&str> = p.sections.iter().map(|s| s.name.as_str()).collect();
    assert_eq!(
        names,
        vec![SEC_A, SEC_B],
        "{cycle}: section names/order not restored"
    );
    let by_id: std::collections::HashMap<_, _> =
        p.slides.iter().map(|s| (s.id, slide_title(s))).collect();
    let members: Vec<Vec<String>> = p
        .sections
        .iter()
        .map(|sec| {
            sec.slides
                .iter()
                .map(|id| by_id.get(id).cloned().unwrap_or_default())
                .collect()
        })
        .collect();
    assert_eq!(
        members,
        vec![vec![T1.to_string(), T2.to_string()], vec![T3.to_string()]],
        "{cycle}: section slide membership not restored"
    );
}

#[test]
fn section_round_trip_probe() {
    // 1-2. Build and export.
    let deck = build_probe_deck();
    let bytes = deckcraft_pptx::export(&deck).expect("probe: export");

    // 3. Package-level inspection of ppt/presentation.xml.
    let pres = part_text(&bytes, "ppt/presentation.xml");
    let lst_at = pres
        .find("<p14:sectionLst")
        .expect("probe: NO p14:sectionLst in presentation.xml — sections were NOT written");
    let lst_end = pres[lst_at..]
        .find("</p14:sectionLst>")
        .map(|j| lst_at + j)
        .expect("probe: sectionLst close");
    println!(
        "--- p14:sectionLst excerpt ---\n{}\n--- end excerpt ---",
        &pres[lst_at..lst_end]
    );

    let sections = parse_section_lst(&pres);
    assert_eq!(
        sections.len(),
        2,
        "probe: expected exactly 2 p14:section entries"
    );
    assert_eq!(sections[0].0, SEC_A, "probe: first section name");
    assert_eq!(sections[1].0, SEC_B, "probe: second section name (CJK)");

    // Resolve section sldIds -> slide parts -> titles.
    let sld_lst = parse_sld_id_lst(&pres);
    assert_eq!(sld_lst.len(), 3, "probe: deck must have exactly 3 slides");
    let rels = part_text(&bytes, "ppt/_rels/presentation.xml.rels");
    let targets = rels_targets(&rels);
    let title_of_sld_id = |sld_id: u32| -> String {
        let rid = &sld_lst
            .iter()
            .find(|(id, _)| *id == sld_id)
            .expect("probe: sldId in lst")
            .1;
        let target = &targets
            .iter()
            .find(|(id, _)| id == rid)
            .expect("probe: rel target")
            .1;
        let part = if let Some(t) = target.strip_prefix("ppt/") {
            target.clone()
        } else {
            format!("ppt/{target}")
        };
        first_a_t(&part_text(&bytes, &part))
    };
    let grouped: Vec<Vec<String>> = sections
        .iter()
        .map(|(_, ids)| ids.iter().map(|id| title_of_sld_id(*id)).collect())
        .collect();
    assert_eq!(
        grouped,
        vec![vec![T1.to_string(), T2.to_string()], vec![T3.to_string()]],
        "probe: sectionLst slide grouping in the written package is wrong"
    );
    let mut filed: Vec<u32> = sections
        .iter()
        .flat_map(|(_, ids)| ids.iter().copied())
        .collect();
    filed.sort_unstable();
    let mut all: Vec<u32> = sld_lst.iter().map(|(id, _)| *id).collect();
    all.sort_unstable();
    assert_eq!(
        filed, all,
        "probe: sections must file every slide exactly once"
    );

    // 4. Re-import and check the model.
    let back = deckcraft_pptx::import(&bytes).expect("probe: re-import");
    assert_model_sections(&back, "cycle 1");

    // 5. Second cycle must be stable.
    let bytes2 = deckcraft_pptx::export(&back).expect("probe: export cycle 2");
    let back2 = deckcraft_pptx::import(&bytes2).expect("probe: re-import cycle 2");
    assert_model_sections(&back2, "cycle 2");

    println!("PROBE PASS: sections written as p14:sectionLst, grouping correct in package, names+membership restored over two cycles");
}

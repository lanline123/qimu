//! File-layer regression scaffold (Phase 3: hand-written zip/XML/OPC
//! code replaced by craft-zip / craft-xml / craft-opc).
//!
//! Every sample is GENERATED here — no binary fixtures in the repo.
//! The tests pin the observable contract of `Package` and friends:
//!
//! * `parity` — behaviour the old hand-written layer had, which the
//!   craft layer must reproduce.
//! * `changed` — behaviour that deliberately changed in the migration
//!   (each test names the old behaviour it replaces; see the Phase-3
//!   report's difference table).
//! * `hostile` — malformed / hostile archives and XML. The old layer
//!   had no limits and several string-scanning failure modes; the
//!   craft layer must refuse or neutralise every sample.
//!
//! Dual-track note: the pre-existing in-tree suite (assemble
//! mixed-order byte identity, nested assembly, Case2 charts,
//! split/reassemble verify on the Redwood deck) exercises the same
//! public API against real decks and runs in the same CI jobs.

use qimu_core::pptx::{self, Package};
use std::collections::BTreeMap;

// ---------------------------------------------------------------------
// CRC-32 (IEEE, bitwise — test data is small)
// ---------------------------------------------------------------------

fn crc32(data: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for &b in data {
        crc ^= b as u32;
        for _ in 0..8 {
            let mask = (crc & 1).wrapping_neg();
            crc = (crc >> 1) ^ (0xEDB8_8320 & mask);
        }
    }
    !crc
}

// ---------------------------------------------------------------------
// Byte-level zip builder: full control over names, CRCs and declared
// sizes, so hostile archives can be constructed exactly.
// ---------------------------------------------------------------------

struct Entry {
    name: String,
    data: Vec<u8>,
    crc_override: Option<u32>,
    /// Lie about the uncompressed size in both headers.
    declared_size_override: Option<u32>,
}

impl Entry {
    fn new(name: &str, data: impl Into<Vec<u8>>) -> Entry {
        Entry {
            name: name.to_string(),
            data: data.into(),
            crc_override: None,
            declared_size_override: None,
        }
    }
}

fn build_zip(entries: &[Entry]) -> Vec<u8> {
    let mut out: Vec<u8> = Vec::new();
    let mut central: Vec<u8> = Vec::new();
    for e in entries {
        let crc = e.crc_override.unwrap_or_else(|| crc32(&e.data));
        let declared = e.declared_size_override.unwrap_or(e.data.len() as u32);
        let offset = out.len() as u32;
        let name = e.name.as_bytes();
        // local header
        out.extend_from_slice(&0x0403_4b50u32.to_le_bytes());
        out.extend_from_slice(&20u16.to_le_bytes()); // version needed
        out.extend_from_slice(&0u16.to_le_bytes()); // flags
        out.extend_from_slice(&0u16.to_le_bytes()); // method: stored
        out.extend_from_slice(&0u16.to_le_bytes()); // mod time
        out.extend_from_slice(&0x21u16.to_le_bytes()); // mod date (1980-01-01-ish)
        out.extend_from_slice(&crc.to_le_bytes());
        out.extend_from_slice(&declared.to_le_bytes()); // compressed size
        out.extend_from_slice(&declared.to_le_bytes()); // uncompressed size
        out.extend_from_slice(&(name.len() as u16).to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes()); // extra len
        out.extend_from_slice(name);
        out.extend_from_slice(&e.data);
        // central directory record
        central.extend_from_slice(&0x0201_4b50u32.to_le_bytes());
        central.extend_from_slice(&20u16.to_le_bytes()); // version made by
        central.extend_from_slice(&20u16.to_le_bytes()); // version needed
        central.extend_from_slice(&0u16.to_le_bytes());
        central.extend_from_slice(&0u16.to_le_bytes());
        central.extend_from_slice(&0u16.to_le_bytes());
        central.extend_from_slice(&0x21u16.to_le_bytes());
        central.extend_from_slice(&crc.to_le_bytes());
        central.extend_from_slice(&declared.to_le_bytes());
        central.extend_from_slice(&declared.to_le_bytes());
        central.extend_from_slice(&(name.len() as u16).to_le_bytes());
        central.extend_from_slice(&0u16.to_le_bytes()); // extra
        central.extend_from_slice(&0u16.to_le_bytes()); // comment
        central.extend_from_slice(&0u16.to_le_bytes()); // disk
        central.extend_from_slice(&0u16.to_le_bytes()); // internal attrs
        central.extend_from_slice(&0u32.to_le_bytes()); // external attrs
        central.extend_from_slice(&offset.to_le_bytes());
        central.extend_from_slice(name);
    }
    let cd_start = out.len() as u32;
    out.extend_from_slice(&central);
    out.extend_from_slice(&0x0605_4b50u32.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    out.extend_from_slice(&(entries.len() as u16).to_le_bytes());
    out.extend_from_slice(&(entries.len() as u16).to_le_bytes());
    out.extend_from_slice(&(central.len() as u32).to_le_bytes());
    out.extend_from_slice(&cd_start.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    out
}

// ---------------------------------------------------------------------
// A minimal but structurally complete two-slide deck, generated.
// ---------------------------------------------------------------------

const RELS_NS: &str = "http://schemas.openxmlformats.org/package/2006/relationships";
const REL_T: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships";

fn rels_xml(rels: &[(&str, &str, &str)]) -> String {
    let mut s = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n<Relationships xmlns=\"{RELS_NS}\">"
    );
    for (id, ty, target) in rels {
        s.push_str(&format!(
            "<Relationship Id=\"{id}\" Type=\"{REL_T}/{ty}\" Target=\"{target}\"/>"
        ));
    }
    s.push_str("</Relationships>");
    s
}

fn slide_xml(text: &str) -> String {
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n\
         <p:sld xmlns:p=\"http://schemas.openxmlformats.org/presentationml/2006/main\" \
         xmlns:a=\"http://schemas.openxmlformats.org/drawingml/2006/main\"><p:cSld><p:spTree>\
         <p:sp><p:txBody><a:p><a:r><a:t>{text}</a:t></a:r></a:p></p:txBody></p:sp>\
         </p:spTree></p:cSld></p:sld>"
    )
}

fn deck_parts() -> Vec<(String, String)> {
    let ct = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n\
         <Types xmlns=\"http://schemas.openxmlformats.org/package/2006/content-types\">\
         <Default Extension=\"rels\" ContentType=\"application/vnd.openxmlformats-package.relationships+xml\"/>\
         <Default Extension=\"xml\" ContentType=\"application/xml\"/>\
         <Override PartName=\"/ppt/presentation.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.presentationml.presentation.main+xml\"/>\
         <Override PartName=\"/ppt/slides/slide1.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.presentationml.slide+xml\"/>\
         <Override PartName=\"/ppt/slides/slide2.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.presentationml.slide+xml\"/>\
         <Override PartName=\"/ppt/slideLayouts/slideLayout1.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.presentationml.slideLayout+xml\"/>\
         <Override PartName=\"/ppt/slideMasters/slideMaster1.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.presentationml.slideMaster+xml\"/>\
         <Override PartName=\"/ppt/theme/theme1.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.theme+xml\"/>\
         </Types>"
    );
    let pres = "\
        <?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n\
        <p:presentation xmlns:p=\"http://schemas.openxmlformats.org/presentationml/2006/main\" \
        xmlns:r=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships\">\
        <p:sldMasterIdLst><p:sldMasterId id=\"2147483648\" r:id=\"rId1\"/></p:sldMasterIdLst>\
        <p:sldIdLst><p:sldId id=\"256\" r:id=\"rId2\"/><p:sldId id=\"257\" r:id=\"rId3\"/></p:sldIdLst>\
        <p:sldSz cx=\"9144000\" cy=\"5143500\"/></p:presentation>"
        .to_string();
    vec![
        ("[Content_Types].xml".into(), ct),
        (
            "_rels/.rels".into(),
            rels_xml(&[("rId1", "officeDocument", "ppt/presentation.xml")]),
        ),
        ("ppt/presentation.xml".into(), pres),
        (
            "ppt/_rels/presentation.xml.rels".into(),
            rels_xml(&[
                ("rId1", "slideMaster", "slideMasters/slideMaster1.xml"),
                ("rId2", "slide", "slides/slide1.xml"),
                ("rId3", "slide", "slides/slide2.xml"),
            ]),
        ),
        ("ppt/slides/slide1.xml".into(), slide_xml("Hello One")),
        (
            "ppt/slides/slide2.xml".into(),
            slide_xml("Fish &amp; Chips &#169;"),
        ),
        (
            "ppt/slides/_rels/slide1.xml.rels".into(),
            rels_xml(&[("rId1", "slideLayout", "../slideLayouts/slideLayout1.xml")]),
        ),
        (
            "ppt/slides/_rels/slide2.xml.rels".into(),
            rels_xml(&[("rId1", "slideLayout", "../slideLayouts/slideLayout1.xml")]),
        ),
        (
            "ppt/slideLayouts/slideLayout1.xml".into(),
            "<p:sldLayout xmlns:p=\"http://schemas.openxmlformats.org/presentationml/2006/main\"/>"
                .into(),
        ),
        (
            "ppt/slideLayouts/_rels/slideLayout1.xml.rels".into(),
            rels_xml(&[("rId1", "slideMaster", "../slideMasters/slideMaster1.xml")]),
        ),
        (
            "ppt/slideMasters/slideMaster1.xml".into(),
            "<p:sldMaster xmlns:p=\"http://schemas.openxmlformats.org/presentationml/2006/main\"/>"
                .into(),
        ),
        (
            "ppt/slideMasters/_rels/slideMaster1.xml.rels".into(),
            rels_xml(&[("rId1", "theme", "../theme/theme1.xml")]),
        ),
        (
            "ppt/theme/theme1.xml".into(),
            "<a:theme xmlns:a=\"http://schemas.openxmlformats.org/drawingml/2006/main\"/>".into(),
        ),
    ]
}

fn deck_bytes() -> Vec<u8> {
    let entries: Vec<Entry> = deck_parts()
        .iter()
        .map(|(n, c)| Entry::new(n, c.clone().into_bytes()))
        .collect();
    build_zip(&entries)
}

fn deck_map() -> BTreeMap<String, Vec<u8>> {
    deck_parts()
        .into_iter()
        .map(|(n, c)| (n, c.into_bytes()))
        .collect()
}

// ---------------------------------------------------------------------
// parity: behaviour the old layer had and the craft layer keeps
// ---------------------------------------------------------------------

mod parity {
    use super::*;
    use std::io::Write as _;

    #[test]
    fn valid_deck_loads_with_all_parts() {
        let pkg = Package::load(&deck_bytes()).unwrap();
        assert_eq!(pkg.parts, deck_map());
        let slides = pptx::slides(&pkg).unwrap();
        assert_eq!(slides.len(), 2);
        assert_eq!(slides[0].part, "ppt/slides/slide1.xml");
        assert_eq!(slides[0].rid, "rId2");
        assert_eq!(slides[1].part, "ppt/slides/slide2.xml");
        assert!(pptx::structural_errors(&pkg).is_empty());
    }

    #[test]
    fn round_trip_preserves_parts() {
        let pkg = Package::load(&deck_bytes()).unwrap();
        let bytes = pkg.to_bytes().unwrap();
        let again = Package::load(&bytes).unwrap();
        assert_eq!(again.parts, pkg.parts);
    }

    #[test]
    fn split_reassemble_verify_ok() {
        let pkg = Package::load(&deck_bytes()).unwrap();
        let report = pptx::split_reassemble_verify(&pkg).unwrap();
        assert!(report.ok, "verify failed: {:?}", report.structural_errors);
        assert_eq!(report.slide_count, 2);
    }

    #[test]
    fn split_one_matches_selected_split_fragment() {
        let pkg = Package::load(&deck_bytes()).unwrap();
        let fragments = pptx::split(&pkg).unwrap();
        for (index, fragment) in fragments.iter().enumerate() {
            let single = pptx::split_one(&pkg, index).unwrap();
            assert_eq!(single.parts, fragment.parts);
            let reloaded = Package::load(&single.to_bytes().unwrap()).unwrap();
            assert_eq!(pptx::slides(&reloaded).unwrap().len(), 1);
            assert!(pptx::structural_errors(&reloaded).is_empty());
        }
        assert!(pptx::split_one(&pkg, fragments.len()).is_err());
    }

    #[test]
    fn assemble_split_deck_ok() {
        let pkg = Package::load(&deck_bytes()).unwrap();
        let frags = pptx::split(&pkg).unwrap();
        assert_eq!(frags.len(), 2);
        // Fragment pruning: slide2's Override is gone from fragment 1,
        // its own stays.
        let ct1 = frags[0].get_text("[Content_Types].xml").unwrap();
        assert!(ct1.contains("/ppt/slides/slide1.xml"));
        assert!(!ct1.contains("/ppt/slides/slide2.xml"));
        let merged = qimu_core::assemble::assemble(&frags).unwrap();
        assert_eq!(pptx::slides(&merged).unwrap().len(), 2);
        assert!(pptx::structural_errors(&merged).is_empty());
    }

    #[test]
    fn deflated_source_loads_equal() {
        // Real-world decks are deflated; build the same deck with the
        // dev-dependency zip writer and deflate compression.
        let mut buf = std::io::Cursor::new(Vec::new());
        {
            let mut zw = zip::ZipWriter::new(&mut buf);
            let opts = zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Deflated);
            for (name, content) in deck_parts() {
                zw.start_file(name, opts).unwrap();
                zw.write_all(content.as_bytes()).unwrap();
            }
            zw.finish().unwrap();
        }
        let pkg = Package::load(&buf.into_inner()).unwrap();
        assert_eq!(pkg.parts, deck_map());
    }

    #[test]
    fn notes_absent_is_empty_and_set_fails_soft() {
        let pkg = Package::load(&deck_bytes()).unwrap();
        assert_eq!(
            qimu_core::text::notes_text(&pkg, "ppt/slides/slide1.xml"),
            ""
        );
        let mut pkg = pkg;
        assert!(!pptx::set_notes_text(&mut pkg, 0, "讲稿"));
    }

    #[test]
    fn text_runs_pair_round_trip() {
        // The page editor's index-coupled read/write pair (landed with
        // the five-features package): indices must agree.
        let mut pkg = Package::load(&deck_bytes()).unwrap();
        assert_eq!(
            pptx::slide_text_runs(&pkg, 0).unwrap(),
            vec!["Hello One".to_string()]
        );
        let applied =
            pptx::replace_slide_text_runs(&mut pkg, 0, &[(0, "A & B".to_string())]).unwrap();
        assert_eq!(applied, 1);
        assert_eq!(
            pptx::slide_text_runs(&pkg, 0).unwrap(),
            vec!["A & B".to_string()]
        );
        let xml = pkg.get_text("ppt/slides/slide1.xml").unwrap();
        assert!(xml.contains("A &amp; B"), "written run not escaped: {xml}");
    }
}

// ---------------------------------------------------------------------
// changed: declared behaviour differences of the migration
// ---------------------------------------------------------------------

mod changed {
    use super::*;

    #[test]
    fn numeric_character_references_are_decoded() {
        // Old: the hand unescape knew only the five named entities, so
        // extract_texts returned "Fish & Chips &#169;" verbatim.
        // New: craft-xml decodes numeric references.
        let pkg = Package::load(&deck_bytes()).unwrap();
        let xml = pkg.get_text("ppt/slides/slide2.xml").unwrap();
        assert_eq!(
            qimu_core::text::extract_texts(&xml),
            vec!["Fish & Chips ©".to_string()]
        );
        // KNOWN DIVERGENCE (pre-existing, not from this migration): the
        // page editor's slide_text_runs still uses its own five-entity
        // unescape, so it returns the reference undecoded. Pinned here
        // so the divergence is visible, not silent.
        assert_eq!(
            pptx::slide_text_runs(&pkg, 1).unwrap(),
            vec!["Fish & Chips &#169;".to_string()]
        );
    }

    #[test]
    fn case_variant_spellings_collapse_to_the_named_part() {
        // Old: both spellings were separate parts. New (ECMA-376):
        // one part; the spelling a relationship names survives.
        let entries = vec![
            Entry::new(
                "ppt/_rels/presentation.xml.rels",
                rels_xml(&[("rId1", "slide", "slides/slide1.xml")]).into_bytes(),
            ),
            Entry::new("PPT/SLIDES/SLIDE1.XML", b"impostor".to_vec()),
            Entry::new("ppt/slides/slide1.xml", b"real".to_vec()),
        ];
        let pkg = Package::load(&build_zip(&entries)).unwrap();
        let slide_keys: Vec<&String> = pkg
            .parts
            .keys()
            .filter(|k| k.to_ascii_lowercase().ends_with("slide1.xml"))
            .collect();
        assert_eq!(slide_keys, vec!["ppt/slides/slide1.xml"]);
        assert_eq!(pkg.parts["ppt/slides/slide1.xml"], b"real".to_vec());
    }

    #[test]
    fn duplicate_entry_name_keeps_one_copy() {
        // Old: zip v2 read every entry and the BTreeMap kept the LAST.
        // New: craft-zip keeps one entry of a repeated name.
        // CALIBRATION: which of the two copies wins is decided by
        // craft-zip's name index (zip v9). The assertion accepts either
        // winner but pins the one-copy invariant; tighten to the
        // observed winner after the first CI run.
        let entries = vec![
            Entry::new("dup.xml", b"first".to_vec()),
            Entry::new("dup.xml", b"second".to_vec()),
        ];
        let pkg = Package::load(&build_zip(&entries)).unwrap();
        assert_eq!(pkg.parts.len(), 1);
        let kept = &pkg.parts["dup.xml"];
        assert!(
            kept == b"first" || kept == b"second",
            "kept unexpected bytes: {kept:?}"
        );
    }

    #[test]
    fn entry_names_are_normalized() {
        // Old: entry names were kept verbatim (backslashes, leading
        // slashes and all). New: `\` becomes `/` and a leading `/` is
        // stripped. `..` segments are NOT resolved — they stay as
        // inert map keys (nothing in qimu-core ever writes a part to
        // disk under its part name; storage is content-addressed).
        let entries = vec![
            Entry::new("..\\win.xml", b"x".to_vec()),
            Entry::new("/abs.xml", b"x".to_vec()),
            Entry::new("../evil.xml", b"x".to_vec()),
        ];
        let pkg = Package::load(&build_zip(&entries)).unwrap();
        let mut keys: Vec<&str> = pkg.parts.keys().map(String::as_str).collect();
        keys.sort_unstable();
        assert_eq!(keys, vec!["../evil.xml", "../win.xml", "abs.xml"]);
    }
}

// ---------------------------------------------------------------------
// hostile: malformed and hostile samples must be refused or defused
// ---------------------------------------------------------------------

mod hostile {
    use super::*;

    #[test]
    fn entry_count_over_limit_is_refused() {
        // PACKAGE_LIMITS caps entries at 50_000; the old layer had no
        // cap at all. Generating 50_001 real (empty) entries.
        let entries: Vec<Entry> = (0..50_001)
            .map(|i| Entry::new(&format!("e{i}.xml"), Vec::new()))
            .collect();
        assert!(Package::load(&build_zip(&entries)).is_err());
    }

    #[test]
    fn declared_oversize_part_is_refused() {
        // A part declaring 300 MiB (over the 256 MiB part limit) with
        // almost no actual data: refused at preflight, before inflate.
        let mut e = Entry::new("big.xml", b"<x/>".to_vec());
        e.declared_size_override = Some(300 << 20);
        assert!(Package::load(&build_zip(&[e])).is_err());
    }

    #[test]
    fn bad_crc_entry_fails_the_whole_load() {
        // Fail-closed, as the old layer was: one damaged entry refuses
        // the package (craft-opc would otherwise skip it with a
        // warning; Package::load promotes any skip to an error).
        let mut entries: Vec<Entry> = deck_parts()
            .iter()
            .map(|(n, c)| Entry::new(n, c.clone().into_bytes()))
            .collect();
        let slide = entries
            .iter_mut()
            .find(|e| e.name == "ppt/slides/slide1.xml")
            .unwrap();
        slide.crc_override = Some(crc32(&slide.data) ^ 0xFFFF);
        assert!(Package::load(&build_zip(&entries)).is_err());
    }

    #[test]
    fn truncated_archive_is_refused() {
        let bytes = deck_bytes();
        assert!(Package::load(&bytes[..bytes.len() / 2]).is_err());
    }

    #[test]
    fn missing_content_types_is_flagged_not_fatal() {
        let entries: Vec<Entry> = deck_parts()
            .iter()
            .filter(|(n, _)| n != "[Content_Types].xml")
            .map(|(n, c)| Entry::new(n, c.clone().into_bytes()))
            .collect();
        let pkg = Package::load(&build_zip(&entries)).unwrap();
        let errors = pptx::structural_errors(&pkg);
        assert!(!errors.is_empty());
        assert!(
            errors.iter().all(|e| e.contains("[Content_Types]")),
            "unexpected errors: {errors:?}"
        );
    }

    #[test]
    fn doctype_in_presentation_is_rejected() {
        // XXE / DTD: qimu's parse options refuse a DOCTYPE with an
        // internal subset (where entity definitions live), so a
        // presentation carrying an external-entity declaration cannot
        // be read as slides. The parser itself never fetches external
        // resources either way.
        let mut parts = deck_map();
        let mut pres = String::from_utf8(parts["ppt/presentation.xml"].clone()).unwrap();
        pres = pres.replacen(
            "?>\n",
            "?>\n<!DOCTYPE p:presentation [<!ENTITY xxe SYSTEM \"file:///etc/passwd\">]>\n",
            1,
        );
        parts.insert("ppt/presentation.xml".into(), pres.into_bytes());
        let entries: Vec<Entry> = parts
            .iter()
            .map(|(n, c)| Entry::new(n, c.clone()))
            .collect();
        let pkg = Package::load(&build_zip(&entries)).unwrap();
        assert!(pptx::slides(&pkg).is_err());
    }

    #[test]
    fn entity_expansion_yields_no_text() {
        // Billion-laughs shape: nested internal entities. The internal
        // subset is refused at parse time (qimu's options), so there
        // is nothing to expand; extraction returns empty instead of
        // the old scanner's partial matches.
        let xml = "<?xml version=\"1.0\"?>\n\
            <!DOCTYPE sld [<!ENTITY a \"xxxxxxxxxxxxxxxxxxxx\">\
            <!ENTITY b \"&a;&a;&a;&a;&a;&a;&a;&a;&a;&a;\">\
            <!ENTITY c \"&b;&b;&b;&b;&b;&b;&b;&b;&b;&b;\">]>\n\
            <p:sld xmlns:p=\"http://schemas.openxmlformats.org/presentationml/2006/main\" \
            xmlns:a=\"http://schemas.openxmlformats.org/drawingml/2006/main\">\
            <p:cSld><p:spTree><p:sp><p:txBody><a:p><a:r><a:t>&c;</a:t></a:r></a:p>\
            </p:txBody></p:sp></p:spTree></p:cSld></p:sld>";
        assert!(qimu_core::text::extract_texts(xml).is_empty());
    }

    #[test]
    fn unknown_entity_yields_no_text() {
        // Old: the string scanner returned "a &nosuch; b" (its
        // unescape left unknown entities alone). New: undefined
        // entities are a parse error under qimu's tightened options
        // (craft-xml's own default would silently drop the entity),
        // so extraction yields nothing.
        let xml = slide_xml("a &nosuch; b");
        assert!(qimu_core::text::extract_texts(&xml).is_empty());
    }

    #[test]
    fn text_in_comments_is_not_extracted() {
        // Old: the scanner matched `<a:t>` inside comments. New: a
        // real parser never sees comment text as elements.
        let xml = slide_xml("real").replace("</p:spTree>", "<!-- <a:t>ghost</a:t> --></p:spTree>");
        assert_eq!(
            qimu_core::text::extract_texts(&xml),
            vec!["real".to_string()]
        );
    }
}

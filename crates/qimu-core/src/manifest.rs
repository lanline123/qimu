//! The manifest embedded in every managed PPTX (spec §3.1/§8.6): deck ID,
//! source index triple, and per-block bindings by ID. It is the only thing
//! the PowerPoint-side component needs to check for updates, and it must
//! survive third-party editors' open→save round trips.
//!
//! Storage: a `qimu_manifest` custom document property (JSON inside
//! docProps/custom.xml — a standard part editors preserve), mirrored into
//! a plain part `qimu/manifest.json` as a fallback read path. A third,
//! minimal carrier — a `qimu:<deck_id>` token inside the standard
//! docProps/core.xml keywords — survives editors that strip custom
//! properties and unknown parts (verified: LibreOffice drops the first
//! two carriers on open→save but preserves core properties); with just
//! the deck ID, the full manifest is recovered from the server's export
//! registry (GET /v1/decks/:deck_id/manifest).

use crate::pptx::Package;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SourceRef {
    pub index_id: String,
    pub branch: String,
    pub commit_id: String,
    pub tag: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Binding {
    pub unit_id: String,
    pub rendition_id: String,
    pub version_id: String,
    pub version_semver: String,
    /// Per-page fingerprints of the pages this binding contributed to the
    /// deck, in deck order (a subset for excerpted bindings).
    pub page_fps: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Manifest {
    pub deck_id: String,
    /// personal | public (personal-binding resolution used at export).
    pub kind: String,
    pub source: Option<SourceRef>,
    pub exported_ms: i64,
    pub bindings: Vec<Binding>,
}

const CUSTOM_PART: &str = "docProps/custom.xml";
const MIRROR_PART: &str = "qimu/manifest.json";
const CORE_PART: &str = "docProps/core.xml";
const FMTID: &str = "{D5CDD505-2E9C-101B-9397-08002B2CF9AE}";

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

fn xml_unescape(s: &str) -> String {
    s.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&amp;", "&")
}

pub fn embed_manifest(deck: &[u8], manifest: &Manifest) -> Result<Vec<u8>, String> {
    let mut pkg = Package::load(deck)?;
    let json = serde_json::to_string(manifest).map_err(|e| e.to_string())?;

    // 1) custom document property
    let prop = format!(
        "<property fmtid=\"{FMTID}\" pid=\"100\" name=\"qimu_manifest\"><vt:lpwstr>{}</vt:lpwstr></property>",
        xml_escape(&json)
    );
    let existing = pkg.get_text(CUSTOM_PART).unwrap_or_default();
    let mut custom = if existing.contains("<Properties") {
        remove_property(&existing, "qimu_manifest")
    } else {
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n<Properties xmlns=\"http://schemas.openxmlformats.org/officeDocument/2006/custom-properties\" xmlns:vt=\"http://schemas.openxmlformats.org/officeDocument/2006/docPropsVTypes\"></Properties>".to_string()
    };
    if let Some(pos) = custom.rfind("</Properties>") {
        custom.insert_str(pos, &prop);
    }
    pkg.parts
        .insert(CUSTOM_PART.to_string(), custom.into_bytes());

    // 2) mirror part
    pkg.parts
        .insert(MIRROR_PART.to_string(), json.clone().into_bytes());

    // 2b) deck-id token in the standard core keywords (survives editors
    // that strip custom properties and unknown parts)
    upsert_keywords_token(&mut pkg, &format!("qimu:{}", manifest.deck_id));

    // 3) content types overrides for both parts
    let ct_name = "[Content_Types].xml";
    let mut ct = pkg.get_text(ct_name)?;
    for (part, ctype) in [
        (
            "/docProps/custom.xml",
            "application/vnd.openxmlformats-officedocument.custom-properties+xml",
        ),
        ("/qimu/manifest.json", "application/json"),
    ] {
        if !ct.contains(&format!("PartName=\"{part}\"")) {
            let ov = format!("<Override PartName=\"{part}\" ContentType=\"{ctype}\"/>");
            if let Some(pos) = ct.rfind("</Types>") {
                ct.insert_str(pos, &ov);
            }
        }
    }
    pkg.parts.insert(ct_name.to_string(), ct.into_bytes());

    // 4) package relationship to custom.xml (needed by strict readers)
    let rels_name = "_rels/.rels";
    let mut rels = pkg.get_text(rels_name).unwrap_or_default();
    if !rels.contains("docProps/custom.xml") {
        let mut n = 1;
        while rels.contains(&format!("Id=\"rId{n}\"")) {
            n += 1;
        }
        let rel = format!(
            "<Relationship Id=\"rId{n}\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/metadata/custom-properties\" Target=\"docProps/custom.xml\"/>"
        );
        if let Some(pos) = rels.rfind("</Relationships>") {
            rels.insert_str(pos, &rel);
            pkg.parts.insert(rels_name.to_string(), rels.into_bytes());
        }
    }

    pkg.to_bytes()
}

/// Add/replace the `qimu:<deck_id>` token inside docProps/core.xml's
/// standard <cp:keywords>, preserving any user keywords.
fn upsert_keywords_token(pkg: &mut Package, token: &str) {
    let core = pkg.get_text(CORE_PART).unwrap_or_default();
    if !core.contains("<cp:coreProperties") {
        let xml = format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n<cp:coreProperties xmlns:cp=\"http://schemas.openxmlformats.org/package/2006/metadata/core-properties\" xmlns:dc=\"http://purl.org/dc/elements/1.1/\" xmlns:dcterms=\"http://purl.org/dc/terms/\" xmlns:dcmitype=\"http://purl.org/dc/dcmitype/\" xmlns:xsi=\"http://www.w3.org/2001/XMLSchema-instance\"><cp:keywords>{token}</cp:keywords></cp:coreProperties>"
        );
        pkg.parts.insert(CORE_PART.to_string(), xml.into_bytes());
        return;
    }
    let mut tokens: Vec<String> = Vec::new();
    if let Some(s) = core.find("<cp:keywords>") {
        let vs = s + "<cp:keywords>".len();
        if let Some(e) = core[vs..].find("</cp:keywords>") {
            tokens = core[vs..vs + e]
                .split_whitespace()
                .map(|t| t.to_string())
                .collect();
        }
    }
    tokens.retain(|t| !t.starts_with("qimu:"));
    tokens.push(token.to_string());
    let joined = tokens.join(" ");
    let new_core = if let Some(s) = core.find("<cp:keywords>") {
        let vs = s + "<cp:keywords>".len();
        let e = core[vs..]
            .find("</cp:keywords>")
            .map(|x| vs + x)
            .unwrap_or(vs);
        let mut out = core.clone();
        out.replace_range(vs..e, &joined);
        out
    } else if core.contains("<cp:keywords/>") {
        core.replacen(
            "<cp:keywords/>",
            &format!("<cp:keywords>{joined}</cp:keywords>"),
            1,
        )
    } else if let Some(pos) = core.rfind("</cp:coreProperties>") {
        let mut out = core.clone();
        out.insert_str(pos, &format!("<cp:keywords>{joined}</cp:keywords>"));
        out
    } else {
        core
    };
    pkg.parts
        .insert(CORE_PART.to_string(), new_core.into_bytes());
}

/// The deck ID from any surviving carrier: full manifest first, then the
/// core-keywords token.
pub fn read_deck_id(deck: &[u8]) -> Option<String> {
    if let Some(m) = read_manifest(deck) {
        return Some(m.deck_id);
    }
    let pkg = Package::load(deck).ok()?;
    let core = pkg.get_text(CORE_PART).ok()?;
    // Editors may normalize the token (LibreOffice turns "qimu:deck_x"
    // into "qimu deck_x" — the colon becomes a keyword separator), so
    // accept any light separator run before the deck_ id.
    let pos = core.find("qimu")?;
    let rest = &core[pos + "qimu".len()..];
    let rest = rest.trim_start_matches(|c| c == ':' || c == ' ' || c == ';' || c == ',');
    let id: String = rest
        .chars()
        .take_while(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '-')
        .collect();
    if id.starts_with("deck_") {
        Some(id)
    } else {
        None
    }
}

fn remove_property(xml: &str, name: &str) -> String {
    let marker = format!("name=\"{name}\"");
    if let Some(mpos) = xml.find(&marker) {
        if let Some(start) = xml[..mpos].rfind("<property") {
            if let Some(endrel) = xml[mpos..].find("</property>") {
                let end = mpos + endrel + "</property>".len();
                let mut out = xml.to_string();
                out.replace_range(start..end, "");
                return out;
            }
        }
    }
    xml.to_string()
}

pub fn read_manifest(deck: &[u8]) -> Option<Manifest> {
    let pkg = Package::load(deck).ok()?;
    // primary: custom property
    if let Ok(custom) = pkg.get_text(CUSTOM_PART) {
        if let Some(mpos) = custom.find("name=\"qimu_manifest\"") {
            let after = &custom[mpos..];
            if let Some(s) = after.find("<vt:lpwstr>") {
                let val_start = s + "<vt:lpwstr>".len();
                if let Some(e) = after[val_start..].find("</vt:lpwstr>") {
                    let raw = &after[val_start..val_start + e];
                    if let Ok(m) = serde_json::from_str::<Manifest>(&xml_unescape(raw)) {
                        return Some(m);
                    }
                }
            }
        }
    }
    // fallback: mirror part
    if let Ok(bytes) = pkg.get(MIRROR_PART) {
        if let Ok(m) = serde_json::from_slice::<Manifest>(bytes) {
            return Some(m);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manifest_round_trip() {
        let Some(bytes) = crate::test_fixtures::redwood_deck() else {
            eprintln!("{}", crate::test_fixtures::skip_msg("Redwood deck"));
            return;
        };
        let m = Manifest {
            deck_id: "deck_test".into(),
            kind: "personal".into(),
            source: Some(SourceRef {
                index_id: "idx_1".into(),
                branch: "main".into(),
                commit_id: "cmt_1".into(),
                tag: Some("V1.0.0".into()),
            }),
            exported_ms: 1,
            bindings: vec![Binding {
                unit_id: "unt_1".into(),
                rendition_id: "rnd_1".into(),
                version_id: "ver_1".into(),
                version_semver: "1.0.0".into(),
                page_fps: vec!["abc".into()],
            }],
        };
        let embedded = embed_manifest(&bytes, &m).unwrap();
        let back = read_manifest(&embedded).expect("manifest readable after embed");
        assert_eq!(back.deck_id, "deck_test");
        // the keywords carrier holds the deck id on its own
        assert_eq!(read_deck_id(&embedded).as_deref(), Some("deck_test"));
        let core = Package::load(&embedded)
            .unwrap()
            .get_text("docProps/core.xml")
            .unwrap();
        assert!(
            core.contains("qimu:deck_test"),
            "keywords token missing: {core}"
        );
        assert_eq!(back.bindings.len(), 1);
        assert_eq!(back.bindings[0].version_semver, "1.0.0");
        // re-embed replaces rather than duplicates
        let again = embed_manifest(&embedded, &m).unwrap();
        let custom = Package::load(&again)
            .unwrap()
            .get_text("docProps/custom.xml")
            .unwrap();
        assert_eq!(custom.matches("qimu_manifest").count(), 1);
    }
}

//! Personal binding / export personalization (spec §3.14).
//!
//! Library fragments are stored TEMPLATE-only: personal data never enters
//! the shared library. Slides carry placeholders like `{{name}}`,
//! `{{phone}}`, `{{company_website}}`; at export time they are resolved
//! per RECIPIENT — the exporter's own profile fields for a 个人版， or
//! company public data only for a 公用版. A placeholder with no value is
//! reported as missing and blocks the export; another person's data is
//! never substituted.
//!
//! Placeholders must sit inside a single text run (`<a:t>`), which is how
//! template slides are authored.

use crate::pptx::Package;
use std::collections::BTreeMap;

/// Placeholder fields bound to the exporter's personal profile.
pub const PERSONAL_FIELDS: &[&str] = &["name", "title", "english_name", "phone", "email", "wechat"];
/// Placeholder fields bound to the company public profile (§3.14 补记八）.
pub const COMPANY_FIELDS: &[&str] = &[
    "company_phone",
    "company_gongzhonghao",
    "company_website",
    "company_address",
];

fn xml_escape(v: &str) -> String {
    v.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// All `{{token}}` names appearing in slide text runs of the package.
pub fn find_tokens(pkg: &Package) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for (name, bytes) in &pkg.parts {
        if !is_slide_part(name) {
            continue;
        }
        let xml = String::from_utf8_lossy(bytes);
        for t in text_runs(&xml) {
            let mut rest = t.as_str();
            while let Some(start) = rest.find("{{") {
                let after = &rest[start + 2..];
                match after.find("}}") {
                    Some(end) => {
                        let tok = after[..end].trim().to_string();
                        if !tok.is_empty() && !out.contains(&tok) {
                            out.push(tok);
                        }
                        rest = &after[end + 2..];
                    }
                    None => break,
                }
            }
        }
    }
    out
}

fn is_slide_part(name: &str) -> bool {
    name.starts_with("ppt/slides/") && name.ends_with(".xml")
}

/// Text contents of every `<a:t>` run, in document order.
fn text_runs(xml: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = xml;
    while let Some(s) = rest.find("<a:t>") {
        let after = &rest[s + 5..];
        match after.find("</a:t>") {
            Some(e) => {
                out.push(after[..e].to_string());
                rest = &after[e + 6..];
            }
            None => break,
        }
    }
    out
}

/// Replace `{{token}}` inside every slide text run. `values` maps token ->
/// replacement (already the right person's data). Tokens listed in `scrub`
/// are replaced with an empty string (public edition: personal data is
/// removed, never filled). Returns the tokens that had no value and were
/// left in place — the caller must treat a non-empty result as blocking.
pub fn substitute(
    pkg: &mut Package,
    values: &BTreeMap<String, String>,
    scrub: &[&str],
) -> Vec<String> {
    let mut missing: Vec<String> = Vec::new();
    let names: Vec<String> = pkg
        .parts
        .keys()
        .filter(|n| is_slide_part(n))
        .cloned()
        .collect();
    for name in names {
        let Some(bytes) = pkg.parts.get(&name).cloned() else {
            continue;
        };
        let xml = String::from_utf8_lossy(&bytes).to_string();
        let mut out = String::with_capacity(xml.len());
        let mut rest = xml.as_str();
        while let Some(s) = rest.find("<a:t>") {
            let after = &rest[s + 5..];
            match after.find("</a:t>") {
                Some(e) => {
                    out.push_str(&rest[..s + 5]);
                    let run = &after[..e];
                    out.push_str(&replace_in_run(run, values, scrub, &mut missing));
                    rest = &after[e..];
                }
                None => break,
            }
        }
        out.push_str(rest);
        if out != xml {
            pkg.parts.insert(name, out.into_bytes());
        }
    }
    missing.sort();
    missing.dedup();
    missing
}

fn replace_in_run(
    run: &str,
    values: &BTreeMap<String, String>,
    scrub: &[&str],
    missing: &mut Vec<String>,
) -> String {
    let mut out = String::with_capacity(run.len());
    let mut rest = run;
    while let Some(start) = rest.find("{{") {
        let after = &rest[start + 2..];
        match after.find("}}") {
            Some(end) => {
                let tok = after[..end].trim();
                out.push_str(&rest[..start]);
                if scrub.contains(&tok) {
                    // public edition: personal data scrubbed to blank
                } else if let Some(v) = values.get(tok) {
                    out.push_str(&xml_escape(v));
                } else {
                    out.push_str(&rest[start..start + 2 + end + 2]);
                    if !tok.is_empty() && !missing.contains(&tok.to_string()) {
                        missing.push(tok.to_string());
                    }
                }
                rest = &after[end + 2..];
            }
            None => break,
        }
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pptx::slides;

    fn redwood() -> Package {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../user/files/Redwood_Visit_Revised_4tph_0_6vcr.pptx"
        );
        // tests run from the workspace; fall back to the home-relative path
        let bytes = std::fs::read(path).unwrap_or_else(|_| {
            std::fs::read("/home/hatch/workspace/user/files/Redwood_Visit_Revised_4tph_0_6vcr.pptx")
                .expect("redwood deck")
        });
        Package::load(&bytes).expect("load deck")
    }

    #[test]
    fn substitute_and_missing_and_scrub() {
        let mut pkg = redwood();
        // doctor slide 1's first text run into a template line
        let first = slides(&pkg).expect("slides")[0].part.clone();
        let xml = String::from_utf8_lossy(&pkg.parts[&first]).to_string();
        let pos = xml.find("<a:t>").unwrap() + 5;
        let end = xml[pos..].find("</a:t>").unwrap() + pos;
        let doctored = format!(
            "{}汇报人：{{{{name}}}} · {{{{phone}}}} · {{{{company_website}}}}{}",
            &xml[..pos],
            &xml[end..]
        );
        pkg.parts.insert(first.clone(), doctored.into_bytes());

        let tokens = find_tokens(&pkg);
        assert!(tokens.contains(&"name".to_string()), "tokens: {tokens:?}");
        assert!(tokens.contains(&"company_website".to_string()));

        let mut values = BTreeMap::new();
        values.insert("name".to_string(), "张三".to_string());
        values.insert("company_website".to_string(), "www.example.com".to_string());
        // phone deliberately missing -> must be reported, left in place
        let missing = substitute(&mut pkg, &values, &[]);
        assert_eq!(missing, vec!["phone".to_string()]);
        let xml = String::from_utf8_lossy(&pkg.parts[&first]).to_string();
        assert!(xml.contains("汇报人：张三"), "name substituted");
        assert!(xml.contains("www.example.com"));
        assert!(xml.contains("{{phone}}"), "missing token left visible");

        // scrub: personal tokens blanked in the public edition
        let missing2 = substitute(&mut pkg, &values, &["phone"]);
        assert!(missing2.is_empty());
        let xml = String::from_utf8_lossy(&pkg.parts[&first]).to_string();
        assert!(!xml.contains("{{phone}}"), "scrubbed token removed");
    }
}

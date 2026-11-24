//! Import orchestration (§5.8 + §3.1) and English-index derivation,
//! sunk from the server's HTTP handlers (local-first P1, design
//! §6.2/§7) so every frontend runs the identical sequence: the
//! server stays a thin shell over these methods, and the desktop
//! client's in-process local library calls them directly — import
//! lands in the library the caller owns, never "in whichever store
//! happens to sit behind an HTTP connection".
//!
//! The sequences below are transcribed from the handlers they
//! replace, step for step and error string for error string —
//! including the pre-existing quirk that a block failing mid-import
//! leaves earlier blocks committed (the "整批导入已回滚" wording of
//! the messages predates this move and is preserved verbatim).

use crate::model::{EntryPolicy, Id, IndexEntry};
use crate::pptx::{self, Package};
use crate::store::{IndexRec, Store, UserRec};
use crate::SemVer;
use serde::Serialize;

/// One block of an import request: the pages of the source deck
/// that become one content unit (the server's `ImportBlock` wire
/// shape, minus transport concerns).
#[derive(Debug, Clone)]
pub struct ImportBlockSpec {
    pub name: String,
    pub pages: Vec<usize>,
    pub form: Option<String>,
    pub lang: Option<String>,
}

/// One created unit of a finished import (the wire shape the
/// import endpoint returns per block).
#[derive(Debug, Clone, Serialize)]
pub struct ImportedUnit {
    pub unit_id: String,
    pub rendition_id: String,
    pub version_id: String,
    pub semver: String,
    pub pages: i64,
}

/// The outcome of a finished import (the endpoint's response,
/// minus its `"ok": true` transport field).
#[derive(Debug, Clone, Serialize)]
pub struct ImportOutcome {
    pub slide_count: usize,
    pub fingerprints_match: bool,
    pub units: Vec<ImportedUnit>,
}

/// Language-code normalization for rendition `lang` values, which
/// users may have entered as 中文/英语/… — a verbatim copy of the
/// server's private helper of the same name (its other call sites
/// are translation-facing and stay server-side).
fn lang_code_of(raw: &str) -> String {
    match raw.trim() {
        "中文" | "简体中文" => "zh".to_string(),
        "英语" | "英文" => "en".to_string(),
        "日语" | "日文" => "ja".to_string(),
        other => other.to_string(),
    }
}

impl Store {
    /// Import a deck (§5.8 + §3.1): the deck is split, reassembled
    /// and verified page-by-page BEFORE anything is committed; a
    /// failed check imports nothing. Each block becomes one content
    /// unit with one rendition and version 1.0.0; the original
    /// whole file is never archived. `blocks = None` imports the
    /// deck without a caller-supplied split: a deck carrying real
    /// PPT sections splits by section (one section = one block,
    /// named after the section; slides in no section form one
    /// block named after the file stem), and a sectionless deck
    /// splits one block per slide (user ruling 2026-11-24,
    /// superseding the former single-block whole-deck import),
    /// each named after the slide's title text, or `第N页` when
    /// the slide carries no title.
    /// `author` is the display name recorded as maintainer and
    /// version author (the requesting user over HTTP, the local
    /// library's seeded owner in-process).
    pub fn import_blocks(
        &self,
        filename: &str,
        bytes: &[u8],
        folder_id: Option<&str>,
        group_id: &str,
        author: &str,
        blocks: Option<Vec<ImportBlockSpec>>,
    ) -> Result<ImportOutcome, String> {
        let pkg = Package::load(bytes)?;
        let report = pptx::split_reassemble_verify(&pkg)?;
        if !report.ok {
            return Err(format!(
                "拆分校验未通过，整批导入已回滚: {}",
                report.structural_errors.join("; ")
            ));
        }
        let singles = pptx::split(&pkg)?;
        let stem = filename
            .rsplit('/')
            .next()
            .unwrap_or("未命名")
            .trim_end_matches(".pptx")
            .trim_end_matches(".PPTX")
            .to_string();
        let blocks = match blocks {
            Some(blocks) => blocks,
            None => match pptx::deck_sections(&pkg)? {
                Some(spans) => spans
                    .iter()
                    .map(|sp| ImportBlockSpec {
                        name: if sp.name.trim().is_empty() {
                            stem.clone()
                        } else {
                            sp.name.clone()
                        },
                        pages: sp.pages.clone(),
                        form: None,
                        lang: None,
                    })
                    .collect(),
                None => {
                    // Sectionless deck (user ruling 2026-11-24):
                    // one block per slide. The block name is the
                    // slide's title text (the same `slide_title`
                    // the proposer reads), falling back to the
                    // numbered-page form when the slide carries
                    // no title. Naming is best-effort only: a
                    // slide listing that cannot be read degrades
                    // every name to the fallback rather than
                    // failing an import that split fine.
                    let slide_refs = pptx::slides(&pkg).unwrap_or_default();
                    (0..singles.len())
                        .map(|i| {
                            let title = slide_refs
                                .get(i)
                                .and_then(|s| pkg.get_text(&s.part).ok())
                                .map(|xml| crate::text::slide_title(&xml))
                                .unwrap_or_default();
                            ImportBlockSpec {
                                name: if title.is_empty() || title == "(无标题)" {
                                    format!("第{}页", i + 1)
                                } else {
                                    title
                                },
                                pages: vec![i],
                                form: None,
                                lang: None,
                            }
                        })
                        .collect()
                }
            },
        };
        let mut created = Vec::new();
        for block in &blocks {
            if block.pages.is_empty() {
                continue;
            }
            let frag_pkgs: Vec<Package> = block
                .pages
                .iter()
                .filter_map(|i| singles.get(*i).cloned())
                .collect();
            if frag_pkgs.len() != block.pages.len() {
                return Err("块包含越界页码，整批导入已回滚".to_string());
            }
            let is_full_deck = block.pages.len() == singles.len()
                && block.pages.iter().copied().collect::<Vec<_>>()
                    == (0..singles.len()).collect::<Vec<_>>();
            let frag = if is_full_deck {
                pptx::reassemble(&frag_pkgs)?
            } else {
                crate::assemble::assemble(&frag_pkgs)?
            };
            let frag_bytes = frag.to_bytes()?;
            let unit = self.create_unit(
                &block.name,
                folder_id,
                group_id,
                author,
                block.lang.as_deref().unwrap_or("zh"),
                "",
            )?;
            let rend = self.get_or_create_rendition(
                &unit.id,
                block.form.as_deref().unwrap_or("标准"),
                block.lang.as_deref().unwrap_or("zh"),
            )?;
            let ver = self.insert_version(
                &rend.id,
                SemVer::new(1, 0, 0),
                unit.current_revision,
                author,
                "导入",
                &frag_bytes,
                "normal",
            )?;
            created.push(ImportedUnit {
                unit_id: unit.id,
                rendition_id: rend.id,
                version_id: ver.id,
                semver: ver.semver,
                pages: ver.page_count,
            });
        }
        Ok(ImportOutcome {
            slide_count: report.slide_count,
            fingerprints_match: report.fingerprints_match,
            units: created,
        })
    }

    /// Derive an English companion index: for every entry of the
    /// source branch head, select the unit's published English
    /// version of the same form (the source version itself when it
    /// already is English) and pin it in a new index. `name` is
    /// pre-trimmed and non-empty (callers validate); `user` is the
    /// actor whose group visibility gates the reads and whose
    /// display name maintains the derived index.
    pub fn derive_english_index(
        &self,
        user: &UserRec,
        source_index: &IndexRec,
        branch: &str,
        name: &str,
    ) -> Result<IndexRec, String> {
        let id = &source_index.id;
        let head = self
            .branch_head(id, branch)?
            .ok_or_else(|| "当前分支没有可派生的提交".to_string())?;
        let source = self.get_commit(&head)?;
        let mut entries = Vec::with_capacity(source.entries.len());
        let mut missing = Vec::new();
        for entry in source.entries {
            let unit = self.get_unit(&entry.unit_id.0)?;
            if !self.can_see_group(user, &unit.group_id)? {
                return Err("没有权限读取索引中的内容块".to_string());
            }
            let original = self.get_rendition(&entry.rendition_id.0)?;
            if original.unit_id != unit.id {
                return Err("条目的形态版本不属于该内容块".to_string());
            }
            let source_version = self.resolve_entry_version(&entry)?;
            let mut selected = None;
            if lang_code_of(&original.lang)
                .to_ascii_lowercase()
                .split(['-', '_'])
                .next()
                == Some("en")
            {
                selected = match entry.policy {
                    EntryPolicy::FollowLatest => self
                        .list_versions(&original.id)?
                        .into_iter()
                        .find(|version| version.status == "normal"),
                    EntryPolicy::Locked { .. } => source_version
                        .clone()
                        .filter(|version| version.status == "normal"),
                };
            } else {
                for rendition in self.list_renditions(&unit.id)? {
                    let lang = lang_code_of(&rendition.lang).to_ascii_lowercase();
                    if rendition.form != original.form
                        || lang.split(['-', '_']).next() != Some("en")
                    {
                        continue;
                    }
                    let matches_excerpt = |version: &crate::store::VersionRec| {
                        entry.excerpt_slots.as_ref().is_none_or(|slots| {
                            source_version.as_ref().is_some_and(|source| {
                                source.basis_revision == version.basis_revision
                                    && source.page_count == version.page_count
                                    // Every excerpt token must
                                    // resolve in the source version
                                    // (page-id tokens by entity,
                                    // legacy tokens positionally —
                                    // excerpt_positions is the one
                                    // compat read).
                                    && crate::store::excerpt_positions(source, slots)
                                        .len()
                                        >= slots.len()
                            })
                        })
                    };
                    selected = self
                        .list_versions(&rendition.id)?
                        .into_iter()
                        .find(|version| version.status == "normal" && matches_excerpt(version));
                    if selected.is_some() {
                        break;
                    }
                }
            }
            if let Some(version) = selected {
                // The derived entry points at another rendition
                // whose pages are different entities (translated
                // content): carry the excerpt across by position —
                // resolve the tokens in the source version, then
                // re-address them as the target version's page ids
                // at the same positions.
                let derived_excerpt = match (&entry.excerpt_slots, &source_version) {
                    (Some(slots), Some(source)) => {
                        let positions = crate::store::excerpt_positions(source, slots);
                        let ids: Vec<String> = positions
                            .iter()
                            .filter_map(|pos| {
                                version.pages.get(*pos).map(|p| p.id.clone())
                            })
                            .filter(|id| !id.is_empty())
                            .collect();
                        if ids.is_empty() {
                            None
                        } else {
                            Some(ids)
                        }
                    }
                    _ => entry.excerpt_slots.clone(),
                };
                entries.push(IndexEntry {
                    rendition_id: Id(version.rendition_id),
                    policy: EntryPolicy::Locked {
                        version_id: Id(version.id),
                    },
                    locked_semver: Some(version.semver),
                    excerpt_slots: derived_excerpt,
                    ..entry
                });
            } else {
                missing.push(unit.name);
            }
        }
        if !missing.is_empty() {
            missing.sort();
            missing.dedup();
            return Err(format!(
                "请先为这些内容块发布同形态的英文版本，并核对摘页：{}",
                missing.join("、")
            ));
        }
        let derived = self.create_english_index(
            source_index,
            branch,
            &head,
            name,
            &user.display_name,
            entries,
        )?;
        Ok(derived)
    }
}

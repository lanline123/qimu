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
use serde::{Deserialize, Serialize};

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

/// One page of an opened deck (staged import, stage 1): the page
/// entity it registered as, the one-page wrapper unit presenting
/// it until grouping absorbs it, and the per-page text / script
/// stage 2 needs to build grouped versions without touching the
/// source package again. Serialized so the client's local
/// dispatch can hand the opened state from the open call to the
/// group call verbatim.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OpenedPage {
    pub index: usize,
    pub page_id: String,
    pub fp: String,
    pub title: String,
    pub text: String,
    pub script: String,
    pub unit_id: String,
    pub rendition_id: String,
    pub version_id: String,
    pub semver: String,
}

/// The result of import stage 1 ("open"): every page registered
/// as an entity and wrapped in its own one-page unit, plus the
/// deck's section list read in the same pass. Stage 2
/// (`import_group`) folds the wrappers into the final blocks.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImportOpened {
    pub slide_count: usize,
    pub fingerprints_match: bool,
    pub stem: String,
    pub sections: Option<Vec<pptx::SectionSpan>>,
    pub pages: Vec<OpenedPage>,
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
    /// Import stage 1 — "open" the deck (staged import, user
    /// ruling 2026-11-25): like opening the file in DeckCraft, the
    /// deck becomes visible page by page before any grouping
    /// happens. The source package is loaded ONCE and split ONCE;
    /// the §3.1 split–reassemble check runs over those very
    /// fragments (`verify_from_singles`, analysis point S6a —
    /// never a second split); every page is registered as a page
    /// entity and wrapped in its own one-page content unit in the
    /// target folder, in deck order; the section list is read
    /// from the same loaded package (it lives in
    /// presentation.xml, so no second pass exists). Nothing is
    /// committed before the §3.1 check passes, exactly as the
    /// one-shot import behaved.
    ///
    /// Fingerprint dedup runs ahead of the split: page identity
    /// derives from the slide fingerprint, which the slide listing
    /// alone provides, so a deck whose every page is already a
    /// registered entity skips the split entirely (its singles
    /// are the entities' stored packages — an optimization only:
    /// if the §3.1 check over those foreign fragments errors or
    /// fails, this deck's own split runs and decides); in a mixed
    /// deck the split still runs once and `register_page` leaves
    /// already-known pages' bytes unstored.
    ///
    /// `blocks` is a plan hint only: a page whose final block is
    /// a one-page block gets that block's name / form / lang on
    /// its wrapper right away. Grouping itself is `import_group`.
    /// `author` is the display name recorded as maintainer and
    /// version author (the requesting user over HTTP, the local
    /// library's seeded owner in-process).
    pub fn import_open(
        &self,
        filename: &str,
        bytes: &[u8],
        folder_id: Option<&str>,
        group_id: &str,
        author: &str,
        blocks: Option<&[ImportBlockSpec]>,
    ) -> Result<ImportOpened, String> {
        let pkg = Package::load(bytes)?;
        let slide_refs = pptx::slides(&pkg)?;
        // Per-page title / searchable text / speaker script, read
        // from the source package in this same pass — the same
        // extractions `insert_version_with_scripts` performs per
        // fragment, done here once per slide.
        let mut titles = Vec::with_capacity(slide_refs.len());
        let mut texts = Vec::with_capacity(slide_refs.len());
        let mut scripts = Vec::with_capacity(slide_refs.len());
        for s in &slide_refs {
            let xml = pkg.get_text(&s.part)?;
            titles.push(crate::text::slide_title(&xml));
            texts.push(crate::text::slide_full_text(&xml));
            scripts.push(crate::text::notes_text(&pkg, &s.part));
        }
        let sections = pptx::deck_sections(&pkg)?;
        let stem = filename
            .rsplit('/')
            .next()
            .unwrap_or("未命名")
            .trim_end_matches(".pptx")
            .trim_end_matches(".PPTX")
            .to_string();
        // Fingerprint pre-check: which pages already exist as
        // entities. Identity derives from the fingerprint, so the
        // slide listing answers this before any split happens.
        let mut known: Vec<Option<crate::store::PageRec>> =
            Vec::with_capacity(slide_refs.len());
        for s in &slide_refs {
            let id = crate::store::page_id_for_fp(&s.fingerprint);
            known.push(self.get_page(&id)?);
        }
        let all_known = !known.is_empty() && known.iter().all(|k| k.is_some());
        let mut from_entities = false;
        let mut singles: Vec<Package> = if all_known {
            // The whole deck was imported before: skip the
            // split; the entities' stored packages are the
            // singles. Any read failure falls back to the one
            // split rather than failing the import.
            let mut loaded = Vec::with_capacity(known.len());
            let mut complete = true;
            for rec in known.iter().flatten() {
                match self
                    .file_get(&rec.hash)
                    .and_then(|b| Package::load(&b).map_err(|e| e.to_string()))
                {
                    Ok(p) => loaded.push(p),
                    Err(_) => {
                        complete = false;
                        break;
                    }
                }
            }
            if complete && loaded.len() == slide_refs.len() {
                from_entities = true;
                loaded
            } else {
                pptx::split(&pkg)?
            }
        } else {
            pptx::split(&pkg)?
        };
        if singles.len() != slide_refs.len() {
            return Err(
                "拆分校验未通过，整批导入已回滚: 拆分页数与幻灯片数不符".to_string(),
            );
        }
        // The §3.1 gate, fed with THE fragments split above. The
        // entity fast path is only an optimization: fragments
        // registered from other decks can disagree on shared
        // bookkeeping parts when reassembled together, so any
        // doubt (an error, or a not-ok verdict) falls back to
        // this deck's own split and lets IT decide.
        let mut report = pptx::verify_from_singles(&pkg, &singles);
        if from_entities && !matches!(&report, Ok(r) if r.ok) {
            singles = pptx::split(&pkg)?;
            report = pptx::verify_from_singles(&pkg, &singles);
        }
        let report = report?;
        if !report.ok {
            return Err(format!(
                "拆分校验未通过，整批导入已回滚: {}",
                report.structural_errors.join("; ")
            ));
        }
        // The sectionless naming rule the one-shot import used:
        // the slide's title text, or `第N页` when it carries none.
        let slide_name = |i: usize| -> String {
            let t = &titles[i];
            if t.is_empty() || t == "(无标题)" {
                format!("第{}页", i + 1)
            } else {
                t.clone()
            }
        };
        let mut pages = Vec::with_capacity(slide_refs.len());
        for (i, s) in slide_refs.iter().enumerate() {
            let (name, form, lang) = match blocks
                .and_then(|bs| bs.iter().find(|b| b.pages.contains(&i)))
            {
                Some(spec) if spec.pages.len() == 1 => {
                    (spec.name.clone(), spec.form.clone(), spec.lang.clone())
                }
                _ => (slide_name(i), None, None),
            };
            let lang = lang.as_deref().unwrap_or("zh");
            // A page that already exists as an entity reuses its
            // stored bytes verbatim (register_page returns it
            // without storing); a new page's fragment is its
            // single's bytes, so the entity's file and the
            // wrapper version's fragment are one stored file.
            let frag_bytes = match &known[i] {
                Some(rec) => self.file_get(&rec.hash)?,
                None => singles[i].to_bytes()?,
            };
            let rec = self.register_page(&s.fingerprint, &titles[i], &frag_bytes)?;
            let unit = self.create_unit(&name, folder_id, group_id, author, lang, "")?;
            let rend = self.get_or_create_rendition(
                &unit.id,
                form.as_deref().unwrap_or("标准"),
                lang,
            )?;
            let info = crate::store::PageInfo {
                id: rec.id.clone(),
                fp: s.fingerprint.clone(),
                title: titles[i].clone(),
            };
            let ver = self.insert_version_from_pages(
                &rend.id,
                SemVer::new(1, 0, 0),
                unit.current_revision,
                author,
                "导入",
                "normal",
                &[info],
                &texts[i..i + 1],
                &scripts[i..i + 1],
                Some(&frag_bytes),
            )?;
            pages.push(OpenedPage {
                index: i,
                page_id: rec.id,
                fp: s.fingerprint.clone(),
                title: titles[i].clone(),
                text: texts[i].clone(),
                script: scripts[i].clone(),
                unit_id: unit.id,
                rendition_id: rend.id,
                version_id: ver.id,
                semver: ver.semver,
            });
        }
        Ok(ImportOpened {
            slide_count: report.slide_count,
            fingerprints_match: report.fingerprints_match,
            stem,
            sections,
            pages,
        })
    }

    /// Import stage 2 — grouping (staged import, user ruling
    /// 2026-11-25): fold an opened deck's one-page wrappers into
    /// its final blocks. With caller-supplied `blocks` those are
    /// the blocks; without them, the deck's sections (read in
    /// stage 1) are; a sectionless deck without supplied blocks
    /// keeps its one-page blocks untouched — this stage is then a
    /// no-op and the wrappers ARE the import result (the standing
    /// sectionless rule: one block per slide, named after the
    /// slide's title or `第N页`).
    ///
    /// A multi-page block is created fresh: its version's
    /// pages_json is written directly as the ordered
    /// page-reference sequence — no structure is discovered from
    /// bytes — and its fragment is assembled ONCE from the member
    /// pages' stored single-page packages (the materialized cache
    /// the byte-exact downstream chains key on; a full-deck block
    /// keeps the reassemble path, the same choice the one-shot
    /// import made). The absorbed pages' wrapper units are
    /// permanently removed; their page entities stay. Pages no
    /// supplied block covers lose their wrappers too: the
    /// one-shot import never created units for them.
    pub fn import_group(
        &self,
        opened: &ImportOpened,
        folder_id: Option<&str>,
        group_id: &str,
        author: &str,
        blocks: Option<Vec<ImportBlockSpec>>,
    ) -> Result<ImportOutcome, String> {
        let specs: Vec<ImportBlockSpec> = match blocks {
            Some(blocks) => blocks,
            None => match &opened.sections {
                Some(spans) => spans
                    .iter()
                    .map(|sp| ImportBlockSpec {
                        name: if sp.name.trim().is_empty() {
                            opened.stem.clone()
                        } else {
                            sp.name.clone()
                        },
                        pages: sp.pages.clone(),
                        form: None,
                        lang: None,
                    })
                    .collect(),
                None => {
                    return Ok(ImportOutcome {
                        slide_count: opened.slide_count,
                        fingerprints_match: opened.fingerprints_match,
                        units: opened
                            .pages
                            .iter()
                            .map(|p| ImportedUnit {
                                unit_id: p.unit_id.clone(),
                                rendition_id: p.rendition_id.clone(),
                                version_id: p.version_id.clone(),
                                semver: p.semver.clone(),
                                pages: 1,
                            })
                            .collect(),
                    });
                }
            },
        };
        let total = opened.pages.len();
        let mut covered = vec![false; total];
        let mut created = Vec::new();
        for spec in &specs {
            if spec.pages.is_empty() {
                continue;
            }
            if spec.pages.iter().any(|p| *p >= total) {
                return Err("块包含越界页码，整批导入已回滚".to_string());
            }
            for p in &spec.pages {
                covered[*p] = true;
            }
            if spec.pages.len() == 1 {
                // The wrapper already IS this block; stage 1 gave
                // it the block's name when it had the plan, and a
                // one-page section span names it only here.
                let p = &opened.pages[spec.pages[0]];
                let unit = self.get_unit(&p.unit_id)?;
                if unit.name != spec.name {
                    self.update_unit(&p.unit_id, Some(&spec.name), None, None, None)?;
                }
                created.push(ImportedUnit {
                    unit_id: p.unit_id.clone(),
                    rendition_id: p.rendition_id.clone(),
                    version_id: p.version_id.clone(),
                    semver: p.semver.clone(),
                    pages: 1,
                });
                continue;
            }
            let frag_pkgs: Vec<Package> = spec
                .pages
                .iter()
                .map(|i| {
                    let rec = self
                        .get_page(&opened.pages[*i].page_id)?
                        .ok_or_else(|| "页实体不存在".to_string())?;
                    let bytes = self.file_get(&rec.hash)?;
                    Package::load(&bytes).map_err(|e| e.to_string())
                })
                .collect::<Result<_, String>>()?;
            let is_full_deck = spec.pages.len() == total
                && spec.pages.iter().copied().collect::<Vec<_>>()
                    == (0..total).collect::<Vec<_>>();
            let frag = if is_full_deck {
                pptx::reassemble(&frag_pkgs)?
            } else {
                crate::assemble::assemble(&frag_pkgs)?
            };
            let frag_bytes = frag.to_bytes()?;
            let lang = spec.lang.as_deref().unwrap_or("zh");
            let unit =
                self.create_unit(&spec.name, folder_id, group_id, author, lang, "")?;
            let rend = self.get_or_create_rendition(
                &unit.id,
                spec.form.as_deref().unwrap_or("标准"),
                lang,
            )?;
            let infos: Vec<crate::store::PageInfo> = spec
                .pages
                .iter()
                .map(|i| {
                    let p = &opened.pages[*i];
                    crate::store::PageInfo {
                        id: p.page_id.clone(),
                        fp: p.fp.clone(),
                        title: p.title.clone(),
                    }
                })
                .collect();
            let texts: Vec<String> =
                spec.pages.iter().map(|i| opened.pages[*i].text.clone()).collect();
            let scripts: Vec<String> =
                spec.pages.iter().map(|i| opened.pages[*i].script.clone()).collect();
            let ver = self.insert_version_from_pages(
                &rend.id,
                SemVer::new(1, 0, 0),
                unit.current_revision,
                author,
                "导入",
                "normal",
                &infos,
                &texts,
                &scripts,
                Some(&frag_bytes),
            )?;
            created.push(ImportedUnit {
                unit_id: unit.id,
                rendition_id: rend.id,
                version_id: ver.id,
                semver: ver.semver,
                pages: ver.page_count,
            });
            // Absorb the grouped pages' wrappers (the page
            // entities themselves stay, referenced by the block).
            for i in &spec.pages {
                self.delete_unit_permanent(&opened.pages[*i].unit_id)?;
            }
        }
        // Pages no block covers (caller-supplied splits only):
        // the one-shot import never created units for them.
        for (i, p) in opened.pages.iter().enumerate() {
            if !covered[i] {
                self.delete_unit_permanent(&p.unit_id)?;
            }
        }
        Ok(ImportOutcome {
            slide_count: opened.slide_count,
            fingerprints_match: opened.fingerprints_match,
            units: created,
        })
    }

    /// Import a deck (§5.8 + §3.1) — the one-shot composition of
    /// the two stages above, kept for the request/response
    /// callers (the server's import handler, the local dispatch's
    /// /v1/import arm): open, then group, in one call, converging
    /// to the staged flow's end state. Each block becomes one
    /// content unit with one rendition and version 1.0.0; the
    /// original whole file is never archived. `blocks = None`
    /// imports without a caller-supplied split: a deck carrying
    /// real PPT sections splits by section (one section = one
    /// block, named after the section; slides in no section form
    /// one block named after the file stem), and a sectionless
    /// deck splits one block per slide (user ruling 2026-11-24),
    /// each named after the slide's title text, or `第N页` when
    /// the slide carries no title. As before, a block failing
    /// mid-import leaves earlier blocks committed.
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
        let opened =
            self.import_open(filename, bytes, folder_id, group_id, author, blocks.as_deref())?;
        self.import_group(&opened, folder_id, group_id, author, blocks)
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

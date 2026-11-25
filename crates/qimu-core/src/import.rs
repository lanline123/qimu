//! Import orchestration (§5.8 + §3.1) and English-index derivation,
//! sunk from the server's HTTP handlers (local-first P1, design
//! §6.2/§7) so every frontend runs the identical sequence: the
//! server stays a thin shell over these methods, and the desktop
//! client's in-process local library calls them directly — import
//! lands in the library the caller owns, never "in whichever store
//! happens to sit behind an HTTP connection".
//!
//! The sequences below are transcribed from the handlers they
//! replace, step for step and error string for error string.
//! One deliberate departure (audit N4): a block failing
//! mid-import used to leave earlier blocks committed while the
//! message claimed the batch was rolled back. The staged flow
//! now rolls its own writes back on failure and is idempotent
//! across retries, and the messages say what actually happens.

use crate::auth::{new_id, now_ms};
use crate::model::{EntryPolicy, Id, IndexEntry};
use crate::pptx::{self, Package};
use crate::store::{IndexRec, PendingImportRec, PendingPage, Store, UserRec};
use crate::SemVer;
use serde::{Deserialize, Serialize};

/// One block of an import request: the pages of the source deck
/// that become one content unit (the server's `ImportBlock` wire
/// shape, minus transport concerns).
#[derive(Debug, Clone, Serialize, Deserialize)]
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

/// One page of a lazily opened deck (lazy import, user ruling
/// 2026-11-25): the wrapper unit presenting the page from the
/// moment the deck opened, and whether its split has landed —
/// an unsplit page is display-only (its card renders from the
/// deck) and unusable until `split` flips.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LazyPage {
    pub index: usize,
    pub page_id: String,
    pub fp: String,
    pub title: String,
    pub unit_id: String,
    pub rendition_id: String,
    pub version_id: String,
    pub split: bool,
}

/// The result of opening a deck the lazy way: the deck is in
/// the library and every page is on the wall as a wrapper unit
/// BEFORE any splitting happened; `pending_id` names the
/// durable split state the background worker then works off.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LazyOpened {
    pub pending_id: String,
    pub slide_count: usize,
    pub stem: String,
    pub sections: Option<Vec<pptx::SectionSpan>>,
    pub pages: Vec<LazyPage>,
}

/// One pending import's split state, as the split worker, the
/// resume path and the toolbar status consume it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LazyState {
    pub pending_id: String,
    pub filename: String,
    pub stem: String,
    pub slide_count: usize,
    pub split_count: usize,
    pub state: String,
    pub error: String,
    pub pages: Vec<LazyPage>,
}

/// The outcome of one page's split step: the running progress,
/// any blocks this step completed (their wrappers are gone,
/// folded into the returned units), and whether the import is
/// finished (its pending row is deleted) or failed (the row
/// stays, marked, with `error`).
#[derive(Debug, Clone, Serialize)]
pub struct LazySplitOutcome {
    pub pending_id: String,
    pub index: usize,
    pub split_count: usize,
    pub slide_count: usize,
    pub grouped_units: Vec<ImportedUnit>,
    pub done: bool,
    pub failed: bool,
    pub error: String,
}

fn lazy_page_of(p: &PendingPage) -> LazyPage {
    LazyPage {
        index: p.index,
        page_id: crate::store::page_id_for_fp(&p.fp),
        fp: p.fp.clone(),
        title: p.title.clone(),
        unit_id: p.unit_id.clone(),
        rendition_id: p.rendition_id.clone(),
        version_id: p.version_id.clone(),
        split: p.split,
    }
}

fn lazy_state_of(rec: &PendingImportRec) -> LazyState {
    LazyState {
        pending_id: rec.id.clone(),
        filename: rec.filename.clone(),
        stem: rec.stem.clone(),
        slide_count: rec.slide_count,
        split_count: rec.pages.iter().filter(|p| p.split || p.gone).count(),
        state: rec.state.clone(),
        error: rec.error.clone(),
        pages: rec.pages.iter().map(lazy_page_of).collect(),
    }
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
    /// An already-landed one-page wrapper for `page_id` in the
    /// same home (folder + group + rendition shape + version
    /// 1.0.0), if one exists — the residue of an earlier
    /// interrupted `import_open` of the same deck, or of a
    /// completed open whose grouping never ran. Re-importing
    /// adopts it instead of minting a duplicate wrapper
    /// (audit N4).
    fn find_page_wrapper(
        &self,
        folder_id: Option<&str>,
        group_id: &str,
        name: &str,
        form: &str,
        lang: &str,
        page_id: &str,
    ) -> Option<(String, String, String, String)> {
        let units = self.list_units(None).ok()?;
        for u in units {
            if u.folder_id.as_deref() != folder_id || u.group_id != group_id || u.name != name {
                continue;
            }
            let Ok(rends) = self.list_renditions(&u.id) else {
                continue;
            };
            for r in rends {
                if r.form != form || r.lang != lang {
                    continue;
                }
                let Ok(vers) = self.list_versions(&r.id) else {
                    continue;
                };
                for v in vers {
                    if v.semver == "1.0.0" && v.pages.len() == 1 && v.pages[0].id == page_id {
                        return Some((u.id.clone(), r.id.clone(), v.id.clone(), v.semver.clone()));
                    }
                }
            }
        }
        None
    }

    /// A wrapper unit in the given home whose matching rendition
    /// carries NO version at all — the residue of an open that
    /// died between creating the unit and inserting the version
    /// (the N8 failure at `lazy_open` left exactly one per
    /// affected library, for its first not-yet-known page). It
    /// can be told apart from another flow's interrupted wrapper
    /// only by home + name + rendition shape — exactly the
    /// identity `find_page_wrapper` keys on, minus the version.
    /// Returns (unit_id, rendition_id).
    fn find_versionless_wrapper(
        &self,
        folder_id: Option<&str>,
        group_id: &str,
        name: &str,
        form: &str,
        lang: &str,
    ) -> Option<(String, String)> {
        let units = self.list_units(None).ok()?;
        for u in units {
            if u.folder_id.as_deref() != folder_id || u.group_id != group_id || u.name != name {
                continue;
            }
            let Ok(rends) = self.list_renditions(&u.id) else {
                continue;
            };
            for r in rends {
                if r.form != form || r.lang != lang {
                    continue;
                }
                match self.list_versions(&r.id) {
                    Ok(vers) if vers.is_empty() => {
                        return Some((u.id.clone(), r.id.clone()));
                    }
                    _ => continue,
                }
            }
        }
        None
    }

    /// A block unit in the given home whose 1.0.0 version's
    /// page-reference sequence is exactly `page_ids`, if one
    /// exists — the block an earlier `import_group` call with
    /// the same plan already created (audit N4 idempotency).
    fn find_block_unit(
        &self,
        folder_id: Option<&str>,
        group_id: &str,
        name: Option<&str>,
        form: &str,
        lang: &str,
        page_ids: &[String],
    ) -> Option<(String, String, String, String, i64)> {
        let units = self.list_units(None).ok()?;
        for u in units {
            if u.folder_id.as_deref() != folder_id || u.group_id != group_id {
                continue;
            }
            if let Some(n) = name {
                if u.name != n {
                    continue;
                }
            }
            let Ok(rends) = self.list_renditions(&u.id) else {
                continue;
            };
            for r in rends {
                if r.form != form || r.lang != lang {
                    continue;
                }
                let Ok(vers) = self.list_versions(&r.id) else {
                    continue;
                };
                for v in vers {
                    if v.semver == "1.0.0"
                        && v.pages.len() == page_ids.len()
                        && v.pages.iter().zip(page_ids).all(|(p, id)| &p.id == id)
                    {
                        return Some((
                            u.id.clone(),
                            r.id.clone(),
                            v.id.clone(),
                            v.semver.clone(),
                            v.page_count,
                        ));
                    }
                }
            }
        }
        None
    }

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
        // The page loop runs as one attempt (audit N4): a
        // failure midway rolls back the wrappers THIS call
        // created, so a retry starts from the pre-import state
        // instead of duplicating every page already landed.
        // Wrappers adopted from an earlier open are not rolled
        // back — they were not created here, and the retry will
        // adopt them again.
        let mut created_units: Vec<String> = Vec::new();
        let mut attempt = || -> Result<Vec<OpenedPage>, String> {
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
            let form = form.as_deref().unwrap_or("标准");
            // Adopt a wrapper an earlier open of this deck
            // already landed for this page (audit N4).
            let page_id = crate::store::page_id_for_fp(&s.fingerprint);
            if let Some((uid, rid, vid, semver)) =
                self.find_page_wrapper(folder_id, group_id, &name, form, lang, &page_id)
            {
                pages.push(OpenedPage {
                    index: i,
                    page_id,
                    fp: s.fingerprint.clone(),
                    title: titles[i].clone(),
                    text: texts[i].clone(),
                    script: scripts[i].clone(),
                    unit_id: uid,
                    rendition_id: rid,
                    version_id: vid,
                    semver,
                });
                continue;
            }
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
            created_units.push(unit.id.clone());
            let rend = self.get_or_create_rendition(&unit.id, form, lang)?;
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
            Ok(pages)
        };
        let pages = match attempt() {
            Ok(pages) => pages,
            Err(e) => {
                for uid in &created_units {
                    let _ = self.delete_unit_permanent(uid);
                }
                return Err(e);
            }
        };
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
        // Phase 0 — validate EVERYTHING before writing anything
        // (audit N4): bounds and page-entity existence for all
        // specs up front. The old loop checked bounds per spec
        // mid-flight, so the "整批导入已回滚" error could fire
        // after earlier specs had already landed and their
        // wrappers were permanently deleted.
        for spec in &specs {
            if spec.pages.iter().any(|p| *p >= total) {
                return Err("块包含越界页码，导入未执行".to_string());
            }
            for p in &spec.pages {
                if self.get_page(&opened.pages[*p].page_id)?.is_none() {
                    return Err("页实体不存在，导入未执行".to_string());
                }
            }
        }
        let mut covered = vec![false; total];
        for spec in &specs {
            for p in &spec.pages {
                covered[*p] = true;
            }
        }
        // Phase 1 — create / adopt blocks and apply renames. Any
        // failure rolls this phase back (created blocks deleted,
        // renames reverted, wrappers untouched), leaving exactly
        // the opened state behind for a retry. Repeat calls are
        // idempotent: a block an earlier call created for the
        // same page sequence is adopted, not duplicated.
        let mut created: Vec<ImportedUnit> = Vec::new();
        let mut made_units: Vec<String> = Vec::new();
        let mut renames: Vec<(String, String)> = Vec::new();
        let mut phase1 = || -> Result<(), String> {
            for spec in &specs {
                if spec.pages.is_empty() {
                    continue;
                }
                let lang = spec.lang.as_deref().unwrap_or("zh");
                let form = spec.form.as_deref().unwrap_or("标准");
                if spec.pages.len() == 1 {
                    // The wrapper already IS this block; stage 1
                    // gave it the block's name when it had the
                    // plan, and a one-page section span names it
                    // only here. On a repeat call the wrapper may
                    // already be absorbed into a block — adopt
                    // whichever block now carries the page.
                    let p = &opened.pages[spec.pages[0]];
                    match self.get_unit(&p.unit_id) {
                        Ok(unit) => {
                            if unit.name != spec.name {
                                self.update_unit(
                                    &p.unit_id,
                                    Some(&spec.name),
                                    None,
                                    None,
                                    None,
                                )?;
                                renames.push((p.unit_id.clone(), unit.name.clone()));
                            }
                            created.push(ImportedUnit {
                                unit_id: p.unit_id.clone(),
                                rendition_id: p.rendition_id.clone(),
                                version_id: p.version_id.clone(),
                                semver: p.semver.clone(),
                                pages: 1,
                            });
                        }
                        Err(_) => {
                            let found = self.find_block_unit(
                                folder_id,
                                group_id,
                                None,
                                form,
                                lang,
                                &[p.page_id.clone()],
                            );
                            match found {
                                Some((uid, rid, vid, semver, count)) => {
                                    created.push(ImportedUnit {
                                        unit_id: uid,
                                        rendition_id: rid,
                                        version_id: vid,
                                        semver,
                                        pages: count,
                                    });
                                }
                                None => return Err("页实体包装块不存在".into()),
                            }
                        }
                    }
                    continue;
                }
                // Idempotency: adopt the block an earlier call
                // with the same plan already created.
                let want_ids: Vec<String> = spec
                    .pages
                    .iter()
                    .map(|i| opened.pages[*i].page_id.clone())
                    .collect();
                if let Some((uid, rid, vid, semver, count)) = self.find_block_unit(
                    folder_id,
                    group_id,
                    Some(&spec.name),
                    form,
                    lang,
                    &want_ids,
                ) {
                    created.push(ImportedUnit {
                        unit_id: uid,
                        rendition_id: rid,
                        version_id: vid,
                        semver,
                        pages: count,
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
                let unit =
                    self.create_unit(&spec.name, folder_id, group_id, author, lang, "")?;
                made_units.push(unit.id.clone());
                let rend = self.get_or_create_rendition(&unit.id, form, lang)?;
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
                let scripts: Vec<String> = spec
                    .pages
                    .iter()
                    .map(|i| opened.pages[*i].script.clone())
                    .collect();
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
            }
            Ok(())
        };
        if let Err(e) = phase1() {
            for uid in &made_units {
                let _ = self.delete_unit_permanent(uid);
            }
            for (uid, old_name) in &renames {
                let _ = self.update_unit(uid, Some(old_name), None, None, None);
            }
            return Err(e);
        }
        // Phase 2 — absorb the grouped pages' wrappers (the page
        // entities themselves stay, referenced by the block). A
        // wrapper that no longer exists was absorbed by an
        // earlier call — not an error.
        for spec in &specs {
            if spec.pages.len() > 1 {
                for i in &spec.pages {
                    let p = &opened.pages[*i];
                    if self.get_unit(&p.unit_id).is_ok() {
                        self.delete_unit_permanent(&p.unit_id)?;
                    }
                }
            }
        }
        // Pages no block covers (caller-supplied splits only):
        // the one-shot import never created units for them.
        for (i, p) in opened.pages.iter().enumerate() {
            if !covered[i] && self.get_unit(&p.unit_id).is_ok() {
                self.delete_unit_permanent(&p.unit_id)?;
            }
        }
        Ok(ImportOutcome {
            slide_count: opened.slide_count,
            fingerprints_match: opened.fingerprints_match,
            units: created,
        })
    }

    /// Fold ONE block spec over an opened deck's one-page
    /// wrappers: a one-page spec renames its wrapper in place; a
    /// multi-page spec creates the block unit, writes its version
    /// directly as the ordered page-reference sequence, assembles
    /// its materialized fragment ONCE from the member pages'
    /// stored single-page packages (a full-deck block keeps the
    /// reassemble path), and permanently removes the absorbed
    /// wrappers (their page entities stay). The lazy import folds
    /// each block the moment its last page finishes splitting;
    /// the staged import's `import_group` keeps its own phased
    /// structure (Phase 0 pre-validation + Phase 1/2) with the
    /// same per-block semantics — audit N4 adoption and rollback
    /// included, mirrored here so a repeated fold (e.g. after a
    /// crash between the fold and the pending record's update)
    /// adopts the block an earlier fold created instead of
    /// duplicating it, and a fold that fails after creating its
    /// unit removes that unit again. `Ok(None)` for an empty
    /// spec.
    fn fold_block(
        &self,
        opened: &ImportOpened,
        spec: &ImportBlockSpec,
        folder_id: Option<&str>,
        group_id: &str,
        author: &str,
    ) -> Result<Option<ImportedUnit>, String> {
        let total = opened.pages.len();
        if spec.pages.is_empty() {
            return Ok(None);
        }
        if spec.pages.iter().any(|p| *p >= total) {
            return Err("块包含越界页码，导入未执行".to_string());
        }
        let lang = spec.lang.as_deref().unwrap_or("zh");
        let form = spec.form.as_deref().unwrap_or("标准");
        if spec.pages.len() == 1 {
            // The wrapper already IS this block; stage 1 gave
            // it the block's name when it had the plan, and a
            // one-page section span names it only here. On a
            // repeat fold the wrapper may already be absorbed
            // into a block — adopt whichever block now carries
            // the page (same fallback as import_group's phase 1).
            let p = &opened.pages[spec.pages[0]];
            match self.get_unit(&p.unit_id) {
                Ok(unit) => {
                    if unit.name != spec.name {
                        self.update_unit(&p.unit_id, Some(&spec.name), None, None, None)?;
                    }
                    return Ok(Some(ImportedUnit {
                        unit_id: p.unit_id.clone(),
                        rendition_id: p.rendition_id.clone(),
                        version_id: p.version_id.clone(),
                        semver: p.semver.clone(),
                        pages: 1,
                    }));
                }
                Err(_) => {
                    let found = self.find_block_unit(
                        folder_id,
                        group_id,
                        None,
                        form,
                        lang,
                        &[p.page_id.clone()],
                    );
                    return match found {
                        Some((uid, rid, vid, semver, count)) => Ok(Some(ImportedUnit {
                            unit_id: uid,
                            rendition_id: rid,
                            version_id: vid,
                            semver,
                            pages: count,
                        })),
                        None => Err("页实体包装块不存在".into()),
                    };
                }
            }
        }
        // Idempotency (audit N4): adopt the block an earlier
        // fold with the same plan already created, and make
        // sure its wrappers are absorbed.
        let want_ids: Vec<String> = spec
            .pages
            .iter()
            .map(|i| opened.pages[*i].page_id.clone())
            .collect();
        if let Some((uid, rid, vid, semver, count)) =
            self.find_block_unit(folder_id, group_id, Some(&spec.name), form, lang, &want_ids)
        {
            for i in &spec.pages {
                let p = &opened.pages[*i];
                if self.get_unit(&p.unit_id).is_ok() {
                    self.delete_unit_permanent(&p.unit_id)?;
                }
            }
            return Ok(Some(ImportedUnit {
                unit_id: uid,
                rendition_id: rid,
                version_id: vid,
                semver,
                pages: count,
            }));
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
        let unit = self.create_unit(&spec.name, folder_id, group_id, author, lang, "")?;
        // A fold that fails after creating its unit removes the
        // unit again (audit N4 attempt rollback, per-block scope
        // — import_group does the same across its whole phase 1).
        let folded = (|| -> Result<ImportedUnit, String> {
            let rend = self.get_or_create_rendition(&unit.id, form, lang)?;
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
            Ok(ImportedUnit {
                unit_id: unit.id.clone(),
                rendition_id: rend.id,
                version_id: ver.id,
                semver: ver.semver,
                pages: ver.page_count,
            })
        })();
        let created = match folded {
            Ok(created) => created,
            Err(e) => {
                let _ = self.delete_unit_permanent(&unit.id);
                return Err(e);
            }
        };
        // Absorb the grouped pages' wrappers (the page
        // entities themselves stay, referenced by the block).
        for i in &spec.pages {
            self.delete_unit_permanent(&opened.pages[*i].unit_id)?;
        }
        Ok(Some(created))
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
    /// the slide carries no title. A block failing mid-import
    /// rolls the import's own writes back (audit N4); retrying
    /// the same import adopts what an interrupted attempt
    /// landed instead of duplicating it.
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

    /// Lazy import, the open (user ruling 2026-11-25, "先打开、
    /// 后分割"): like opening the file in DeckCraft, this does NO
    /// splitting and NO §3.1 gate — it loads the package once for
    /// the slide listing (titles / texts / scripts / sections,
    /// the same single pass `import_open` makes), stores the
    /// whole deck in the file store as the pages' backing, wraps
    /// every page in its own one-page unit whose version already
    /// carries the page-reference sequence but NO fragment yet,
    /// and records the durable split state (`PendingImportRec`)
    /// the background worker then drains page by page. The end
    /// state after the split is exactly the staged import's —
    /// page entities plus section blocks — only the timing
    /// differs, so nothing downstream ever learns this path
    /// existed.
    ///
    /// A page the library already holds as an entity is split
    /// by definition: its wrapper adopts the entity's stored
    /// package as its fragment at once (the fingerprint fast
    /// path, same as `import_open`). If EVERY page is already
    /// known, grouping and the deck-level gate run inline and no
    /// pending row survives the call.
    ///
    /// A not-yet-known page's wrapper version is born pending
    /// (`insert_pending_version_from_pages`, empty fragment):
    /// routing it through `insert_version_from_pages` with a
    /// None fragment trips the audit-N8 birth guard and kills
    /// the open at the first new page — the failure this flow
    /// originally shipped with, since the guard and the lazy
    /// open were written on parallel baselines. Re-opening the
    /// same deck adopts the wrappers an earlier open landed
    /// (completed or still-pending) and the versionless unit an
    /// interrupted open left behind, and a failed open rolls
    /// back the wrappers it created itself (audit N4), so a
    /// retry converges instead of duplicating.
    pub fn lazy_open(
        &self,
        filename: &str,
        bytes: &[u8],
        folder_id: Option<&str>,
        group_id: &str,
        author: &str,
        blocks: Option<&[ImportBlockSpec]>,
    ) -> Result<LazyOpened, String> {
        let pkg = Package::load(bytes)?;
        let slide_refs = pptx::slides(&pkg)?;
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
        let deck_hash = self.file_put(bytes)?;
        // The sectionless naming rule (shared with import_open):
        // the slide's title text, or `第N页` when it carries none.
        let slide_name = |i: usize| -> String {
            let t = &titles[i];
            if t.is_empty() || t == "(无标题)" {
                format!("第{}页", i + 1)
            } else {
                t.clone()
            }
        };
        let pending_id = new_id("lzi");
        // The page loop runs as one attempt (audit N4, mirrored
        // from import_open): a failure midway rolls back the
        // wrappers THIS call created, so a retry starts from the
        // pre-open state instead of duplicating every page
        // already landed. Wrappers adopted from an earlier open
        // are not rolled back — they were not created here, and
        // the retry will adopt them again.
        let mut created_units: Vec<String> = Vec::new();
        let mut attempt = || -> Result<Vec<PendingPage>, String> {
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
                let form = form.as_deref().unwrap_or("标准");
                let page_id = crate::store::page_id_for_fp(&s.fingerprint);
                let info = crate::store::PageInfo {
                    id: page_id.clone(),
                    fp: s.fingerprint.clone(),
                    title: titles[i].clone(),
                };
                let known = self.get_page(&page_id)?;
                // Adopt a wrapper an earlier open of this deck
                // already landed for this page (audit N4): a
                // completed wrapper (its version carries the
                // fragment) counts as split; a still-pending one
                // (empty fragment, from an interrupted lazy open)
                // is re-tracked by THIS record, and its split
                // fills the SAME version in when it lands.
                if let Some((uid, rid, vid, semver)) = self.find_page_wrapper(
                    folder_id,
                    group_id,
                    &name,
                    form,
                    lang,
                    &page_id,
                ) {
                    let ver = self.get_version(&vid)?;
                    pages.push(PendingPage {
                        index: i,
                        fp: s.fingerprint.clone(),
                        title: titles[i].clone(),
                        text: texts[i].clone(),
                        script: scripts[i].clone(),
                        unit_id: uid,
                        rendition_id: rid,
                        version_id: vid,
                        semver,
                        split: !ver.fragment_hash.is_empty(),
                        gone: false,
                    });
                    continue;
                }
                // Adopt the residue of an open that died between
                // creating the wrapper unit and inserting its
                // version — the N8 failure at this very call left
                // exactly one such unit (for the first
                // not-yet-known page) in every affected library.
                // The unit and rendition are reused and the
                // version is born on them below, so re-importing
                // the same deck heals the residue instead of
                // piling a duplicate wrapper beside it.
                let (unit, rend) = match self.find_versionless_wrapper(
                    folder_id,
                    group_id,
                    &name,
                    form,
                    lang,
                ) {
                    Some((uid, rid)) => {
                        (self.get_unit(&uid)?, self.get_rendition(&rid)?)
                    }
                    None => {
                        let unit =
                            self.create_unit(&name, folder_id, group_id, author, lang, "")?;
                        created_units.push(unit.id.clone());
                        let rend = self.get_or_create_rendition(&unit.id, form, lang)?;
                        (unit, rend)
                    }
                };
                // A page the library already holds as an entity
                // is split by definition: its wrapper version
                // adopts the entity's stored package at once. Any
                // other page's wrapper is born pending
                // (`insert_pending_version_from_pages`): passing
                // its (nonexistent) fragment to
                // `insert_version_from_pages` instead is what
                // tripped the N8 guard and killed the open at the
                // first new page — the split owes this version
                // the entity's package, paid by `lazy_split_page`
                // via `set_version_fragment_hash`.
                let (ver, split) = match &known {
                    Some(rec) => {
                        let frag_bytes = self.file_get(&rec.hash)?;
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
                        (ver, true)
                    }
                    None => {
                        let ver = self.insert_pending_version_from_pages(
                            &rend.id,
                            SemVer::new(1, 0, 0),
                            unit.current_revision,
                            author,
                            "导入",
                            "normal",
                            &[info],
                            &texts[i..i + 1],
                            &scripts[i..i + 1],
                        )?;
                        (ver, false)
                    }
                };
                pages.push(PendingPage {
                    index: i,
                    fp: s.fingerprint.clone(),
                    title: titles[i].clone(),
                    text: texts[i].clone(),
                    script: scripts[i].clone(),
                    unit_id: unit.id.clone(),
                    rendition_id: rend.id.clone(),
                    version_id: ver.id.clone(),
                    semver: ver.semver.clone(),
                    split,
                    gone: false,
                });
            }
            Ok(pages)
        };
        let pages = match attempt() {
            Ok(pages) => pages,
            Err(e) => {
                for uid in &created_units {
                    let _ = self.delete_unit_permanent(uid);
                }
                return Err(e);
            }
        };
        let rec = PendingImportRec {
            id: pending_id.clone(),
            filename: filename.to_string(),
            stem: stem.clone(),
            deck_hash,
            folder_id: folder_id.map(|f| f.to_string()),
            group_id: group_id.to_string(),
            author: author.to_string(),
            slide_count: slide_refs.len(),
            sections: sections.clone(),
            pages,
            blocks: blocks.map(|b| b.to_vec()),
            grouped: Vec::new(),
            verified: None,
            state: "splitting".to_string(),
            error: String::new(),
            created_ms: now_ms(),
        };
        let opened = LazyOpened {
            pending_id: pending_id.clone(),
            slide_count: rec.slide_count,
            stem: stem.clone(),
            sections,
            pages: rec.pages.iter().map(lazy_page_of).collect(),
        };
        if rec.pages.is_empty() {
            // Nothing to split: no durable state to track.
            return Ok(opened);
        }
        self.insert_pending_import(&rec)?;
        // Every page was already an entity: nothing is left for
        // the worker — fold and verify inline so no pending row
        // survives (its LazyOpened above stays truthful: those
        // page units are the import result or were just folded).
        if rec.pages.iter().all(|p| p.split) && !rec.pages.is_empty() {
            let mut rec = rec;
            let (_, done) = self.lazy_advance(&mut rec)?;
            if done {
                self.delete_pending_import(&rec.id)?;
            } else {
                self.update_pending_import(&rec)?;
            }
        }
        Ok(opened)
    }

    /// The block specs a pending import folds by: the caller's
    /// plan when one was supplied, else the deck's sections
    /// (named after the section, or the file stem when a section
    /// carries no name — the staged import's rule), else none (a
    /// sectionless deck's wrappers ARE its final blocks).
    fn lazy_specs(rec: &PendingImportRec) -> Option<Vec<ImportBlockSpec>> {
        if let Some(blocks) = &rec.blocks {
            return Some(blocks.clone());
        }
        rec.sections.as_ref().map(|spans| {
            spans
                .iter()
                .map(|sp| ImportBlockSpec {
                    name: if sp.name.trim().is_empty() {
                        rec.stem.clone()
                    } else {
                        sp.name.clone()
                    },
                    pages: sp.pages.clone(),
                    form: None,
                    lang: None,
                })
                .collect()
        })
    }

    /// An `ImportOpened` view of a pending import's current
    /// state, so grouping reuses the staged import's fold
    /// verbatim (`fold_block` reads page info / texts / scripts
    /// and the wrapper ids from it).
    fn lazy_opened_view(rec: &PendingImportRec) -> ImportOpened {
        ImportOpened {
            slide_count: rec.slide_count,
            fingerprints_match: rec.verified.unwrap_or(false),
            stem: rec.stem.clone(),
            sections: rec.sections.clone(),
            pages: rec
                .pages
                .iter()
                .map(|p| OpenedPage {
                    index: p.index,
                    page_id: crate::store::page_id_for_fp(&p.fp),
                    fp: p.fp.clone(),
                    title: p.title.clone(),
                    text: p.text.clone(),
                    script: p.script.clone(),
                    unit_id: p.unit_id.clone(),
                    rendition_id: p.rendition_id.clone(),
                    version_id: p.version_id.clone(),
                    semver: p.semver.clone(),
                })
                .collect(),
        }
    }

    /// Advance a pending import after a page state change: fold
    /// every block whose pages are now all resolved, then — when
    /// every page is resolved — run the deck-level half of the
    /// §3.1 gate (the split–reassemble equivalence over the whole
    /// deck, the same `verify_from_singles` the staged import
    /// runs before committing anything; here it is recorded on
    /// the pending row instead of gating the open, because the
    /// per-page half already gated each page's admission as an
    /// entity). Returns the units folded by this advance and
    /// whether the import is complete (the caller then deletes
    /// the row). A block containing a page whose wrapper vanished
    /// before its split is left unfolded — its surviving pages
    /// stay usable single-page blocks — rather than failing the
    /// whole import for one deleted card.
    fn lazy_advance(
        &self,
        rec: &mut PendingImportRec,
    ) -> Result<(Vec<ImportedUnit>, bool), String> {
        let mut grouped_units = Vec::new();
        if let Some(specs) = Self::lazy_specs(rec) {
            let opened = Self::lazy_opened_view(rec);
            for (pos, spec) in specs.iter().enumerate() {
                if rec.grouped.contains(&pos) {
                    continue;
                }
                if spec.pages.is_empty() {
                    rec.grouped.push(pos);
                    continue;
                }
                let resolved = spec.pages.iter().all(|i| {
                    rec.pages
                        .get(*i)
                        .map(|p| p.split || p.gone)
                        .unwrap_or(false)
                });
                if !resolved {
                    continue;
                }
                let has_gone = spec
                    .pages
                    .iter()
                    .any(|i| rec.pages.get(*i).map(|p| p.gone).unwrap_or(false));
                match self.fold_block(
                    &opened,
                    spec,
                    rec.folder_id.as_deref(),
                    &rec.group_id,
                    &rec.author,
                ) {
                    Ok(unit) => {
                        rec.grouped.push(pos);
                        if let Some(unit) = unit {
                            grouped_units.push(unit);
                        }
                    }
                    Err(e) => {
                        if !has_gone {
                            return Err(e);
                        }
                    }
                }
            }
        }
        let all_resolved = rec.pages.iter().all(|p| p.split || p.gone);
        if !all_resolved || rec.state == "failed" {
            return Ok((grouped_units, false));
        }
        if rec.verified.is_none() {
            let deck_bytes = self.file_get(&rec.deck_hash)?;
            let pkg = Package::load(&deck_bytes)?;
            let singles = pptx::split(&pkg)?;
            match pptx::verify_from_singles(&pkg, &singles) {
                Ok(report) => {
                    rec.verified = Some(report.ok);
                    if !report.ok {
                        rec.state = "failed".to_string();
                        rec.error = format!(
                            "拆分校验未通过: {}",
                            report.structural_errors.join("; ")
                        );
                    }
                }
                Err(e) => {
                    rec.verified = Some(false);
                    rec.state = "failed".to_string();
                    rec.error = format!("拆分校验未通过: {e}");
                }
            }
        }
        Ok((grouped_units, rec.state != "failed"))
    }

    /// Split ONE page of a pending import (the background
    /// worker's unit of work, called in whatever priority order
    /// the client chooses): build just that page's fragment from
    /// the backed deck (`split_one` — no other page is touched),
    /// run the per-page half of the §3.1 gate (the fragment must
    /// be a valid one-slide package whose slide fingerprint is
    /// exactly the one the open pass listed for this position —
    /// a page that fails it is NOT admitted and the import is
    /// marked failed, like the deck-level gate does),
    /// register the page entity, and point the wrapper version
    /// at the entity's package: from here the page is a normal
    /// staged-import wrapper in every respect. Then advance
    /// (fold completed blocks; on the last page, the deck-level
    /// gate). A page whose wrapper unit was deleted before its
    /// turn is marked gone and skipped, never resurrected.
    /// Idempotent: an already-resolved page just advances.
    pub fn lazy_split_page(
        &self,
        pending_id: &str,
        index: usize,
    ) -> Result<LazySplitOutcome, String> {
        let mut rec = self
            .get_pending_import(pending_id)?
            .ok_or_else(|| "待分割记录不存在".to_string())?;
        if rec.state == "failed" {
            return Ok(LazySplitOutcome {
                pending_id: rec.id.clone(),
                index,
                split_count: rec.pages.iter().filter(|p| p.split || p.gone).count(),
                slide_count: rec.slide_count,
                grouped_units: Vec::new(),
                done: false,
                failed: true,
                error: rec.error.clone(),
            });
        }
        let resolved = rec
            .pages
            .get(index)
            .map(|p| p.split || p.gone)
            .ok_or_else(|| "页位越界".to_string())?;
        let (grouped_units, done) = if resolved {
            self.lazy_advance(&mut rec)?
        } else if self.get_unit(&rec.pages[index].unit_id).is_err() {
            rec.pages[index].gone = true;
            self.lazy_advance(&mut rec)?
        } else {
            let deck_bytes = self.file_get(&rec.deck_hash)?;
            let pkg = Package::load(&deck_bytes)?;
            let single = pptx::split_one(&pkg, index)?;
            let single_slides = pptx::slides(&single)?;
            if single_slides.len() != 1
                || single_slides[0].fingerprint != rec.pages[index].fp
            {
                // The per-page gate failed: the page is NOT
                // admitted, and the import is marked failed —
                // the same terminal handling as the deck-level
                // gate — instead of leaving the worker to retry
                // a page that can never pass.
                rec.state = "failed".to_string();
                rec.error = "拆分校验未通过：单页片段与源页不一致".to_string();
                (Vec::new(), false)
            } else {
                let single_bytes = single.to_bytes()?;
                let fp = rec.pages[index].fp.clone();
                let title = rec.pages[index].title.clone();
                let version_id = rec.pages[index].version_id.clone();
                let entity = self.register_page(&fp, &title, &single_bytes)?;
                self.set_version_fragment_hash(&version_id, &entity.hash)?;
                rec.pages[index].split = true;
                self.lazy_advance(&mut rec)?
            }
        };
        let failed = rec.state == "failed";
        let error = rec.error.clone();
        let split_count = rec.pages.iter().filter(|p| p.split || p.gone).count();
        let slide_count = rec.slide_count;
        if done {
            self.delete_pending_import(&rec.id)?;
        } else {
            self.update_pending_import(&rec)?;
        }
        Ok(LazySplitOutcome {
            pending_id: rec.id.clone(),
            index,
            split_count,
            slide_count,
            grouped_units,
            done,
            failed,
            error,
        })
    }

    /// Every pending import's current split state (the worker's
    /// work list and the client's resume source at startup).
    pub fn lazy_states(&self) -> Result<Vec<LazyState>, String> {
        Ok(self
            .list_pending_imports()?
            .iter()
            .map(lazy_state_of)
            .collect())
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
                                    && slots.iter().all(|tok| {
                                        // Per token (audit N5):
                                        // a pg_ token expands to
                                        // EVERY position of that
                                        // entity, so comparing the
                                        // expanded count with the
                                        // token count let a
                                        // duplicated page mask a
                                        // token that no longer
                                        // resolves at all.
                                        !crate::store::excerpt_positions(
                                            source,
                                            std::slice::from_ref(tok),
                                        )
                                        .is_empty()
                                    })
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

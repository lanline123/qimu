//! Storage layer: SQLite metadata + content-hash file store, in one
//! portable data directory (spec §8: backup = copy the directory).
//!
//! Visibility rule (spec §7): a non-admin user only ever sees content whose
//! group they hold at least one capability on; everything else does not
//! exist as far as queries are concerned.

use crate::auth::{hash_password, new_id, new_token, now_ms, verify_password};
use crate::model::IndexEntry;
use crate::pptx::Package;
use crate::semver::SemVer;
use crate::text::{slide_desc, slide_full_text, slide_title};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashSet;
use std::path::{Path, PathBuf};

pub type SResult<T> = Result<T, String>;

/// The five sidebar folder trees (sidebar isolation, user ruling
/// 2026-11-24). A root folder's scope must be one of these;
/// anything else is refused instead of minting a folder that no
/// sidebar lists (audit round-2 N4).
pub const FOLDER_SCOPES: [&str; 5] = ["showroom", "library", "inbox", "content", "index"];

fn validate_folder_scope(scope: &str) -> SResult<String> {
    if FOLDER_SCOPES.contains(&scope) {
        Ok(scope.to_string())
    } else {
        Err(format!("未知的文件夹作用域：{scope}"))
    }
}

/// Attachment filenames are display names that clients save back
/// to disk on download (audit L7): keep only the final path
/// component, drop control characters, and never let the stored
/// name be empty or a traversal spelling.
fn sanitize_attachment_filename(name: &str) -> String {
    let base = name
        .rsplit(|c| c == '/' || c == '\\')
        .next()
        .unwrap_or("")
        .chars()
        .filter(|c| !c.is_control())
        .collect::<String>();
    let trimmed = base.trim().trim_start_matches('.');
    let mut out: String = trimmed.chars().take(200).collect();
    if out.is_empty() {
        out = "未命名附件".to_string();
    }
    out
}

fn db_err(e: rusqlite::Error) -> String {
    // Audit2 F-N8: rusqlite's raw messages name tables, columns and
    // constraints — schema internals that API responses used to carry
    // verbatim to any connected client. Callers get a generic
    // database-error message; the original text stays available in
    // the process log (stderr) for diagnosis.
    eprintln!("[qimu] db error: {e}");
    "数据库操作失败".to_string()
}

fn parse_increment(value: &str) -> SResult<crate::semver::Increment> {
    match value {
        "major" => Ok(crate::semver::Increment::Major),
        "minor" => Ok(crate::semver::Increment::Minor),
        "patch" => Ok(crate::semver::Increment::Patch),
        _ => Err("未知的发布版本类型".into()),
    }
}

/// A real Argon2 hash of a throwaway password, computed once per
/// process, so `login_with_ip` can spend the same verification
/// cost on unknown usernames as on wrong passwords (audit M7
/// timing equalization).
fn dummy_hash() -> &'static str {
    static DUMMY: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    DUMMY.get_or_init(|| {
        hash_password("qimu-timing-equalizer")
            .unwrap_or_else(|_| "$argon2id$v=19$m=19456,t=2,p=1$AAAAAAAAAAAAAAAAAAAAAA$AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA".to_string())
    })
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserRec {
    pub id: String,
    pub username: String,
    pub display_name: String,
    pub dept_id: Option<String>,
    pub role: String, // super | dept_admin | member
    pub disabled: bool,
    /// First-login forced password change (audit round-2 H6):
    /// set on the seeded super admin when its password came from
    /// the built-in default. While set, the server refuses every
    /// named endpoint except password change, logout and `me`
    /// (see qimu-server's auth gate). Defaults to false so rows
    /// and payloads from before the flag existed read cleanly.
    #[serde(default)]
    pub must_change_password: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeptRec {
    pub id: String,
    pub name: String,
    pub parent_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FolderRec {
    pub id: String,
    pub name: String,
    pub parent_id: Option<String>,
    pub sort: i64,
    /// The folder's designated main index (主索引 = repo + main branch),
    /// spec §3.10. Points at an index whose home folder is this folder.
    #[serde(default)]
    pub main_index_id: Option<String>,
    /// Sidebar scope: which surface's folder tree this folder
    /// belongs to (showroom | library | inbox | content | index).
    /// Sidebar trees are isolated per surface (user ruling
    /// 2026-11-24): a folder is listed, created under, moved and
    /// renumbered only inside its own scope. Empty in rows from
    /// before scoping; Store::open assigns those a scope from
    /// homing evidence (see backfill_folder_scopes).
    #[serde(default)]
    pub scope: String,
    /// Sidebar folder color (user ruling 2026-11-24): a "#RRGGBB"
    /// hex string from the client's preset palette, or None for
    /// the default icon tint. The client dyes the folder's icon
    /// with it; the folder name keeps its normal color.
    #[serde(default)]
    pub color: Option<String>,
    /// The folder's free-text note （备注）: shown in small type
    /// beside the folder name on the content page's folder-flow
    /// view (user ruling 2026-11-24) and edited there in place.
    /// Empty when the folder carries no note.
    #[serde(default)]
    pub note: String,
    /// Where this folder row was born: "" for a folder created in
    /// this library, "server" for one pulled down from a server
    /// through the folder sync (user ruling 2026-11-25). A synced
    /// folder is a local copy from the moment it lands: it keeps
    /// the server's id (a folder's identity is its id — the name
    /// is just a detail on the id card and may repeat), it can be
    /// renamed and deleted locally without touching the server,
    /// and it survives disconnects and server-side deletions.
    /// The sidebar marks "server" rows with a small globe badge.
    #[serde(default)]
    pub origin: String,
}

/// How `Store::delete_folder` treats a folder's contents (user
/// ruling 2026-11-24: deleting a folder always offers the user
/// this exact choice).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum FolderDeleteMode {
    /// Delete only the folder row: child folders, content units
    /// and indexes homed in it are lifted to the folder's parent
    /// (contents of a root folder land at the root — folder-less,
    /// which the index page shows as 未分类).
    Lift,
    /// Delete the folder and its whole subtree: every content
    /// unit, index and showroom item homed in the subtree goes
    /// behind the trash overlay, then the folder rows are removed.
    Trash,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GroupRec {
    pub id: String,
    pub name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GrantRec {
    pub id: String,
    pub group_id: String,
    pub subject_type: String, // dept | user
    pub subject_id: String,
    pub capability: String, // view | use | publish | manage
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UnitRec {
    pub id: String,
    pub name: String,
    pub folder_id: Option<String>,
    pub group_id: String,
    pub maintainer: String,
    pub lead_lang: String,
    pub description: String,
    pub current_revision: i64,
    pub created_ms: i64,
    /// Wall order within the unit's home folder — the folders'
    /// `sort` caliber applied to units (card drag-and-drop,
    /// user ruling 2026-11-25): `list_units` orders by it and
    /// `move_units` renumbers it over the whole library, so a
    /// folder's cards keep exactly the order the wall shows.
    /// Rows from before the column existed all carry 0 and fall
    /// back to the historical name order until the first move.
    #[serde(default)]
    pub sort: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RenditionRec {
    pub id: String,
    pub unit_id: String,
    pub form: String,
    pub lang: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PageInfo {
    /// Page entity id: the page's stable identity, shared by every
    /// version whose reference sequence includes this page. Derived
    /// from the content fingerprint (see `page_id_for_fp`), so the
    /// same page content is the same entity in every version, block
    /// and store — inserting or deleting sibling pages never shifts
    /// it. Empty in version rows written before page entity-ization;
    /// Store::open's migration fills those in.
    #[serde(default)]
    pub id: String,
    pub fp: String,
    pub title: String,
    /// The page's one-line content summary (the prototype's
    /// per-page `d`): the first visible-text paragraph after
    /// the title, extracted at import / version creation (see
    /// `crate::text::slide_desc`). Empty when the page carries
    /// no body text — and on every row written before this
    /// field existed (serde default), which the UI presents
    /// as the prototype's empty state rather than a stand-in.
    #[serde(default)]
    pub desc: String,
}

/// The page entity id for a content fingerprint. Pages are content,
/// so identity derives from content: `pg_` + the fingerprint's
/// first 32 hex chars (128 bits of the SHA-256 the fingerprint
/// already is). The prefix keeps page ids unmistakably distinct
/// from legacy numeric excerpt tokens and from other id families.
pub fn page_id_for_fp(fp: &str) -> String {
    let take = fp.len().min(32);
    format!("pg_{}", &fp[..take])
}

/// One page entity: a single slide as an independently stored,
/// independently referenceable unit of content. The entity's bytes
/// are a single-page PPTX package in the file store (`hash`); the
/// first package registered for a fingerprint is canonical — later
/// packages with the same fingerprint are the same page by the
/// system's own change-detection definition (page_fingerprint) and
/// do not replace it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PageRec {
    pub id: String,
    pub fp: String,
    pub hash: String,
    pub title: String,
    /// The page's one-line content summary as first registered
    /// (see PageInfo::desc). First registration is canonical,
    /// so a stock page keeps the empty desc it was born with.
    pub desc: String,
}

/// One page of a deck whose import is still splitting (lazy
/// import, user ruling 2026-11-25): the wrapper unit presenting
/// the page from the moment the deck was opened, and whether the
/// background split has turned the page into a registered entity
/// yet (`split`) or the wrapper vanished mid-split (`gone` — the
/// page is then skipped by grouping and counts as resolved for
/// progress).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PendingPage {
    pub index: usize,
    pub fp: String,
    pub title: String,
    /// The page's one-line summary (PageInfo::desc), extracted
    /// in the open pass; serde-defaulted so pending rows written
    /// before this field existed still parse (desc stays empty).
    #[serde(default)]
    pub desc: String,
    pub text: String,
    pub script: String,
    pub unit_id: String,
    pub rendition_id: String,
    pub version_id: String,
    pub semver: String,
    pub split: bool,
    pub gone: bool,
}

/// A deck that has been opened but not fully split (lazy import):
/// the whole deck file sits in the file store under `deck_hash`
/// as the pages' backing, and this row is the split's durable
/// state — it survives restarts, so an interrupted split resumes
/// where it stopped. The row is deleted when the split completes
/// (`state` only ever reads "splitting" or "failed" on disk).
/// `verified` records the deck-level half of the §3.1 gate (the
/// split–reassemble equivalence over all pages), run when the
/// last page splits; the per-page half runs inside each page's
/// split. `grouped` holds the positions (in the effective block
/// spec list) whose blocks have already been folded.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PendingImportRec {
    pub id: String,
    pub filename: String,
    pub stem: String,
    pub deck_hash: String,
    pub folder_id: Option<String>,
    pub group_id: String,
    pub author: String,
    pub slide_count: usize,
    pub sections: Option<Vec<crate::pptx::SectionSpan>>,
    pub pages: Vec<PendingPage>,
    pub blocks: Option<Vec<crate::import::ImportBlockSpec>>,
    pub grouped: Vec<usize>,
    pub verified: Option<bool>,
    pub state: String,
    pub error: String,
    pub created_ms: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VersionRec {
    pub id: String,
    pub rendition_id: String,
    pub semver: String,
    pub basis_revision: i64,
    pub author: String,
    pub note: String,
    pub created_ms: i64,
    pub fragment_hash: String,
    pub page_count: i64,
    pub pages: Vec<PageInfo>,
    /// normal | recalled | draft | working | rejected. `working`
    /// is the publish-governance UNNUMBERED draft (user ruling
    /// 2026-11-25): semver '' with major/minor/patch = -1, at most
    /// one per rendition, converted in place to a numbered normal
    /// version when published or insert-numbered. `rejected` is
    /// the governance not-passed state; its reason rides the
    /// pub_* fields below.
    pub status: String,
    /// Per-page speaker scripts (讲稿), aligned with `pages`. Imported
    /// from the fragment's notes; editable without bumping the version.
    #[serde(default)]
    pub scripts: Vec<String>,
    /// Position in the rendition's version line when the user
    /// has arranged the line by dragging history cards (display
    /// order only — resolution never reads it). None while the
    /// line has never been arranged.
    #[serde(default)]
    pub line_pos: Option<i64>,
    /// Not-passed (未通过) record for status = "rejected": the
    /// reason, who rejected, and when (ms). Empty/0 otherwise.
    #[serde(default)]
    pub pub_reason: String,
    #[serde(default)]
    pub pub_by: String,
    #[serde(default)]
    pub pub_at: i64,
}

/// One entry of the renumber ledger (`version_renumbers`):
/// every governance renumber — a super admin's manual renumber,
/// a 定号插入 and each cascade shift it causes — leaves one of
/// these, so a version's number history is auditable even
/// though its identity (the row id) never changes.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RenumberRec {
    pub id: String,
    pub version_id: String,
    pub from_semver: String,
    pub to_semver: String,
    pub reason: String,
    pub by: String,
    pub at_ms: i64,
}

/// One explicit history language mount (`version_lang_mounts`):
/// the state a user cycled a history card's language chip to.
/// Absence of a row means the state is derived at render time.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LangMountRec {
    pub unit_id: String,
    pub version_id: String,
    pub lang: String,
    pub state: String, // synced | stale | missing
}

/// Display name of a language for the header status badges.
fn badge_lang_name(code: &str) -> String {
    match code {
        "en" => "英语".to_string(),
        "ja" => "日语".to_string(),
        "zh" => "中文".to_string(),
        "ko" => "韩语".to_string(),
        other => other.to_uppercase(),
    }
}

/// One unit embedded in another (see `Store::embedded_children`):
/// the child unit plus the units embedded in the child itself
/// (attached one level deep — the #101 kids zone only counts them
/// for its 「内含 N 个子块」 badge).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmbeddedChild {
    pub unit: UnitRec,
    pub grandchildren: Vec<UnitRec>,
}

/// First index at which `needle` occurs in `hay` as a contiguous
/// run, or None. Page-entity sequences are short (a block's page
/// count), so the naive window scan is the right tool.
fn subseq_pos(hay: &[String], needle: &[String]) -> Option<usize> {
    if needle.is_empty() || needle.len() > hay.len() {
        return None;
    }
    hay.windows(needle.len()).position(|w| w == needle)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IndexRec {
    pub id: String,
    pub name: String,
    pub folder_id: Option<String>,
    pub group_id: String,
    pub maintainer: String,
    pub lifecycle: String, // active | history | void
    pub scope: String,     // company | parent_org | dept_sub | dept | private
    pub created_ms: i64,
    /// Free-text note （备注）, edited in place in the index
    /// detail page's backbar (user ruling 2026-11-27: the index
    /// name and note are both double-click editable there, the
    /// same treatment folders got for the content page flow).
    pub note: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct MergeConflict {
    /// Entry key: "{unit_id}:{rendition_id}#{occurrence}".
    pub key: String,
    pub unit_id: String,
    pub rendition_id: String,
    pub base: Option<IndexEntry>,
    pub ours: Option<IndexEntry>,
    pub theirs: Option<IndexEntry>,
}

#[derive(Debug, Clone, Serialize)]
pub struct MergePreview {
    pub base_commit: Option<String>,
    pub from_head: String,
    pub into_head: String,
    pub clean: bool,
    /// Auto-merged entries (conflicts excluded until adjudicated).
    pub merged: Vec<IndexEntry>,
    pub conflicts: Vec<MergeConflict>,
}

/// Resolve an index entry's excerpt selection to page positions
/// in its resolved version. Excerpt tokens are page entity ids
/// (current writes); pre-entity commits carry decimal slot indices,
/// which keep resolving positionally — the compat read. A token
/// carrying the `pg_` prefix only ever matches page ids; a numeric
/// token only ever matches a position (page ids are never numeric),
/// so the two generations can never be confused. Positions come
/// back in token order, deduplicated; tokens that resolve to
/// nothing (a followed version whose page was edited into a new
/// entity, an out-of-range legacy index) select nothing — an
/// excerpt pins exact pages, not positions, once it is written
/// with ids.
/// SemVer components are u64 in the model but i64 columns in
/// storage (audit L7): saturate instead of wrapping to a
/// negative, which would corrupt every ordering over versions.
fn semver_part(v: u64) -> i64 {
    i64::try_from(v).unwrap_or(i64::MAX)
}

/// Whether a stored SHA-256 hex digest has exactly the shape
/// `file_put` produces (64 lowercase hex chars). Paths built
/// from a hash are only ever safe to touch when this holds
/// (audit H1/M1).
fn is_sha256_hex(h: &str) -> bool {
    h.len() == 64 && h.bytes().all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
}

pub fn excerpt_positions(version: &VersionRec, tokens: &[String]) -> Vec<usize> {
    let mut chosen: Vec<usize> = Vec::new();
    for tok in tokens {
        if tok.starts_with("pg_") {
            for (i, p) in version.pages.iter().enumerate() {
                if p.id == *tok && !chosen.contains(&i) {
                    chosen.push(i);
                }
            }
        } else if let Ok(n) = tok.parse::<usize>() {
            if n < version.pages.len() && !chosen.contains(&n) {
                chosen.push(n);
            }
        }
    }
    chosen
}

/// Occurrence-suffixed keys so the same block used twice in one index
/// merges instance by instance.
fn entry_keys(entries: &[IndexEntry]) -> Vec<String> {
    let mut counts: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    entries
        .iter()
        .map(|e| {
            let base = format!("{}:{}", e.unit_id.0, e.rendition_id.0);
            let n = counts.entry(base.clone()).or_default();
            let key = format!("{base}#{n}");
            *n += 1;
            key
        })
        .collect()
}

/// The three-way merge itself. `resolutions` maps a conflict key to
/// "ours" or "theirs"; without a resolution the entry stays a conflict.
fn compute_merge(
    base: &[IndexEntry],
    ours: &[IndexEntry],
    theirs: &[IndexEntry],
    resolutions: Option<&std::collections::HashMap<String, String>>,
) -> (Vec<IndexEntry>, Vec<MergeConflict>) {
    use std::collections::HashMap;
    let (bk, ok, tk) = (entry_keys(base), entry_keys(ours), entry_keys(theirs));
    let bmap: HashMap<&str, &IndexEntry> = bk.iter().map(|k| k.as_str()).zip(base.iter()).collect();
    let omap: HashMap<&str, &IndexEntry> = ok.iter().map(|k| k.as_str()).zip(ours.iter()).collect();
    let tmap: HashMap<&str, &IndexEntry> =
        tk.iter().map(|k| k.as_str()).zip(theirs.iter()).collect();

    let mut decided: HashMap<String, Option<IndexEntry>> = HashMap::new();
    let mut conflicts: Vec<MergeConflict> = Vec::new();
    let mut all_keys: Vec<String> = ok.clone();
    for k in tk.iter().chain(bk.iter()) {
        if !all_keys.contains(k) {
            all_keys.push(k.clone());
        }
    }
    for key in &all_keys {
        let (b, o, t) = (
            bmap.get(key.as_str()).copied(),
            omap.get(key.as_str()).copied(),
            tmap.get(key.as_str()).copied(),
        );
        let decision = if o == t {
            o.cloned()
        } else if b == o {
            t.cloned()
        } else if b == t {
            o.cloned()
        } else {
            match resolutions
                .and_then(|r| r.get(key.as_str()))
                .map(|s| s.as_str())
            {
                Some("ours") => o.cloned(),
                Some("theirs") => t.cloned(),
                _ => {
                    let (unit_id, rendition_id) = key
                        .split_once(':')
                        .map(|(u, rest)| {
                            (
                                u.to_string(),
                                rest.split('#').next().unwrap_or("").to_string(),
                            )
                        })
                        .unwrap_or_default();
                    conflicts.push(MergeConflict {
                        key: key.clone(),
                        unit_id,
                        rendition_id,
                        base: b.cloned(),
                        ours: o.cloned(),
                        theirs: t.cloned(),
                    });
                    continue;
                }
            }
        };
        decided.insert(key.clone(), decision);
    }
    // Result order: the target branch's order first, then entries that
    // only exist on the source branch, in source order.
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut ordered = Vec::new();
    for k in ok.iter().chain(tk.iter()) {
        if seen.insert(k.to_string()) {
            if let Some(Some(e)) = decided.get(k.as_str()) {
                ordered.push(e.clone());
            }
        }
    }
    (ordered, conflicts)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommitRec {
    pub id: String,
    pub index_id: String,
    pub branch: String,
    pub parent_id: Option<String>,
    pub author: String,
    pub message: String,
    pub created_ms: i64,
    pub entries: Vec<IndexEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IndexBranchLineage {
    pub name: String,
    pub head_commit: Option<String>,
    pub source_index_id: Option<String>,
    pub source_branch: Option<String>,
    pub fork_commit_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IndexGraphCommit {
    pub id: String,
    pub branch: String,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IndexGraph {
    pub branches: Vec<IndexBranchLineage>,
    pub commits: Vec<IndexGraphCommit>,
    pub tags: Vec<IndexTagRec>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IndexTagRec {
    pub id: String,
    pub index_id: String,
    pub branch: String,
    pub name: String,
    pub commit_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExportRec {
    pub id: String,
    pub index_id: String,
    pub branch: String,
    pub commit_id: String,
    pub tag: Option<String>,
    pub user_id: String,
    pub created_ms: i64,
    pub deck_hash: String,
    pub page_count: i64,
    pub kind: String, // personal | public
    /// The manifest's deck_id + full manifest JSON, kept server-side so a
    /// deck whose embedded manifest was stripped by an editor can be
    /// recovered from the surviving deck-id token.
    #[serde(default)]
    pub deck_id: String,
    #[serde(default)]
    pub manifest_json: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AttachmentRec {
    pub id: String,
    pub parent_type: String, // unit | index
    pub parent_id: String,
    pub filename: String,
    pub hash: String,
    pub size: i64,
    pub uploader: String,
    pub created_ms: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VerTagRec {
    pub id: String,
    pub version_id: String,
    pub dim: String, // customer | scenario | free
    pub value: String,
    pub strength: String, // fits | exclusive (customer dim)
    pub author: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PageBindingRec {
    pub id: String,
    pub unit_id: String,
    pub from_version_id: String,
    pub from_slot: i64,
    pub to_version_id: String,
    pub to_slot: i64,
    /// manual | import_auto | ai
    pub kind: String,
    pub author: String,
    pub created_ms: i64,
    /// Page entity ids the slots address: the binding's addressing
    /// of record. Versions are immutable, so (version, slot) and
    /// (version, page id) name the same page forever; the ids are
    /// what survives as the addressing scheme (excerpt tokens made
    /// the same move). Filled at write time and by Store::open's
    /// migration for pre-entity rows; empty only when the bound
    /// version's page could not be resolved at migration time.
    #[serde(default)]
    pub from_page_id: String,
    #[serde(default)]
    pub to_page_id: String,
}

/// One row of the trash (soft-delete overlay). `kind` is the client
/// filter vocabulary: index | unit | showroom. `origin` is the
/// human-readable place the object lived (folder / source view),
/// shown in the trash as 「原位置」.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrashRec {
    pub id: String,
    pub kind: String,
    pub object_id: String,
    pub title: String,
    pub origin: String,
    pub deleted_by: String,
    pub deleted_ms: i64,
}

/// One unit's position in the wall order (undo support): its
/// home folder and its global `sort` seat. `move_units`
/// renumbers the whole library, so undoing a move restores a
/// full-library snapshot of these triples — restoring only the
/// dragged batch would leave every other card's seat at its
/// post-move value and scramble the groups the batch crossed.
#[derive(Debug, Clone)]
pub struct UnitPosition {
    pub id: String,
    pub folder_id: Option<String>,
    pub sort: i64,
}

/// One version row, verbatim (undo support): every column of
/// the versions table, with `pages_json` / `scripts_json` kept
/// as the stored strings so a snapshot compares byte-for-byte
/// with the row it was taken from (a VersionRec round-trip
/// would re-serialize the page list and could differ in
/// whitespace or field order from what an earlier writer left).
#[derive(Debug, Clone)]
pub struct VersionRowSnap {
    pub id: String,
    pub rendition_id: String,
    pub semver: String,
    pub major: i64,
    pub minor: i64,
    pub patch: i64,
    pub basis_revision: i64,
    pub author: String,
    pub note: String,
    pub created_ms: i64,
    pub fragment_hash: String,
    pub page_count: i64,
    pub pages_json: String,
    pub text_all: String,
    pub status: String,
    pub scripts_json: String,
}

/// Everything `delete_unit_rows` would remove for one unit
/// (undo support): the snapshot a merge-into-draft takes of its
/// source before the source dissolves, so the undo can
/// resurrect the unit exactly — same ids, same versions, same
/// bindings — instead of rebuilding an approximation. Page
/// entities and CAS files are not part of it: a dissolve never
/// touches them, so there is nothing to restore. Trash overlay
/// rows naming the unit or its versions ride along for the
/// same reason (a version trashed before the merge keeps its
/// row across the dissolve, and the resurrection restores it).
#[derive(Debug, Clone)]
pub struct UnitSnapshot {
    pub unit: UnitRec,
    pub renditions: Vec<RenditionRec>,
    pub versions: Vec<VersionRowSnap>,
    pub version_tags: Vec<VerTagRec>,
    pub bindings: Vec<PageBindingRec>,
    pub attachments: Vec<AttachmentRec>,
    pub trash_rows: Vec<TrashRec>,
}

/// One folder's tree position (undo support): parent and
/// `sort` seat within its sidebar scope. `move_folder`
/// renumbers only its own scope, so a scope-wide snapshot of
/// these triples is the exact inverse image of a folder move.
#[derive(Debug, Clone)]
pub struct FolderPosition {
    pub id: String,
    pub parent_id: Option<String>,
    pub sort: i64,
}

/// One object a folder delete's trash mode sends behind the
/// trash overlay (undo support): its kind and id, its title
/// (for refusal messages), and whether it already had a trash
/// row before the delete — those rows are the object's own
/// earlier delete, not this one's, and the undo must leave
/// them in place.
#[derive(Debug, Clone)]
pub struct FolderTrashContent {
    pub kind: String,
    pub object_id: String,
    pub title: String,
    pub had_trash_row: bool,
}

/// The pre-image of a `delete_folder` call (undo support):
/// everything the delete rewrites, captured before it runs.
/// Lift mode records the folder row plus the direct children /
/// units / indexes whose home the delete rewrites to the
/// folder's parent. Trash mode records the whole subtree's
/// folder rows plus the contents the delete trashes (the
/// contents themselves keep their rows — only the folder rows
/// and the new trash overlay rows change).
#[derive(Debug, Clone)]
pub struct FolderDeleteSnapshot {
    pub trash_mode: bool,
    pub root: FolderRec,
    pub subtree: Vec<FolderRec>,
    pub lifted_children: Vec<String>,
    pub lifted_units: Vec<String>,
    pub lifted_indexes: Vec<String>,
    pub contents: Vec<FolderTrashContent>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ShowroomRec {
    pub id: String,
    pub title: String,
    pub intro: String,
    pub source_index_id: Option<String>,
    pub commit_id: Option<String>,
    pub submitted_by: String,
    pub state: String, // pending | live | rejected
    pub created_ms: i64,
    /// Review outcome (approval queue): the reason recorded when the
    /// item was rejected (empty for approvals / not yet reviewed),
    /// who reviewed it and when. Defaults keep old rows readable.
    #[serde(default)]
    pub review_note: String,
    #[serde(default)]
    pub reviewed_by: String,
    #[serde(default)]
    pub reviewed_ms: i64,
    /// Visibility scope (org-model DESIGN §2.1). This phase only
    /// 'org_public' is produced or consumed; the remaining tiers
    /// are reserved vocabulary, not enabled behavior.
    pub scope: String,
}

/// The one scope tier this phase enables (org-model DESIGN §2.1):
/// content any connection of the organization — named or
/// anonymous — may read. Reserved, not yet produced or consumed:
/// `scoped:<dept_id>:<descendants>` (the subtree / node tiers),
/// `users:<id,id,...>`, `private`.
pub const SCOPE_ORG_PUBLIC: &str = "org_public";

/// Whether an anonymous (guest) session may read content carrying
/// this scope. Only the explicit org_public tier — a guest's
/// visible set is never derived from any position in the org tree
/// (DESIGN §2.3: the root-node reading would dissolve every
/// scoped tier).
/// Total byte size of a directory tree (0 when the path does not
/// exist). Used by the submission staging quota: the staging
/// directories are the staged truth, so usage is measured on disk
/// rather than trusted from manifests.
fn dir_tree_bytes(path: &std::path::Path) -> u64 {
    let mut total = 0u64;
    let mut stack = vec![path.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in rd.flatten() {
            let p = entry.path();
            if p.is_dir() {
                stack.push(p);
            } else if let Ok(md) = entry.metadata() {
                total += md.len();
            }
        }
    }
    total
}

pub fn scope_allows_guest(scope: &str) -> bool {
    scope == SCOPE_ORG_PUBLIC
}

/// Session lifetimes (org-model DESIGN §4.2; user-ruled
/// 2026-11-23): guest sessions live 24h from issue (the client
/// silently re-issues); named sessions expire after 30 days
/// idle, sliding — activity pushes the window forward.
pub const GUEST_TTL_MS: i64 = 24 * 3600 * 1000;
pub const USER_IDLE_TTL_MS: i64 = 30 * 24 * 3600 * 1000;
const SESSION_TOUCH_MS: i64 = 60 * 1000;

/// One row of the submissions table (local-first P3, design §6.3):
/// a real-byte submission waiting in the server's staging zone.
/// The package's content files live outside the landed store, in
/// `<data root>/staging/<id>/`; the manifest rides in the row
/// (read back via `submission_manifest`, deliberately not part of
/// this record so list responses stay small). A submission is NOT
/// landed content: no units / indexes / versions / showroom rows
/// exist for it until `land_submission` runs at approval time.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubmissionRec {
    pub id: String,
    pub title: String,
    pub intro: String,
    pub submitter_id: String,
    pub submitter_name: String,
    pub state: String, // pending | landed | rejected | expired
    /// The submitter's own note riding with the package.
    pub note: String,
    /// Rejection reason, returned to the submitter verbatim.
    #[serde(default)]
    pub review_note: String,
    #[serde(default)]
    pub reviewed_by: String,
    #[serde(default)]
    pub reviewed_ms: i64,
    pub created_ms: i64,
    /// The rejected submission this one replaces (resubmission).
    pub supersedes: Option<String>,
    /// The submitter-side (local library) index id — provenance
    /// for the submitter's own client only; the server never
    /// resolves it against its own indexes.
    pub source_index_id: Option<String>,
    pub landed_index_id: Option<String>,
    pub landed_showroom_id: Option<String>,
    pub entry_count: i64,
    pub page_count: i64,
    /// Last landing failure, recorded when an approval's landing
    /// transaction rolled back (the submission stays pending).
    #[serde(default)]
    pub land_error: String,
}

/// One message-center row (full version, user ruling 2026-11-25).
/// The same table shape serves both stores: on the SERVER it is
/// the notification stream written at workflow transition points
/// (`source = 'server'`, `read` unused — always 0); in a client's
/// LOCAL library it is the mirror the message center renders —
/// pulled server rows copied in with their server ids, plus
/// `source = 'local'` rows for this machine's own operation
/// feedback (kind `system`). `read` is managed only on the local
/// side (C3: read state never leaves the machine).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NotificationRec {
    pub id: String,
    /// Recipient user id (server rows). Local system rows carry
    /// the signed-in user id at creation time, or "" when the
    /// action ran logged-out; the local list face shows local
    /// rows regardless of this field.
    pub user_id: String,
    /// rejected | approved | pending_review | expired | offline
    /// | system
    pub kind: String,
    pub title: String,
    #[serde(default)]
    pub body: String,
    /// The business object the message is about ("" when none):
    /// ref_kind is "submission" | "showroom" | "".
    #[serde(default)]
    pub ref_kind: String,
    #[serde(default)]
    pub ref_id: String,
    pub created_ms: i64,
    #[serde(default)]
    pub read: i64,
    /// server | local
    #[serde(default)]
    pub source: String,
}

/// The content manifest of a submission package (design §6.3):
/// one local index pinned at one commit, with every referenced
/// unit / rendition / version's metadata. Fragment bytes travel
/// separately, keyed by SHA-256 hash (the `files` map of the
/// upload), so the manifest itself stays small and reviewable.
/// All ids are the *submitter's local* ids; landing re-mints
/// server ids and records the mapping on the submission row
/// (design §9 R4).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubmissionManifest {
    pub format: u32,
    pub index_name: String,
    pub index_scope: String,
    pub commit: SubmissionCommit,
    pub units: Vec<SubmissionUnit>,
    pub renditions: Vec<SubmissionRendition>,
    pub versions: Vec<SubmissionVersion>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubmissionCommit {
    pub branch: String,
    pub message: String,
    pub entries: Vec<SubmissionEntry>,
}

/// One commit entry plus the version it resolved to in the
/// submitter's library — the exact version whose fragment the
/// package ships for this entry (the same resolution the export
/// path performs).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubmissionEntry {
    pub entry: IndexEntry,
    pub version_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubmissionUnit {
    pub id: String,
    pub name: String,
    pub lead_lang: String,
    pub description: String,
    pub current_revision: i64,
    pub maintainer: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubmissionRendition {
    pub id: String,
    pub unit_id: String,
    pub form: String,
    pub lang: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubmissionVersion {
    pub id: String,
    pub rendition_id: String,
    pub semver: String,
    pub basis_revision: i64,
    pub author: String,
    pub note: String,
    pub status: String,
    pub created_ms: i64,
    pub fragment_hash: String,
    pub page_count: i64,
    /// Per-page speaker scripts, aligned with the fragment's pages.
    pub scripts: Vec<String>,
}

/// The content group submission landings rebuild into. Created on
/// first landing with view/use grants for every root department
/// (so landed content is visible in the org's material faces, not
/// only through the showroom); grants are issued only at creation
/// time and never re-asserted afterwards.
pub const LANDING_GROUP_NAME: &str = "演示厅落地";

/// One row of the sessions table (org-model G1): named user
/// sessions and anonymous guest sessions share the table and the
/// opaque-token form; `kind` is the only distinction, and a guest
/// row's user_id is NULL — never a fake user row, which would
/// poison every authorization walk over the users table (§4.2).
#[derive(Debug, Clone)]
pub struct SessionRec {
    pub kind: String, // user | guest
    pub user_id: Option<String>,
    pub client_ip: Option<String>,
    pub created_ms: i64,
    pub last_seen_ms: i64,
}

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS departments(id TEXT PRIMARY KEY, name TEXT NOT NULL, parent_id TEXT);
CREATE TABLE IF NOT EXISTS users(id TEXT PRIMARY KEY, username TEXT UNIQUE NOT NULL, display_name TEXT NOT NULL, password_hash TEXT NOT NULL, dept_id TEXT, role TEXT NOT NULL DEFAULT 'member', disabled INTEGER NOT NULL DEFAULT 0, must_change_password INTEGER NOT NULL DEFAULT 0);
CREATE TABLE IF NOT EXISTS user_depts(user_id TEXT NOT NULL, dept_id TEXT NOT NULL, PRIMARY KEY(user_id, dept_id));
CREATE TABLE IF NOT EXISTS sessions(token TEXT PRIMARY KEY, user_id TEXT, created_ms INTEGER NOT NULL, kind TEXT NOT NULL DEFAULT 'user', client_ip TEXT, last_seen_ms INTEGER NOT NULL DEFAULT 0);
CREATE TABLE IF NOT EXISTS folders(id TEXT PRIMARY KEY, name TEXT NOT NULL, parent_id TEXT, sort INTEGER NOT NULL DEFAULT 0, main_index_id TEXT, scope TEXT NOT NULL DEFAULT '', color TEXT, note TEXT NOT NULL DEFAULT '', origin TEXT NOT NULL DEFAULT '');
CREATE TABLE IF NOT EXISTS groups_tbl(id TEXT PRIMARY KEY, name TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS group_grants(id TEXT PRIMARY KEY, group_id TEXT NOT NULL, subject_type TEXT NOT NULL, subject_id TEXT NOT NULL, capability TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS units(id TEXT PRIMARY KEY, name TEXT NOT NULL, folder_id TEXT, group_id TEXT NOT NULL, maintainer TEXT NOT NULL, lead_lang TEXT NOT NULL DEFAULT 'zh', description TEXT NOT NULL DEFAULT '', current_revision INTEGER NOT NULL DEFAULT 1, created_ms INTEGER NOT NULL, sort INTEGER NOT NULL DEFAULT 0);
CREATE TABLE IF NOT EXISTS renditions(id TEXT PRIMARY KEY, unit_id TEXT NOT NULL, form TEXT NOT NULL, lang TEXT NOT NULL, UNIQUE(unit_id, form, lang));
CREATE TABLE IF NOT EXISTS versions(id TEXT PRIMARY KEY, rendition_id TEXT NOT NULL, semver TEXT NOT NULL, major INTEGER NOT NULL, minor INTEGER NOT NULL, patch INTEGER NOT NULL, basis_revision INTEGER NOT NULL, author TEXT NOT NULL, note TEXT NOT NULL DEFAULT '', created_ms INTEGER NOT NULL, fragment_hash TEXT NOT NULL, page_count INTEGER NOT NULL, pages_json TEXT NOT NULL, text_all TEXT NOT NULL DEFAULT '', status TEXT NOT NULL DEFAULT 'normal', scripts_json TEXT NOT NULL DEFAULT '[]', line_pos INTEGER, pub_reason TEXT NOT NULL DEFAULT '', pub_by TEXT NOT NULL DEFAULT '', pub_at INTEGER NOT NULL DEFAULT 0);
CREATE TABLE IF NOT EXISTS version_renumbers(id TEXT PRIMARY KEY, version_id TEXT NOT NULL, from_semver TEXT NOT NULL DEFAULT '', to_semver TEXT NOT NULL, reason TEXT NOT NULL DEFAULT '', by TEXT NOT NULL DEFAULT '', at_ms INTEGER NOT NULL DEFAULT 0);
CREATE TABLE IF NOT EXISTS version_use_sel(unit_id TEXT NOT NULL, rendition_id TEXT NOT NULL, version_id TEXT NOT NULL, PRIMARY KEY(unit_id, rendition_id));
CREATE TABLE IF NOT EXISTS version_lang_mounts(unit_id TEXT NOT NULL, version_id TEXT NOT NULL, lang TEXT NOT NULL, state TEXT NOT NULL, updated_ms INTEGER NOT NULL DEFAULT 0, PRIMARY KEY(unit_id, version_id, lang));
CREATE TABLE IF NOT EXISTS indexes_tbl(id TEXT PRIMARY KEY, name TEXT NOT NULL, folder_id TEXT, group_id TEXT NOT NULL, maintainer TEXT NOT NULL, lifecycle TEXT NOT NULL DEFAULT 'active', scope TEXT NOT NULL DEFAULT 'company', created_ms INTEGER NOT NULL, note TEXT NOT NULL DEFAULT '');
CREATE TABLE IF NOT EXISTS index_branches(index_id TEXT NOT NULL, name TEXT NOT NULL, head_commit TEXT, PRIMARY KEY(index_id, name));
CREATE TABLE IF NOT EXISTS index_commits(id TEXT PRIMARY KEY, index_id TEXT NOT NULL, branch TEXT NOT NULL, parent_id TEXT, author TEXT NOT NULL, message TEXT NOT NULL, created_ms INTEGER NOT NULL, entries_json TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS index_tags(id TEXT PRIMARY KEY, index_id TEXT NOT NULL, branch TEXT NOT NULL, name TEXT NOT NULL, commit_id TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS exports(id TEXT PRIMARY KEY, index_id TEXT NOT NULL, branch TEXT NOT NULL, commit_id TEXT NOT NULL, tag TEXT, user_id TEXT NOT NULL, created_ms INTEGER NOT NULL, deck_hash TEXT NOT NULL, page_count INTEGER NOT NULL, kind TEXT NOT NULL DEFAULT 'personal', deck_id TEXT NOT NULL DEFAULT '', manifest_json TEXT NOT NULL DEFAULT '');
CREATE TABLE IF NOT EXISTS attachments(id TEXT PRIMARY KEY, parent_type TEXT NOT NULL, parent_id TEXT NOT NULL, filename TEXT NOT NULL, hash TEXT NOT NULL, size INTEGER NOT NULL, uploader TEXT NOT NULL, created_ms INTEGER NOT NULL);
CREATE TABLE IF NOT EXISTS version_tags(id TEXT PRIMARY KEY, version_id TEXT NOT NULL, dim TEXT NOT NULL, value TEXT NOT NULL, strength TEXT NOT NULL DEFAULT 'fits', author TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS showroom(id TEXT PRIMARY KEY, title TEXT NOT NULL, intro TEXT NOT NULL DEFAULT '', source_index_id TEXT, commit_id TEXT, submitted_by TEXT NOT NULL, state TEXT NOT NULL DEFAULT 'pending', created_ms INTEGER NOT NULL, scope TEXT NOT NULL DEFAULT 'org_public');
CREATE TABLE IF NOT EXISTS showroom_pins(showroom_id TEXT NOT NULL, pos INTEGER NOT NULL, version_id TEXT NOT NULL, commit_id TEXT NOT NULL DEFAULT '', PRIMARY KEY(showroom_id, pos));
CREATE TABLE IF NOT EXISTS submissions(id TEXT PRIMARY KEY, title TEXT NOT NULL, intro TEXT NOT NULL DEFAULT '', submitter_id TEXT NOT NULL, submitter_name TEXT NOT NULL, state TEXT NOT NULL DEFAULT 'pending', note TEXT NOT NULL DEFAULT '', review_note TEXT NOT NULL DEFAULT '', reviewed_by TEXT NOT NULL DEFAULT '', reviewed_ms INTEGER NOT NULL DEFAULT 0, created_ms INTEGER NOT NULL, supersedes TEXT, source_index_id TEXT, landed_index_id TEXT, landed_showroom_id TEXT, entry_count INTEGER NOT NULL DEFAULT 0, page_count INTEGER NOT NULL DEFAULT 0, land_error TEXT NOT NULL DEFAULT '', manifest_json TEXT NOT NULL DEFAULT '');
CREATE TABLE IF NOT EXISTS trash_items(id TEXT PRIMARY KEY, kind TEXT NOT NULL, object_id TEXT NOT NULL, title TEXT NOT NULL DEFAULT '', origin TEXT NOT NULL DEFAULT '', deleted_by TEXT NOT NULL DEFAULT '', deleted_ms INTEGER NOT NULL DEFAULT 0, UNIQUE(kind, object_id));
CREATE TABLE IF NOT EXISTS assemblies(id TEXT PRIMARY KEY, user_id TEXT NOT NULL, name TEXT NOT NULL, entries_json TEXT NOT NULL, created_ms INTEGER NOT NULL);
CREATE TABLE IF NOT EXISTS page_bindings(id TEXT PRIMARY KEY, unit_id TEXT NOT NULL, from_version_id TEXT NOT NULL, from_slot INTEGER NOT NULL, to_version_id TEXT NOT NULL, to_slot INTEGER NOT NULL, kind TEXT NOT NULL DEFAULT 'manual', author TEXT NOT NULL, created_ms INTEGER NOT NULL);
CREATE TABLE IF NOT EXISTS pages(id TEXT PRIMARY KEY, fp TEXT NOT NULL, hash TEXT NOT NULL, title TEXT NOT NULL DEFAULT '', desc TEXT NOT NULL DEFAULT '');
CREATE TABLE IF NOT EXISTS custom_groups(id TEXT PRIMARY KEY, name TEXT NOT NULL, owner_id TEXT NOT NULL, created_ms INTEGER NOT NULL);
CREATE TABLE IF NOT EXISTS custom_group_members(group_id TEXT NOT NULL, user_id TEXT NOT NULL, PRIMARY KEY(group_id, user_id));
CREATE TABLE IF NOT EXISTS shares(id TEXT PRIMARY KEY, item_kind TEXT NOT NULL, item_id TEXT NOT NULL, title TEXT NOT NULL, from_user TEXT NOT NULL, target_kind TEXT NOT NULL, target_id TEXT NOT NULL, note TEXT NOT NULL DEFAULT '', created_ms INTEGER NOT NULL);
CREATE TABLE IF NOT EXISTS library_entries(id TEXT PRIMARY KEY, user_id TEXT NOT NULL, item_kind TEXT NOT NULL, item_id TEXT NOT NULL, title TEXT NOT NULL, folder TEXT NOT NULL DEFAULT '', added_ms INTEGER NOT NULL);
CREATE TABLE IF NOT EXISTS profile_fields(user_id TEXT NOT NULL, field TEXT NOT NULL, value TEXT NOT NULL, PRIMARY KEY(user_id, field));
CREATE TABLE IF NOT EXISTS company_profile(field TEXT PRIMARY KEY, value TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS settings(key TEXT PRIMARY KEY, value TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS pending_imports(id TEXT PRIMARY KEY, filename TEXT NOT NULL, stem TEXT NOT NULL DEFAULT '', deck_hash TEXT NOT NULL, folder_id TEXT, group_id TEXT NOT NULL DEFAULT '', author TEXT NOT NULL DEFAULT '', slide_count INTEGER NOT NULL DEFAULT 0, sections_json TEXT NOT NULL DEFAULT '', pages_json TEXT NOT NULL DEFAULT '[]', blocks_json TEXT NOT NULL DEFAULT '', grouped_json TEXT NOT NULL DEFAULT '[]', verified INTEGER, state TEXT NOT NULL DEFAULT 'splitting', error TEXT NOT NULL DEFAULT '', created_ms INTEGER NOT NULL DEFAULT 0);
CREATE TABLE IF NOT EXISTS notifications(id TEXT PRIMARY KEY, user_id TEXT NOT NULL, kind TEXT NOT NULL, title TEXT NOT NULL, body TEXT NOT NULL DEFAULT '', ref_kind TEXT NOT NULL DEFAULT '', ref_id TEXT NOT NULL DEFAULT '', created_ms INTEGER NOT NULL, read INTEGER NOT NULL DEFAULT 0, source TEXT NOT NULL DEFAULT 'server');
CREATE TABLE IF NOT EXISTS notification_tombstones(server_id TEXT PRIMARY KEY, deleted_ms INTEGER NOT NULL DEFAULT 0);
";

pub struct Store {
    conn: Connection,
    root: PathBuf,
}

impl Store {
    pub fn open(root: &Path) -> SResult<Store> {
        std::fs::create_dir_all(root.join("files")).map_err(|e| e.to_string())?;
        let conn = Connection::open(root.join("qimu.db")).map_err(db_err)?;
        // Concurrency posture (audit L5): several processes may
        // legitimately open one data directory (the server, the
        // CLI, a second Store in-process). Without a busy timeout
        // every write contention failed instantly with "database
        // is locked"; WAL lets readers and one writer coexist.
        conn.busy_timeout(std::time::Duration::from_secs(5))
            .map_err(db_err)?;
        let _ = conn.pragma_update(None, "journal_mode", "WAL");
        conn.execute_batch(SCHEMA).map_err(db_err)?;
        // Version-number uniqueness (audit L1): one row per
        // (rendition, semver). Best-effort index — a legacy
        // database that already carries duplicate numbers keeps
        // opening (the index simply is not created there); the
        // insert paths enforce the same rule in code, so new
        // duplicates cannot form either way.
        let _ = conn.execute_batch(
            "CREATE UNIQUE INDEX IF NOT EXISTS idx_versions_rendition_semver ON versions(rendition_id, major, minor, patch);",
        );
        // Sessions rebuild (org-model G1): guest sessions carry a
        // NULL user_id, which the original `user_id TEXT NOT NULL`
        // shape forbids, so a pre-G1 sessions table is rebuilt in
        // the new shape. Rows are preserved as kind='user';
        // last_seen_ms starts at the migration moment, so no
        // existing login is cut off by the new 30-day idle expiry
        // the day it lands — the window simply starts at upgrade.
        {
            let user_id_notnull: i64 = conn
                .query_row(
                    "SELECT \"notnull\" FROM pragma_table_info('sessions') WHERE name='user_id'",
                    [],
                    |r| r.get(0),
                )
                .unwrap_or(0);
            if user_id_notnull == 1 {
                // One transaction (audit L9): the previous
                // batch-by-batch rebuild could lose the whole
                // sessions table to a crash between DROP and
                // RENAME.
                let tx = conn.unchecked_transaction().map_err(db_err)?;
                tx.execute_batch(
                    "CREATE TABLE sessions_new(token TEXT PRIMARY KEY, user_id TEXT, created_ms INTEGER NOT NULL, kind TEXT NOT NULL DEFAULT 'user', client_ip TEXT, last_seen_ms INTEGER NOT NULL DEFAULT 0);",
                )
                .map_err(db_err)?;
                tx.execute(
                    "INSERT INTO sessions_new(token, user_id, created_ms, kind, client_ip, last_seen_ms) SELECT token, user_id, created_ms, 'user', NULL, ?1 FROM sessions",
                    params![now_ms()],
                )
                .map_err(db_err)?;
                tx.execute_batch(
                    "DROP TABLE sessions; ALTER TABLE sessions_new RENAME TO sessions;",
                )
                .map_err(db_err)?;
                tx.commit().map_err(db_err)?;
            }
        }
        // migrations for databases created before these columns existed
        for (table, col, decl) in [
            ("exports", "deck_id", "TEXT NOT NULL DEFAULT ''"),
            ("exports", "manifest_json", "TEXT NOT NULL DEFAULT ''"),
            ("versions", "scripts_json", "TEXT NOT NULL DEFAULT '[]'"),
            ("folders", "main_index_id", "TEXT"),
            ("folders", "scope", "TEXT NOT NULL DEFAULT ''"),
            ("folders", "color", "TEXT"),
            ("folders", "note", "TEXT NOT NULL DEFAULT ''"),
            ("indexes_tbl", "note", "TEXT NOT NULL DEFAULT ''"),
            ("folders", "origin", "TEXT NOT NULL DEFAULT ''"),
            ("index_branches", "source_index_id", "TEXT"),
            ("index_branches", "source_branch", "TEXT"),
            ("index_branches", "fork_commit_id", "TEXT"),
            ("showroom", "scope", "TEXT NOT NULL DEFAULT 'org_public'"),
            ("sessions", "kind", "TEXT NOT NULL DEFAULT 'user'"),
            ("sessions", "client_ip", "TEXT"),
            ("sessions", "last_seen_ms", "INTEGER NOT NULL DEFAULT 0"),
            ("showroom", "review_note", "TEXT NOT NULL DEFAULT ''"),
            ("showroom", "reviewed_by", "TEXT NOT NULL DEFAULT ''"),
            ("showroom", "reviewed_ms", "INTEGER NOT NULL DEFAULT 0"),
            ("shares", "note", "TEXT NOT NULL DEFAULT ''"),
            ("page_bindings", "from_page_id", "TEXT NOT NULL DEFAULT ''"),
            ("page_bindings", "to_page_id", "TEXT NOT NULL DEFAULT ''"),
            ("showroom_pins", "commit_id", "TEXT NOT NULL DEFAULT ''"),
            ("users", "must_change_password", "INTEGER NOT NULL DEFAULT 0"),
            ("pages", "desc", "TEXT NOT NULL DEFAULT ''"),
            ("units", "sort", "INTEGER NOT NULL DEFAULT 0"),
            ("versions", "line_pos", "INTEGER"),
            ("versions", "pub_reason", "TEXT NOT NULL DEFAULT ''"),
            ("versions", "pub_by", "TEXT NOT NULL DEFAULT ''"),
            ("versions", "pub_at", "INTEGER NOT NULL DEFAULT 0"),
        ] {
            let has: bool = conn
                .prepare(&format!("PRAGMA table_info({table})"))
                .and_then(|mut s| {
                    let rows = s.query_map([], |r| r.get::<_, String>(1))?;
                    Ok(rows
                        .collect::<Result<Vec<_>, _>>()?
                        .iter()
                        .any(|n| n == col))
                })
                .unwrap_or(false);
            if !has {
                conn.execute_batch(&format!("ALTER TABLE {table} ADD COLUMN {col} {decl}"))
                    .map_err(db_err)?;
            }
        }
        let store = Store {
            conn,
            root: root.to_path_buf(),
        };
        store.backfill_folder_scopes()?;
        // The page-entity migration is best-effort by design
        // (audit N6): a content-layer failure in one version, or
        // a transient DB error inside the migration, must never
        // make the whole library unopenable — un-migrated
        // versions keep reading through the fragment fallback.
        if let Err(e) = store.migrate_page_entities() {
            eprintln!("page entity migration skipped: {e}");
        }
        Ok(store)
    }

    /// One-time scope assignment for folder rows that predate
    /// sidebar isolation (scope = ''). Evidence first, over the
    /// folder's whole subtree: a subtree homing indexes belongs
    /// to the index tree, one homing content units to the
    /// content tree; a subtree homing both follows its folder's
    /// own weight (a designated main index or the majority kind,
    /// ties to index). A subtree homing nothing is assigned to
    /// the showroom tree — an empty folder holds no content, so
    /// nothing is lost whichever tree lists it. Rows are never
    /// deleted and only '' rows are touched, so a folder ends up
    /// in exactly one tree.
    fn backfill_folder_scopes(&self) -> SResult<()> {
        let folders = self.list_folders()?;
        if !folders.iter().any(|f| f.scope.is_empty()) {
            return Ok(());
        }
        let homes = |sql: &str| -> SResult<std::collections::HashMap<String, i64>> {
            let mut stmt = self.conn.prepare(sql).map_err(db_err)?;
            let rows = stmt
                .query_map([], |r| {
                    Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?))
                })
                .map_err(db_err)?
                .collect::<Result<Vec<_>, _>>()
                .map_err(db_err)?;
            Ok(rows.into_iter().collect())
        };
        let idx_homes = homes(
            "SELECT folder_id, COUNT(*) FROM indexes_tbl WHERE folder_id IS NOT NULL GROUP BY folder_id",
        )?;
        let unit_homes = homes(
            "SELECT folder_id, COUNT(*) FROM units WHERE folder_id IS NOT NULL GROUP BY folder_id",
        )?;
        let mut children: std::collections::HashMap<String, Vec<String>> =
            std::collections::HashMap::new();
        for f in &folders {
            if let Some(p) = &f.parent_id {
                children.entry(p.clone()).or_default().push(f.id.clone());
            }
        }
        // Subtree homing totals (indexes, units); the depth cap
        // keeps a broken parent cycle from looping forever.
        fn subtree_totals(
            id: &str,
            children: &std::collections::HashMap<String, Vec<String>>,
            idx_homes: &std::collections::HashMap<String, i64>,
            unit_homes: &std::collections::HashMap<String, i64>,
            depth: usize,
        ) -> (i64, i64) {
            let mut totals = (
                idx_homes.get(id).copied().unwrap_or(0),
                unit_homes.get(id).copied().unwrap_or(0),
            );
            if depth >= 64 {
                return totals;
            }
            if let Some(kids) = children.get(id) {
                for kid in kids {
                    let (i, u) =
                        subtree_totals(kid, children, idx_homes, unit_homes, depth + 1);
                    totals.0 += i;
                    totals.1 += u;
                }
            }
            totals
        }
        for f in &folders {
            if !f.scope.is_empty() {
                continue;
            }
            let (i, u) = subtree_totals(&f.id, &children, &idx_homes, &unit_homes, 0);
            let scope = match (i > 0, u > 0) {
                (true, false) => "index",
                (false, true) => "content",
                (true, true) => {
                    if f.main_index_id.is_some() || i >= u {
                        "index"
                    } else {
                        "content"
                    }
                }
                (false, false) => "showroom",
            };
            self.conn
                .execute(
                    "UPDATE folders SET scope=?2 WHERE id=?1 AND scope=''",
                    params![f.id, scope],
                )
                .map_err(db_err)?;
        }
        Ok(())
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    // ---------- file store ----------

    pub fn file_put(&self, bytes: &[u8]) -> SResult<String> {
        let mut h = Sha256::new();
        h.update(bytes);
        let hash = format!("{:x}", h.finalize());
        let path = self.file_path(&hash);
        if !path.exists() {
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
            }
            // Crash-safe write via craft-atomic (storytold/craft-libs,
            // MIT OR Apache-2.0): content-addressed readers never see a
            // partially written file.
            craft_atomic::atomic_write(&path, bytes).map_err(|e| e.to_string())?;
        }
        Ok(hash)
    }

    pub fn file_get(&self, hash: &str) -> SResult<Vec<u8>> {
        std::fs::read(self.file_path(hash)).map_err(|e| format!("file {hash}: {e}"))
    }

    pub fn file_path(&self, hash: &str) -> PathBuf {
        let sub = if hash.len() >= 2 { &hash[..2] } else { "00" };
        self.root
            .join("files")
            .join(sub)
            .join(format!("{hash}.bin"))
    }

    // ---------- page entities ----------

    /// Register a page entity from its single-page package bytes.
    /// Identity derives from the content fingerprint: if the page
    /// already exists, the existing record is returned and the new
    /// bytes are NOT stored (first registration is canonical — a
    /// same-fingerprint package is the same page). Otherwise the
    /// package goes into the file store and the row is created.
    pub fn register_page(
        &self,
        fp: &str,
        title: &str,
        desc: &str,
        single: &[u8],
    ) -> SResult<PageRec> {
        let id = page_id_for_fp(fp);
        if let Some(rec) = self.get_page(&id)? {
            return Ok(rec);
        }
        let hash = self.file_put(single)?;
        self.conn
            .execute(
                "INSERT OR IGNORE INTO pages(id, fp, hash, title, desc) VALUES(?1,?2,?3,?4,?5)",
                params![id, fp, hash, title, desc],
            )
            .map_err(db_err)?;
        self.get_page(&id)?.ok_or_else(|| "页实体登记失败".to_string())
    }

    pub fn get_page(&self, id: &str) -> SResult<Option<PageRec>> {
        self.conn
            .query_row(
                "SELECT id, fp, hash, title, desc FROM pages WHERE id=?1",
                params![id],
                |r| {
                    Ok(PageRec {
                        id: r.get(0)?,
                        fp: r.get(1)?,
                        hash: r.get(2)?,
                        title: r.get(3)?,
                        desc: r.get(4)?,
                    })
                },
            )
            .optional()
            .map_err(db_err)
    }

    /// The single-page package bytes of one of a version's pages,
    /// resolved through the page entity the version references.
    pub fn version_page_package(&self, version: &VersionRec, pos: usize) -> SResult<Package> {
        let info = version
            .pages
            .get(pos)
            .ok_or_else(|| "页位越界".to_string())?;
        if !info.id.is_empty() {
            if let Some(rec) = self.get_page(&info.id)? {
                let bytes = self.file_get(&rec.hash)?;
                return Package::load(&bytes);
            }
        }
        // Legacy fallback: a version whose pages carry no entity
        // ids (migration could not split its fragment) still reads
        // through its stored fragment.
        let bytes = self.file_get(&version.fragment_hash)?;
        let pkg = Package::load(&bytes)?;
        let singles = crate::pptx::split(&pkg)?;
        singles
            .into_iter()
            .nth(pos)
            .ok_or_else(|| "页位越界".to_string())
    }

    /// All of a version's pages as single-page packages, in the
    /// version's reference order — the page-level resolution every
    /// export/assembly path consumes. Falls back to splitting the
    /// stored fragment when any page lacks a resolvable entity.
    pub fn version_page_packages(&self, version: &VersionRec) -> SResult<Vec<Package>> {
        let entity_backed = !version.pages.is_empty()
            && version
                .pages
                .iter()
                .all(|p| !p.id.is_empty());
        if entity_backed {
            let mut out = Vec::with_capacity(version.pages.len());
            for pos in 0..version.pages.len() {
                match self.version_page_package(version, pos) {
                    Ok(pkg) => out.push(pkg),
                    Err(_) => return self.version_page_packages_legacy(version),
                }
            }
            return Ok(out);
        }
        self.version_page_packages_legacy(version)
    }

    fn version_page_packages_legacy(&self, version: &VersionRec) -> SResult<Vec<Package>> {
        // A lazy-import wrapper whose page has not been split
        // yet carries no fragment (see
        // `insert_pending_version_from_pages`): name that cause
        // here instead of surfacing a missing-file error for the
        // empty hash (audit N8's intent, enforced at the door).
        if version.fragment_hash.is_empty() {
            return Err("版本尚无片段字节（页面未拆分完成），无法组包".into());
        }
        let bytes = self.file_get(&version.fragment_hash)?;
        let pkg = Package::load(&bytes)?;
        crate::pptx::split(&pkg)
    }

    /// The version's fragment PPTX bytes. For entity-backed
    /// versions this is the stored materialization of exactly the
    /// fragment the version was inserted from (versions are
    /// immutable, so the materialization cannot drift from the
    /// reference sequence); consumers that need pages rather than
    /// the whole fragment should prefer `version_page_packages`.
    pub fn version_fragment_bytes(&self, version: &VersionRec) -> SResult<Vec<u8>> {
        // The submission byte chain's door (audit N8): a
        // not-yet-split lazy wrapper has no fragment to ship —
        // refuse here with the cause named rather than letting
        // an empty hash fail far away as a missing file.
        if version.fragment_hash.is_empty() {
            return Err("版本尚无片段字节（页面未拆分完成），无法组包".into());
        }
        self.file_get(&version.fragment_hash)
    }

    /// Point a version at a stored fragment after the fact. The
    /// lazy import inserts a wrapper version with no fragment at
    /// open time (the page is not split yet); when the page's
    /// split lands, its version adopts the page entity's package
    /// — the same content-addressed file `register_page` stored —
    /// and from then on the wrapper is indistinguishable from a
    /// staged import's stage-1 version.
    pub fn set_version_fragment_hash(&self, version_id: &str, hash: &str) -> SResult<()> {
        self.conn
            .execute(
                "UPDATE versions SET fragment_hash=?2 WHERE id=?1",
                params![version_id, hash],
            )
            .map_err(db_err)?;
        Ok(())
    }

    // ---------- pending imports (lazy import) ----------

    fn row_to_pending_import(r: &rusqlite::Row) -> rusqlite::Result<PendingImportRec> {
        let sections_json: String = r.get(8)?;
        let pages_json: String = r.get(9)?;
        let blocks_json: String = r.get(10)?;
        let grouped_json: String = r.get(11)?;
        let verified: Option<i64> = r.get(12)?;
        Ok(PendingImportRec {
            id: r.get(0)?,
            filename: r.get(1)?,
            stem: r.get(2)?,
            deck_hash: r.get(3)?,
            folder_id: r.get(4)?,
            group_id: r.get(5)?,
            author: r.get(6)?,
            slide_count: r.get::<_, i64>(7)? as usize,
            sections: serde_json::from_str(&sections_json).unwrap_or_default(),
            pages: serde_json::from_str(&pages_json).unwrap_or_default(),
            blocks: if blocks_json.is_empty() {
                None
            } else {
                serde_json::from_str(&blocks_json).unwrap_or_default()
            },
            grouped: serde_json::from_str(&grouped_json).unwrap_or_default(),
            verified: verified.map(|v| v != 0),
            state: r.get(13)?,
            error: r.get(14)?,
            created_ms: r.get(15)?,
        })
    }

    const PENDING_IMPORT_COLS: &'static str = "id, filename, stem, deck_hash, folder_id, group_id, author, slide_count, sections_json, pages_json, blocks_json, grouped_json, verified, state, error, created_ms";

    pub fn insert_pending_import(&self, rec: &PendingImportRec) -> SResult<()> {
        self.conn
            .execute(
                "INSERT INTO pending_imports(id, filename, stem, deck_hash, folder_id, group_id, author, slide_count, sections_json, pages_json, blocks_json, grouped_json, verified, state, error, created_ms) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16)",
                params![
                    rec.id,
                    rec.filename,
                    rec.stem,
                    rec.deck_hash,
                    rec.folder_id,
                    rec.group_id,
                    rec.author,
                    rec.slide_count as i64,
                    serde_json::to_string(&rec.sections).unwrap_or_default(),
                    serde_json::to_string(&rec.pages).unwrap_or_default(),
                    rec.blocks
                        .as_ref()
                        .map(|b| serde_json::to_string(b).unwrap_or_default())
                        .unwrap_or_default(),
                    serde_json::to_string(&rec.grouped).unwrap_or_default(),
                    rec.verified.map(|v| v as i64),
                    rec.state,
                    rec.error,
                    rec.created_ms,
                ],
            )
            .map_err(db_err)?;
        Ok(())
    }

    pub fn get_pending_import(&self, id: &str) -> SResult<Option<PendingImportRec>> {
        self.conn
            .query_row(
                &format!(
                    "SELECT {} FROM pending_imports WHERE id=?1",
                    Self::PENDING_IMPORT_COLS
                ),
                params![id],
                Self::row_to_pending_import,
            )
            .optional()
            .map_err(db_err)
    }

    /// Every import still splitting (or failed mid-split),
    /// oldest first — the resume list a starting client works
    /// from.
    pub fn list_pending_imports(&self) -> SResult<Vec<PendingImportRec>> {
        let mut stmt = self
            .conn
            .prepare(&format!(
                "SELECT {} FROM pending_imports ORDER BY created_ms, id",
                Self::PENDING_IMPORT_COLS
            ))
            .map_err(db_err)?;
        let rows = stmt
            .query_map([], Self::row_to_pending_import)
            .map_err(db_err)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(db_err)?;
        Ok(rows)
    }

    /// Rewrite a pending import's mutable state (per-page split
    /// flags, folded blocks, the deck-level verdict, lifecycle).
    pub fn update_pending_import(&self, rec: &PendingImportRec) -> SResult<()> {
        self.conn
            .execute(
                "UPDATE pending_imports SET pages_json=?2, grouped_json=?3, verified=?4, state=?5, error=?6 WHERE id=?1",
                params![
                    rec.id,
                    serde_json::to_string(&rec.pages).unwrap_or_default(),
                    serde_json::to_string(&rec.grouped).unwrap_or_default(),
                    rec.verified.map(|v| v as i64),
                    rec.state,
                    rec.error,
                ],
            )
            .map_err(db_err)?;
        Ok(())
    }

    pub fn delete_pending_import(&self, id: &str) -> SResult<()> {
        self.conn
            .execute("DELETE FROM pending_imports WHERE id=?1", params![id])
            .map_err(db_err)?;
        Ok(())
    }

    /// The whole-deck backing of a not-yet-split wrapper version:
    /// (deck file hash, slide index, page entity id). The client
    /// renders such a page's thumbnail straight from the deck —
    /// rendering is not "using" the page — while every other
    /// consumer only ever meets the page after its split.
    pub fn lazy_backing_for_version(
        &self,
        version_id: &str,
    ) -> SResult<Option<(String, usize, String)>> {
        for rec in self.list_pending_imports()? {
            for p in &rec.pages {
                if p.version_id == version_id && !p.split && !p.gone {
                    return Ok(Some((
                        rec.deck_hash.clone(),
                        p.index,
                        crate::store::page_id_for_fp(&p.fp),
                    )));
                }
            }
        }
        Ok(None)
    }

    /// The same backing, addressed by page entity id (the
    /// thumbnail resolver's key): (deck file hash, slide index)
    /// for a page that is still waiting for its split.
    pub fn lazy_backing_for_page(
        &self,
        page_id: &str,
    ) -> SResult<Option<(String, usize)>> {
        for rec in self.list_pending_imports()? {
            for p in &rec.pages {
                if !p.split
                    && !p.gone
                    && crate::store::page_id_for_fp(&p.fp) == page_id
                {
                    return Ok(Some((rec.deck_hash.clone(), p.index)));
                }
            }
        }
        Ok(None)
    }

    /// Page entity-ization migration (Store::open): versions
    /// written before page entities carry PageInfo rows without
    /// ids. For each such version, split its stored fragment,
    /// register every page as an entity and rewrite pages_json
    /// with the reference sequence. Idempotent — versions whose
    /// pages all carry ids are skipped, and a version whose
    /// fragment cannot be read or split is left untouched (its
    /// reads keep working through the legacy fragment fallback).
    /// Page bindings gain their page-id addressing the same way:
    /// (version, slot) resolves to the version's page reference.
    fn migrate_page_entities(&self) -> SResult<()> {
        // Incremental posture (audit N6): the scan below only
        // selects versions that still carry id-less pages, and
        // versions whose migration already failed once are
        // remembered by id (settings) and not retried on every
        // launch — a single corrupt version must not tax every
        // startup with a full fragment parse forever. Failed
        // versions keep reading through the fragment fallback.
        let mut failed: Vec<String> = self
            .get_setting("page_entity_migration_failed")
            .ok()
            .flatten()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default();
        let failed_before = failed.clone();
        let pending: Vec<(String, String, String)> = {
            let mut stmt = self
                .conn
                .prepare("SELECT id, fragment_hash, pages_json FROM versions WHERE pages_json LIKE '%\"id\":\"\"%'")
                .map_err(db_err)?;
            let rows = stmt
                .query_map([], |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, String>(2)?,
                    ))
                })
                .map_err(db_err)?
                .collect::<Result<Vec<_>, _>>()
                .map_err(db_err)?;
            rows
        };
        for (version_id, fragment_hash, pages_json) in &pending {
            if failed.contains(version_id) {
                continue;
            }
            let mut pages: Vec<PageInfo> =
                serde_json::from_str(pages_json).unwrap_or_default();
            if pages.iter().all(|p| !p.id.is_empty()) {
                continue;
            }
            let Ok(bytes) = self.file_get(fragment_hash) else {
                failed.push(version_id.clone());
                continue;
            };
            let Ok(pkg) = Package::load(&bytes) else {
                failed.push(version_id.clone());
                continue;
            };
            let Ok(singles) = crate::pptx::split(&pkg) else {
                failed.push(version_id.clone());
                continue;
            };
            if singles.len() != pages.len() {
                failed.push(version_id.clone());
                continue;
            }
            let mut filled = true;
            for (i, page) in pages.iter_mut().enumerate() {
                if !page.id.is_empty() {
                    continue;
                }
                let Ok(single_bytes) = singles[i].to_bytes() else {
                    filled = false;
                    break;
                };
                match self.register_page(&page.fp, &page.title, &page.desc, &single_bytes) {
                    Ok(rec) => page.id = rec.id,
                    Err(_) => {
                        filled = false;
                        break;
                    }
                }
            }
            if !filled {
                failed.push(version_id.clone());
                continue;
            }
            if self
                .conn
                .execute(
                    "UPDATE versions SET pages_json=?2 WHERE id=?1",
                    params![
                        version_id,
                        serde_json::to_string(&pages).unwrap_or_default()
                    ],
                )
                .is_err()
            {
                failed.push(version_id.clone());
                continue;
            }
        }
        if failed != failed_before {
            let _ = self.set_setting(
                "page_entity_migration_failed",
                &serde_json::to_string(&failed).unwrap_or_default(),
            );
        }
        // Page bindings: fill page-id addressing from (version, slot).
        let bindings: Vec<(String, String, i64, String, i64)> = {
            let mut stmt = self
                .conn
                .prepare(
                    "SELECT id, from_version_id, from_slot, to_version_id, to_slot FROM page_bindings WHERE from_page_id='' OR to_page_id=''",
                )
                .map_err(db_err)?;
            let rows = stmt
                .query_map([], |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, i64>(2)?,
                        r.get::<_, String>(3)?,
                        r.get::<_, i64>(4)?,
                    ))
                })
                .map_err(db_err)?
                .collect::<Result<Vec<_>, _>>()
                .map_err(db_err)?;
            rows
        };
        for (binding_id, from_vid, from_slot, to_vid, to_slot) in &bindings {
            let page_id_at = |vid: &str, slot: i64| -> String {
                if slot < 0 {
                    return String::new();
                }
                self.get_version(vid)
                    .ok()
                    .and_then(|v| v.pages.get(slot as usize).map(|p| p.id.clone()))
                    .unwrap_or_default()
            };
            let from_pid = page_id_at(from_vid, *from_slot);
            let to_pid = page_id_at(to_vid, *to_slot);
            if from_pid.is_empty() && to_pid.is_empty() {
                continue;
            }
            // Best-effort like the rest of this migration: one
            // binding's write failure must not fail the open.
            let _ = self.conn.execute(
                "UPDATE page_bindings SET from_page_id=?2, to_page_id=?3 WHERE id=?1",
                params![binding_id, from_pid, to_pid],
            );
        }
        Ok(())
    }

    // ---------- users / auth ----------

    pub fn user_count(&self) -> SResult<i64> {
        self.conn
            .query_row("SELECT COUNT(*) FROM users", [], |r| r.get(0))
            .map_err(db_err)
    }

    pub fn create_user(
        &self,
        username: &str,
        display_name: &str,
        password: &str,
        dept_id: Option<&str>,
        role: &str,
    ) -> SResult<UserRec> {
        // Role is an enum, never free text (audit H1): an unknown
        // role string must not land in the table, where the
        // negative `role == "member"` checks elsewhere would
        // silently treat it as an administrator. The caller's own
        // ceiling (who may mint which role) is enforced by the
        // server handler; this is the last-line invariant.
        if !matches!(role, "super" | "dept_admin" | "member") {
            return Err(format!("未知角色：{role}"));
        }
        // Minimal password policy (audit M7): no empty or short
        // passwords on any creation path.
        if password.chars().count() < 8 {
            return Err("口令至少需要 8 位".into());
        }
        let id = new_id("usr");
        let hash = hash_password(password)?;
        self.conn
            .execute(
                "INSERT INTO users(id, username, display_name, password_hash, dept_id, role) VALUES(?1,?2,?3,?4,?5,?6)",
                params![id, username, display_name, hash, dept_id, role],
            )
            .map_err(db_err)?;
        if let Some(d) = dept_id {
            self.conn
                .execute(
                    "INSERT OR IGNORE INTO user_depts(user_id, dept_id) VALUES(?1,?2)",
                    params![id, d],
                )
                .map_err(db_err)?;
        }
        self.user_by_id(&id)
    }

    fn row_to_user(r: &rusqlite::Row) -> rusqlite::Result<UserRec> {
        Ok(UserRec {
            id: r.get(0)?,
            username: r.get(1)?,
            display_name: r.get(2)?,
            dept_id: r.get(3)?,
            role: r.get(4)?,
            disabled: r.get::<_, i64>(5)? != 0,
            must_change_password: r.get::<_, i64>(6)? != 0,
        })
    }

    pub fn user_by_id(&self, id: &str) -> SResult<UserRec> {
        self.conn
            .query_row(
                "SELECT id, username, display_name, dept_id, role, disabled, must_change_password FROM users WHERE id=?1",
                params![id],
                Self::row_to_user,
            )
            .map_err(db_err)
    }

    pub fn user_by_username(&self, username: &str) -> SResult<UserRec> {
        self.conn
            .query_row(
                "SELECT id, username, display_name, dept_id, role, disabled, must_change_password FROM users WHERE username=?1",
                params![username],
                Self::row_to_user,
            )
            .map_err(db_err)
    }

    pub fn list_users(&self) -> SResult<Vec<UserRec>> {
        let mut stmt = self
            .conn
            .prepare("SELECT id, username, display_name, dept_id, role, disabled, must_change_password FROM users ORDER BY username")
            .map_err(db_err)?;
        let rows = stmt
            .query_map([], Self::row_to_user)
            .map_err(db_err)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(db_err)?;
        Ok(rows)
    }

    pub fn login(&self, username: &str, password: &str) -> SResult<(String, UserRec)> {
        self.login_with_ip(username, password, None)
    }

    pub fn login_with_ip(
        &self,
        username: &str,
        password: &str,
        client_ip: Option<&str>,
    ) -> SResult<(String, UserRec)> {
        let found = self
            .conn
            .query_row(
                "SELECT id, username, display_name, dept_id, role, disabled, must_change_password, password_hash FROM users WHERE username=?1",
                params![username],
                |r| {
                    Ok((
                        UserRec {
                            id: r.get(0)?,
                            username: r.get(1)?,
                            display_name: r.get(2)?,
                            dept_id: r.get(3)?,
                            role: r.get(4)?,
                            disabled: r.get::<_, i64>(5)? != 0,
                            must_change_password: r.get::<_, i64>(6)? != 0,
                        },
                        r.get::<_, String>(7)?,
                    ))
                },
            )
            .optional()
            .map_err(db_err)?;
        let (user, hash) = match found {
            Some(pair) => pair,
            None => {
                // Timing equalization (audit M7): an unknown
                // username runs the same Argon2 verification
                // against a dummy hash, so it costs what a wrong
                // password costs and cannot be enumerated by
                // response time.
                let _ = verify_password(password, dummy_hash());
                return Err("用户名或密码不正确".into());
            }
        };
        // Verify first, report disabled second (audit M7): the
        // old order answered "account disabled" to any password,
        // an oracle for which usernames exist and are disabled.
        // Now the disabled state is revealed only to a caller who
        // proved knowledge of the password.
        if !verify_password(password, &hash) {
            return Err("用户名或密码不正确".into());
        }
        if user.disabled {
            return Err("账号已停用".into());
        }
        let token = new_token();
        let now = now_ms();
        self.conn
            .execute(
                "INSERT INTO sessions(token, user_id, created_ms, kind, client_ip, last_seen_ms) VALUES(?1,?2,?3,'user',?4,?3)",
                params![token, user.id, now, client_ip],
            )
            .map_err(db_err)?;
        Ok((token, user))
    }

    /// Load a session row, enforcing its lifetime: an expired row
    /// is deleted on sight and reads as absent. Guest sessions age
    /// from issue (created_ms, 24h); named sessions age from last
    /// activity (last_seen_ms, 30 days) and slide forward on use —
    /// the touch is throttled so plain reads stay reads, mostly.
    pub fn session_by_token(&self, token: &str) -> SResult<Option<SessionRec>> {
        let row = self.conn.query_row(
            "SELECT kind, user_id, client_ip, created_ms, last_seen_ms FROM sessions WHERE token=?1",
            params![token],
            |r| {
                Ok(SessionRec {
                    kind: r.get(0)?,
                    user_id: r.get(1)?,
                    client_ip: r.get(2)?,
                    created_ms: r.get(3)?,
                    last_seen_ms: r.get(4)?,
                })
            },
        );
        let s = match row {
            Ok(s) => s,
            Err(rusqlite::Error::QueryReturnedNoRows) => return Ok(None),
            Err(e) => return Err(db_err(e)),
        };
        let now = now_ms();
        let expired = if s.kind == "guest" {
            now - s.created_ms > GUEST_TTL_MS
        } else {
            now - s.last_seen_ms > USER_IDLE_TTL_MS
        };
        if expired {
            let _ = self
                .conn
                .execute("DELETE FROM sessions WHERE token=?1", params![token]);
            return Ok(None);
        }
        if s.kind != "guest" && now - s.last_seen_ms > SESSION_TOUCH_MS {
            let _ = self.conn.execute(
                "UPDATE sessions SET last_seen_ms=?2 WHERE token=?1",
                params![token, now],
            );
        }
        Ok(Some(s))
    }

    pub fn user_by_token(&self, token: &str) -> SResult<UserRec> {
        let s = self
            .session_by_token(token)?
            .ok_or_else(|| "未登录".to_string())?;
        // A guest session is not a user session: its NULL user_id
        // must not open any named-identity door.
        let uid = s.user_id.ok_or_else(|| "未登录".to_string())?;
        self.user_by_id(&uid)
    }

    /// Issue an anonymous guest session (org-model G1): the client
    /// presents only the server address it connects to; the server
    /// hands back an opaque token whose row remembers the issuing
    /// IP and time. That row is the whole of the minimal audit
    /// trail for anonymous reads this phase (G4 — no separate
    /// audit table). Expired guest rows are swept opportunistically
    /// here.
    pub fn create_guest_session(&self, client_ip: Option<&str>) -> SResult<String> {
        let now = now_ms();
        let _ = self.conn.execute(
            "DELETE FROM sessions WHERE kind='guest' AND created_ms < ?1",
            params![now - GUEST_TTL_MS],
        );
        let token = new_token();
        self.conn
            .execute(
                "INSERT INTO sessions(token, user_id, created_ms, kind, client_ip, last_seen_ms) VALUES(?1,NULL,?2,'guest',?3,?2)",
                params![token, now, client_ip],
            )
            .map_err(db_err)?;
        Ok(token)
    }

    /// Revoke every guest session issued to one IP (the abuse
    /// response the guest surface is governed by; named sessions
    /// are revoked per user — see set_user_disabled).
    pub fn delete_guest_sessions_for_ip(&self, ip: &str) -> SResult<usize> {
        self.conn
            .execute(
                "DELETE FROM sessions WHERE kind='guest' AND client_ip=?1",
                params![ip],
            )
            .map_err(db_err)
    }

    /// The seeded owner of a standalone installation: the first
    /// super-admin account (ensure_seed creates exactly one).
    pub fn seed_owner(&self) -> SResult<UserRec> {
        self.conn
            .query_row(
                "SELECT id, username, display_name, dept_id, role, disabled, must_change_password FROM users WHERE role='super' ORDER BY rowid LIMIT 1",
                [],
                Self::row_to_user,
            )
            .map_err(db_err)
    }

    /// Standalone local-server mapping (org-model standalone gap;
    /// standalone-fix §4 option 1): a tokenless loopback request is
    /// the machine's owner. The mapping runs through a formal
    /// session row — an existing unexpired owner session from this
    /// IP is reused (and slid forward by session_by_token's touch),
    /// otherwise one is minted with the normal session insert.
    /// Callers gate on standalone mode + loopback themselves; this
    /// only maintains the session.
    pub fn standalone_owner_session(&self, client_ip: Option<&str>) -> SResult<UserRec> {
        let owner = self.seed_owner()?;
        if owner.disabled {
            return Err("种子所有者账号已停用".into());
        }
        let existing: Option<String> = self
            .conn
            .query_row(
                "SELECT token FROM sessions WHERE user_id=?1 AND kind='user' AND client_ip IS ?2 ORDER BY created_ms DESC LIMIT 1",
                params![owner.id, client_ip],
                |r| r.get(0),
            )
            .ok();
        if let Some(t) = existing {
            if self.session_by_token(&t)?.is_some() {
                return Ok(owner);
            }
        }
        let token = new_token();
        let now = now_ms();
        self.conn
            .execute(
                "INSERT INTO sessions(token, user_id, created_ms, kind, client_ip, last_seen_ms) VALUES(?1,?2,?3,'user',?4,?3)",
                params![token, owner.id, now, client_ip],
            )
            .map_err(db_err)?;
        Ok(owner)
    }

    pub fn logout(&self, token: &str) -> SResult<()> {
        self.conn
            .execute("DELETE FROM sessions WHERE token=?1", params![token])
            .map_err(db_err)?;
        Ok(())
    }

    /// Set or clear the first-login forced-change flag (audit
    /// H6). The server sets it on the seeded super admin when the
    /// seed password came from the built-in default.
    pub fn set_must_change_password(&self, id: &str, flag: bool) -> SResult<()> {
        self.conn
            .execute(
                "UPDATE users SET must_change_password=?2 WHERE id=?1",
                params![id, flag as i64],
            )
            .map_err(db_err)?;
        Ok(())
    }

    /// Change a user's password after proving the current one
    /// (audit H6's change door). Clears the forced-change flag.
    /// Session revocation is the caller's move (the server keeps
    /// the calling session and drops the user's others).
    pub fn change_password(
        &self,
        user_id: &str,
        current: &str,
        new_password: &str,
    ) -> SResult<()> {
        let hash: String = self
            .conn
            .query_row(
                "SELECT password_hash FROM users WHERE id=?1",
                params![user_id],
                |r| r.get(0),
            )
            .map_err(|_| "用户不存在".to_string())?;
        if !verify_password(current, &hash) {
            return Err("当前口令不正确".into());
        }
        if new_password.chars().count() < 8 {
            return Err("新口令至少需要 8 位".into());
        }
        if new_password == current {
            return Err("新口令不能与当前口令相同".into());
        }
        let new_hash = hash_password(new_password)?;
        self.conn
            .execute(
                "UPDATE users SET password_hash=?2, must_change_password=0 WHERE id=?1",
                params![user_id, new_hash],
            )
            .map_err(db_err)?;
        Ok(())
    }

    /// Drop every session of a user except one (used after a
    /// password change: the caller's own session survives, any
    /// session an attacker may hold does not).
    pub fn delete_other_sessions(&self, user_id: &str, keep_token: &str) -> SResult<()> {
        self.conn
            .execute(
                "DELETE FROM sessions WHERE user_id=?1 AND token<>?2",
                params![user_id, keep_token],
            )
            .map_err(db_err)?;
        Ok(())
    }

    pub fn set_user_disabled(&self, id: &str, disabled: bool) -> SResult<()> {
        self.conn
            .execute(
                "UPDATE users SET disabled=?2 WHERE id=?1",
                params![id, disabled as i64],
            )
            .map_err(db_err)?;
        if disabled {
            self.conn
                .execute("DELETE FROM sessions WHERE user_id=?1", params![id])
                .map_err(db_err)?;
        }
        Ok(())
    }

    /// Reassign a user's department and/or system role (super-only).
    pub fn update_user_org(
        &self,
        id: &str,
        dept_id: Option<&str>,
        role: &str,
    ) -> SResult<UserRec> {
        if !matches!(role, "super" | "dept_admin" | "member") {
            return Err(format!("未知角色：{role}"));
        }
        if let Some(d) = dept_id {
            if !self.list_departments()?.iter().any(|x| x.id == d) {
                return Err("目标部门不存在".into());
            }
        }
        let changed = self
            .conn
            .execute(
                "UPDATE users SET dept_id=?2, role=?3 WHERE id=?1",
                params![id, dept_id, role],
            )
            .map_err(db_err)?;
        if changed == 0 {
            return Err("账号不存在".into());
        }
        self.conn
            .execute("DELETE FROM user_depts WHERE user_id=?1", params![id])
            .map_err(db_err)?;
        if let Some(d) = dept_id {
            self.conn
                .execute(
                    "INSERT OR IGNORE INTO user_depts(user_id, dept_id) VALUES(?1,?2)",
                    params![id, d],
                )
                .map_err(db_err)?;
        }
        self.user_by_id(id)
    }

    pub fn user_dept_ids(&self, user_id: &str) -> SResult<Vec<String>> {
        let mut stmt = self
            .conn
            .prepare("SELECT dept_id FROM user_depts WHERE user_id=?1")
            .map_err(db_err)?;
        let rows = stmt
            .query_map(params![user_id], |r| r.get::<_, String>(0))
            .map_err(db_err)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(db_err)?;
        Ok(rows)
    }

    // ---------- departments / folders / groups ----------

    pub fn create_department(&self, name: &str, parent_id: Option<&str>) -> SResult<DeptRec> {
        let id = new_id("dep");
        self.conn
            .execute(
                "INSERT INTO departments(id, name, parent_id) VALUES(?1,?2,?3)",
                params![id, name, parent_id],
            )
            .map_err(db_err)?;
        Ok(DeptRec {
            id,
            name: name.into(),
            parent_id: parent_id.map(|s| s.to_string()),
        })
    }

    pub fn rename_department(&self, id: &str, name: &str) -> SResult<()> {
        let trimmed = name.trim();
        if trimmed.is_empty() {
            return Err("部门名称不能为空".into());
        }
        let changed = self
            .conn
            .execute(
                "UPDATE departments SET name=?2 WHERE id=?1",
                params![id, trimmed],
            )
            .map_err(db_err)?;
        if changed == 0 {
            return Err("部门不存在".into());
        }
        Ok(())
    }

    pub fn list_departments(&self) -> SResult<Vec<DeptRec>> {
        let mut stmt = self
            .conn
            .prepare("SELECT id, name, parent_id FROM departments ORDER BY name")
            .map_err(db_err)?;
        let rows = stmt
            .query_map([], |r| {
                Ok(DeptRec {
                    id: r.get(0)?,
                    name: r.get(1)?,
                    parent_id: r.get(2)?,
                })
            })
            .map_err(db_err)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(db_err)?;
        Ok(rows)
    }

    /// dept id + all its ancestors (grants on an ancestor flow down to
    /// sub-departments, spec §7).
    pub fn dept_chain_up(&self, dept_id: &str) -> SResult<Vec<String>> {
        let depts = self.list_departments()?;
        let mut chain = vec![dept_id.to_string()];
        let mut cur = dept_id.to_string();
        loop {
            match depts
                .iter()
                .find(|d| d.id == cur)
                .and_then(|d| d.parent_id.clone())
            {
                Some(p) => {
                    chain.push(p.clone());
                    cur = p;
                }
                None => break,
            }
        }
        Ok(chain)
    }

    pub fn create_folder(
        &self,
        name: &str,
        parent_id: Option<&str>,
        scope: &str,
    ) -> SResult<FolderRec> {
        let id = new_id("fld");
        // A child folder lives in its parent's tree: with a
        // parent, the scope is always the parent's scope, so a
        // tree can never grow a branch into another surface's
        // sidebar (sidebar isolation, user ruling 2026-11-24).
        // A root folder's scope is caller-supplied, so it is
        // validated against the five sidebar trees (audit
        // round-2 N4): free text here would mint a folder that
        // hangs on no tree at all. The same validation covers a
        // child whose parent predates scoping (empty scope) and
        // therefore falls back to the supplied value.
        let parent_scope = match parent_id {
            Some(pid) => self
                .list_folders()?
                .into_iter()
                .find(|f| f.id == pid)
                .map(|f| f.scope)
                .filter(|sc| !sc.is_empty()),
            None => None,
        };
        let scope = match parent_scope {
            Some(sc) => sc,
            None => validate_folder_scope(scope)?,
        };
        let sort: i64 = self
            .conn
            .query_row(
                "SELECT COALESCE(MAX(sort),0)+1 FROM folders WHERE scope=?1",
                params![scope],
                |r| r.get(0),
            )
            .map_err(db_err)?;
        self.conn
            .execute(
                "INSERT INTO folders(id, name, parent_id, sort, scope) VALUES(?1,?2,?3,?4,?5)",
                params![id, name, parent_id, sort, scope],
            )
            .map_err(db_err)?;
        Ok(FolderRec {
            id,
            name: name.into(),
            parent_id: parent_id.map(|s| s.to_string()),
            sort,
            main_index_id: None,
            scope,
            color: None,
            note: String::new(),
            origin: String::new(),
        })
    }

    pub fn list_folders(&self) -> SResult<Vec<FolderRec>> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT id, name, parent_id, sort, main_index_id, scope, color, note, origin FROM folders ORDER BY sort, name",
            )
            .map_err(db_err)?;
        let rows = stmt
            .query_map([], |r| {
                Ok(FolderRec {
                    id: r.get(0)?,
                    name: r.get(1)?,
                    parent_id: r.get(2)?,
                    sort: r.get(3)?,
                    main_index_id: r.get(4)?,
                    scope: r.get(5)?,
                    color: r.get(6)?,
                    note: r.get(7)?,
                    origin: r.get(8)?,
                })
            })
            .map_err(db_err)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(db_err)?;
        Ok(rows)
    }

    /// The folders of one sidebar scope, in tree order. The
    /// folder faces (the server route and the local dispatch)
    /// serve exactly this when a scope is asked for; the plain
    /// list_folders stays for id-based internal lookups (trash
    /// origins, the main-index response), where the caller's
    /// object already fixes the row.
    pub fn list_folders_in(&self, scope: &str) -> SResult<Vec<FolderRec>> {
        Ok(self
            .list_folders()?
            .into_iter()
            .filter(|f| f.scope == scope)
            .collect())
    }

    /// Pull server folders down into this library (folder sync,
    /// user ruling 2026-11-25): the user picks folders on a
    /// server and they are copied into the local library, where
    /// they stay for good — mixed into the same trees as local
    /// folders, marked origin="server".
    ///
    /// Reconciliation is by id alone (a folder's identity is its
    /// id; the name is just a detail on the id card): a row whose
    /// id already exists locally is skipped untouched, so a
    /// re-sync never duplicates a folder and never overwrites a
    /// local rename. A new row adopts the server's id verbatim —
    /// folder ids are `fld_` + 64 random bits minted by the same
    /// generator on both sides, so the id space is shared and a
    /// collision would be the same folder by construction. The
    /// server parent_id is kept verbatim too: when the parent was
    /// not picked it dangles for now and the trees surface the
    /// folder at the root; when the parent is synced later the
    /// folder re-attaches on its own. `main_index_id` is NOT
    /// copied — it designates an index on the server, which does
    /// not exist in this library. Rows with a scope outside the
    /// five sidebar trees are skipped (they would hang on no
    /// tree). Returns how many rows were inserted.
    pub fn sync_server_folders(&self, server_folders: &[FolderRec]) -> SResult<usize> {
        let existing: HashSet<String> = self
            .list_folders()?
            .into_iter()
            .map(|f| f.id)
            .collect();
        let tx = self.conn.unchecked_transaction().map_err(db_err)?;
        let mut inserted = 0usize;
        for f in server_folders {
            if existing.contains(&f.id) || !FOLDER_SCOPES.contains(&f.scope.as_str()) {
                continue;
            }
            tx.execute(
                "INSERT INTO folders(id, name, parent_id, sort, scope, color, note, origin) VALUES(?1,?2,?3,?4,?5,?6,?7,'server')",
                params![f.id, f.name, f.parent_id, f.sort, f.scope, f.color, f.note],
            )
            .map_err(db_err)?;
            inserted += 1;
        }
        tx.commit().map_err(db_err)?;
        Ok(inserted)
    }

    pub fn rename_folder(&self, id: &str, name: &str) -> SResult<()> {
        let n = self
            .conn
            .execute("UPDATE folders SET name=?2 WHERE id=?1", params![id, name])
            .map_err(db_err)?;
        if n == 0 {
            return Err("文件夹不存在".into());
        }
        Ok(())
    }

    /// Set (or clear, with an empty string) the folder's note
    /// （备注）. The content page's folder-flow header edits it in
    /// place; the sidebar tree does not surface it.
    pub fn set_folder_note(&self, id: &str, note: &str) -> SResult<()> {
        let n = self
            .conn
            .execute("UPDATE folders SET note=?2 WHERE id=?1", params![id, note])
            .map_err(db_err)?;
        if n == 0 {
            return Err("文件夹不存在".into());
        }
        Ok(())
    }

    /// Move a folder (sidebar drag-and-drop): reparent it and/or
    /// reorder it among its new siblings. `before_id` names the
    /// sibling it should sit in front of (None = last among the
    /// new siblings). Sibling order persists in the single global
    /// `sort` column: after the move the whole tree is renumbered
    /// along a depth-first walk, which is exactly the order
    /// list_folders returns and the client's walk reproduces, so
    /// every sibling group's relative order is preserved. Moving
    /// a folder under itself or one of its own descendants is
    /// refused (it would create a cycle). The move is confined
    /// to the folder's own sidebar scope (sidebar isolation,
    /// user ruling 2026-11-24): a parent or insert point from
    /// another surface's tree is refused, and the renumber walk
    /// touches only that scope's folders, so no other tree's
    /// order changes.
    pub fn move_folder(
        &self,
        id: &str,
        new_parent_id: Option<&str>,
        before_id: Option<&str>,
    ) -> SResult<()> {
        let all = self.list_folders()?;
        let Some(me) = all.iter().find(|f| f.id == id) else {
            return Err("文件夹不存在".into());
        };
        if let Some(p) = new_parent_id {
            if !all.iter().any(|f| f.id == p) {
                return Err("目标文件夹不存在".into());
            }
            if all.iter().find(|f| f.id == p).map(|f| f.scope.as_str())
                != Some(me.scope.as_str())
            {
                return Err("不能把文件夹移动到另一棵目录树下".into());
            }
            // Cycle guard: walk the parent chain up from the
            // proposed parent; meeting the folder itself means the
            // parent is the folder or one of its descendants.
            let mut cur = Some(p);
            while let Some(c) = cur {
                if c == id {
                    return Err("不能把文件夹移动到它自己或子文件夹下".into());
                }
                cur = all
                    .iter()
                    .find(|f| f.id == c)
                    .and_then(|f| f.parent_id.as_deref());
            }
        }
        if before_id == Some(id) {
            return Ok(()); // dropped in front of itself: no-op
        }
        if let Some(b) = before_id {
            if let Some(bf) = all.iter().find(|f| f.id == b) {
                if bf.scope != me.scope {
                    return Err("目标位置不在同一棵目录树内".into());
                }
            }
        }
        // Sibling lists keyed by parent id ("" = root), each in
        // current list order — this scope's folders only, so the
        // renumber below rewrites no other tree's sort values.
        let mut groups: std::collections::HashMap<String, Vec<String>> =
            std::collections::HashMap::new();
        for f in all.iter().filter(|f| f.scope == me.scope) {
            groups
                .entry(f.parent_id.clone().unwrap_or_default())
                .or_default()
                .push(f.id.clone());
        }
        let old_key = me.parent_id.clone().unwrap_or_default();
        if let Some(list) = groups.get_mut(&old_key) {
            list.retain(|x| x != id);
        }
        let new_key = new_parent_id.unwrap_or("").to_string();
        let list = groups.entry(new_key).or_default();
        let pos = before_id
            .and_then(|b| list.iter().position(|x| x == b))
            .unwrap_or(list.len());
        list.insert(pos, id.to_string());
        // Depth-first renumber over the regrouped tree.
        fn walk(
            groups: &std::collections::HashMap<String, Vec<String>>,
            parent: &str,
            out: &mut Vec<String>,
        ) {
            if let Some(list) = groups.get(parent) {
                for fid in list {
                    out.push(fid.clone());
                    walk(groups, fid, out);
                }
            }
        }
        let mut order: Vec<String> = Vec::new();
        walk(&groups, "", &mut order);
        // Folders unreachable from the root (broken parent chain,
        // or a pre-isolation parent living in another scope) keep
        // their relative order at the end.
        for f in all.iter().filter(|f| f.scope == me.scope) {
            if !order.contains(&f.id) {
                order.push(f.id.clone());
            }
        }
        let tx = self.conn.unchecked_transaction().map_err(db_err)?;
        if me.parent_id.as_deref() != new_parent_id {
            tx.execute(
                "UPDATE folders SET parent_id=?2 WHERE id=?1",
                params![id, new_parent_id],
            )
            .map_err(db_err)?;
        }
        for (idx, fid) in order.iter().enumerate() {
            tx.execute(
                "UPDATE folders SET sort=?2 WHERE id=?1",
                params![fid, (idx + 1) as i64],
            )
            .map_err(db_err)?;
        }
        tx.commit().map_err(db_err)?;
        Ok(())
    }

    /// Set (or clear, with None) the folder's sidebar color: a
    /// "#RRGGBB" hex string from the client's preset palette.
    pub fn set_folder_color(&self, id: &str, color: Option<&str>) -> SResult<()> {
        let n = self
            .conn
            .execute(
                "UPDATE folders SET color=?2 WHERE id=?1",
                params![id, color],
            )
            .map_err(db_err)?;
        if n == 0 {
            return Err("文件夹不存在".into());
        }
        Ok(())
    }

    /// Delete a folder, in the shape the user picks at delete
    /// time (user ruling 2026-11-24 — the client always offers
    /// both):
    ///
    /// - Lift: only this folder's row goes away. Its child
    ///   folders, content units and indexes are re-homed to the
    ///   folder's parent; for a root folder that means parent_id /
    ///   folder_id NULL — the root level, which the index page
    ///   presents as 未分类. Showroom items follow their source
    ///   index automatically (the plaza homes an item by its
    ///   index's folder), so nothing else needs to move.
    /// - Trash: the folder's whole subtree goes. Every content
    ///   unit and index homed in the subtree, and every showroom
    ///   item whose source index is homed there, gets a
    ///   trash_items row (kind / origin follow the single-item
    ///   trash paths: origin is the home folder's name), then the
    ///   folder rows themselves are deleted. Trashed objects keep
    ///   their (now dangling) folder_id, exactly like the trash
    ///   overlay treats every other field — restoring one brings
    ///   it back folder-less, visible under 全部 / 未分类.
    ///
    /// `deleted_by` lands on the trash rows in Trash mode (the
    /// server passes the caller's user id, the local store its
    /// owner's display name — the same values the two faces'
    /// single-item trash paths record).
    pub fn delete_folder(
        &self,
        id: &str,
        mode: FolderDeleteMode,
        deleted_by: &str,
    ) -> SResult<()> {
        let all = self.list_folders()?;
        let Some(me) = all.iter().find(|f| f.id == id) else {
            return Err("文件夹不存在".into());
        };
        let parent = me.parent_id.clone();
        match mode {
            FolderDeleteMode::Lift => {
                let tx = self.conn.unchecked_transaction().map_err(db_err)?;
                tx.execute(
                    "UPDATE folders SET parent_id=?2 WHERE parent_id=?1",
                    params![id, parent],
                )
                .map_err(db_err)?;
                tx.execute(
                    "UPDATE units SET folder_id=?2 WHERE folder_id=?1",
                    params![id, parent],
                )
                .map_err(db_err)?;
                tx.execute(
                    "UPDATE indexes_tbl SET folder_id=?2 WHERE folder_id=?1",
                    params![id, parent],
                )
                .map_err(db_err)?;
                tx.execute("DELETE FROM folders WHERE id=?1", params![id])
                    .map_err(db_err)?;
                tx.commit().map_err(db_err)?;
            }
            FolderDeleteMode::Trash => {
                // The subtree: this folder plus every descendant.
                let mut subtree: Vec<String> = vec![id.to_string()];
                let mut grew = true;
                while grew {
                    grew = false;
                    for f in &all {
                        if let Some(p) = f.parent_id.as_deref() {
                            if subtree.iter().any(|x| x == p)
                                && !subtree.iter().any(|x| x == &f.id)
                            {
                                subtree.push(f.id.clone());
                                grew = true;
                            }
                        }
                    }
                }
                let folder_name = |fid: &str| -> String {
                    all.iter()
                        .find(|f| f.id == fid)
                        .map(|f| f.name.clone())
                        .unwrap_or_default()
                };
                // Contents, gathered before the transaction:
                // (kind, object_id, title, origin).
                let mut contents: Vec<(&str, String, String, String)> = Vec::new();
                let mut index_ids: Vec<String> = Vec::new();
                for fid in &subtree {
                    let mut stmt = self
                        .conn
                        .prepare("SELECT id, name FROM units WHERE folder_id=?1")
                        .map_err(db_err)?;
                    let units = stmt
                        .query_map(params![fid], |r| {
                            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
                        })
                        .map_err(db_err)?
                        .collect::<Result<Vec<_>, _>>()
                        .map_err(db_err)?;
                    drop(stmt);
                    for (uid, name) in units {
                        contents.push(("unit", uid, name, folder_name(fid)));
                    }
                    let mut stmt = self
                        .conn
                        .prepare("SELECT id, name FROM indexes_tbl WHERE folder_id=?1")
                        .map_err(db_err)?;
                    let indexes = stmt
                        .query_map(params![fid], |r| {
                            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
                        })
                        .map_err(db_err)?
                        .collect::<Result<Vec<_>, _>>()
                        .map_err(db_err)?;
                    drop(stmt);
                    for (xid, name) in indexes {
                        contents.push(("index", xid.clone(), name, folder_name(fid)));
                        index_ids.push(xid);
                    }
                }
                // Showroom items are homed by their source index:
                // the items of every trashed index go too (their
                // origin is the index's home folder).
                for xid in &index_ids {
                    let origin = self
                        .get_index(xid)
                        .ok()
                        .and_then(|x| x.folder_id)
                        .map(|fid| folder_name(&fid))
                        .unwrap_or_else(|| "演示厅".to_string());
                    let mut stmt = self
                        .conn
                        .prepare("SELECT id, title FROM showroom WHERE source_index_id=?1")
                        .map_err(db_err)?;
                    let items = stmt
                        .query_map(params![xid], |r| {
                            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
                        })
                        .map_err(db_err)?
                        .collect::<Result<Vec<_>, _>>()
                        .map_err(db_err)?;
                    drop(stmt);
                    for (sid, title) in items {
                        contents.push(("showroom", sid, title, origin.clone()));
                    }
                }
                let tx = self.conn.unchecked_transaction().map_err(db_err)?;
                let now = now_ms();
                for (kind, oid, title, origin) in &contents {
                    // INSERT OR IGNORE mirrors trash_add's
                    // idempotence (UNIQUE(kind, object_id)): an
                    // object already in the trash keeps its
                    // original row.
                    tx.execute(
                        "INSERT OR IGNORE INTO trash_items(id, kind, object_id, title, origin, deleted_by, deleted_ms) VALUES(?1,?2,?3,?4,?5,?6,?7)",
                        params![new_id("trs"), kind, oid, title, origin, deleted_by, now],
                    )
                    .map_err(db_err)?;
                }
                for fid in &subtree {
                    tx.execute("DELETE FROM folders WHERE id=?1", params![fid])
                        .map_err(db_err)?;
                }
                tx.commit().map_err(db_err)?;
            }
        }
        Ok(())
    }

    /// Designate (or clear, with None) the folder's main index. The
    /// caller validates that the index's home folder is this folder.
    pub fn set_folder_main_index(&self, folder_id: &str, index_id: Option<&str>) -> SResult<()> {
        let n = self
            .conn
            .execute(
                "UPDATE folders SET main_index_id=?2 WHERE id=?1",
                params![folder_id, index_id],
            )
            .map_err(db_err)?;
        if n == 0 {
            return Err("文件夹不存在".into());
        }
        Ok(())
    }

    pub fn create_group(&self, name: &str) -> SResult<GroupRec> {
        let id = new_id("grp");
        self.conn
            .execute(
                "INSERT INTO groups_tbl(id, name) VALUES(?1,?2)",
                params![id, name],
            )
            .map_err(db_err)?;
        Ok(GroupRec {
            id,
            name: name.into(),
        })
    }

    pub fn list_groups(&self) -> SResult<Vec<GroupRec>> {
        let mut stmt = self
            .conn
            .prepare("SELECT id, name FROM groups_tbl ORDER BY name")
            .map_err(db_err)?;
        let rows = stmt
            .query_map([], |r| {
                Ok(GroupRec {
                    id: r.get(0)?,
                    name: r.get(1)?,
                })
            })
            .map_err(db_err)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(db_err)?;
        Ok(rows)
    }

    pub fn add_grant(
        &self,
        group_id: &str,
        subject_type: &str,
        subject_id: &str,
        capability: &str,
    ) -> SResult<GrantRec> {
        let id = new_id("grt");
        self.conn
            .execute(
                "INSERT INTO group_grants(id, group_id, subject_type, subject_id, capability) VALUES(?1,?2,?3,?4,?5)",
                params![id, group_id, subject_type, subject_id, capability],
            )
            .map_err(db_err)?;
        Ok(GrantRec {
            id,
            group_id: group_id.into(),
            subject_type: subject_type.into(),
            subject_id: subject_id.into(),
            capability: capability.into(),
        })
    }

    pub fn list_grants(&self) -> SResult<Vec<GrantRec>> {
        let mut stmt = self
            .conn
            .prepare("SELECT id, group_id, subject_type, subject_id, capability FROM group_grants")
            .map_err(db_err)?;
        let rows = stmt
            .query_map([], |r| {
                Ok(GrantRec {
                    id: r.get(0)?,
                    group_id: r.get(1)?,
                    subject_type: r.get(2)?,
                    subject_id: r.get(3)?,
                    capability: r.get(4)?,
                })
            })
            .map_err(db_err)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(db_err)?;
        Ok(rows)
    }

    /// Capabilities a user holds on a group (super admin: all).
    pub fn capabilities(&self, user: &UserRec, group_id: &str) -> SResult<HashSet<String>> {
        if user.role == "super" {
            return Ok(["view", "use", "publish", "manage"]
                .iter()
                .map(|s| s.to_string())
                .collect());
        }
        let mut dept_subjects: HashSet<String> = HashSet::new();
        for d in self.user_dept_ids(&user.id)? {
            for anc in self.dept_chain_up(&d)? {
                dept_subjects.insert(anc);
            }
        }
        let mut caps = HashSet::new();
        for g in self.list_grants()? {
            if g.group_id != group_id {
                continue;
            }
            let hit = (g.subject_type == "user" && g.subject_id == user.id)
                || (g.subject_type == "dept" && dept_subjects.contains(&g.subject_id));
            if hit {
                caps.insert(g.capability);
            }
        }
        Ok(caps)
    }

    /// Groups the user can see at all; None = unrestricted (super admin).
    pub fn visible_groups(&self, user: &UserRec) -> SResult<Option<HashSet<String>>> {
        if user.role == "super" {
            return Ok(None);
        }
        let mut out = HashSet::new();
        for g in self.list_groups()? {
            if !self.capabilities(user, &g.id)?.is_empty() {
                out.insert(g.id);
            }
        }
        Ok(Some(out))
    }

    pub fn can_see_group(&self, user: &UserRec, group_id: &str) -> SResult<bool> {
        match self.visible_groups(user)? {
            None => Ok(true),
            Some(set) => Ok(set.contains(group_id)),
        }
    }
}

// ==================== content library ====================

impl Store {
    fn row_to_unit(r: &rusqlite::Row) -> rusqlite::Result<UnitRec> {
        Ok(UnitRec {
            id: r.get(0)?,
            name: r.get(1)?,
            folder_id: r.get(2)?,
            group_id: r.get(3)?,
            maintainer: r.get(4)?,
            lead_lang: r.get(5)?,
            description: r.get(6)?,
            current_revision: r.get(7)?,
            created_ms: r.get(8)?,
            sort: r.get(9)?,
        })
    }

    /// Home-scope gate for new entities (sidebar isolation as a
    /// data constraint, not just client discipline — unified
    /// content model, user ruling 2026-11-25): a content unit may
    /// only be created under a content-scope folder, an index
    /// only under an index-scope folder. No folder at all (None
    /// or empty) is the virtual 未分类 home and always legal. A
    /// folder id that does not exist, or whose scope belongs to
    /// another surface's tree, is refused with a plain error
    /// instead of landing as-is. Every creation path funnels
    /// through here: the REST handlers, the client's local
    /// dispatch, import, and internal derivations all call
    /// create_unit / create_index.
    fn check_home_folder(
        &self,
        folder_id: Option<&str>,
        want_scope: &str,
        what: &str,
    ) -> SResult<()> {
        let Some(fid) = folder_id.filter(|f| !f.is_empty()) else {
            return Ok(());
        };
        let folder = self.list_folders()?.into_iter().find(|f| f.id == fid);
        match folder {
            None => Err("目标文件夹不存在".into()),
            Some(f) if f.scope == want_scope => Ok(()),
            Some(f) => {
                let area = match f.scope.as_str() {
                    "showroom" => "演示·公共",
                    "library" => "演示·我的",
                    "inbox" => "演示·分享",
                    "content" => "页面",
                    "index" => "索引",
                    _ => "其他",
                };
                Err(format!(
                    "{what}不能建在这个文件夹下：该文件夹属于「{area}」目录"
                ))
            }
        }
    }

    pub fn create_unit(
        &self,
        name: &str,
        folder_id: Option<&str>,
        group_id: &str,
        maintainer: &str,
        lead_lang: &str,
        description: &str,
    ) -> SResult<UnitRec> {
        self.check_home_folder(folder_id, "content", "内容块")?;
        let id = new_id("unt");
        // New units land at the end of the wall order (the
        // folders' create does the same with its sort column).
        let sort: i64 = self
            .conn
            .query_row("SELECT COALESCE(MAX(sort),0)+1 FROM units", [], |r| {
                r.get(0)
            })
            .map_err(db_err)?;
        self.conn
            .execute(
                "INSERT INTO units(id, name, folder_id, group_id, maintainer, lead_lang, description, current_revision, created_ms, sort) VALUES(?1,?2,?3,?4,?5,?6,?7,1,?8,?9)",
                params![id, name, folder_id, group_id, maintainer, lead_lang, description, now_ms(), sort],
            )
            .map_err(db_err)?;
        self.get_unit(&id)
    }

    pub fn get_unit(&self, id: &str) -> SResult<UnitRec> {
        self.conn
            .query_row(
                "SELECT id, name, folder_id, group_id, maintainer, lead_lang, description, current_revision, created_ms, sort FROM units WHERE id=?1",
                params![id],
                Self::row_to_unit,
            )
            .map_err(db_err)
    }

    pub fn list_units(&self, visible: Option<&HashSet<String>>) -> SResult<Vec<UnitRec>> {
        let all = {
            let mut stmt = self
                .conn
                .prepare("SELECT id, name, folder_id, group_id, maintainer, lead_lang, description, current_revision, created_ms, sort FROM units ORDER BY sort, name")
                .map_err(db_err)?;
            let rows = stmt
                .query_map([], Self::row_to_unit)
                .map_err(db_err)?
                .collect::<Result<Vec<_>, _>>()
                .map_err(db_err)?;
            rows
        };
        let trashed = self.trashed_ids("unit")?;
        let all: Vec<UnitRec> = all
            .into_iter()
            .filter(|u| !trashed.contains(&u.id))
            .collect();
        Ok(match visible {
            None => all,
            Some(set) => all
                .into_iter()
                .filter(|u| set.contains(&u.group_id))
                .collect(),
        })
    }

    /// The units embedded in `unit_id` — the #101 「内嵌内容块」
    /// zone's data (D4). The data model keeps no block→block
    /// relation table: embedding exists only as page-entity
    /// containment. A version's page sequence is a list of
    /// content-addressed page entities shared by every block, so
    /// unit V is embedded in unit U exactly when V's lead
    /// rendition's latest version's page sequence occurs as a
    /// contiguous run inside U's lead rendition's latest version's
    /// page sequence — which is precisely what 组合为内容块 and
    /// paste-as-block produce (the combined block's pages ARE the
    /// source blocks' pages, in order). The containment must be
    /// strict (a verbatim copy is not an embedding) and the child
    /// must hold at least one page. Children are ordered by where
    /// their first page lands in the parent's sequence, then by
    /// name. `visible` filters candidates exactly like
    /// `list_units` (trashed units never appear). Each child's own
    /// embedded units ride along as `grandchildren` (one level —
    /// the zone only counts them). Cost is bounded: one
    /// `list_units` plus ONE join query fetching every unit's lead
    /// latest page sequence — no per-unit query fan-out, however
    /// many units the library holds. `limit` caps the child list.
    pub fn embedded_children(
        &self,
        unit_id: &str,
        visible: Option<&HashSet<String>>,
        limit: usize,
    ) -> SResult<Vec<EmbeddedChild>> {
        let units = self.list_units(visible)?;
        if !units.iter().any(|u| u.id == unit_id) {
            return Ok(Vec::new());
        }
        // Every unit's lead rendition's latest version's page
        // sequence, in one query. Lead rendition = first by
        // (form, lang) — renditions are UNIQUE(unit_id, form,
        // lang), so this is the same row `list_renditions` puts
        // first; latest version = first by the `list_versions`
        // ordering among non-recalled rows, as `latest_version`.
        let seqs: std::collections::HashMap<String, Vec<String>> = {
            let mut stmt = self
                .conn
                .prepare(
                    "SELECT u.id, v.pages_json FROM units u \
                     JOIN renditions r ON r.unit_id = u.id AND r.id = (\
                       SELECT r2.id FROM renditions r2 WHERE r2.unit_id = u.id \
                       ORDER BY r2.form, r2.lang LIMIT 1) \
                     JOIN versions v ON v.rendition_id = r.id AND v.id = (\
                       SELECT v2.id FROM versions v2 WHERE v2.rendition_id = r.id \
                       AND v2.status != 'recalled' \
                       ORDER BY v2.major DESC, v2.minor DESC, v2.patch DESC, \
                       v2.created_ms DESC, v2.rowid DESC LIMIT 1)",
                )
                .map_err(db_err)?;
            let rows = stmt
                .query_map([], |r| {
                    Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
                })
                .map_err(db_err)?
                .collect::<Result<Vec<_>, _>>()
                .map_err(db_err)?;
            rows.into_iter()
                .map(|(uid, pages_json)| {
                    let pages: Vec<PageInfo> =
                        serde_json::from_str(&pages_json).unwrap_or_default();
                    let seq = pages
                        .iter()
                        .map(|p| {
                            if p.id.is_empty() {
                                page_id_for_fp(&p.fp)
                            } else {
                                p.id.clone()
                            }
                        })
                        .collect();
                    (uid, seq)
                })
                .collect()
        };
        let parent_seq = match seqs.get(unit_id) {
            Some(s) if s.len() >= 2 => s.clone(),
            _ => return Ok(Vec::new()),
        };
        // (position in parent, unit) for every strict containment.
        let mut found: Vec<(usize, &UnitRec)> = Vec::new();
        for u in &units {
            if u.id == unit_id {
                continue;
            }
            let Some(seq) = seqs.get(&u.id) else {
                continue;
            };
            if seq.is_empty() || seq.len() >= parent_seq.len() {
                continue;
            }
            if let Some(pos) = subseq_pos(&parent_seq, seq) {
                found.push((pos, u));
            }
        }
        found.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.name.cmp(&b.1.name)));
        found.truncate(limit);
        let mut out = Vec::new();
        for (_, child) in found {
            let child_seq = seqs.get(&child.id).cloned().unwrap_or_default();
            let mut grands: Vec<(usize, &UnitRec)> = Vec::new();
            if child_seq.len() >= 2 {
                for u in &units {
                    if u.id == child.id || u.id == unit_id {
                        continue;
                    }
                    let Some(seq) = seqs.get(&u.id) else {
                        continue;
                    };
                    if seq.is_empty() || seq.len() >= child_seq.len() {
                        continue;
                    }
                    if let Some(pos) = subseq_pos(&child_seq, seq) {
                        grands.push((pos, u));
                    }
                }
                grands.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.name.cmp(&b.1.name)));
            }
            out.push(EmbeddedChild {
                unit: child.clone(),
                grandchildren: grands.into_iter().map(|(_, u)| u.clone()).collect(),
            });
        }
        Ok(out)
    }

    pub fn update_unit(
        &self,
        id: &str,
        name: Option<&str>,
        folder_id: Option<Option<&str>>,
        maintainer: Option<&str>,
        description: Option<&str>,
    ) -> SResult<UnitRec> {
        let mut u = self.get_unit(id)?;
        if let Some(n) = name {
            u.name = n.into();
        }
        if let Some(f) = folder_id {
            // Moving a unit is a home change like creating one
            // (audit N3): the create-side scope gate must hold
            // here too, or sidebar isolation is creation-time
            // discipline only and the unit lands in another
            // surface's tree (or a dangling folder) and vanishes
            // from every wall.
            self.check_home_folder(f, "content", "内容块")?;
            u.folder_id = f.map(|s| s.to_string());
        }
        if let Some(m) = maintainer {
            u.maintainer = m.into();
        }
        if let Some(d) = description {
            u.description = d.into();
        }
        self.conn
            .execute(
                "UPDATE units SET name=?2, folder_id=?3, maintainer=?4, description=?5 WHERE id=?1",
                params![u.id, u.name, u.folder_id, u.maintainer, u.description],
            )
            .map_err(db_err)?;
        self.get_unit(id)
    }

    pub fn bump_unit_revision(&self, unit_id: &str) -> SResult<i64> {
        self.conn
            .execute(
                "UPDATE units SET current_revision = current_revision + 1 WHERE id=?1",
                params![unit_id],
            )
            .map_err(db_err)?;
        Ok(self.get_unit(unit_id)?.current_revision)
    }

    /// Move one or more units (content-wall card drag-and-drop,
    /// user ruling 2026-11-25): rehome the batch into
    /// `target_folder` (None = the folder-less tail group) and
    /// seat it in front of `before_id` there (None = last). The
    /// batch keeps its current relative order. Persistence is
    /// the folders' caliber, not a new one: the single global
    /// `sort` column is renumbered over the whole library along
    /// folder groups, which is exactly the order `list_units`
    /// returns and the wall renders group by group, so every
    /// folder's relative card order is preserved. The home gate
    /// is `update_unit`'s (`check_home_folder`): the target must
    /// be a content-scope folder, so a drag can never file a
    /// block into another surface's tree. A `before_id` that is
    /// inside the batch or absent from the target group seats
    /// the batch at the group's end.
    pub fn move_units(
        &self,
        ids: &[String],
        target_folder: Option<&str>,
        before_id: Option<&str>,
    ) -> SResult<()> {
        if ids.is_empty() {
            return Ok(());
        }
        self.check_home_folder(target_folder, "content", "内容块")?;
        let all = self.list_units(None)?;
        let folders = self.list_folders()?;
        // Group key of a unit: its folder's id when that folder
        // still exists, "" for the folder-less tail group (the
        // wall renders dangling-folder units in the tail group
        // too, so the two share one order).
        let key_of = |u: &UnitRec| -> String {
            match &u.folder_id {
                Some(fid) if folders.iter().any(|f| &f.id == fid) => fid.clone(),
                _ => String::new(),
            }
        };
        // The batch in current list (display) order.
        let want: HashSet<&String> = ids.iter().collect();
        let mut batch: Vec<String> = Vec::new();
        for u in &all {
            if want.contains(&u.id) && !batch.contains(&u.id) {
                batch.push(u.id.clone());
            }
        }
        if batch.len() != want.len() {
            return Err("要移动的内容块不存在".into());
        }
        let target_key = match target_folder {
            Some(fid) if !fid.is_empty() => fid.to_string(),
            _ => String::new(),
        };
        // Per-folder groups, each in current list order.
        let mut groups: std::collections::HashMap<String, Vec<String>> =
            std::collections::HashMap::new();
        for u in &all {
            groups.entry(key_of(u)).or_default().push(u.id.clone());
        }
        for list in groups.values_mut() {
            list.retain(|x| !batch.contains(x));
        }
        let list = groups.entry(target_key.clone()).or_default();
        let pos = before_id
            .and_then(|b| list.iter().position(|x| x == b))
            .unwrap_or(list.len());
        for (off, id) in batch.iter().enumerate() {
            list.insert(pos + off, id.clone());
        }
        // Renumber along the groups: folders in folder-list
        // order, the tail group last. Only intra-group order is
        // visible (the wall groups by folder), and this walk
        // preserves every group's relative order.
        let mut order: Vec<String> = Vec::new();
        for f in &folders {
            if let Some(list) = groups.get(&f.id) {
                order.extend(list.iter().cloned());
            }
        }
        if let Some(list) = groups.get("") {
            order.extend(list.iter().cloned());
        }
        for (key, list) in &groups {
            if !key.is_empty() && !folders.iter().any(|f| &f.id == key) {
                order.extend(list.iter().cloned());
            }
        }
        let tx = self.conn.unchecked_transaction().map_err(db_err)?;
        let new_home: Option<&str> = if target_key.is_empty() {
            None
        } else {
            Some(target_key.as_str())
        };
        for id in &batch {
            tx.execute(
                "UPDATE units SET folder_id=?2 WHERE id=?1",
                params![id, new_home],
            )
            .map_err(db_err)?;
        }
        for (idx, uid) in order.iter().enumerate() {
            tx.execute(
                "UPDATE units SET sort=?2 WHERE id=?1",
                params![uid, (idx + 1) as i64],
            )
            .map_err(db_err)?;
        }
        tx.commit().map_err(db_err)?;
        Ok(())
    }

    pub fn get_or_create_rendition(
        &self,
        unit_id: &str,
        form: &str,
        lang: &str,
    ) -> SResult<RenditionRec> {
        let existing = self
            .conn
            .query_row(
                "SELECT id, unit_id, form, lang FROM renditions WHERE unit_id=?1 AND form=?2 AND lang=?3",
                params![unit_id, form, lang],
                |r| {
                    Ok(RenditionRec {
                        id: r.get(0)?,
                        unit_id: r.get(1)?,
                        form: r.get(2)?,
                        lang: r.get(3)?,
                    })
                },
            )
            .ok();
        if let Some(r) = existing {
            return Ok(r);
        }
        let id = new_id("rnd");
        self.conn
            .execute(
                "INSERT INTO renditions(id, unit_id, form, lang) VALUES(?1,?2,?3,?4)",
                params![id, unit_id, form, lang],
            )
            .map_err(db_err)?;
        Ok(RenditionRec {
            id,
            unit_id: unit_id.into(),
            form: form.into(),
            lang: lang.into(),
        })
    }

    pub fn get_rendition(&self, id: &str) -> SResult<RenditionRec> {
        self.conn
            .query_row(
                "SELECT id, unit_id, form, lang FROM renditions WHERE id=?1",
                params![id],
                |r| {
                    Ok(RenditionRec {
                        id: r.get(0)?,
                        unit_id: r.get(1)?,
                        form: r.get(2)?,
                        lang: r.get(3)?,
                    })
                },
            )
            .map_err(db_err)
    }

    pub fn list_renditions(&self, unit_id: &str) -> SResult<Vec<RenditionRec>> {
        let mut stmt = self
            .conn
            .prepare("SELECT id, unit_id, form, lang FROM renditions WHERE unit_id=?1 ORDER BY form, lang")
            .map_err(db_err)?;
        let rows = stmt
            .query_map(params![unit_id], |r| {
                Ok(RenditionRec {
                    id: r.get(0)?,
                    unit_id: r.get(1)?,
                    form: r.get(2)?,
                    lang: r.get(3)?,
                })
            })
            .map_err(db_err)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(db_err)?;
        Ok(rows)
    }

    fn row_to_version(r: &rusqlite::Row) -> rusqlite::Result<VersionRec> {
        let pages_json: String = r.get(9)?;
        let scripts_json: String = r.get(11)?;
        Ok(VersionRec {
            id: r.get(0)?,
            rendition_id: r.get(1)?,
            semver: r.get(2)?,
            basis_revision: r.get(3)?,
            author: r.get(4)?,
            note: r.get(5)?,
            created_ms: r.get(6)?,
            fragment_hash: r.get(7)?,
            page_count: r.get(8)?,
            pages: serde_json::from_str(&pages_json).unwrap_or_default(),
            status: r.get(10)?,
            scripts: serde_json::from_str(&scripts_json).unwrap_or_default(),
            line_pos: r.get(12)?,
            pub_reason: r.get(13)?,
            pub_by: r.get(14)?,
            pub_at: r.get(15)?,
        })
    }

    const VERSION_COLS: &'static str = "id, rendition_id, semver, basis_revision, author, note, created_ms, fragment_hash, page_count, pages_json, status, scripts_json, line_pos, pub_reason, pub_by, pub_at";

    /// Insert a version from a fragment PPTX: computes per-page fingerprints,
    /// titles and searchable text from the fragment itself.
    pub fn insert_version(
        &self,
        rendition_id: &str,
        semver: SemVer,
        basis_revision: i64,
        author: &str,
        note: &str,
        fragment: &[u8],
        status: &str,
    ) -> SResult<VersionRec> {
        self.insert_version_with_scripts(
            rendition_id,
            semver,
            basis_revision,
            author,
            note,
            fragment,
            status,
            None,
        )
    }

    pub fn insert_version_with_scripts(
        &self,
        rendition_id: &str,
        semver: SemVer,
        basis_revision: i64,
        author: &str,
        note: &str,
        fragment: &[u8],
        status: &str,
        scripts_override: Option<&[String]>,
    ) -> SResult<VersionRec> {
        self.check_semver_free(rendition_id, semver, None)?;
        let pkg = Package::load(fragment)?;
        let slide_refs = crate::pptx::slides(&pkg)?;
        let mut pages = Vec::new();
        let mut texts = Vec::new();
        let mut scripts = Vec::new();
        for s in &slide_refs {
            let xml = pkg.get_text(&s.part)?;
            pages.push(PageInfo {
                id: String::new(),
                fp: s.fingerprint.clone(),
                title: slide_title(&xml),
                desc: slide_desc(&xml),
            });
            texts.push(slide_full_text(&xml));
            scripts.push(crate::text::notes_text(&pkg, &s.part));
        }
        if let Some(override_notes) = scripts_override {
            if override_notes.len() != pages.len() {
                return Err("讲稿数量与页面数量不匹配".into());
            }
            scripts = override_notes.to_vec();
        }
        // Page entity-ization: split the fragment and register
        // every page as an independent entity; the version stores
        // the ordered page-reference sequence (pages_json carries
        // each page's entity id). Unchanged pages re-register to
        // their existing entities, so editing one page of a version
        // mints exactly one new page entity and leaves every other
        // reference untouched. If the split disagrees with the
        // slide listing (exotic package), the version degrades to
        // id-less pages, which read through the fragment fallback.
        // Registration failures are no longer swallowed whole
        // (audit N8): a version whose pages all failed to
        // register would carry an id-less reference sequence,
        // and id-addressed excerpts over it (notably a landed
        // version's) silently resolve to nothing. A published
        // (normal) version in that state is refused outright;
        // partial failure keeps the version — reads fall back
        // to the fragment — but is reported on stderr.
        let mut entity_failures = 0usize;
        match crate::pptx::split(&pkg) {
            Ok(singles) if singles.len() == pages.len() => {
                for (i, page) in pages.iter_mut().enumerate() {
                    match singles[i]
                        .to_bytes()
                        .and_then(|b| self.register_page(&page.fp, &page.title, &page.desc, &b))
                    {
                        Ok(rec) => page.id = rec.id,
                        Err(_) => entity_failures += 1,
                    }
                }
            }
            _ => entity_failures = pages.len(),
        }
        if entity_failures > 0 {
            eprintln!(
                "insert_version: {entity_failures}/{} pages failed entity registration (rendition {rendition_id})",
                pages.len()
            );
            if entity_failures == pages.len() && status == "normal" {
                return Err("页面实体登记全部失败，已发布的版本未写入".into());
            }
        }
        let hash = self.file_put(fragment)?;
        let id = new_id("ver");
        let line_pos = self.birth_line_pos(rendition_id)?;
        self.conn
            .execute(
                "INSERT INTO versions(id, rendition_id, semver, major, minor, patch, basis_revision, author, note, created_ms, fragment_hash, page_count, pages_json, text_all, status, scripts_json, line_pos) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17)",
                params![
                    id,
                    rendition_id,
                    semver.to_string(),
                    semver_part(semver.major),
                    semver_part(semver.minor),
                    semver_part(semver.patch),
                    basis_revision,
                    author,
                    note,
                    now_ms(),
                    hash,
                    pages.len() as i64,
                    serde_json::to_string(&pages).unwrap_or_default(),
                    texts.join("\n"),
                    status,
                    serde_json::to_string(&scripts).unwrap_or_default(),
                    line_pos
                ],
            )
            .map_err(db_err)?;
        self.get_version(&id)
    }

    /// Insert a version directly from a page-reference sequence:
    /// `pages` already carry their page entity ids, and the caller
    /// supplies the per-page searchable texts and speaker scripts
    /// aligned with them. Unlike `insert_version_with_scripts`
    /// this performs NO package work at all — no load, no split,
    /// no fingerprinting — because the staged import (line ⑤) has
    /// already registered every page as an entity in its single
    /// pass over the source; the version's structure IS this
    /// reference sequence. `fragment` is the version's
    /// materialized fragment bytes (for a one-page version, the
    /// page's own single-page package; for a grouped version, the
    /// one assembly of the member singles) — the byte-exact chains
    /// downstream (submission staging verifies SHA-256 against
    /// `fragment_hash`) require it to exist, so a caller must
    /// supply it: `None` is refused at birth (audit N8, below).
    /// The one legitimate byte-less birth — a lazy-import wrapper
    /// whose page has not been split yet — goes through
    /// `insert_pending_version_from_pages`, never through here.
    #[allow(clippy::too_many_arguments)]
    pub fn insert_version_from_pages(
        &self,
        rendition_id: &str,
        semver: SemVer,
        basis_revision: i64,
        author: &str,
        note: &str,
        status: &str,
        pages: &[PageInfo],
        texts: &[String],
        scripts: &[String],
        fragment: Option<&[u8]>,
    ) -> SResult<VersionRec> {
        // N8 (chain line): a None fragment used to land as
        // fragment_hash = "" and only failed much later, far
        // from the cause — at submission-package build, where
        // the byte chain reads the hash back and finds nothing.
        // Fail here, at the version's birth, with the cause
        // named: every caller of THIS entry has the bytes, and a
        // caller with nothing byte-facing must not create
        // byte-chained versions.
        let hash = match fragment {
            Some(bytes) => self.file_put(bytes)?,
            None => {
                return Err(
                    "建版本缺少片段字节：无字节的版本会让投稿组包在远处失败，请传入片段"
                        .into(),
                )
            }
        };
        self.insert_version_from_pages_hashed(
            rendition_id,
            semver,
            basis_revision,
            author,
            note,
            status,
            pages,
            texts,
            scripts,
            hash,
        )
    }

    /// Insert a NOT-YET-MATERIALIZED version from a page-reference
    /// sequence: the lazy import's wrapper birth. The page has
    /// not been split at open time — splitting it eagerly would
    /// defeat the lazy open — so the version is born with an
    /// empty `fragment_hash` and is owed exactly one future
    /// write: when the page's split lands, `lazy_split_page`
    /// registers the page entity and points this version at the
    /// entity's stored package via `set_version_fragment_hash`,
    /// after which the wrapper is indistinguishable from a
    /// staged import's stage-1 version.
    ///
    /// This does NOT reopen the hole audit N8 closed. The N8
    /// guard on `insert_version_from_pages` stands untouched for
    /// every byte-facing caller; this entry is reachable only
    /// from `lazy_open`, and the version it creates lives inside
    /// a durable `pending_imports` record that tracks the debt
    /// until the split pays it. The byte chains are guarded at
    /// their own doors in the meantime: `version_fragment_bytes`
    /// and `version_page_packages_legacy` refuse an empty-hash
    /// version with the cause named, at the point of use, instead
    /// of failing far away on a missing content-addressed file.
    #[allow(clippy::too_many_arguments)]
    pub fn insert_pending_version_from_pages(
        &self,
        rendition_id: &str,
        semver: SemVer,
        basis_revision: i64,
        author: &str,
        note: &str,
        status: &str,
        pages: &[PageInfo],
        texts: &[String],
        scripts: &[String],
    ) -> SResult<VersionRec> {
        self.insert_version_from_pages_hashed(
            rendition_id,
            semver,
            basis_revision,
            author,
            note,
            status,
            pages,
            texts,
            scripts,
            String::new(),
        )
    }

    /// The shared insert body behind `insert_version_from_pages`
    /// and `insert_pending_version_from_pages`: the reference
    /// sequence checks and the row write, with the fragment hash
    /// already decided by the entry point.
    #[allow(clippy::too_many_arguments)]
    fn insert_version_from_pages_hashed(
        &self,
        rendition_id: &str,
        semver: SemVer,
        basis_revision: i64,
        author: &str,
        note: &str,
        status: &str,
        pages: &[PageInfo],
        texts: &[String],
        scripts: &[String],
        hash: String,
    ) -> SResult<VersionRec> {
        if scripts.len() != pages.len() {
            return Err("讲稿数量与页面数量不匹配".into());
        }
        if texts.len() != pages.len() {
            return Err("页面文本数量与页面数量不匹配".into());
        }
        // This API's whole contract is the reference sequence:
        // a page without an entity id is not a reference
        // (audit N8) — refuse instead of storing a sequence
        // excerpt resolution will silently read as empty.
        if pages.iter().any(|p| p.id.is_empty()) {
            return Err("页引用缺少实体 id，版本未写入".into());
        }
        self.check_semver_free(rendition_id, semver, None)?;
        let id = new_id("ver");
        let line_pos = self.birth_line_pos(rendition_id)?;
        self.conn
            .execute(
                "INSERT INTO versions(id, rendition_id, semver, major, minor, patch, basis_revision, author, note, created_ms, fragment_hash, page_count, pages_json, text_all, status, scripts_json, line_pos) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17)",
                params![
                    id,
                    rendition_id,
                    semver.to_string(),
                    semver_part(semver.major),
                    semver_part(semver.minor),
                    semver_part(semver.patch),
                    basis_revision,
                    author,
                    note,
                    now_ms(),
                    hash,
                    pages.len() as i64,
                    serde_json::to_string(&pages).unwrap_or_default(),
                    texts.join("\n"),
                    status,
                    serde_json::to_string(&scripts).unwrap_or_default(),
                    line_pos
                ],
            )
            .map_err(db_err)?;
        self.get_version(&id)
    }

    /// Replace a version's per-page scripts. Editing scripts never bumps
    /// the version or the content revision (§3.16).
    pub fn set_version_scripts(&self, version_id: &str, scripts: &[String]) -> SResult<()> {
        let n = self
            .conn
            .execute(
                "UPDATE versions SET scripts_json=?2 WHERE id=?1",
                params![
                    version_id,
                    serde_json::to_string(scripts).unwrap_or_default()
                ],
            )
            .map_err(db_err)?;
        if n == 0 {
            return Err("版本不存在".into());
        }
        Ok(())
    }

    /// Rewrite a DRAFT version's page order in place (the #101
    /// draft wall's drag reorder, user ruling 2026-11-25). The
    /// draft is the working copy, so — like `set_version_scripts`
    /// — the edit lands on the version row itself and never bumps
    /// a version; published (normal / recalled) versions are
    /// immutable history and are refused at this door.
    /// `page_ids` must be exactly the draft's current page
    /// references in the new order (same multiset; duplicates
    /// matched by occurrence). Pages and their aligned scripts
    /// move together, and the stored fragment is rebuilt from
    /// the page packages in the new order through the birth
    /// path's own `pptx::reassemble`, so fragment-order
    /// consumers (fragment download, the page editor's base
    /// package, legacy id-less thumbs) can never diverge from
    /// the reference sequence. Bindings keep addressing the same
    /// pages: their page-id columns already name them, and their
    /// slot columns are rewritten through the same permutation
    /// (a missing mark's to_slot = -1 addresses no page and
    /// stays). An identity order is a no-op success — no write.
    pub fn reorder_draft_pages(
        &self,
        version_id: &str,
        page_ids: &[String],
    ) -> SResult<VersionRec> {
        let ver = self.get_version(version_id)?;
        // Numbered drafts and the unnumbered working draft are
        // both editable working copies; published / recalled /
        // rejected versions are closed history.
        if ver.status != "draft" && ver.status != "working" {
            return Err("只有草稿版本可以调整页序".into());
        }
        let cur: Vec<String> = ver.pages.iter().map(|p| p.id.clone()).collect();
        if cur.iter().any(|id| id.is_empty()) {
            return Err("该草稿的页面尚未实体化，无法调整页序".into());
        }
        if page_ids.len() != cur.len() {
            return Err("页序与草稿页面不一致，请刷新后重试".into());
        }
        // perm[new_pos] = old_pos, occurrence-stable so a page
        // referenced twice keeps its two slots distinguishable.
        let mut used = vec![false; cur.len()];
        let mut perm: Vec<usize> = Vec::with_capacity(cur.len());
        for id in page_ids {
            match cur.iter().enumerate().find(|(i, c)| !used[*i] && *c == id) {
                Some((i, _)) => {
                    used[i] = true;
                    perm.push(i);
                }
                None => return Err("页序与草稿页面不一致，请刷新后重试".into()),
            }
        }
        if perm.iter().enumerate().all(|(new, old)| new == *old) {
            return Ok(ver);
        }
        // Rebuild the fragment in the new order BEFORE touching
        // the row: any failure here leaves the draft untouched.
        let singles = self.version_page_packages(&ver)?;
        if singles.len() != cur.len() {
            return Err("草稿页面组包数量不一致，无法调整页序".into());
        }
        let ordered: Vec<Package> = perm.iter().map(|&old| singles[old].clone()).collect();
        let frag = crate::pptx::reassemble(&ordered)?;
        let hash = self.file_put(&frag.to_bytes()?)?;
        let pages: Vec<PageInfo> = perm.iter().map(|&old| ver.pages[old].clone()).collect();
        let scripts: Vec<String> = perm
            .iter()
            .map(|&old| ver.scripts.get(old).cloned().unwrap_or_default())
            .collect();
        // text_all is the pages' search concatenation: rebuild
        // it in the new order when every page's text re-reads
        // cleanly; a page that cannot be re-read keeps the
        // stored concatenation (search matching is
        // order-insensitive) rather than failing the reorder.
        let text_all = ordered
            .iter()
            .map(|pkg| -> Option<String> {
                let slide = crate::pptx::slides(pkg).ok()?.into_iter().next()?;
                let xml = pkg.get_text(&slide.part).ok()?;
                Some(crate::text::slide_full_text(&xml))
            })
            .collect::<Option<Vec<_>>>()
            .map(|texts| texts.join("\n"));
        let pages_json = serde_json::to_string(&pages).unwrap_or_default();
        let scripts_json = serde_json::to_string(&scripts).unwrap_or_default();
        match text_all {
            Some(text_all) => {
                self.conn
                    .execute(
                        "UPDATE versions SET pages_json=?2, scripts_json=?3, fragment_hash=?4, text_all=?5 WHERE id=?1",
                        params![version_id, pages_json, scripts_json, hash, text_all],
                    )
                    .map_err(db_err)?;
            }
            None => {
                self.conn
                    .execute(
                        "UPDATE versions SET pages_json=?2, scripts_json=?3, fragment_hash=?4 WHERE id=?1",
                        params![version_id, pages_json, scripts_json, hash],
                    )
                    .map_err(db_err)?;
            }
        }
        // Binding slots that address this draft's pages follow
        // the same permutation; their page-id columns already
        // name the pages and do not change. Stale slots outside
        // the draft's range are left as they lie.
        let rend = self.get_rendition(&ver.rendition_id)?;
        let mut new_pos_of = vec![0usize; perm.len()];
        for (new, old) in perm.iter().enumerate() {
            new_pos_of[*old] = new;
        }
        for b in self.list_page_bindings(&rend.unit_id)? {
            let remap = |slot: i64, hit: bool| -> i64 {
                if hit && slot >= 0 && (slot as usize) < new_pos_of.len() {
                    new_pos_of[slot as usize] as i64
                } else {
                    slot
                }
            };
            let from_slot = remap(b.from_slot, b.from_version_id == version_id);
            let to_slot = remap(b.to_slot, b.to_version_id == version_id);
            if from_slot != b.from_slot || to_slot != b.to_slot {
                self.conn
                    .execute(
                        "UPDATE page_bindings SET from_slot=?2, to_slot=?3 WHERE id=?1",
                        params![b.id, from_slot, to_slot],
                    )
                    .map_err(db_err)?;
            }
        }
        self.get_version(version_id)
    }

    pub fn get_version(&self, id: &str) -> SResult<VersionRec> {
        self.conn
            .query_row(
                &format!("SELECT {} FROM versions WHERE id=?1", Self::VERSION_COLS),
                params![id],
                Self::row_to_version,
            )
            .map_err(db_err)
    }

    /// Versions of a rendition, newest (highest semver) first.
    /// One version number per rendition (audit L1): inserting or
    /// renumbering onto an existing (major, minor, patch) is
    /// refused, so "the latest" and every resolution over the
    /// rendition stay deterministic. A best-effort unique index
    /// (Store::open) backs this on databases that can take it.
    fn check_semver_free(
        &self,
        rendition_id: &str,
        semver: SemVer,
        exclude_id: Option<&str>,
    ) -> SResult<()> {
        let clash: bool = self
            .conn
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM versions WHERE rendition_id=?1 AND major=?2 AND minor=?3 AND patch=?4 AND id IS NOT ?5)",
                params![
                    rendition_id,
                    semver_part(semver.major),
                    semver_part(semver.minor),
                    semver_part(semver.patch),
                    exclude_id
                ],
                |r| r.get(0),
            )
            .map_err(db_err)?;
        if clash {
            return Err(format!(
                "该形态版本已存在版本号 {semver}，不能重复使用"
            ));
        }
        Ok(())
    }

    pub fn list_versions(&self, rendition_id: &str) -> SResult<Vec<VersionRec>> {
        let mut stmt = self
            .conn
            .prepare(&format!(
                "SELECT {} FROM versions WHERE rendition_id=?1 ORDER BY major DESC, minor DESC, patch DESC, created_ms DESC, rowid DESC",
                Self::VERSION_COLS
            ))
            .map_err(db_err)?;
        let rows = stmt
            .query_map(params![rendition_id], Self::row_to_version)
            .map_err(db_err)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(db_err)?;
        // Trashed versions hide exactly like trashed units /
        // indexes / showroom items hide from their lists (the
        // trash overlay): every consumer of this list — the
        // detail history, latest_version, published resolution,
        // import lookups — sees the version as deleted until the
        // trash row is restored, and purge makes it permanent.
        // Direct reads (get_version) are unaffected, which is
        // what purge / restore / cover resolution rely on.
        let trashed = self.trashed_ids("version")?;
        Ok(rows
            .into_iter()
            .filter(|v| !trashed.contains(&v.id))
            .collect())
    }

    /// The newest surviving version, drafts included: this is
    /// the working-copy accessor the editing faces use (a draft
    /// IS the latest work). Published resolution — what an
    /// index entry, a showroom item or a guest may see — is
    /// `resolve_entry_version`, whose follow-latest arm only
    /// ever lands on a formally published (normal) version
    /// (audit M2). Callers must not serve content to a
    /// distribution face from this accessor.
    pub fn latest_version(&self, rendition_id: &str) -> SResult<Option<VersionRec>> {
        Ok(self.list_versions(rendition_id)?.into_iter().find(|v| {
            // The unnumbered working draft and a not-passed
            // version are never the working head: the draft is
            // reached through `working_version`, and a rejected
            // version is closed business like a recalled one.
            v.status != "recalled" && v.status != "working" && v.status != "rejected"
        }))
    }

    pub fn set_version_status(&self, id: &str, status: &str) -> SResult<()> {
        let n = self
            .conn
            .execute(
                "UPDATE versions SET status=?2 WHERE id=?1",
                params![id, status],
            )
            .map_err(db_err)?;
        if n == 0 {
            return Err("版本不存在".into());
        }
        Ok(())
    }

    pub fn delete_version(&self, id: &str) -> SResult<()> {
        // Publish-pin guard (pin audit N1): a version a live
        // showroom item pins as its formal version is not
        // deletable — deleting it would silently re-resolve the
        // item onto a substitute. The caller must re-push the
        // item or take it down first. Pins of non-live items do
        // not block: their items are not publishing anything.
        let pinned_live: i64 = self
            .conn
            .query_row(
                "SELECT COUNT(*) FROM showroom_pins p JOIN showroom s ON s.id = p.showroom_id WHERE p.version_id = ?1 AND s.state = 'live'",
                params![id],
                |r| r.get(0),
            )
            .map_err(db_err)?;
        if pinned_live > 0 {
            return Err(
                "该版本正被演示厅条目钉为正式版，不能删除；请先更新该条目的演示推送或将其下架"
                    .into(),
            );
        }
        // Cascade (audit L8): the version's tags and the page
        // bindings addressed at it die with it, instead of
        // lingering as orphans consumers trip over. (Whether a
        // version pinned by a live showroom item may be deleted
        // at all is the pin line's guard, applied separately.)
        self.conn
            .execute("DELETE FROM version_tags WHERE version_id=?1", params![id])
            .map_err(db_err)?;
        self.conn
            .execute(
                "DELETE FROM page_bindings WHERE from_version_id=?1 OR to_version_id=?1",
                params![id],
            )
            .map_err(db_err)?;
        // Governance rows addressed at the version die with it
        // too (the renumber ledger is kept — it is an audit
        // trail, not working state).
        self.conn
            .execute(
                "DELETE FROM version_lang_mounts WHERE version_id=?1",
                params![id],
            )
            .map_err(db_err)?;
        self.conn
            .execute(
                "DELETE FROM version_use_sel WHERE version_id=?1",
                params![id],
            )
            .map_err(db_err)?;
        let n = self
            .conn
            .execute("DELETE FROM versions WHERE id=?1", params![id])
            .map_err(db_err)?;
        if n == 0 {
            return Err("版本不存在".into());
        }
        Ok(())
    }

    pub fn renumber_version(&self, id: &str, semver: SemVer) -> SResult<()> {
        let cur = self.get_version(id)?;
        self.check_semver_free(&cur.rendition_id, semver, Some(id))?;
        let n = self
            .conn
            .execute(
                "UPDATE versions SET semver=?2, major=?3, minor=?4, patch=?5 WHERE id=?1",
                params![
                    id,
                    semver.to_string(),
                    semver_part(semver.major),
                    semver_part(semver.minor),
                    semver_part(semver.patch)
                ],
            )
            .map_err(db_err)?;
        if n == 0 {
            return Err("版本不存在".into());
        }
        Ok(())
    }

    // ---------- publish governance (user ruling 2026-11-25) ----------
    //
    // The governance model the #101 rebuild carries: an
    // UNNUMBERED working draft (status "working", semver '' with
    // major/minor/patch = -1 so the rendition-semver unique index
    // and `check_semver_free` never see it as a number), a
    // not-passed state (status "rejected" + pub_* fields), a
    // renumber ledger (`version_renumbers`), a per-rendition
    // version-use marker (`version_use_sel`), explicit history
    // language mounts (`version_lang_mounts`), and a user-arranged
    // display order for the version line (`versions.line_pos`).
    // Numbered drafts (status "draft") are the stock form and
    // keep their existing publish path untouched.

    /// Line position for a version born into a rendition whose
    /// line the user has arranged by dragging history cards:
    /// the new version takes the head (min - 1). An unarranged
    /// line stays position-less, so the stock semver display
    /// order is untouched until the first arrangement.
    fn birth_line_pos(&self, rendition_id: &str) -> SResult<Option<i64>> {
        let min: Option<i64> = self
            .conn
            .query_row(
                "SELECT MIN(line_pos) FROM versions WHERE rendition_id=?1 AND line_pos IS NOT NULL",
                params![rendition_id],
                |r| r.get(0),
            )
            .map_err(db_err)?;
        Ok(min.map(|m| m - 1))
    }

    /// The rendition's unnumbered working draft, if one exists.
    pub fn working_version(&self, rendition_id: &str) -> SResult<Option<VersionRec>> {
        Ok(self
            .list_versions(rendition_id)?
            .into_iter()
            .find(|v| v.status == "working"))
    }

    /// Birth (or fetch) the rendition's unnumbered working
    /// draft: a copy of the current head version's page
    /// reference sequence and scripts, its fragment reassembled
    /// from the page packages exactly the way a draft reorder
    /// rebuilds it. Governance edits (page drags on a published
    /// version, binds, script edits) land on this row; publish
    /// converts it in place, so everything addressed at the row
    /// survives numbering. Refused while a NUMBERED draft is the
    /// head — that draft is the working copy then, and a second
    /// working surface would fork the edit stream.
    pub fn seed_working_draft(&self, rendition_id: &str, author: &str) -> SResult<VersionRec> {
        if let Some(w) = self.working_version(rendition_id)? {
            return Ok(w);
        }
        let src = self
            .latest_version(rendition_id)?
            .ok_or_else(|| "该形态尚无版本，无法建立未定号草稿".to_string())?;
        if src.status != "normal" {
            return Err("当前已有草稿版本，先处理草稿后再建立未定号草稿".into());
        }
        let singles = self.version_page_packages(&src)?;
        if singles.len() != src.pages.len() {
            return Err("源版本页面组包数量不一致，无法建立未定号草稿".into());
        }
        let frag = crate::pptx::reassemble(&singles)?;
        let hash = self.file_put(&frag.to_bytes()?)?;
        let text_all: String = self
            .conn
            .query_row(
                "SELECT text_all FROM versions WHERE id=?1",
                params![src.id],
                |r| r.get(0),
            )
            .map_err(db_err)?;
        let id = new_id("ver");
        self.conn
            .execute(
                "INSERT INTO versions(id, rendition_id, semver, major, minor, patch, basis_revision, author, note, created_ms, fragment_hash, page_count, pages_json, text_all, status, scripts_json) VALUES(?1,?2,'',-1,-1,-1,?3,?4,'',?5,?6,?7,?8,?9,'working',?10)",
                params![
                    id,
                    rendition_id,
                    src.basis_revision,
                    author,
                    now_ms(),
                    hash,
                    src.pages.len() as i64,
                    serde_json::to_string(&src.pages).unwrap_or_default(),
                    text_all,
                    serde_json::to_string(&src.scripts).unwrap_or_default()
                ],
            )
            .map_err(db_err)?;
        self.get_version(&id)
    }

    /// Publish the unnumbered working draft: the SAME row takes
    /// the selected number and turns normal. An empty note takes
    /// the prototype's default text with the count of pages the
    /// draft added over the published head.
    pub fn publish_working_draft(
        &self,
        version_id: &str,
        note: &str,
        author: &str,
        bump: Option<&str>,
    ) -> SResult<VersionRec> {
        let ver = self.get_version(version_id)?;
        if ver.status != "working" {
            return Err("只有未定号草稿可以这样发布".into());
        }
        let rend_id = ver.rendition_id.clone();
        let versions = self.list_versions(&rend_id)?;
        let head_normal = versions.iter().find(|v| v.status == "normal");
        let added = match head_normal {
            Some(h) => {
                let hs: HashSet<&str> = h.pages.iter().map(|p| p.id.as_str()).collect();
                ver.pages
                    .iter()
                    .filter(|p| !hs.contains(p.id.as_str()))
                    .count()
            }
            None => ver.pages.len(),
        };
        let inc = match bump {
            Some(value) => parse_increment(value)?,
            None => crate::semver::Increment::Minor,
        };
        let base_sv = head_normal
            .map(|v| v.semver.clone())
            .or_else(|| {
                versions
                    .iter()
                    .find(|v| v.status != "working")
                    .map(|v| v.semver.clone())
            })
            .and_then(|s| SemVer::parse(&s).ok())
            .unwrap_or(SemVer::new(0, 0, 0));
        let mut cand = base_sv.bumped(inc);
        // The candidate must be free across ALL rows of the
        // rendition (a numbered draft may already hold it).
        let mut guard = 0;
        while self.check_semver_free(&rend_id, cand, None).is_err() {
            cand = cand.bumped(inc);
            guard += 1;
            if guard > 10000 {
                return Err("版本号分配失败".into());
            }
        }
        let note_final = if note.trim().is_empty() {
            format!("草稿发布为正式最新版本（新增 {} 页）", added)
        } else {
            note.trim().to_string()
        };
        let line_pos = self.birth_line_pos(&rend_id)?;
        self.conn
            .execute(
                "UPDATE versions SET semver=?2, major=?3, minor=?4, patch=?5, status='normal', note=?6, author=?7, created_ms=?8, line_pos=?9 WHERE id=?1",
                params![
                    version_id,
                    cand.to_string(),
                    semver_part(cand.major),
                    semver_part(cand.minor),
                    semver_part(cand.patch),
                    note_final,
                    author,
                    now_ms(),
                    line_pos
                ],
            )
            .map_err(db_err)?;
        self.get_version(version_id)
    }

    /// Publish a numbered draft. Old callers omit `bump` and keep
    /// the preallocated number; the #101 dialog always selects a
    /// segment and publishes above the current normal head.
    pub fn publish_draft(
        &self,
        version_id: &str,
        note: Option<&str>,
        author: &str,
        bump: Option<&str>,
    ) -> SResult<VersionRec> {
        let ver = self.get_version(version_id)?;
        if ver.status != "draft" {
            return Err("只有草稿版本可以发布".into());
        }
        let Some(bump) = bump else {
            match note {
                Some(n) => {
                    self.conn
                        .execute(
                            "UPDATE versions SET status='normal', note=?2 WHERE id=?1",
                            params![version_id, n],
                        )
                        .map_err(db_err)?;
                }
                None => self.set_version_status(version_id, "normal")?,
            }
            return self.get_version(version_id);
        };
        let inc = parse_increment(bump)?;
        let rend_id = ver.rendition_id.clone();
        let versions = self.list_versions(&rend_id)?;
        let base = versions
            .iter()
            .find(|v| v.status == "normal")
            .map(|v| v.semver.clone())
            .unwrap_or_else(|| ver.semver.clone());
        let mut cand = SemVer::parse(&base)
            .map_err(|_| "当前版本编号无效".to_string())?
            .bumped(inc);
        let mut guard = 0;
        while self.check_semver_free(&rend_id, cand, Some(version_id)).is_err() {
            cand = cand.bumped(inc);
            guard += 1;
            if guard > 10000 {
                return Err("版本号分配失败".into());
            }
        }
        let note_final = note
            .map(|n| n.trim().to_string())
            .unwrap_or_else(|| ver.note.clone());
        let line_pos = self.birth_line_pos(&rend_id)?;
        let tx = self.conn.unchecked_transaction().map_err(db_err)?;
        tx.execute(
            "UPDATE versions SET semver=?2, major=?3, minor=?4, patch=?5, status='normal', note=?6, line_pos=?7 WHERE id=?1",
            params![
                version_id,
                cand.to_string(),
                semver_part(cand.major),
                semver_part(cand.minor),
                semver_part(cand.patch),
                note_final,
                line_pos
            ],
        )
        .map_err(db_err)?;
        tx.execute(
            "INSERT INTO version_renumbers(id, version_id, from_semver, to_semver, reason, by, at_ms) VALUES(?1,?2,?3,?4,?5,?6,?7)",
            params![
                new_id("rnm"),
                version_id,
                ver.semver,
                cand.to_string(),
                "草稿发布时选择版本类型",
                author,
                now_ms()
            ],
        )
        .map_err(db_err)?;
        tx.commit().map_err(db_err)?;
        self.get_version(version_id)
    }

    /// 定号插入： convert the unnumbered working draft into a
    /// numbered version slotted right after `base_version_id`
    /// in the line (the prototype's applyHistInsert): the
    /// inserted number is base's minor bump, and every version
    /// displayed NEWER than base is minor-bumped in turn (from
    /// base's neighbour toward the head, each strictly above
    /// the last assigned), every shift leaving a renumber
    /// record. The cascade runs over the DISPLAY order — the
    /// line as the user arranged it — exactly like the
    /// prototype's array. Returns the converted version and
    /// the (from, to) shift list in assignment order.
    pub fn insert_working_at(
        &self,
        version_id: &str,
        base_version_id: &str,
        author: &str,
    ) -> SResult<(VersionRec, Vec<(String, String)>)> {
        let ver = self.get_version(version_id)?;
        if ver.status != "working" {
            return Err("只有未定号草稿可以定号插入".into());
        }
        let base = self.get_version(base_version_id)?;
        if base.rendition_id != ver.rendition_id || base.status == "working" {
            return Err("插入基点无效".into());
        }
        let rend_id = ver.rendition_id.clone();
        let line = self.version_line_order(&rend_id)?;
        let Some(k) = line.iter().position(|v| v.id == base.id) else {
            return Err("插入基点不在版本线内".into());
        };
        let base_sv = SemVer::parse(&base.semver)
            .map_err(|_| "基点版本编号无效".to_string())?;
        let inserted = base_sv.bumped(crate::semver::Increment::Minor);
        let mut last = inserted;
        let mut plan: Vec<(String, SemVer, SemVer)> = Vec::new();
        for v in line[..k].iter().rev() {
            let from = SemVer::parse(&v.semver)
                .map_err(|_| "版本线编号无效".to_string())?;
            let mut nv = from.bumped(crate::semver::Increment::Minor);
            while nv <= last {
                nv = nv.bumped(crate::semver::Increment::Minor);
            }
            plan.push((v.id.clone(), from, nv));
            last = nv;
        }
        let at = now_ms();
        // Phase 1: park every shifting version on a temporary
        // high number so the unique index never sees two rows
        // holding the same number mid-cascade.
        for (idx, (vid, from, _)) in plan.iter().enumerate() {
            let tmp_major = from.major + 1_000_000 + idx as u64;
            self.conn
                .execute(
                    "UPDATE versions SET semver=?2, major=?3 WHERE id=?1",
                    params![vid, format!("{tmp_major}.{}.{}", from.minor, from.patch), tmp_major as i64],
                )
                .map_err(db_err)?;
        }
        // Phase 2: land the final numbers, each with its record.
        let reason = format!("插入新版 {} 于其后，编号级联顺移", inserted);
        for (vid, from, to) in &plan {
            self.conn
                .execute(
                    "UPDATE versions SET semver=?2, major=?3, minor=?4, patch=?5 WHERE id=?1",
                    params![
                        vid,
                        to.to_string(),
                        semver_part(to.major),
                        semver_part(to.minor),
                        semver_part(to.patch)
                    ],
                )
                .map_err(db_err)?;
            self.conn
                .execute(
                    "INSERT INTO version_renumbers(id, version_id, from_semver, to_semver, reason, by, at_ms) VALUES(?1,?2,?3,?4,?5,?6,?7)",
                    params![new_id("rnm"), vid, from.to_string(), to.to_string(), reason, author, at],
                )
                .map_err(db_err)?;
        }
        let note = format!(
            "手动拖拽插入归位的新版（以 {} 为基 · 小版本定号）",
            base.semver
        );
        self.conn
            .execute(
                "UPDATE versions SET semver=?2, major=?3, minor=?4, patch=?5, status='normal', note=?6, author=?7, created_ms=?8 WHERE id=?1",
                params![
                    version_id,
                    inserted.to_string(),
                    semver_part(inserted.major),
                    semver_part(inserted.minor),
                    semver_part(inserted.patch),
                    note,
                    author,
                    at
                ],
            )
            .map_err(db_err)?;
        self.conn
            .execute(
                "INSERT INTO version_renumbers(id, version_id, from_semver, to_semver, reason, by, at_ms) VALUES(?1,?2,'',?3,?4,?5,?6)",
                params![
                    new_id("rnm"),
                    version_id,
                    inserted.to_string(),
                    format!("手动拖拽插入定号（以 {} 为基 · 小版本定号）", base.semver),
                    author,
                    at
                ],
            )
            .map_err(db_err)?;
        // An arranged line keeps its arrangement: the inserted
        // version takes the slot right before its base.
        if line.iter().any(|v| v.line_pos.is_some()) {
            let mut ids: Vec<String> = line.iter().map(|v| v.id.clone()).collect();
            ids.insert(k, version_id.to_string());
            self.set_version_line_order(&rend_id, &ids)?;
        }
        Ok((
            self.get_version(version_id)?,
            plan.iter()
                .map(|(_, f, t)| (f.to_string(), t.to_string()))
                .collect(),
        ))
    }

    /// The rendition's numbered versions in DISPLAY order: the
    /// user's arranged order when any row carries a line
    /// position, else the stock semver order. The working
    /// draft is not part of the line (its card always leads).
    pub fn version_line_order(&self, rendition_id: &str) -> SResult<Vec<VersionRec>> {
        let mut versions: Vec<VersionRec> = self
            .list_versions(rendition_id)?
            .into_iter()
            .filter(|v| v.status != "working")
            .collect();
        if versions.iter().any(|v| v.line_pos.is_some()) {
            versions.sort_by_key(|v| v.line_pos.unwrap_or(i64::MAX));
        }
        Ok(versions)
    }

    /// Write a full display order for the rendition's line
    /// (history-card drag). The id set must be exactly the
    /// line's current numbered versions — a stale order from
    /// an out-of-date page is refused, never half-applied.
    pub fn set_version_line_order(
        &self,
        rendition_id: &str,
        version_ids: &[String],
    ) -> SResult<()> {
        let current = self.version_line_order(rendition_id)?;
        if version_ids.len() != current.len()
            || !current.iter().all(|v| version_ids.contains(&v.id))
        {
            return Err("版本线顺序与当前版本不一致，请刷新后重试".into());
        }
        for (pos, vid) in version_ids.iter().enumerate() {
            self.conn
                .execute(
                    "UPDATE versions SET line_pos=?2 WHERE id=?1 AND rendition_id=?3",
                    params![vid, pos as i64, rendition_id],
                )
                .map_err(db_err)?;
        }
        Ok(())
    }

    /// Reorder numbered versions and reassign the existing semver
    /// pool in descending display order. IDs and content stay put;
    /// every changed number is recorded in the renumber ledger.
    pub fn reorder_versions_recorded(
        &self,
        rendition_id: &str,
        version_ids: &[String],
        author: &str,
    ) -> SResult<Vec<(String, String, String)>> {
        let current = self.version_line_order(rendition_id)?;
        if version_ids.len() != current.len()
            || !current.iter().all(|v| version_ids.contains(&v.id))
        {
            return Err("版本线顺序与当前版本不一致，请刷新后重试".into());
        }
        if current
            .iter()
            .zip(version_ids)
            .all(|(version, id)| version.id == *id)
        {
            return Ok(Vec::new());
        }
        let mut pool: Vec<SemVer> = current
            .iter()
            .map(|v| SemVer::parse(&v.semver))
            .collect::<Result<_, _>>()?;
        pool.sort_by(|a, b| b.cmp(a));
        let mut plan = Vec::new();
        for (id, to) in version_ids.iter().zip(pool) {
            let from = current
                .iter()
                .find(|v| &v.id == id)
                .map(|v| SemVer::parse(&v.semver))
                .ok_or_else(|| "版本线编号无效".to_string())??;
            if from != to {
                plan.push((id.clone(), from, to));
            }
        }
        let tx = self.conn.unchecked_transaction().map_err(db_err)?;
        for (idx, (id, from, _)) in plan.iter().enumerate() {
            let tmp_major = i64::MIN + idx as i64;
            tx.execute(
                "UPDATE versions SET semver=?2, major=?3 WHERE id=?1",
                params![id, format!("{tmp_major}.{}.{}", from.minor, from.patch), tmp_major],
            )
            .map_err(db_err)?;
        }
        let at = now_ms();
        for (id, from, to) in &plan {
            tx.execute(
                "UPDATE versions SET semver=?2, major=?3, minor=?4, patch=?5 WHERE id=?1",
                params![
                    id,
                    to.to_string(),
                    semver_part(to.major),
                    semver_part(to.minor),
                    semver_part(to.patch)
                ],
            )
            .map_err(db_err)?;
            tx.execute(
                "INSERT INTO version_renumbers(id, version_id, from_semver, to_semver, reason, by, at_ms) VALUES(?1,?2,?3,?4,?5,?6,?7)",
                params![
                    new_id("rnm"),
                    id,
                    from.to_string(),
                    to.to_string(),
                    "拖拽版本线后按位置重新编号",
                    author,
                    at
                ],
            )
            .map_err(db_err)?;
        }
        for (pos, id) in version_ids.iter().enumerate() {
            tx.execute(
                "UPDATE versions SET line_pos=?2 WHERE id=?1 AND rendition_id=?3",
                params![id, pos as i64, rendition_id],
            )
            .map_err(db_err)?;
        }
        tx.commit().map_err(db_err)?;
        Ok(plan
            .into_iter()
            .map(|(id, from, to)| (id, from.to_string(), to.to_string()))
            .collect())
    }

    /// Renumber with a ledger record (the governance 改号）.
    /// `renumber_version` itself is unchanged for its existing
    /// callers; every governance renumber goes through here.
    pub fn renumber_version_recorded(
        &self,
        id: &str,
        semver: SemVer,
        by: &str,
        reason: &str,
    ) -> SResult<()> {
        let cur = self.get_version(id)?;
        let from = cur.semver.clone();
        self.renumber_version(id, semver)?;
        self.conn
            .execute(
                "INSERT INTO version_renumbers(id, version_id, from_semver, to_semver, reason, by, at_ms) VALUES(?1,?2,?3,?4,?5,?6,?7)",
                params![new_id("rnm"), id, from, semver.to_string(), reason, by, now_ms()],
            )
            .map_err(db_err)?;
        Ok(())
    }

    /// The renumber ledger of one version, oldest first.
    pub fn list_renumbers(&self, version_id: &str) -> SResult<Vec<RenumberRec>> {
        let mut stmt = self
            .conn
            .prepare("SELECT id, version_id, from_semver, to_semver, reason, by, at_ms FROM version_renumbers WHERE version_id=?1 ORDER BY at_ms, rowid")
            .map_err(db_err)?;
        let rows = stmt
            .query_map(params![version_id], |r| {
                Ok(RenumberRec {
                    id: r.get(0)?,
                    version_id: r.get(1)?,
                    from_semver: r.get(2)?,
                    to_semver: r.get(3)?,
                    reason: r.get(4)?,
                    by: r.get(5)?,
                    at_ms: r.get(6)?,
                })
            })
            .map_err(db_err)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(db_err)?;
        Ok(rows)
    }

    /// Mark a version not-passed （未通过） with its reason.
    /// The state arrives from the review flow outside #101;
    /// the page only renders it. A working draft has no number
    /// to fail and is refused.
    pub fn reject_version(&self, id: &str, reason: &str, by: &str) -> SResult<()> {
        let ver = self.get_version(id)?;
        if ver.status == "working" {
            return Err("未定号草稿不能标记未通过".into());
        }
        let n = self
            .conn
            .execute(
                "UPDATE versions SET status='rejected', pub_reason=?2, pub_by=?3, pub_at=?4 WHERE id=?1",
                params![id, reason, by, now_ms()],
            )
            .map_err(db_err)?;
        if n == 0 {
            return Err("版本不存在".into());
        }
        Ok(())
    }

    /// Undo a not-passed mark: back to normal, record cleared.
    pub fn restore_version(&self, id: &str) -> SResult<()> {
        let ver = self.get_version(id)?;
        if ver.status != "rejected" {
            return Err("只有未通过版本可以恢复".into());
        }
        self.conn
            .execute(
                "UPDATE versions SET status='normal', pub_reason='', pub_by='', pub_at=0 WHERE id=?1",
                params![id],
            )
            .map_err(db_err)?;
        Ok(())
    }

    /// The version a rendition's line is marked as using
    /// (USE_SEL): a display/governance marker only — entry
    /// resolution never reads it.
    pub fn get_use_sel(&self, unit_id: &str, rendition_id: &str) -> SResult<Option<String>> {
        self.conn
            .query_row(
                "SELECT version_id FROM version_use_sel WHERE unit_id=?1 AND rendition_id=?2",
                params![unit_id, rendition_id],
                |r| r.get(0),
            )
            .optional()
            .map_err(db_err)
    }

    pub fn set_use_sel(
        &self,
        unit_id: &str,
        rendition_id: &str,
        version_id: Option<&str>,
    ) -> SResult<()> {
        match version_id {
            Some(vid) => {
                let ver = self.get_version(vid)?;
                if ver.rendition_id != rendition_id {
                    return Err("选用版本不属于该形态".into());
                }
                self.conn
                    .execute(
                        "INSERT OR REPLACE INTO version_use_sel(unit_id, rendition_id, version_id) VALUES(?1,?2,?3)",
                        params![unit_id, rendition_id, vid],
                    )
                    .map_err(db_err)?;
            }
            None => {
                self.conn
                    .execute(
                        "DELETE FROM version_use_sel WHERE unit_id=?1 AND rendition_id=?2",
                        params![unit_id, rendition_id],
                    )
                    .map_err(db_err)?;
            }
        }
        Ok(())
    }

    /// Explicit history language mounts of a unit (the
    /// prototype's 历史_LANG): which translation hangs on
    /// which version, where a user has cycled the state by
    /// hand. Absence of a row means "derive it" (the client
    /// derives from basis revisions).
    pub fn list_lang_mounts(&self, unit_id: &str) -> SResult<Vec<LangMountRec>> {
        let mut stmt = self
            .conn
            .prepare("SELECT unit_id, version_id, lang, state FROM version_lang_mounts WHERE unit_id=?1")
            .map_err(db_err)?;
        let rows = stmt
            .query_map(params![unit_id], |r| {
                Ok(LangMountRec {
                    unit_id: r.get(0)?,
                    version_id: r.get(1)?,
                    lang: r.get(2)?,
                    state: r.get(3)?,
                })
            })
            .map_err(db_err)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(db_err)?;
        Ok(rows)
    }

    pub fn set_lang_mount(
        &self,
        unit_id: &str,
        version_id: &str,
        lang: &str,
        state: &str,
    ) -> SResult<()> {
        if !matches!(state, "synced" | "stale" | "missing") {
            return Err("语言挂载状态无效".into());
        }
        self.get_version(version_id)?;
        self.conn
            .execute(
                "INSERT INTO version_lang_mounts(unit_id, version_id, lang, state, updated_ms) VALUES(?1,?2,?3,?4,?5) ON CONFLICT(unit_id, version_id, lang) DO UPDATE SET state=excluded.state, updated_ms=excluded.updated_ms",
                params![unit_id, version_id, lang, state, now_ms()],
            )
            .map_err(db_err)?;
        Ok(())
    }

    /// The header status badges of a unit, derived at assembly
    /// (the prototype's real.status): the head rendition's
    /// published freshness, multi-form availability, and
    /// per-language staleness against the unit's current
    /// revision. Returns (kind, text) with kind "ok" | "warn".
    /// The prototype's 编辑中 badge has no state source in the
    /// client (no external-editor session is tracked) and is
    /// deliberately not fabricated here.
    pub fn unit_status_badges(&self, unit_id: &str) -> SResult<Vec<(String, String)>> {
        let unit = self.get_unit(unit_id)?;
        let rends = self.list_renditions(unit_id)?;
        let mut out: Vec<(String, String)> = Vec::new();
        if let Some(head) = rends.first() {
            if let Some(latest) = self.latest_version(&head.id)? {
                if latest.status == "normal" {
                    out.push(("ok".to_string(), "最新版".to_string()));
                }
            }
        }
        let mut forms: Vec<&str> = Vec::new();
        for r in &rends {
            if !forms.contains(&r.form.as_str()) {
                forms.push(r.form.as_str());
            }
        }
        if forms.len() > 1 {
            out.push(("warn".to_string(), "形态可选".to_string()));
        }
        for (idx, r) in rends.iter().enumerate() {
            if idx == 0 {
                continue;
            }
            if let Some(latest) = self.latest_version(&r.id)? {
                if latest.basis_revision < unit.current_revision {
                    if r.lang == "bi" {
                        out.push(("warn".to_string(), "双语待同步".to_string()));
                    } else {
                        out.push((
                            "warn".to_string(),
                            format!("内容过期 · {}版待更新", badge_lang_name(&r.lang)),
                        ));
                    }
                }
            }
        }
        Ok(out)
    }

    /// The governed page-reorder entry (the #101 wall drag):
    /// a draft or working version reorders in place; a
    /// PUBLISHED version's drag first seeds the unnumbered
    /// working draft from it and lands the new order there —
    /// published history is never rewritten (user ruling
    /// 2026-11-25: drag on any version, the result is a new
    /// version — here, the unnumbered draft awaiting its
    /// number). Returns the version that now holds the order.
    pub fn reorder_version_pages(
        &self,
        version_id: &str,
        page_ids: &[String],
        author: &str,
    ) -> SResult<VersionRec> {
        let ver = self.get_version(version_id)?;
        match ver.status.as_str() {
            "draft" | "working" => self.reorder_draft_pages(version_id, page_ids),
            "normal" => {
                let w = self.seed_working_draft(&ver.rendition_id, author)?;
                self.reorder_draft_pages(&w.id, page_ids)
            }
            _ => Err("该版本不可调整页序".into()),
        }
    }

    // ---------- version tags (collaborative tagging, spec §3.6) ----------

    pub fn add_version_tag(
        &self,
        version_id: &str,
        dim: &str,
        value: &str,
        strength: &str,
        author: &str,
    ) -> SResult<VerTagRec> {
        let id = new_id("vtg");
        self.conn
            .execute(
                "INSERT INTO version_tags(id, version_id, dim, value, strength, author) VALUES(?1,?2,?3,?4,?5,?6)",
                params![id, version_id, dim, value, strength, author],
            )
            .map_err(db_err)?;
        Ok(VerTagRec {
            id,
            version_id: version_id.into(),
            dim: dim.into(),
            value: value.into(),
            strength: strength.into(),
            author: author.into(),
        })
    }

    pub fn list_version_tags(&self, version_id: &str) -> SResult<Vec<VerTagRec>> {
        let mut stmt = self
            .conn
            .prepare("SELECT id, version_id, dim, value, strength, author FROM version_tags WHERE version_id=?1")
            .map_err(db_err)?;
        let rows = stmt
            .query_map(params![version_id], |r| {
                Ok(VerTagRec {
                    id: r.get(0)?,
                    version_id: r.get(1)?,
                    dim: r.get(2)?,
                    value: r.get(3)?,
                    strength: r.get(4)?,
                    author: r.get(5)?,
                })
            })
            .map_err(db_err)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(db_err)?;
        Ok(rows)
    }

    // ---------- page bindings (§3.12 language binding) ----------

    /// Bind one page (slot) of one version of a unit to a page of another
    /// version of the SAME unit — the per-page language correspondence.
    /// 1-to-many is allowed in both directions. Idempotent per tuple.
    pub fn add_page_binding(
        &self,
        unit_id: &str,
        from_version_id: &str,
        from_slot: i64,
        to_version_id: &str,
        to_slot: i64,
        kind: &str,
        author: &str,
    ) -> SResult<PageBindingRec> {
        // both versions must belong to renditions of this unit
        for vid in [from_version_id, to_version_id] {
            let v = self.get_version(vid)?;
            let r = self.get_rendition(&v.rendition_id)?;
            if r.unit_id != unit_id {
                return Err("版本不属于该内容块".into());
            }
            // A "missing" mark (#101 bind panel: 标缺译 / 无对应页)
            // names a target version but no target page: it is the
            // one row allowed to carry to_slot = -1. Its to_page_id
            // resolves to "" (page_id_at below), which the read
            // paths treat as "no page", never as a real binding.
            let missing_mark = kind == "missing" && to_slot == -1;
            if from_slot < 0 || (to_slot < 0 && !missing_mark) {
                return Err("页位不能为负".into());
            }
        }
        let existing = self.list_page_bindings(unit_id)?.into_iter().find(|b| {
            b.from_version_id == from_version_id
                && b.from_slot == from_slot
                && b.to_version_id == to_version_id
                && b.to_slot == to_slot
        });
        if let Some(b) = existing {
            return Ok(b);
        }
        // Page-id addressing: resolve both slots to the page
        // entities they name in their (immutable) versions.
        let page_id_at = |vid: &str, slot: i64| -> String {
            if slot < 0 {
                return String::new();
            }
            self.get_version(vid)
                .ok()
                .and_then(|v| v.pages.get(slot as usize).map(|p| p.id.clone()))
                .unwrap_or_default()
        };
        let rec = PageBindingRec {
            id: new_id("bnd"),
            unit_id: unit_id.into(),
            from_version_id: from_version_id.into(),
            from_slot,
            to_version_id: to_version_id.into(),
            to_slot,
            kind: kind.into(),
            author: author.into(),
            created_ms: now_ms(),
            from_page_id: page_id_at(from_version_id, from_slot),
            to_page_id: page_id_at(to_version_id, to_slot),
        };
        self.conn
            .execute(
                "INSERT INTO page_bindings(id, unit_id, from_version_id, from_slot, to_version_id, to_slot, kind, author, created_ms, from_page_id, to_page_id) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11)",
                params![
                    rec.id,
                    rec.unit_id,
                    rec.from_version_id,
                    rec.from_slot,
                    rec.to_version_id,
                    rec.to_slot,
                    rec.kind,
                    rec.author,
                    rec.created_ms,
                    rec.from_page_id,
                    rec.to_page_id
                ],
            )
            .map_err(db_err)?;
        Ok(rec)
    }

    pub fn list_page_bindings(&self, unit_id: &str) -> SResult<Vec<PageBindingRec>> {
        let mut stmt = self
            .conn
            .prepare("SELECT id, unit_id, from_version_id, from_slot, to_version_id, to_slot, kind, author, created_ms, from_page_id, to_page_id FROM page_bindings WHERE unit_id=?1 ORDER BY created_ms")
            .map_err(db_err)?;
        let rows = stmt
            .query_map(params![unit_id], |r| {
                Ok(PageBindingRec {
                    id: r.get(0)?,
                    unit_id: r.get(1)?,
                    from_version_id: r.get(2)?,
                    from_slot: r.get(3)?,
                    to_version_id: r.get(4)?,
                    to_slot: r.get(5)?,
                    kind: r.get(6)?,
                    author: r.get(7)?,
                    created_ms: r.get(8)?,
                    from_page_id: r.get(9)?,
                    to_page_id: r.get(10)?,
                })
            })
            .map_err(db_err)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(db_err)?;
        Ok(rows)
    }

    pub fn delete_page_binding(&self, unit_id: &str, binding_id: &str) -> SResult<()> {
        let n = self
            .conn
            .execute(
                "DELETE FROM page_bindings WHERE id=?1 AND unit_id=?2",
                params![binding_id, unit_id],
            )
            .map_err(db_err)?;
        if n == 0 {
            return Err("页绑定不存在".into());
        }
        Ok(())
    }

    // ---------- search ----------

    /// Full-text-ish search over extracted slide text and titles; only units
    /// in visible groups are returned (invisible content must not leak).
    pub fn search(
        &self,
        q: &str,
        visible: Option<&HashSet<String>>,
    ) -> SResult<Vec<(UnitRec, VersionRec)>> {
        let like = format!("%{q}%");
        let mut stmt = self
            .conn
            .prepare(&format!(
                "SELECT {} FROM versions WHERE text_all LIKE ?1 OR pages_json LIKE ?1 ORDER BY created_ms DESC LIMIT 100",
                Self::VERSION_COLS
            ))
            .map_err(db_err)?;
        let versions = stmt
            .query_map(params![like], Self::row_to_version)
            .map_err(db_err)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(db_err)?;
        let mut out = Vec::new();
        for v in versions {
            let rend = self.get_rendition(&v.rendition_id)?;
            let unit = self.get_unit(&rend.unit_id)?;
            if let Some(set) = visible {
                if !set.contains(&unit.group_id) {
                    continue;
                }
            }
            out.push((unit, v));
        }
        Ok(out)
    }
}

// ==================== indexes (Git-like repos) ====================

impl Store {
    fn row_to_index(r: &rusqlite::Row) -> rusqlite::Result<IndexRec> {
        Ok(IndexRec {
            id: r.get(0)?,
            name: r.get(1)?,
            folder_id: r.get(2)?,
            group_id: r.get(3)?,
            maintainer: r.get(4)?,
            lifecycle: r.get(5)?,
            scope: r.get(6)?,
            created_ms: r.get(7)?,
            note: r.get(8)?,
        })
    }

    const INDEX_COLS: &'static str =
        "id, name, folder_id, group_id, maintainer, lifecycle, scope, created_ms, note";

    pub fn rename_index(&self, id: &str, name: &str) -> SResult<()> {
        let n = self
            .conn
            .execute("UPDATE indexes_tbl SET name=?2 WHERE id=?1", params![id, name])
            .map_err(db_err)?;
        if n == 0 {
            return Err("索引不存在".into());
        }
        Ok(())
    }

    /// Set (or clear, with an empty string) the index's note
    /// （备注）, edited in place in the detail page's backbar.
    pub fn set_index_note(&self, id: &str, note: &str) -> SResult<()> {
        let n = self
            .conn
            .execute("UPDATE indexes_tbl SET note=?2 WHERE id=?1", params![id, note])
            .map_err(db_err)?;
        if n == 0 {
            return Err("索引不存在".into());
        }
        Ok(())
    }

    /// Create an index. `folder_id` None means the index's home is
    /// 未分类 — the system collection of folder-less indexes
    /// (indexes_tbl.folder_id IS NULL), and it is the default home
    /// of every new index: the server handler and the client's
    /// local dispatch both funnel into this function, so the
    /// default is identical on both paths. Callers pass an explicit
    /// folder only when the user created the index from inside
    /// that folder.
    pub fn create_index(
        &self,
        name: &str,
        folder_id: Option<&str>,
        group_id: &str,
        maintainer: &str,
        scope: &str,
        entries: Vec<IndexEntry>,
    ) -> SResult<IndexRec> {
        self.check_home_folder(folder_id, "index", "索引")?;
        let id = new_id("idx");
        self.conn
            .execute(
                "INSERT INTO indexes_tbl(id, name, folder_id, group_id, maintainer, lifecycle, scope, created_ms) VALUES(?1,?2,?3,?4,?5,'active',?6,?7)",
                params![id, name, folder_id, group_id, maintainer, scope, now_ms()],
            )
            .map_err(db_err)?;
        self.conn
            .execute(
                "INSERT INTO index_branches(index_id, name, head_commit) VALUES(?1,'main',NULL)",
                params![id],
            )
            .map_err(db_err)?;
        self.create_commit(&id, "main", maintainer, "创建索引", entries)?;
        self.get_index(&id)
    }

    pub fn get_index(&self, id: &str) -> SResult<IndexRec> {
        self.conn
            .query_row(
                &format!("SELECT {} FROM indexes_tbl WHERE id=?1", Self::INDEX_COLS),
                params![id],
                Self::row_to_index,
            )
            .map_err(db_err)
    }

    pub fn list_indexes(&self, visible: Option<&HashSet<String>>) -> SResult<Vec<IndexRec>> {
        let mut stmt = self
            .conn
            .prepare(&format!(
                "SELECT {} FROM indexes_tbl ORDER BY name",
                Self::INDEX_COLS
            ))
            .map_err(db_err)?;
        let rows = stmt
            .query_map([], Self::row_to_index)
            .map_err(db_err)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(db_err)?;
        let trashed = self.trashed_ids("index")?;
        let rows: Vec<IndexRec> = rows
            .into_iter()
            .filter(|i| !trashed.contains(&i.id))
            .collect();
        Ok(match visible {
            None => rows,
            Some(set) => rows
                .into_iter()
                .filter(|i| set.contains(&i.group_id))
                .collect(),
        })
    }

    pub fn set_index_lifecycle(&self, id: &str, lifecycle: &str) -> SResult<()> {
        // Lifecycle is a closed vocabulary (audit L2): anything
        // else would strand the index outside the state machine
        // its readers match on.
        if !matches!(lifecycle, "active" | "history" | "void") {
            return Err(format!("未知的索引生命周期：{lifecycle}"));
        }
        let n = self
            .conn
            .execute(
                "UPDATE indexes_tbl SET lifecycle=?2 WHERE id=?1",
                params![id, lifecycle],
            )
            .map_err(db_err)?;
        if n == 0 {
            return Err("索引不存在".into());
        }
        Ok(())
    }

    pub fn branch_head(&self, index_id: &str, branch: &str) -> SResult<Option<String>> {
        // A missing branch row is a legitimate None (the branch
        // has no commits yet — or does not exist; callers that
        // must tell the two apart, like create_commit, check
        // existence separately). A database error is NOT
        // (audit M6): the old `.ok()` folded both into None,
        // which let create_commit report success for a commit
        // no branch head would ever point at.
        let head: Option<Option<String>> = self
            .conn
            .query_row(
                "SELECT head_commit FROM index_branches WHERE index_id=?1 AND name=?2",
                params![index_id, branch],
                |r| r.get(0),
            )
            .optional()
            .map_err(db_err)?;
        Ok(head.flatten())
    }

    pub fn list_branches(&self, index_id: &str) -> SResult<Vec<String>> {
        let mut stmt = self
            .conn
            .prepare("SELECT name FROM index_branches WHERE index_id=?1 ORDER BY name")
            .map_err(db_err)?;
        let rows = stmt
            .query_map(params![index_id], |r| r.get::<_, String>(0))
            .map_err(db_err)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(db_err)?;
        Ok(rows)
    }

    pub fn create_branch(&self, index_id: &str, name: &str, from_branch: &str) -> SResult<()> {
        let inserted = self.conn.execute(
            "INSERT INTO index_branches(index_id, name, head_commit, source_index_id, source_branch, fork_commit_id)
             SELECT index_id, ?2, head_commit, index_id, name, head_commit
             FROM index_branches WHERE index_id=?1 AND name=?3",
            params![index_id, name, from_branch],
        ).map_err(db_err)?;
        if inserted == 0 {
            return Err("来源分支不存在".into());
        }
        Ok(())
    }

    pub fn index_graph(&self, index_id: &str) -> SResult<IndexGraph> {
        let mut branches = self
            .conn
            .prepare(
                "SELECT name, head_commit, source_index_id, source_branch, fork_commit_id
             FROM index_branches WHERE index_id=?1 ORDER BY name <> 'main', rowid",
            )
            .map_err(db_err)?;
        let branches = branches
            .query_map(params![index_id], |row| {
                Ok(IndexBranchLineage {
                    name: row.get(0)?,
                    head_commit: row.get(1)?,
                    source_index_id: row.get(2)?,
                    source_branch: row.get(3)?,
                    fork_commit_id: row.get(4)?,
                })
            })
            .map_err(db_err)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(db_err)?;
        let mut commits = self
            .conn
            .prepare(
                "SELECT id, branch, message FROM index_commits WHERE index_id=?1 ORDER BY rowid",
            )
            .map_err(db_err)?;
        let commits = commits
            .query_map(params![index_id], |row| {
                Ok(IndexGraphCommit {
                    id: row.get(0)?,
                    branch: row.get(1)?,
                    message: row.get(2)?,
                })
            })
            .map_err(db_err)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(db_err)?;
        Ok(IndexGraph {
            branches,
            commits,
            tags: self.list_tags(index_id)?,
        })
    }

    /// Move an index into a folder; `folder_id` None moves it into
    /// 未分类 (folder_id set to NULL).
    pub fn move_index_to_folder(&self, id: &str, folder_id: Option<&str>) -> SResult<()> {
        let tx = self.conn.unchecked_transaction().map_err(db_err)?;
        if let Some(folder_id) = folder_id {
            // Existence AND scope (audit N3): create_index gates
            // the home folder through check_home_folder; moving
            // an index is the same home change and must hold the
            // same line, or the index lands in another surface's
            // tree and vanishes from the index walls.
            let scope: Option<String> = tx
                .query_row(
                    "SELECT scope FROM folders WHERE id=?1",
                    params![folder_id],
                    |row| row.get(0),
                )
                .optional()
                .map_err(db_err)?;
            match scope {
                None => return Err("目标文件夹不存在".into()),
                Some(s) if s == "index" => {}
                Some(s) => {
                    let area = match s.as_str() {
                        "showroom" => "演示·公共",
                        "library" => "演示·我的",
                        "inbox" => "演示·分享",
                        "content" => "页面",
                        _ => "其他",
                    };
                    return Err(format!(
                        "索引不能移到这个文件夹下：该文件夹属于「{area}」目录"
                    ));
                }
            }
        }
        if tx
            .execute(
                "UPDATE indexes_tbl SET folder_id=?2 WHERE id=?1",
                params![id, folder_id],
            )
            .map_err(db_err)?
            == 0
        {
            return Err("索引不存在".into());
        }
        tx.execute(
            "UPDATE folders SET main_index_id=NULL WHERE main_index_id=?1 AND id IS NOT ?2",
            params![id, folder_id],
        )
        .map_err(db_err)?;
        tx.commit().map_err(db_err)
    }

    pub fn create_english_index(
        &self,
        source: &IndexRec,
        source_branch: &str,
        source_commit: &str,
        name: &str,
        maintainer: &str,
        entries: Vec<IndexEntry>,
    ) -> SResult<IndexRec> {
        let tx = self.conn.unchecked_transaction().map_err(db_err)?;
        let derived = self.create_index(
            name,
            source.folder_id.as_deref(),
            &source.group_id,
            maintainer,
            &source.scope,
            entries,
        )?;
        tx.execute(
            "UPDATE index_branches SET source_index_id=?2, source_branch=?3, fork_commit_id=?4
             WHERE index_id=?1 AND name='main'",
            params![derived.id, source.id, source_branch, source_commit],
        )
        .map_err(db_err)?;
        tx.commit().map_err(db_err)?;
        Ok(derived)
    }

    pub fn create_commit(
        &self,
        index_id: &str,
        branch: &str,
        author: &str,
        message: &str,
        entries: Vec<IndexEntry>,
    ) -> SResult<CommitRec> {
        // Existence first (audit M6): committing to an index or
        // branch that does not exist used to insert an orphan
        // commit, update zero branch rows and still report
        // success — the save looked done and was unreachable.
        if self.get_index(index_id).is_err() {
            return Err("索引不存在".into());
        }
        let branch_exists: bool = self
            .conn
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM index_branches WHERE index_id=?1 AND name=?2)",
                params![index_id, branch],
                |r| r.get(0),
            )
            .map_err(db_err)?;
        if !branch_exists {
            return Err("分支不存在".into());
        }
        let parent = self.branch_head(index_id, branch)?;
        let id = new_id("cmt");
        let rec = CommitRec {
            id: id.clone(),
            index_id: index_id.into(),
            branch: branch.into(),
            parent_id: parent.clone(),
            author: author.into(),
            message: message.into(),
            created_ms: now_ms(),
            entries,
        };
        // Insert + head update atomically: a crash between
        // them used to leave the same orphan commit. When the
        // caller already runs inside a transaction
        // (land_submission, create_english_index), join it —
        // a nested BEGIN would fail outright.
        let write = |conn: &Connection| -> SResult<()> {
            conn.execute(
                "INSERT INTO index_commits(id, index_id, branch, parent_id, author, message, created_ms, entries_json) VALUES(?1,?2,?3,?4,?5,?6,?7,?8)",
                params![
                    rec.id,
                    rec.index_id,
                    rec.branch,
                    rec.parent_id,
                    rec.author,
                    rec.message,
                    rec.created_ms,
                    serde_json::to_string(&rec.entries).unwrap_or_default()
                ],
            )
            .map_err(db_err)?;
            let n = conn
                .execute(
                    "UPDATE index_branches SET head_commit=?3 WHERE index_id=?1 AND name=?2",
                    params![index_id, branch, id],
                )
                .map_err(db_err)?;
            if n == 0 {
                return Err("分支不存在".into());
            }
            Ok(())
        };
        if self.conn.is_autocommit() {
            let tx = self.conn.unchecked_transaction().map_err(db_err)?;
            write(&tx)?;
            tx.commit().map_err(db_err)?;
        } else {
            write(&self.conn)?;
        }
        Ok(rec)
    }

    pub fn get_commit(&self, id: &str) -> SResult<CommitRec> {
        self.conn
            .query_row(
                "SELECT id, index_id, branch, parent_id, author, message, created_ms, entries_json FROM index_commits WHERE id=?1",
                params![id],
                |r| {
                    let entries_json: String = r.get(7)?;
                    Ok(CommitRec {
                        id: r.get(0)?,
                        index_id: r.get(1)?,
                        branch: r.get(2)?,
                        parent_id: r.get(3)?,
                        author: r.get(4)?,
                        message: r.get(5)?,
                        created_ms: r.get(6)?,
                        entries: serde_json::from_str(&entries_json).unwrap_or_default(),
                    })
                },
            )
            .map_err(db_err)
    }

    /// Ancestor commit IDs of `commit_id`, starting with itself and
    /// walking the (single-parent) chain to the root.
    pub fn ancestors(&self, commit_id: &str) -> SResult<Vec<String>> {
        let mut out = Vec::new();
        let mut cur = Some(commit_id.to_string());
        while let Some(cid) = cur {
            let c = self.get_commit(&cid)?;
            out.push(cid);
            cur = c.parent_id.clone();
            if out.len() > 1000 {
                // Silent truncation made merge_base quietly
                // miss the true common ancestor and report a
                // wall of phantom conflicts (audit L4); an
                // explicit error is the honest failure mode.
                return Err("提交祖先链超过 1000 层，无法完整遍历".into());
            }
        }
        Ok(out)
    }

    /// Nearest common ancestor of two commits (the fork point), if any.
    pub fn merge_base(&self, a: &str, b: &str) -> SResult<Option<String>> {
        let b_set: std::collections::HashSet<String> = self.ancestors(b)?.into_iter().collect();
        for cand in self.ancestors(a)? {
            if b_set.contains(&cand) {
                return Ok(Some(cand));
            }
        }
        Ok(None)
    }

    /// Entry-level three-way merge preview (§3.10): `ours` = the target
    /// branch head, `theirs` = the source branch head, base = fork point.
    /// Entries are keyed by (unit, rendition, occurrence) so a block used
    /// twice merges instance by instance. One-sided changes merge
    /// automatically; two-sided changes to the same entry are conflicts
    /// for a human to adjudicate — slide content is never merged.
    pub fn preview_merge(
        &self,
        index_id: &str,
        from_branch: &str,
        into_branch: &str,
    ) -> SResult<MergePreview> {
        let from_head = self
            .branch_head(index_id, from_branch)?
            .ok_or_else(|| format!("分支 {from_branch} 尚无提交"))?;
        let into_head = self
            .branch_head(index_id, into_branch)?
            .ok_or_else(|| format!("分支 {into_branch} 尚无提交"))?;
        let base_id = self.merge_base(&from_head, &into_head)?;
        let base_entries = match &base_id {
            Some(b) => self.get_commit(b)?.entries,
            None => Vec::new(),
        };
        let ours = self.get_commit(&into_head)?.entries;
        let theirs = self.get_commit(&from_head)?.entries;
        let (merged, conflicts) = compute_merge(&base_entries, &ours, &theirs, None);
        Ok(MergePreview {
            base_commit: base_id,
            from_head,
            into_head,
            clean: conflicts.is_empty(),
            merged,
            conflicts,
        })
    }

    /// Execute a merge: applies the three-way merge, filling conflicts
    /// from `resolutions` (key -> "ours" | "theirs"). Fails listing any
    /// conflict left unresolved. Creates the merge commit on the target
    /// branch; the source head is recorded in the commit message.
    pub fn execute_merge(
        &self,
        index_id: &str,
        from_branch: &str,
        into_branch: &str,
        author: &str,
        resolutions: &std::collections::HashMap<String, String>,
    ) -> SResult<CommitRec> {
        let preview = self.preview_merge(index_id, from_branch, into_branch)?;
        if preview.from_head == preview.into_head {
            return Err("两个分支指向同一提交，无需合并".into());
        }
        let base_entries = match &preview.base_commit {
            Some(b) => self.get_commit(b)?.entries,
            None => Vec::new(),
        };
        let ours = self.get_commit(&preview.into_head)?.entries;
        let theirs = self.get_commit(&preview.from_head)?.entries;
        let (merged, conflicts) = compute_merge(&base_entries, &ours, &theirs, Some(resolutions));
        if !conflicts.is_empty() {
            let keys: Vec<&str> = conflicts.iter().map(|c| c.key.as_str()).collect();
            return Err(format!("仍有未裁决的冲突条目：{}", keys.join("、")));
        }
        self.create_commit(
            index_id,
            into_branch,
            author,
            &format!(
                "合并分支 {from_branch}（源提交 {}）",
                &preview.from_head[..preview.from_head.len().min(12)]
            ),
            merged,
        )
    }

    /// Commit history of a branch, newest first (walks the parent chain).
    pub fn list_commits(&self, index_id: &str, branch: &str) -> SResult<Vec<CommitRec>> {
        let mut out = Vec::new();
        let mut cur = self.branch_head(index_id, branch)?;
        while let Some(cid) = cur {
            let c = self.get_commit(&cid)?;
            cur = c.parent_id.clone();
            out.push(c);
            if out.len() > 500 {
                break;
            }
        }
        Ok(out)
    }

    pub fn create_tag(&self, index_id: &str, branch: &str, name: &str) -> SResult<IndexTagRec> {
        // One name, one tag (audit L8): a duplicate (index,
        // branch, name) would let a publisher silently re-point
        // what "the published version" means, since readers pick
        // among same-named tags by list order.
        let dup: i64 = self
            .conn
            .query_row(
                "SELECT COUNT(*) FROM index_tags WHERE index_id=?1 AND branch=?2 AND name=?3",
                params![index_id, branch, name],
                |r| r.get(0),
            )
            .map_err(db_err)?;
        if dup > 0 {
            return Err(format!("标签「{name}」已存在"));
        }
        let head = self.branch_head(index_id, branch)?.ok_or("分支尚无提交")?;
        let id = new_id("tag");
        self.conn
            .execute(
                "INSERT INTO index_tags(id, index_id, branch, name, commit_id) VALUES(?1,?2,?3,?4,?5)",
                params![id, index_id, branch, name, head],
            )
            .map_err(db_err)?;
        Ok(IndexTagRec {
            id,
            index_id: index_id.into(),
            branch: branch.into(),
            name: name.into(),
            commit_id: head,
        })
    }

    pub fn list_tags(&self, index_id: &str) -> SResult<Vec<IndexTagRec>> {
        let mut stmt = self
            .conn
            .prepare("SELECT id, index_id, branch, name, commit_id FROM index_tags WHERE index_id=?1 ORDER BY rowid DESC")
            .map_err(db_err)?;
        let rows = stmt
            .query_map(params![index_id], |r| {
                Ok(IndexTagRec {
                    id: r.get(0)?,
                    index_id: r.get(1)?,
                    branch: r.get(2)?,
                    name: r.get(3)?,
                    commit_id: r.get(4)?,
                })
            })
            .map_err(db_err)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(db_err)?;
        Ok(rows)
    }

    /// Resolve an index entry to the version it currently points at.
    /// Follow-latest → newest PUBLISHED version; locked → the locked one,
    /// with deletion substitution (nearest surviving version of the same
    /// rendition, searching upward from the locked number).
    /// Substitution candidates are formally published (normal)
    /// versions only (pin audit N1): a draft never substitutes
    /// for a locked or pinned version — when no published
    /// candidate exists the entry resolves to nothing rather than
    /// drifting onto unreviewed content.
    pub fn resolve_entry_version(&self, entry: &IndexEntry) -> SResult<Option<VersionRec>> {
        let versions = self.list_versions(&entry.rendition_id.0)?;
        let surviving: Vec<VersionRec> = versions
            .into_iter()
            .filter(|v| v.status != "recalled")
            .collect();
        match &entry.policy {
            // Follow-latest follows PUBLISHED versions only
            // (audit M2): a draft is by definition "not yet
            // formally published", so a newer draft must never
            // become what an entry, an export or a guest sees
            // merely by existing. (The locked arm's deletion
            // substitution is likewise restricted to published
            // versions — see below; the direct locked hit is an
            // explicit pin and keeps resolving as pinned.)
            crate::model::EntryPolicy::FollowLatest => Ok(surviving
                .into_iter()
                .find(|v| v.status == "normal")),
            crate::model::EntryPolicy::Locked { version_id } => {
                if let Some(v) = surviving.iter().find(|v| v.id == version_id.0) {
                    return Ok(Some(v.clone()));
                }
                // deletion substitution over published versions
                // only: nearest normal version at or above the
                // locked display number, else the latest normal
                // one, else nothing.
                let published: Vec<VersionRec> = surviving
                    .into_iter()
                    .filter(|v| v.status == "normal")
                    .collect();
                if let Some(ls) = &entry.locked_semver {
                    if let Ok(want) = SemVer::parse(ls) {
                        let mut above: Vec<(SemVer, &VersionRec)> = published
                            .iter()
                            .filter_map(|v| {
                                SemVer::parse(&v.semver)
                                    .ok()
                                    .filter(|s| *s >= want)
                                    .map(|s| (s, v))
                            })
                            .collect();
                        above.sort_by_key(|(s, _)| *s);
                        if let Some((_, v)) = above.first() {
                            return Ok(Some((*v).clone()));
                        }
                    }
                }
                Ok(published.into_iter().next())
            }
        }
    }

    // ---------- showroom publish pins (unified content model F) ----------
    //
    // A showroom item is the formal, confirmed version of its
    // source index: what the pusher saw in the push summary and
    // confirmed is what the item publishes, permanently. The
    // pinned commit (showroom.commit_id) already froze the entry
    // list; the pins in `showroom_pins` freeze each entry's
    // resolved version alongside it, one version id per entry
    // position of that commit. Index-layer commits keep their
    // own policies untouched (a follow-latest entry stays
    // follow-latest in the index); the pin is consulted only at
    // the showroom boundary — display, presentation, export and
    // the guest funnel resolve through it, so the formal version
    // never drifts when a block publishes a newer version.

    /// Replace an item's whole pin set (one version id per entry
    /// position of the item's pinned commit, in entry order).
    /// Called at publish / landing time with exactly the versions
    /// the confirmation summary displayed.
    ///
    /// The replacement is atomic (pin audit N2): a savepoint
    /// wraps the delete + inserts, so a failure mid-set can no
    /// longer leave a torn set of half-old, half-missing pins —
    /// standalone callers get a transaction of their own, and
    /// callers already inside one (the landing transaction) just
    /// nest. Every row also records the commit it belongs to:
    /// the pin set and the item's pinned commit are written as
    /// one fact, and readers verify the pairing instead of
    /// applying a stale set to a newer commit by position.
    pub fn set_showroom_pins(
        &self,
        item_id: &str,
        commit_id: &str,
        version_ids: &[String],
    ) -> SResult<()> {
        self.conn
            .execute_batch("SAVEPOINT qimu_set_showroom_pins")
            .map_err(db_err)?;
        let attempt = (|| -> SResult<()> {
            self.conn
                .execute(
                    "DELETE FROM showroom_pins WHERE showroom_id=?1",
                    params![item_id],
                )
                .map_err(db_err)?;
            for (pos, vid) in version_ids.iter().enumerate() {
                self.conn
                    .execute(
                        "INSERT INTO showroom_pins(showroom_id, pos, version_id, commit_id) VALUES(?1,?2,?3,?4)",
                        params![item_id, pos as i64, vid, commit_id],
                    )
                    .map_err(db_err)?;
            }
            Ok(())
        })();
        match attempt {
            Ok(()) => {
                self.conn
                    .execute_batch("RELEASE qimu_set_showroom_pins")
                    .map_err(db_err)?;
                Ok(())
            }
            Err(e) => {
                let _ = self
                    .conn
                    .execute_batch("ROLLBACK TO qimu_set_showroom_pins");
                let _ = self.conn.execute_batch("RELEASE qimu_set_showroom_pins");
                Err(e)
            }
        }
    }

    /// An item's pins, ordered by entry position. Empty when the
    /// item predates publish pinning (legacy rows resolve exactly
    /// as they always did).
    pub fn showroom_pins(&self, item_id: &str) -> SResult<Vec<String>> {
        Ok(self
            .showroom_pin_rows(item_id)?
            .into_iter()
            .map(|(vid, _)| vid)
            .collect())
    }

    /// An item's pin rows as (version id, recorded commit id)
    /// pairs, ordered by entry position. Rows written before pin
    /// sets recorded their commit carry an empty commit id.
    pub fn showroom_pin_rows(&self, item_id: &str) -> SResult<Vec<(String, String)>> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT version_id, commit_id FROM showroom_pins WHERE showroom_id=?1 ORDER BY pos",
            )
            .map_err(db_err)?;
        let rows = stmt
            .query_map(params![item_id], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
            })
            .map_err(db_err)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(db_err)?;
        Ok(rows)
    }

    /// Verify an item's pin set belongs to `commit_id` (pin
    /// audit N2). A row bound to a different commit means the
    /// item's commit and its pins were written out of step —
    /// applying that set to this commit by position would
    /// silently publish wrong versions, so it is an error, not a
    /// fallback. Rows with an empty recorded commit predate the
    /// binding and pass unverifiable (the declared legacy seam).
    fn check_pin_commit(&self, item_id: &str, commit_id: &str) -> SResult<()> {
        let rows = self.showroom_pin_rows(item_id)?;
        if rows
            .iter()
            .any(|(_, c)| !c.is_empty() && c != commit_id)
        {
            return Err(
                "演示条目的钉版与条目版本不一致（钉版集与提交失同步），请重新推送或重新落地"
                    .into(),
            );
        }
        Ok(())
    }

    /// An entry with its policy materialized onto a pinned
    /// version: the locked-entry resolution path (including
    /// deletion substitution) then applies verbatim. The
    /// substitution anchor is the entry's own locked_semver when
    /// it carries one, else the pinned version's own number.
    fn pinned_entry(&self, entry: &IndexEntry, version_id: &str) -> IndexEntry {
        let mut eff = entry.clone();
        if eff.locked_semver.is_none() {
            eff.locked_semver = self.get_version(version_id).ok().map(|v| v.semver);
        }
        eff.policy = crate::model::EntryPolicy::Locked {
            version_id: crate::model::Id(version_id.to_string()),
        };
        eff
    }

    /// Resolve one entry the way a showroom item publishes it:
    /// with a pin for its position, the pinned version wins;
    /// without one, the entry's own policy decides (the legacy
    /// behavior for items that predate pinning).
    pub fn resolve_showroom_entry(
        &self,
        entry: &IndexEntry,
        pin: Option<&str>,
    ) -> SResult<Option<VersionRec>> {
        match pin {
            Some(vid) => self.resolve_entry_version(&self.pinned_entry(entry, vid)),
            None => self.resolve_entry_version(entry),
        }
    }

    /// A commit's entries with the item's pins applied — the
    /// entry list export / assembly consumers must resolve for
    /// this item. Entries without a pin pass through unchanged.
    pub fn pinned_commit_entries(
        &self,
        item: &ShowroomRec,
        commit: &CommitRec,
    ) -> SResult<Vec<IndexEntry>> {
        self.check_pin_commit(&item.id, &commit.id)?;
        let pins = self.showroom_pins(&item.id)?;
        Ok(commit
            .entries
            .iter()
            .enumerate()
            .map(|(pos, e)| match pins.get(pos) {
                Some(vid) => self.pinned_entry(e, vid),
                None => e.clone(),
            })
            .collect())
    }

    // ---------- exports / attachments / showroom / assemblies ----------

    pub fn insert_export(&self, rec: &ExportRec) -> SResult<()> {
        self.conn
            .execute(
                "INSERT INTO exports(id, index_id, branch, commit_id, tag, user_id, created_ms, deck_hash, page_count, kind, deck_id, manifest_json) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12)",
                params![
                    rec.id,
                    rec.index_id,
                    rec.branch,
                    rec.commit_id,
                    rec.tag,
                    rec.user_id,
                    rec.created_ms,
                    rec.deck_hash,
                    rec.page_count,
                    rec.kind,
                    rec.deck_id,
                    rec.manifest_json
                ],
            )
            .map_err(db_err)?;
        Ok(())
    }

    pub fn list_exports(&self, index_id: Option<&str>) -> SResult<Vec<ExportRec>> {
        // The unfiltered list keeps its 500-row recency window
        // (it backs list faces). A per-index query must NOT be
        // windowed (audit L3): filtering after the window made
        // an index's older exports silently invisible.
        let sql = "SELECT id, index_id, branch, commit_id, tag, user_id, created_ms, deck_hash, page_count, kind, deck_id, manifest_json FROM exports";
        let row = |r: &rusqlite::Row| {
            Ok(ExportRec {
                id: r.get(0)?,
                index_id: r.get(1)?,
                branch: r.get(2)?,
                commit_id: r.get(3)?,
                tag: r.get(4)?,
                user_id: r.get(5)?,
                created_ms: r.get(6)?,
                deck_hash: r.get(7)?,
                page_count: r.get(8)?,
                kind: r.get(9)?,
                deck_id: r.get(10)?,
                manifest_json: r.get(11)?,
            })
        };
        let rows = match index_id {
            None => {
                let mut stmt = self
                    .conn
                    .prepare(&format!("{sql} ORDER BY created_ms DESC LIMIT 500"))
                    .map_err(db_err)?;
                let rows = stmt
                    .query_map([], row)
                    .map_err(db_err)?
                    .collect::<Result<Vec<_>, _>>()
                    .map_err(db_err)?;
                rows
            }
            Some(iid) => {
                let mut stmt = self
                    .conn
                    .prepare(&format!(
                        "{sql} WHERE index_id=?1 ORDER BY created_ms DESC"
                    ))
                    .map_err(db_err)?;
                let rows = stmt
                    .query_map(params![iid], row)
                    .map_err(db_err)?
                    .collect::<Result<Vec<_>, _>>()
                    .map_err(db_err)?;
                rows
            }
        };
        Ok(rows)
    }

    /// The export record for a manifest deck_id (editor-stripped deck
    /// recovery). Latest export wins. Queried directly (audit L3):
    /// routing through the 500-row list window made recovery of an
    /// older deck silently fail once 500 newer exports existed.
    pub fn export_by_deck_id(&self, deck_id: &str) -> SResult<Option<ExportRec>> {
        let rec = self
            .conn
            .query_row(
                "SELECT id, index_id, branch, commit_id, tag, user_id, created_ms, deck_hash, page_count, kind, deck_id, manifest_json FROM exports WHERE deck_id=?1 AND manifest_json<>'' ORDER BY created_ms DESC, rowid DESC LIMIT 1",
                params![deck_id],
                |r| {
                    Ok(ExportRec {
                        id: r.get(0)?,
                        index_id: r.get(1)?,
                        branch: r.get(2)?,
                        commit_id: r.get(3)?,
                        tag: r.get(4)?,
                        user_id: r.get(5)?,
                        created_ms: r.get(6)?,
                        deck_hash: r.get(7)?,
                        page_count: r.get(8)?,
                        kind: r.get(9)?,
                        deck_id: r.get(10)?,
                        manifest_json: r.get(11)?,
                    })
                },
            )
            .optional()
            .map_err(db_err)?;
        Ok(rec)
    }

    pub fn insert_attachment(
        &self,
        parent_type: &str,
        parent_id: &str,
        filename: &str,
        bytes: &[u8],
        uploader: &str,
    ) -> SResult<AttachmentRec> {
        let hash = self.file_put(bytes)?;
        let id = new_id("att");
        let rec = AttachmentRec {
            id: id.clone(),
            parent_type: parent_type.into(),
            parent_id: parent_id.into(),
            filename: sanitize_attachment_filename(filename),
            hash,
            size: bytes.len() as i64,
            uploader: uploader.into(),
            created_ms: now_ms(),
        };
        self.conn
            .execute(
                "INSERT INTO attachments(id, parent_type, parent_id, filename, hash, size, uploader, created_ms) VALUES(?1,?2,?3,?4,?5,?6,?7,?8)",
                params![
                    rec.id,
                    rec.parent_type,
                    rec.parent_id,
                    rec.filename,
                    rec.hash,
                    rec.size,
                    rec.uploader,
                    rec.created_ms
                ],
            )
            .map_err(db_err)?;
        Ok(rec)
    }

    pub fn list_attachments(&self, parent_id: &str) -> SResult<Vec<AttachmentRec>> {
        let mut stmt = self
            .conn
            .prepare("SELECT id, parent_type, parent_id, filename, hash, size, uploader, created_ms FROM attachments WHERE parent_id=?1 ORDER BY created_ms")
            .map_err(db_err)?;
        let rows = stmt
            .query_map(params![parent_id], |r| {
                Ok(AttachmentRec {
                    id: r.get(0)?,
                    parent_type: r.get(1)?,
                    parent_id: r.get(2)?,
                    filename: r.get(3)?,
                    hash: r.get(4)?,
                    size: r.get(5)?,
                    uploader: r.get(6)?,
                    created_ms: r.get(7)?,
                })
            })
            .map_err(db_err)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(db_err)?;
        Ok(rows)
    }

    pub fn get_attachment(&self, id: &str) -> SResult<AttachmentRec> {
        self.conn
            .query_row(
                "SELECT id, parent_type, parent_id, filename, hash, size, uploader, created_ms FROM attachments WHERE id=?1",
                params![id],
                |r| {
                    Ok(AttachmentRec {
                        id: r.get(0)?,
                        parent_type: r.get(1)?,
                        parent_id: r.get(2)?,
                        filename: r.get(3)?,
                        hash: r.get(4)?,
                        size: r.get(5)?,
                        uploader: r.get(6)?,
                        created_ms: r.get(7)?,
                    })
                },
            )
            .map_err(db_err)
    }

    pub fn insert_showroom(
        &self,
        title: &str,
        intro: &str,
        source_index_id: Option<&str>,
        commit_id: Option<&str>,
        submitted_by: &str,
        state: &str,
    ) -> SResult<ShowroomRec> {
        let id = new_id("shr");
        let rec = ShowroomRec {
            id: id.clone(),
            title: title.into(),
            intro: intro.into(),
            source_index_id: source_index_id.map(|s| s.to_string()),
            commit_id: commit_id.map(|s| s.to_string()),
            submitted_by: submitted_by.into(),
            state: state.into(),
            created_ms: now_ms(),
            scope: SCOPE_ORG_PUBLIC.to_string(),
            review_note: String::new(),
            reviewed_by: String::new(),
            reviewed_ms: 0,
        };
        self.conn
            .execute(
                "INSERT INTO showroom(id, title, intro, source_index_id, commit_id, submitted_by, state, created_ms, scope) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9)",
                params![
                    rec.id,
                    rec.title,
                    rec.intro,
                    rec.source_index_id,
                    rec.commit_id,
                    rec.submitted_by,
                    rec.state,
                    rec.created_ms,
                    rec.scope
                ],
            )
            .map_err(db_err)?;
        Ok(rec)
    }

    /// Re-point a showroom row at a new source commit (a
    /// local re-submit updates the standing row's pinning in
    /// place instead of duplicating it): the title follows
    /// the source index's current name, and the caller
    /// rewrites the pins against the new commit.
    pub fn update_showroom_commit(
        &self,
        id: &str,
        title: &str,
        commit_id: &str,
    ) -> SResult<()> {
        let n = self
            .conn
            .execute(
                "UPDATE showroom SET title=?2, commit_id=?3 WHERE id=?1",
                params![id, title, commit_id],
            )
            .map_err(db_err)?;
        if n == 0 {
            return Err("演示条目不存在".into());
        }
        Ok(())
    }

    pub fn list_showroom(&self, state: Option<&str>) -> SResult<Vec<ShowroomRec>> {
        let mut stmt = self
            .conn
            .prepare("SELECT id, title, intro, source_index_id, commit_id, submitted_by, state, created_ms, scope, review_note, reviewed_by, reviewed_ms FROM showroom ORDER BY created_ms DESC")
            .map_err(db_err)?;
        let rows = stmt
            .query_map([], |r| {
                Ok(ShowroomRec {
                    id: r.get(0)?,
                    title: r.get(1)?,
                    intro: r.get(2)?,
                    source_index_id: r.get(3)?,
                    commit_id: r.get(4)?,
                    submitted_by: r.get(5)?,
                    state: r.get(6)?,
                    created_ms: r.get(7)?,
                    scope: r.get(8)?,
                    review_note: r.get(9)?,
                    reviewed_by: r.get(10)?,
                    reviewed_ms: r.get(11)?,
                })
            })
            .map_err(db_err)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(db_err)?;
        let trashed = self.trashed_ids("showroom")?;
        let rows: Vec<ShowroomRec> = rows
            .into_iter()
            .filter(|r| !trashed.contains(&r.id))
            .collect();
        Ok(match state {
            None => rows,
            Some(s) => rows.into_iter().filter(|r| r.state == s).collect(),
        })
    }

    /// Record a review decision (approval queue): the new state plus
    /// who decided, when, and the note (the rejection reason, which
    /// the prototype requires to travel back to the submitter).
    pub fn review_showroom(
        &self,
        id: &str,
        state: &str,
        note: &str,
        reviewed_by: &str,
    ) -> SResult<()> {
        // Showroom state is a closed vocabulary (audit L2).
        if !matches!(state, "pending" | "live" | "rejected") {
            return Err(format!("未知的演示状态：{state}"));
        }
        let n = self
            .conn
            .execute(
                "UPDATE showroom SET state=?2, review_note=?3, reviewed_by=?4, reviewed_ms=?5 WHERE id=?1",
                params![id, state, note, reviewed_by, now_ms()],
            )
            .map_err(db_err)?;
        if n == 0 {
            return Err("演示条目不存在".into());
        }
        Ok(())
    }

    pub fn set_showroom_state(&self, id: &str, state: &str) -> SResult<()> {
        // Same closed vocabulary as review_showroom (audit L2).
        if !matches!(state, "pending" | "live" | "rejected") {
            return Err(format!("未知的演示状态：{state}"));
        }
        let n = self
            .conn
            .execute(
                "UPDATE showroom SET state=?2 WHERE id=?1",
                params![id, state],
            )
            .map_err(db_err)?;
        if n == 0 {
            return Err("演示条目不存在".into());
        }
        Ok(())
    }

    /// Personal reorder copy (N4, user ruling 2026-11-25):
    /// materialize a reordered page sequence of some deck as a
    /// NEW showroom item owned by `submitted_by` — the ruling is
    /// "调整只动自己": the source item and its pins are never
    /// written; the copy is new rows only. The flattened order
    /// cannot be expressed by reordering the source item's
    /// entries (a permutation can cut across blocks), so the
    /// copy gets its own personal index: one commit whose
    /// entries are one Locked entry per maximal run of
    /// consecutive pages of the same version, each entry's
    /// excerpt slots naming its pages in the new order
    /// (`excerpt_positions` resolves tokens in the order given,
    /// so the run's page order is the token order; a page that
    /// repeats inside one run starts a new entry, because one
    /// entry's resolution dedups repeated tokens). The item
    /// points at that commit with the same versions pinned, so
    /// it publishes exactly the sequence the user arranged —
    /// the pin model's semantics, reused, not extended.
    ///
    /// `pages` is the flattened (version_id, slot) sequence in
    /// the new order. Every version must live in THIS store:
    /// callers surface the copy from this store's own showroom
    /// face, so a version held elsewhere aborts the save in the
    /// read phase, before anything is written. The write phase
    /// runs in one transaction (land_submission's shell), so a
    /// failure midway leaves no half-built copy behind.
    pub fn create_reordered_showroom_copy(
        &self,
        title: &str,
        intro: &str,
        submitted_by: &str,
        group_id: &str,
        maintainer: &str,
        pages: &[(String, usize)],
    ) -> SResult<ShowroomRec> {
        if pages.is_empty() {
            return Err("没有可另存的页面".into());
        }
        // Resolve + group (reads only): any failure here leaves
        // the store untouched.
        let mut entries: Vec<IndexEntry> = Vec::new();
        let mut pins: Vec<String> = Vec::new();
        for (version_id, slot) in pages {
            let ver = self.get_version(version_id).map_err(|_| {
                "这份演示的内容不在本地库中，副本保存在本地库，本次无法另存".to_string()
            })?;
            let page = ver.pages.get(*slot).ok_or_else(|| {
                "页序与内容不一致，请重新打开演示后再试".to_string()
            })?;
            // Page-entity tokens are position-independent; rows
            // written before page entity-ization may carry an
            // empty id, where the positional token is the only
            // name the page has (excerpt_positions reads both).
            let token = if page.id.is_empty() {
                slot.to_string()
            } else {
                page.id.clone()
            };
            let extends_last = entries.last().map_or(false, |e| {
                matches!(&e.policy, crate::model::EntryPolicy::Locked { version_id: lv } if lv.0 == ver.id)
                    && !e
                        .excerpt_slots
                        .as_ref()
                        .map(|t| t.contains(&token))
                        .unwrap_or(true)
            });
            if extends_last {
                entries
                    .last_mut()
                    .unwrap()
                    .excerpt_slots
                    .as_mut()
                    .unwrap()
                    .push(token);
            } else {
                let rend = self.get_rendition(&ver.rendition_id)?;
                entries.push(IndexEntry {
                    unit_id: crate::model::Id(rend.unit_id.clone()),
                    rendition_id: crate::model::Id(ver.rendition_id.clone()),
                    policy: crate::model::EntryPolicy::Locked {
                        version_id: crate::model::Id(ver.id.clone()),
                    },
                    locked_semver: Some(ver.semver.clone()),
                    excerpt_slots: Some(vec![token]),
                });
                pins.push(ver.id.clone());
            }
        }
        self.conn.execute_batch("BEGIN IMMEDIATE").map_err(db_err)?;
        let attempt = (|| -> SResult<ShowroomRec> {
            let index =
                self.create_index(title, None, group_id, maintainer, "company", entries)?;
            let commit_id = self
                .branch_head(&index.id, "main")?
                .ok_or_else(|| "副本索引缺少提交".to_string())?;
            let rec = self.insert_showroom(
                title,
                intro,
                Some(&index.id),
                Some(&commit_id),
                submitted_by,
                "live",
            )?;
            self.set_showroom_pins(&rec.id, &commit_id, &pins)?;
            Ok(rec)
        })();
        match attempt {
            Ok(rec) => {
                self.conn.execute_batch("COMMIT").map_err(db_err)?;
                Ok(rec)
            }
            Err(e) => {
                let _ = self.conn.execute_batch("ROLLBACK");
                Err(e)
            }
        }
    }

    // ---------- trash (soft delete, prototype #trashView) ----------
    // Deleting is an overlay: the object row stays in its own table
    // and a trash_items row hides it from the normal lists, so a
    // restore is just dropping the overlay row ("原版本与语言绑定
    // 原样保留"). Purge is the only destructive step and cascades
    // per kind. Fragment files are content-addressed and shared, so
    // purge never touches the file store.

    /// Move an object to the trash. Idempotent: re-deleting an
    /// already-trashed object returns the existing row.
    // ---------- submissions (local-first P3, design §6.3) ----------
    //
    // The server-side staging zone of the submission chain. Rows
    // here are *not* landed content: the package files sit in
    // `<root>/staging/<id>/`, outside the content-addressed store,
    // and no units / indexes / versions / showroom rows exist for
    // a pending submission — which is what makes pending content
    // structurally invisible to every distribution and material
    // face (they all read only the landed tables). Approval runs
    // `land_submission`; rejection only stamps the row.

    const SUBMISSION_COLS: &str = "id, title, intro, submitter_id, submitter_name, state, note, review_note, reviewed_by, reviewed_ms, created_ms, supersedes, source_index_id, landed_index_id, landed_showroom_id, entry_count, page_count, land_error";

    fn row_to_submission(r: &rusqlite::Row) -> rusqlite::Result<SubmissionRec> {
        Ok(SubmissionRec {
            id: r.get(0)?,
            title: r.get(1)?,
            intro: r.get(2)?,
            submitter_id: r.get(3)?,
            submitter_name: r.get(4)?,
            state: r.get(5)?,
            note: r.get(6)?,
            review_note: r.get(7)?,
            reviewed_by: r.get(8)?,
            reviewed_ms: r.get(9)?,
            created_ms: r.get(10)?,
            supersedes: r.get(11)?,
            source_index_id: r.get(12)?,
            landed_index_id: r.get(13)?,
            landed_showroom_id: r.get(14)?,
            entry_count: r.get(15)?,
            page_count: r.get(16)?,
            land_error: r.get(17)?,
        })
    }

    pub fn insert_submission(
        &self,
        rec: &SubmissionRec,
        manifest_json: &str,
    ) -> SResult<SubmissionRec> {
        self.conn
            .execute(
                "INSERT INTO submissions(id, title, intro, submitter_id, submitter_name, state, note, review_note, reviewed_by, reviewed_ms, created_ms, supersedes, source_index_id, landed_index_id, landed_showroom_id, entry_count, page_count, land_error, manifest_json) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18,?19)",
                params![
                    rec.id,
                    rec.title,
                    rec.intro,
                    rec.submitter_id,
                    rec.submitter_name,
                    rec.state,
                    rec.note,
                    rec.review_note,
                    rec.reviewed_by,
                    rec.reviewed_ms,
                    rec.created_ms,
                    rec.supersedes,
                    rec.source_index_id,
                    rec.landed_index_id,
                    rec.landed_showroom_id,
                    rec.entry_count,
                    rec.page_count,
                    rec.land_error,
                    manifest_json
                ],
            )
            .map_err(db_err)?;
        self.get_submission(&rec.id)?
            .ok_or_else(|| "提交写入后无法读回".to_string())
    }

    pub fn get_submission(&self, id: &str) -> SResult<Option<SubmissionRec>> {
        self.conn
            .query_row(
                &format!(
                    "SELECT {} FROM submissions WHERE id=?1",
                    Self::SUBMISSION_COLS
                ),
                params![id],
                Self::row_to_submission,
            )
            .optional()
            .map_err(db_err)
    }

    pub fn list_submissions(&self, state: Option<&str>) -> SResult<Vec<SubmissionRec>> {
        let sql = match state {
            Some(_) => format!(
                "SELECT {} FROM submissions WHERE state=?1 ORDER BY created_ms DESC",
                Self::SUBMISSION_COLS
            ),
            None => format!(
                "SELECT {} FROM submissions ORDER BY created_ms DESC",
                Self::SUBMISSION_COLS
            ),
        };
        let mut stmt = self.conn.prepare(&sql).map_err(db_err)?;
        let rows = match state {
            Some(s) => stmt
                .query_map(params![s], Self::row_to_submission)
                .map_err(db_err)?
                .collect::<Result<Vec<_>, _>>(),
            None => stmt
                .query_map([], Self::row_to_submission)
                .map_err(db_err)?
                .collect::<Result<Vec<_>, _>>(),
        };
        rows.map_err(db_err)
    }

    pub fn list_submissions_by_submitter(
        &self,
        user_id: &str,
    ) -> SResult<Vec<SubmissionRec>> {
        let mut stmt = self
            .conn
            .prepare(&format!(
                "SELECT {} FROM submissions WHERE submitter_id=?1 ORDER BY created_ms DESC",
                Self::SUBMISSION_COLS
            ))
            .map_err(db_err)?;
        let rows = stmt
            .query_map(params![user_id], Self::row_to_submission)
            .map_err(db_err)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(db_err)?;
        Ok(rows)
    }

    /// The stored package manifest JSON for one submission (kept
    /// out of SubmissionRec so list responses stay small).
    pub fn submission_manifest(&self, id: &str) -> SResult<String> {
        self.conn
            .query_row(
                "SELECT manifest_json FROM submissions WHERE id=?1",
                params![id],
                |r| r.get(0),
            )
            .map_err(db_err)
    }

    pub fn set_submission_rejected(&self, id: &str, note: &str, by: &str) -> SResult<()> {
        // State-machine guard (audit M5 / chain F4): only a
        // pending submission can be rejected — a stale or
        // double-fired review must not rewrite a landed/rejected
        // row's decision record. (Merge note: the G5 wording is
        // authoritative for this function per the chain line's
        // junction note; the state guard itself is identical in
        // both lines.)
        let n = self
            .conn
            .execute(
                "UPDATE submissions SET state='rejected', review_note=?2, reviewed_by=?3, reviewed_ms=?4, land_error='' WHERE id=?1 AND state='pending'",
                params![id, note, by, now_ms()],
            )
            .map_err(db_err)?;
        if n == 0 {
            return Err("该提交已处理，不能重复审批".into());
        }
        Ok(())
    }

    /// Record why a landing attempt rolled back. The submission
    /// itself stays pending; only the error note changes.
    pub fn set_submission_land_error(&self, id: &str, err: &str) -> SResult<()> {
        let n = self
            .conn
            .execute(
                "UPDATE submissions SET land_error=?2 WHERE id=?1",
                params![id, err],
            )
            .map_err(db_err)?;
        if n == 0 {
            return Err("提交不存在".into());
        }
        Ok(())
    }

    /// Delete a submission row and its staged package. Landed
    /// content (if any) is a separate object by then and is not
    /// touched — this removes the submission record only.
    ///
    /// A LANDED row is never deleted (F2): the row itself is the
    /// lineage mapping `landed_source_target` reads, so removing
    /// one breaks the mapping — and removing only the newest
    /// landed row silently re-points the mapping at an older
    /// landed index, forking the source's lineage into two index /
    /// entry lines. The server handler refuses the same way.
    pub fn delete_submission(&self, id: &str) -> SResult<()> {
        if let Some(rec) = self.get_submission(id)? {
            if rec.state == "landed" {
                return Err("已落地的提交记录是落地谱系凭证，不能删除".into());
            }
        }
        self.conn
            .execute("DELETE FROM submissions WHERE id=?1", params![id])
            .map_err(db_err)?;
        self.clear_submission_staging(id);
        Ok(())
    }

    /// The staging directory of one submission package:
    /// `<root>/staging/<id>/`, deliberately outside `files/` (the
    /// landed CAS) so staged bytes are unreachable through every
    /// content-addressed read path.
    ///
    /// The id segment is sanitized (audit H1): submission ids are
    /// server-minted, but this is a destructive-path API
    /// (`clear_submission_staging` runs `remove_dir_all` on the
    /// result), so the data layer never lets a caller-supplied
    /// id become a path traversal. A plain id (the only shape
    /// the system mints) maps to itself; anything containing a
    /// separator, a dot segment or a NUL maps to a hash-derived
    /// name inside the same staging root — consistent across
    /// stage / get / clear, and never outside it.
    pub fn submission_staging_dir(&self, id: &str) -> PathBuf {
        let plain = !id.is_empty()
            && id != "."
            && id != ".."
            && !id.contains('/')
            && !id.contains('\\')
            && !id.contains('\0');
        let name = if plain {
            id.to_string()
        } else {
            let mut h = Sha256::new();
            h.update(id.as_bytes());
            format!("invalid-{:x}", h.finalize())
        };
        self.root.join("staging").join(name)
    }

    /// Store one package file in the staging zone, verifying the
    /// bytes against the manifest's claimed SHA-256 (the same
    /// hash the landed CAS will key on after landing).
    pub fn stage_submission_file(&self, id: &str, hash: &str, bytes: &[u8]) -> SResult<()> {
        let mut h = Sha256::new();
        h.update(bytes);
        let actual = format!("{:x}", h.finalize());
        if actual != hash {
            return Err(format!(
                "提交包文件哈希不符（清单 {hash}，实际 {actual}）"
            ));
        }
        let sub = if hash.len() >= 2 { &hash[..2] } else { "00" };
        let dir = self.submission_staging_dir(id).join("files").join(sub);
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        let path = dir.join(format!("{hash}.bin"));
        if !path.exists() {
            craft_atomic::atomic_write(&path, bytes).map_err(|e| e.to_string())?;
        }
        Ok(())
    }

    pub fn staged_file_get(&self, id: &str, hash: &str) -> SResult<Vec<u8>> {
        // The hash comes from the client-supplied manifest
        // (audit M1): it must have exactly the stored-digest
        // shape before it may become a path, and the bytes read
        // back are re-verified against it. Without this, a
        // crafted fragment_hash could walk out of the staging
        // zone into the landed CAS and land content the package
        // never uploaded.
        if !is_sha256_hex(hash) {
            return Err(format!("待审包文件哈希不合法: {hash}"));
        }
        let path = self
            .submission_staging_dir(id)
            .join("files")
            .join(&hash[..2])
            .join(format!("{hash}.bin"));
        let bytes = std::fs::read(&path).map_err(|e| format!("待审包文件 {hash}: {e}"))?;
        let mut h = Sha256::new();
        h.update(&bytes);
        let actual = format!("{:x}", h.finalize());
        if actual != hash {
            return Err(format!(
                "待审包文件 {hash} 哈希复验不符（实际 {actual}）"
            ));
        }
        Ok(bytes)
    }

    pub fn clear_submission_staging(&self, id: &str) {
        let _ = std::fs::remove_dir_all(self.submission_staging_dir(id));
    }

    /// Bytes one submission's staged package occupies on disk
    /// (0 once its staging has been cleared).
    pub fn submission_staged_bytes(&self, id: &str) -> u64 {
        dir_tree_bytes(&self.submission_staging_dir(id))
    }

    /// The settings key holding the pending-submission retention
    /// window, in days (server settings table; written only
    /// through the super-gated settings endpoint).
    pub const SUBMISSION_TTL_KEY: &'static str = "submission_pending_ttl_days";

    /// Retention default (user ruling 2026-11-25, item 11): a
    /// pending submission's staged package is kept 30 days.
    pub const SUBMISSION_TTL_DEFAULT_DAYS: i64 = 30;

    /// The effective retention window in days. A stored value of
    /// 0 (or negative) means "never expire" — an explicit escape
    /// hatch a super admin can set; a missing or unparsable value
    /// falls back to the ruled default rather than failing.
    pub fn submission_pending_ttl_days(&self) -> SResult<i64> {
        match self.get_setting(Self::SUBMISSION_TTL_KEY)? {
            Some(v) => Ok(v
                .trim()
                .parse::<i64>()
                .unwrap_or(Self::SUBMISSION_TTL_DEFAULT_DAYS)),
            None => Ok(Self::SUBMISSION_TTL_DEFAULT_DAYS),
        }
    }

    /// Expire every pending submission older than the retention
    /// window: its staged package is deleted and the row moves
    /// to the terminal `expired` state (the record itself is
    /// kept — it stays visible in the submitter's 「我的提交」
    /// and in the review records, with the expiry explanation in
    /// review_note). Returns the ids that actually transitioned.
    ///
    /// Concurrency: the state flip is a guarded UPDATE
    /// (`... WHERE state='pending'`, the set_submission_rejected
    /// pattern) and the staging directory is removed only when
    /// this call won that flip. Landing re-checks the state
    /// inside its BEGIN IMMEDIATE transaction, so a submission
    /// being landed concurrently either commits first (the
    /// UPDATE then matches 0 rows — nothing is expired and no
    /// file is touched) or observes `expired` on its in-flight
    /// re-check and rolls back before reading any staged byte.
    /// A submission can therefore never lose its files mid-land.
    pub fn sweep_expired_submissions(&self, now: i64) -> SResult<Vec<String>> {
        let days = self.submission_pending_ttl_days()?;
        if days <= 0 {
            return Ok(Vec::new());
        }
        let cutoff = now - days * 86_400_000;
        let mut stmt = self
            .conn
            .prepare(
                "SELECT id, submitter_id, title FROM submissions WHERE state='pending' AND created_ms<?1 ORDER BY created_ms",
            )
            .map_err(db_err)?;
        let rows: Vec<(String, String, String)> = stmt
            .query_map(params![cutoff], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?))
            })
            .map_err(db_err)?
            .collect::<Result<_, _>>()
            .map_err(db_err)?;
        drop(stmt);
        let note = format!(
            "待审超过保留天数（{days} 天），暂存已自动清理，请重新提交"
        );
        let mut expired = Vec::new();
        for (id, submitter_id, title) in &rows {
            let n = self
                .conn
                .execute(
                    "UPDATE submissions SET state='expired', review_note=?2, reviewed_by='', reviewed_ms=?3, land_error='' WHERE id=?1 AND state='pending'",
                    params![id.as_str(), note, now],
                )
                .map_err(db_err)?;
            if n == 1 {
                self.clear_submission_staging(id);
                // Message center (full version): the submitter
                // learns of the expiry from the notification
                // stream, not only by re-checking 我的提交. The
                // flip already committed above; a failed message
                // write must never fail the sweep, so the result
                // is deliberately dropped.
                let _ = self.notify_user(
                    submitter_id,
                    "expired",
                    "投稿已过期",
                    &format!("《{title}》{note}"),
                    "submission",
                    id,
                );
                expired.push(id.clone());
            }
        }
        Ok(expired)
    }

    // ---------------- notifications (message center, full
    // version — user ruling 2026-11-25) ----------------
    //
    // One table, two stores (see NotificationRec): the server
    // writes rows at workflow transition points and serves them
    // through GET /v1/notifications; a client library mirrors
    // those rows (same ids) next to its own local `system` rows
    // and owns the read flags and the deletion tombstones.

    /// Write one server-side notification row. Callers at
    /// transition points deliberately drop the Result: a failed
    /// message write must never roll back the workflow action
    /// that produced it.
    pub fn notify_user(
        &self,
        user_id: &str,
        kind: &str,
        title: &str,
        body: &str,
        ref_kind: &str,
        ref_id: &str,
    ) -> SResult<String> {
        if user_id.is_empty() {
            return Err("通知缺少接收人".into());
        }
        let id = new_id("ntf");
        self.conn
            .execute(
                "INSERT INTO notifications(id, user_id, kind, title, body, ref_kind, ref_id, created_ms, read, source) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,0,'server')",
                params![id, user_id, kind, title, body, ref_kind, ref_id, now_ms()],
            )
            .map_err(db_err)?;
        Ok(id)
    }

    /// Every enabled super-admin's user id — the reviewer set
    /// that "有新待审批" notifications fan out to (submission and
    /// showroom review are both super-gated).
    pub fn super_user_ids(&self) -> SResult<Vec<String>> {
        let mut stmt = self
            .conn
            .prepare("SELECT id FROM users WHERE role='super' AND disabled=0 ORDER BY rowid")
            .map_err(db_err)?;
        let ids = stmt
            .query_map([], |r| r.get(0))
            .map_err(db_err)?
            .collect::<Result<Vec<String>, _>>()
            .map_err(db_err)?;
        Ok(ids)
    }

    /// Fan one notification out to every super admin. Fully
    /// best-effort: per-recipient failures are swallowed here so
    /// a caller at a transition point has nothing to handle.
    pub fn notify_supers(
        &self,
        kind: &str,
        title: &str,
        body: &str,
        ref_kind: &str,
        ref_id: &str,
    ) {
        if let Ok(ids) = self.super_user_ids() {
            for uid in ids {
                let _ = self.notify_user(&uid, kind, title, body, ref_kind, ref_id);
            }
        }
    }

    /// The server pull face: one user's notifications, newest
    /// first. `since_ms` is an incremental cursor (strictly
    /// greater); clients merge by id, so an overlapping window
    /// is harmless.
    pub fn list_notifications(
        &self,
        user_id: &str,
        since_ms: i64,
    ) -> SResult<Vec<NotificationRec>> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT id, user_id, kind, title, body, ref_kind, ref_id, created_ms, read, source FROM notifications WHERE user_id=?1 AND created_ms>?2 ORDER BY created_ms DESC, rowid DESC LIMIT 500",
            )
            .map_err(db_err)?;
        let rows = stmt
            .query_map(params![user_id, since_ms], |r| {
                Ok(NotificationRec {
                    id: r.get(0)?,
                    user_id: r.get(1)?,
                    kind: r.get(2)?,
                    title: r.get(3)?,
                    body: r.get(4)?,
                    ref_kind: r.get(5)?,
                    ref_id: r.get(6)?,
                    created_ms: r.get(7)?,
                    read: r.get(8)?,
                    source: r.get(9)?,
                })
            })
            .map_err(db_err)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(db_err)?;
        Ok(rows)
    }

    /// Record one of this machine's own operation-feedback
    /// messages in the local library (kind `system`). `user_id`
    /// is the signed-in user at the time, or "" — the local list
    /// face shows local rows regardless (they are this machine's
    /// feedback, not an account's data).
    pub fn insert_local_system_message(
        &self,
        title: &str,
        body: &str,
        user_id: &str,
    ) -> SResult<NotificationRec> {
        let rec = NotificationRec {
            id: new_id("loc"),
            user_id: user_id.to_string(),
            kind: "system".into(),
            title: title.to_string(),
            body: body.to_string(),
            ref_kind: String::new(),
            ref_id: String::new(),
            created_ms: now_ms(),
            read: 0,
            source: "local".into(),
        };
        self.conn
            .execute(
                "INSERT INTO notifications(id, user_id, kind, title, body, ref_kind, ref_id, created_ms, read, source) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,0,'local')",
                params![
                    rec.id,
                    rec.user_id,
                    rec.kind,
                    rec.title,
                    rec.body,
                    rec.ref_kind,
                    rec.ref_id,
                    rec.created_ms
                ],
            )
            .map_err(db_err)?;
        Ok(rec)
    }

    /// Merge pulled server rows into the local mirror. Rules:
    /// a tombstoned id is skipped (locally deleted — never
    /// resurrects, ruling C2); an id already present is left
    /// untouched, so its local read flag survives; a new id
    /// lands unread with source='server'. Returns how many rows
    /// were actually inserted.
    pub fn merge_server_notifications(
        &self,
        items: &[NotificationRec],
    ) -> SResult<usize> {
        let mut inserted = 0usize;
        for it in items {
            let tombstoned: bool = self
                .conn
                .query_row(
                    "SELECT COUNT(*) FROM notification_tombstones WHERE server_id=?1",
                    params![it.id],
                    |r| r.get::<_, i64>(0),
                )
                .map_err(db_err)?
                > 0;
            if tombstoned {
                continue;
            }
            let n = self
                .conn
                .execute(
                    "INSERT OR IGNORE INTO notifications(id, user_id, kind, title, body, ref_kind, ref_id, created_ms, read, source) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,0,'server')",
                    params![
                        it.id,
                        it.user_id,
                        it.kind,
                        it.title,
                        it.body,
                        it.ref_kind,
                        it.ref_id,
                        it.created_ms
                    ],
                )
                .map_err(db_err)?;
            inserted += n;
        }
        Ok(inserted)
    }

    /// The local list face: this machine's own rows plus the
    /// signed-in user's mirrored server rows, newest first.
    /// Logged out (user_id ""), only local rows show — server
    /// rows wait for their owner.
    pub fn list_local_messages(&self, user_id: &str) -> SResult<Vec<NotificationRec>> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT id, user_id, kind, title, body, ref_kind, ref_id, created_ms, read, source FROM notifications WHERE source='local' OR user_id=?1 ORDER BY created_ms DESC, rowid DESC LIMIT 500",
            )
            .map_err(db_err)?;
        let rows = stmt
            .query_map(params![user_id], |r| {
                Ok(NotificationRec {
                    id: r.get(0)?,
                    user_id: r.get(1)?,
                    kind: r.get(2)?,
                    title: r.get(3)?,
                    body: r.get(4)?,
                    ref_kind: r.get(5)?,
                    ref_id: r.get(6)?,
                    created_ms: r.get(7)?,
                    read: r.get(8)?,
                    source: r.get(9)?,
                })
            })
            .map_err(db_err)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(db_err)?;
        Ok(rows)
    }

    /// Unread count over the same visible set as
    /// `list_local_messages` — the toolbar bell's real number.
    pub fn count_local_unread(&self, user_id: &str) -> SResult<i64> {
        let n = self
            .conn
            .query_row(
                "SELECT COUNT(*) FROM notifications WHERE read=0 AND (source='local' OR user_id=?1)",
                params![user_id],
                |r| r.get(0),
            )
            .map_err(db_err)?;
        Ok(n)
    }

    /// Mark one local message read (the per-card click). A
    /// missing id is a no-op, not an error — the list may have
    /// been cleared between render and click.
    pub fn mark_local_message_read(&self, id: &str) -> SResult<()> {
        self.conn
            .execute(
                "UPDATE notifications SET read=1 WHERE id=?1",
                params![id],
            )
            .map_err(db_err)?;
        Ok(())
    }

    fn tombstone_server_message(&self, server_id: &str) -> SResult<()> {
        self.conn
            .execute(
                "INSERT OR IGNORE INTO notification_tombstones(server_id, deleted_ms) VALUES(?1,?2)",
                params![server_id, now_ms()],
            )
            .map_err(db_err)?;
        Ok(())
    }

    /// Delete one local message. A server-origin row also drops
    /// a tombstone, so the next pull does not resurrect it; the
    /// server's own row is never touched (ruling C2).
    pub fn delete_local_message(&self, id: &str) -> SResult<()> {
        let source: Option<String> = self
            .conn
            .query_row(
                "SELECT source FROM notifications WHERE id=?1",
                params![id],
                |r| r.get(0),
            )
            .optional()
            .map_err(db_err)?;
        let Some(source) = source else {
            return Ok(());
        };
        if source == "server" {
            self.tombstone_server_message(id)?;
        }
        self.conn
            .execute("DELETE FROM notifications WHERE id=?1", params![id])
            .map_err(db_err)?;
        Ok(())
    }

    /// The clear dialog's three modes over the visible set:
    /// `mode` is "all" | "kind" | "older" (older uses
    /// `older_days`, the dialog's default 7). Read and unread
    /// alike — the condition is the only filter. Server-origin
    /// rows in the deleted set are tombstoned first. Returns the
    /// number of rows removed.
    pub fn clear_local_messages(
        &self,
        user_id: &str,
        mode: &str,
        kind: &str,
        older_days: i64,
    ) -> SResult<usize> {
        let cond = match mode {
            "all" => "",
            "kind" => " AND kind=?2",
            "older" => " AND created_ms<?2",
            _ => return Err(format!("未知的清理方式：{mode}")),
        };
        let cutoff = now_ms() - older_days.max(0) * 86_400_000;
        let select = format!(
            "SELECT id, source FROM notifications WHERE (source='local' OR user_id=?1){cond}"
        );
        let mut stmt = self.conn.prepare(&select).map_err(db_err)?;
        let map_row = |r: &rusqlite::Row<'_>| Ok((r.get(0)?, r.get(1)?));
        let victims: Vec<(String, String)> = match mode {
            "kind" => stmt
                .query_map(params![user_id, kind], map_row)
                .map_err(db_err)?
                .collect::<Result<Vec<_>, _>>()
                .map_err(db_err)?,
            "older" => stmt
                .query_map(params![user_id, cutoff], map_row)
                .map_err(db_err)?
                .collect::<Result<Vec<_>, _>>()
                .map_err(db_err)?,
            _ => stmt
                .query_map(params![user_id], map_row)
                .map_err(db_err)?
                .collect::<Result<Vec<_>, _>>()
                .map_err(db_err)?,
        };
        drop(stmt);
        for (id, source) in &victims {
            if source == "server" {
                self.tombstone_server_message(id)?;
            }
        }
        let delete = format!(
            "DELETE FROM notifications WHERE (source='local' OR user_id=?1){cond}"
        );
        let n = match mode {
            "kind" => self
                .conn
                .execute(&delete, params![user_id, kind])
                .map_err(db_err)?,
            "older" => self
                .conn
                .execute(&delete, params![user_id, cutoff])
                .map_err(db_err)?,
            _ => self
                .conn
                .execute(&delete, params![user_id])
                .map_err(db_err)?,
        };
        Ok(n)
    }

    /// (pending count, staged bytes) a submitter currently holds
    /// in the staging zone — the create-submission quota (F5)
    /// reads this. Bytes are measured on disk: the staging
    /// directories are the staged truth.
    pub fn pending_submission_usage(&self, submitter_id: &str) -> SResult<(usize, u64)> {
        let mut count = 0usize;
        let mut bytes = 0u64;
        for sub in self.list_submissions_by_submitter(submitter_id)? {
            if sub.state != "pending" {
                continue;
            }
            count += 1;
            bytes += self.submission_staged_bytes(&sub.id);
        }
        Ok((count, bytes))
    }

    /// The server index (and showroom entry) a submitter's local
    /// index already landed as, if any. The mapping key is
    /// (submitter_id, source_index_id): the client-side id is only
    /// ever trusted together with the submitter who recorded it,
    /// never resolved on its own. The submissions rows themselves
    /// are the mapping record — every landed row stamps
    /// landed_index_id / landed_showroom_id — so no separate
    /// mapping table exists; the newest landed row wins. Returns
    /// None when the source never landed, or when the landed index
    /// has since been deleted or trashed; the caller then lands
    /// fresh, exactly as a first submission.
    pub fn landed_source_target(
        &self,
        submitter_id: &str,
        source_index_id: &str,
    ) -> SResult<Option<(String, Option<String>)>> {
        let row = self
            .conn
            .query_row(
                "SELECT landed_index_id, landed_showroom_id FROM submissions WHERE submitter_id=?1 AND source_index_id=?2 AND state='landed' AND landed_index_id IS NOT NULL ORDER BY created_ms DESC, rowid DESC LIMIT 1",
                params![submitter_id, source_index_id],
                |r| Ok((r.get::<_, String>(0)?, r.get::<_, Option<String>>(1)?)),
            )
            .optional()
            .map_err(db_err)?;
        let Some((index_id, showroom_id)) = row else {
            return Ok(None);
        };
        if self.get_index(&index_id).is_err() {
            return Ok(None);
        }
        if self.trash_find("index", &index_id)?.is_some() {
            return Ok(None);
        }
        Ok(Some((index_id, showroom_id)))
    }

    /// The showroom entry a submitter's source index most recently
    /// landed as — the entry half of the lineage mapping, read
    /// WITHOUT the index-side validity checks
    /// `landed_source_target` applies (F3). Landing uses it when
    /// the landed index itself is no longer reusable (trashed or
    /// deleted): the entry may still exist and must be refreshed
    /// onto the new landing rather than duplicated, or the source
    /// ends up owning two live entries.
    pub fn landed_source_entry(
        &self,
        submitter_id: &str,
        source_index_id: &str,
    ) -> SResult<Option<String>> {
        self.conn
            .query_row(
                "SELECT landed_showroom_id FROM submissions WHERE submitter_id=?1 AND source_index_id=?2 AND state='landed' AND landed_showroom_id IS NOT NULL ORDER BY created_ms DESC, rowid DESC LIMIT 1",
                params![submitter_id, source_index_id],
                |r| r.get::<_, String>(0),
            )
            .optional()
            .map_err(db_err)
    }

    /// One showroom row by id, WITHOUT the trash overlay filter
    /// `get_showroom` applies (F3): landing needs the row even
    /// while it is trashed, so it can refresh (and un-trash) the
    /// single entry a source owns instead of inserting a second
    /// one that would collide with it on restore.
    pub fn get_showroom_any(&self, id: &str) -> SResult<Option<ShowroomRec>> {
        self.conn
            .query_row(
                "SELECT id, title, intro, source_index_id, commit_id, submitted_by, state, created_ms, scope, review_note, reviewed_by, reviewed_ms FROM showroom WHERE id=?1",
                params![id],
                |r| {
                    Ok(ShowroomRec {
                        id: r.get(0)?,
                        title: r.get(1)?,
                        intro: r.get(2)?,
                        source_index_id: r.get(3)?,
                        commit_id: r.get(4)?,
                        submitted_by: r.get(5)?,
                        state: r.get(6)?,
                        created_ms: r.get(7)?,
                        scope: r.get(8)?,
                        review_note: r.get(9)?,
                        reviewed_by: r.get(10)?,
                        reviewed_ms: r.get(11)?,
                    })
                },
            )
            .optional()
            .map_err(db_err)
    }

    /// The landing transaction (design §6.3): rebuild a pending
    /// submission's package as organization content — units,
    /// renditions, versions, an index with its pinned commit, and
    /// a live showroom entry — and stamp the submission landed.
    /// All database writes run inside one SQLite transaction: any
    /// failure rolls the whole landing back, leaving the
    /// submission pending (the caller records the reason via
    /// `set_submission_land_error`) and the staged package intact.
    /// Fragment bytes move through the content-addressed store
    /// (`insert_version` → `file_put`); files written before a
    /// rollback stay in the CAS as unreferenced orphans, harmless
    /// by construction (content-addressed, deduplicated, never
    /// served without a version row pointing at them). Ids are
    /// re-minted server-side (design §9 R4); a Locked entry
    /// re-locks onto the landed counterpart of the version the
    /// package resolved for it. Source lineage: when the
    /// submitter's local index already landed once (see
    /// `landed_source_target`), the package lands as a new
    /// commit on that same server index and refreshes its
    /// single showroom entry, explicitly re-pinning it to the
    /// landing's new commit and its reviewed version set — so
    /// the entry's formal version only ever changes through a
    /// reviewed landing, never by following the head — instead
    /// of minting a duplicate index and entry. When the landed
    /// index is no longer reusable (trashed or deleted), the
    /// package lands on a fresh index but still refreshes the
    /// source's existing entry (see `landed_source_entry`), so
    /// a source never owns two entries.
    pub fn land_submission(
        &self,
        id: &str,
        manifest: &SubmissionManifest,
        reviewer: &str,
    ) -> SResult<(String, String)> {
        let sub = self
            .get_submission(id)?
            .ok_or_else(|| "提交不存在".to_string())?;
        if sub.state == "expired" {
            return Err(
                "该提交已过期（待审超过保留天数，暂存已清理），不能落地，请提交人重新提交"
                    .into(),
            );
        }
        if sub.state != "pending" {
            return Err("该提交已处理，不能重复落地".into());
        }
        // Manifest consistency, checked before the transaction so
        // a malformed package fails fast and stays pending.
        if manifest.format != 1 {
            return Err("提交包格式版本不受支持".into());
        }
        if manifest.commit.entries.is_empty() {
            return Err("提交包没有任何条目".into());
        }
        let unit_ids: HashSet<&str> = manifest.units.iter().map(|u| u.id.as_str()).collect();
        let rendition_ids: HashSet<&str> =
            manifest.renditions.iter().map(|r| r.id.as_str()).collect();
        // No duplicate ids inside the manifest itself (audit
        // N7): the landing maps below key on id, so a repeated
        // unit id would mint a second block the map overwrites
        // — an empty orphan in the landing group — and a
        // repeated version id would land the same number twice
        // on one rendition.
        if unit_ids.len() != manifest.units.len() {
            return Err("提交包清单不一致：内容块 id 重复".into());
        }
        if rendition_ids.len() != manifest.renditions.len() {
            return Err("提交包清单不一致：rendition id 重复".into());
        }
        let version_ids: HashSet<&str> =
            manifest.versions.iter().map(|v| v.id.as_str()).collect();
        if version_ids.len() != manifest.versions.len() {
            return Err("提交包清单不一致：版本 id 重复".into());
        }
        for r in &manifest.renditions {
            if !unit_ids.contains(r.unit_id.as_str()) {
                return Err(format!(
                    "提交包清单不一致：rendition {} 引用了未声明的内容块",
                    r.id
                ));
            }
        }
        for v in &manifest.versions {
            if !rendition_ids.contains(v.rendition_id.as_str()) {
                return Err(format!(
                    "提交包清单不一致：版本 {} 引用了未声明的 rendition",
                    v.id
                ));
            }
        }

        // Only a formally published version may land. The
        // version status set is normal | recalled | draft
        // (model::VersionStatus): a recalled version can never
        // be a resolution result, a draft is not yet published
        // (and resolves like any surviving version once landed,
        // so it must never enter as one), and an unrecognized
        // status fails closed. The submitting client refuses
        // all three when building the package; landing re-checks
        // because the manifest is client-supplied. Every version
        // in the package is an entry's resolution result, so
        // gating the whole set gates exactly what review saw.
        for v in &manifest.versions {
            match v.status.as_str() {
                "normal" => {}
                "recalled" => {
                    return Err(format!(
                        "提交包清单不一致：版本 {} 已被召回，不能作为条目解析结果",
                        v.id
                    ));
                }
                "draft" => {
                    return Err(format!(
                        "提交包清单不一致：版本 {} 仍是草稿，未正式发布，不能落地",
                        v.id
                    ));
                }
                other => {
                    return Err(format!(
                        "提交包清单不一致：版本 {} 的状态无法识别（{other}）",
                        v.id
                    ));
                }
            }
        }
        for e in &manifest.commit.entries {
            if !unit_ids.contains(e.entry.unit_id.0.as_str())
                || !rendition_ids.contains(e.entry.rendition_id.0.as_str())
            {
                return Err(
                    "提交包清单不一致：索引条目引用了未声明的内容块或 rendition".into(),
                );
            }
            // Ownership, not just existence: the entry's
            // rendition must belong to the entry's unit, and the
            // version the entry resolved to (the exact version
            // review saw) must belong to the entry's rendition.
            // The submitting client checks both, but the
            // manifest is client-supplied input — landing
            // re-checks rather than trusting it, or a package
            // could pair an entry with another rendition's
            // version and land content review never saw for
            // this entry.
            let r = manifest
                .renditions
                .iter()
                .find(|r| r.id == e.entry.rendition_id.0)
                .ok_or_else(|| {
                    "提交包清单不一致：索引条目引用了未声明的内容块或 rendition".to_string()
                })?;
            if r.unit_id != e.entry.unit_id.0 {
                return Err(
                    "提交包清单不一致：索引条目的 rendition 不属于其内容块".into(),
                );
            }
            let v = manifest
                .versions
                .iter()
                .find(|v| v.id == e.version_id)
                .ok_or_else(|| "提交包清单不一致：索引条目引用了未声明的版本".to_string())?;
            if v.rendition_id != e.entry.rendition_id.0 {
                return Err(format!(
                    "提交包清单不一致：条目解析的版本 {} 不属于其 rendition",
                    v.id
                ));
            }
        }

        self.conn.execute_batch("BEGIN IMMEDIATE").map_err(db_err)?;
        let attempt = || -> SResult<(String, String)> {
            // State re-check under the write lock (F4): the
            // pending check before the transaction cannot see a
            // concurrent review that landed this same submission
            // first. BEGIN IMMEDIATE serializes landings, so the
            // state read here is the committed truth — a second
            // approval of the same submission fails and rolls
            // back instead of landing a full duplicate set of
            // units / versions / entries.
            let fresh = self
                .get_submission(id)?
                .ok_or_else(|| "提交不存在".to_string())?;
            if fresh.state == "expired" {
                return Err(
                    "该提交已过期（待审超过保留天数，暂存已清理），不能落地，请提交人重新提交"
                        .into(),
                );
            }
            if fresh.state != "pending" {
                return Err("该提交已处理，不能重复落地".into());
            }
            // Landing group: created once with root-department
            // view/use grants, reused afterwards (later grant
            // edits by an admin are never re-asserted).
            let group_id = match self
                .list_groups()?
                .into_iter()
                .find(|g| g.name == LANDING_GROUP_NAME)
            {
                Some(g) => g.id,
                None => {
                    let g = self.create_group(LANDING_GROUP_NAME)?;
                    for dept in self
                        .list_departments()?
                        .into_iter()
                        .filter(|d| d.parent_id.is_none())
                    {
                        self.add_grant(&g.id, "dept", &dept.id, "view")?;
                        self.add_grant(&g.id, "dept", &dept.id, "use")?;
                    }
                    g.id
                }
            };
            let mut unit_map = std::collections::HashMap::new();
            for u in &manifest.units {
                let nu = self.create_unit(
                    &u.name,
                    None,
                    &group_id,
                    &sub.submitter_name,
                    &u.lead_lang,
                    &u.description,
                )?;
                if u.current_revision != 1 {
                    self.conn
                        .execute(
                            "UPDATE units SET current_revision=?2 WHERE id=?1",
                            params![nu.id, u.current_revision],
                        )
                        .map_err(db_err)?;
                }
                unit_map.insert(u.id.clone(), nu.id);
            }
            let mut rendition_map = std::collections::HashMap::new();
            for r in &manifest.renditions {
                let nr =
                    self.get_or_create_rendition(&unit_map[r.unit_id.as_str()], &r.form, &r.lang)?;
                rendition_map.insert(r.id.clone(), nr.id);
            }
            let mut version_map = std::collections::HashMap::new();
            for v in &manifest.versions {
                let bytes = self.staged_file_get(id, &v.fragment_hash)?;
                let semver = SemVer::parse(&v.semver)?;
                // Statuses were gated before the transaction
                // (only formally published versions land), so
                // every version reaching this loop is "normal".
                let nv = self.insert_version_with_scripts(
                    &rendition_map[v.rendition_id.as_str()],
                    semver,
                    v.basis_revision,
                    &v.author,
                    &v.note,
                    &bytes,
                    &v.status,
                    Some(v.scripts.as_slice()),
                )?;
                if nv.created_ms != v.created_ms {
                    self.conn
                        .execute(
                            "UPDATE versions SET created_ms=?2 WHERE id=?1",
                            params![nv.id, v.created_ms],
                        )
                        .map_err(db_err)?;
                }
                version_map.insert(v.id.clone(), nv.id);
            }
            let mut entries: Vec<IndexEntry> = Vec::new();
            // The reviewed version of every entry, in entry
            // order — these become the showroom item's publish
            // pins below, so the formal version displays exactly
            // the set review confirmed, follow-latest entries
            // included (the landed index commit keeps its
            // follow-latest policies; only the showroom face is
            // pinned).
            let mut landed_pins: Vec<String> = Vec::new();
            for e in &manifest.commit.entries {
                let policy = match &e.entry.policy {
                    crate::model::EntryPolicy::FollowLatest => {
                        crate::model::EntryPolicy::FollowLatest
                    }
                    crate::model::EntryPolicy::Locked { .. } => {
                        crate::model::EntryPolicy::Locked {
                            version_id: crate::model::Id(
                                version_map[e.version_id.as_str()].clone(),
                            ),
                        }
                    }
                };
                entries.push(IndexEntry {
                    unit_id: crate::model::Id(unit_map[e.entry.unit_id.0.as_str()].clone()),
                    rendition_id: crate::model::Id(
                        rendition_map[e.entry.rendition_id.0.as_str()].clone(),
                    ),
                    policy,
                    locked_semver: e.entry.locked_semver.clone(),
                    excerpt_slots: e.entry.excerpt_slots.clone(),
                });
                landed_pins.push(version_map[e.version_id.as_str()].clone());
            }
            // Follow-latest entries keep their policy — following
            // is a post-landing behavior — but the landing itself
            // must display exactly what review approved: resolve
            // each landed follow-latest entry through the same
            // path the display faces use and require the reviewed
            // version's landed counterpart. A package whose
            // follow-latest resolution would drift at the landing
            // instant (extra versions of the rendition riding
            // along, a mis-paired pin) fails and stays pending.
            // Locked entries need no such check: they re-lock
            // onto the reviewed version by construction.
            for (e, landed) in manifest.commit.entries.iter().zip(entries.iter()) {
                if landed.policy != crate::model::EntryPolicy::FollowLatest {
                    continue;
                }
                let want = &version_map[e.version_id.as_str()];
                let resolved = self.resolve_entry_version(landed)?;
                match resolved {
                    Some(v) if v.id == *want => {}
                    _ => {
                        return Err(format!(
                            "提交包清单不一致：跟随最新条目落地后将展示与审核不一致的版本（条目解析版本 {}）",
                            e.version_id
                        ));
                    }
                }
            }
            let scope = if ["company", "parent_org", "dept_sub", "dept", "private"]
                .contains(&manifest.index_scope.as_str())
            {
                manifest.index_scope.as_str()
            } else {
                "company"
            };
            // Source lineage: if this submitter's local index
            // already landed once, reuse its server index — the
            // package becomes a new commit on its main branch
            // below — instead of minting a duplicate index. The
            // index keeps its original group; its name and scope
            // follow the newly approved package.
            let prior = match sub.source_index_id.as_deref() {
                Some(src_id) if !src_id.is_empty() => {
                    self.landed_source_target(&sub.submitter_id, src_id)?
                }
                _ => None,
            };
            let index_id = match &prior {
                Some((existing, _)) => {
                    self.conn
                        .execute(
                            "UPDATE indexes_tbl SET name=?2, scope=?3 WHERE id=?1",
                            params![existing.as_str(), manifest.index_name, scope],
                        )
                        .map_err(db_err)?;
                    self.conn
                        .execute(
                            "INSERT OR IGNORE INTO index_branches(index_id, name, head_commit) VALUES(?1,'main',NULL)",
                            params![existing.as_str()],
                        )
                        .map_err(db_err)?;
                    existing.clone()
                }
                None => {
                    let index_id = new_id("idx");
                    self.conn
                        .execute(
                            "INSERT INTO indexes_tbl(id, name, folder_id, group_id, maintainer, lifecycle, scope, created_ms) VALUES(?1,?2,NULL,?3,?4,'active',?5,?6)",
                            params![
                                index_id,
                                manifest.index_name,
                                group_id,
                                sub.submitter_name,
                                scope,
                                now_ms()
                            ],
                        )
                        .map_err(db_err)?;
                    self.conn
                        .execute(
                            "INSERT INTO index_branches(index_id, name, head_commit) VALUES(?1,'main',NULL)",
                            params![index_id],
                        )
                        .map_err(db_err)?;
                    index_id
                }
            };
            let message = if manifest.commit.message.trim().is_empty() {
                "提交落地"
            } else {
                manifest.commit.message.as_str()
            };
            // The package pins one commit on the submitter's main
            // branch; it lands as the new index's main head.
            let commit =
                self.create_commit(&index_id, "main", &sub.submitter_name, message, entries)?;
            // One showroom entry per landed source: a repeat
            // landing refreshes the existing entry and re-pins it
            // — explicitly, to this landing's newly confirmed
            // commit and reviewed version set — instead of
            // unpinning it into follow-the-head mode. The formal
            // version an item publishes therefore only ever
            // changes through a reviewed landing, never by drift:
            // the item always points at a commit it confirmed,
            // with the pins to match. A first landing (or an
            // entry permanently deleted since) inserts the entry
            // the same way.
            //
            // The existing entry is resolved independently of
            // the index-side reuse above (F3): when the landed
            // index was trashed or deleted, `prior` is None but
            // the lineage's entry half still names the one entry
            // this source owns — refreshing it onto the new
            // index/commit replaces the old live row instead of
            // leaving it live next to a second inserted entry.
            // The lookup also bypasses the trash overlay: a
            // trashed entry is refreshed in place and un-trashed
            // below, never shadowed by a duplicate that would
            // collide with it when restored.
            let prior_entry_id = match &prior {
                Some((_, entry)) => entry.clone(),
                None => match sub.source_index_id.as_deref() {
                    Some(src_id) if !src_id.is_empty() => {
                        self.landed_source_entry(&sub.submitter_id, src_id)?
                    }
                    _ => None,
                },
            };
            let prior_entry = prior_entry_id
                .as_ref()
                .and_then(|eid| self.get_showroom_any(eid).ok().flatten());
            let item_id = match &prior_entry {
                Some(entry) => {
                    // Refresh content, pins and the source pointer
                    // (the entry may still name the trashed /
                    // deleted index it first landed with). State
                    // is NOT unconditionally revived (F3): a
                    // rejected entry stays rejected — a submission
                    // approval refreshes content, it does not
                    // overrule the showroom face's own takedown;
                    // re-publishing a rejected entry goes through
                    // showroom review. Any other state publishes
                    // as before.
                    self.conn
                        .execute(
                            "UPDATE showroom SET title=?2, intro=?3, source_index_id=?4, commit_id=?5, state=CASE WHEN state='rejected' THEN 'rejected' ELSE 'live' END WHERE id=?1",
                            params![entry.id, sub.title, sub.intro, index_id, commit.id],
                        )
                        .map_err(db_err)?;
                    self.set_showroom_pins(&entry.id, &commit.id, &landed_pins)?;
                    // A trashed entry comes back as the source's
                    // single entry carrying this landing's
                    // content; leaving the overlay row in place
                    // would hide the refreshed entry and invite
                    // the duplicate this resolution exists to
                    // prevent.
                    if let Some(t) = self.trash_find("showroom", &entry.id)? {
                        self.trash_restore(&t.id)?;
                    }
                    entry.id.clone()
                }
                None => {
                    let rec = self.insert_showroom(
                        &sub.title,
                        &sub.intro,
                        Some(&index_id),
                        Some(&commit.id),
                        &sub.submitter_id,
                        "live",
                    )?;
                    self.set_showroom_pins(&rec.id, &commit.id, &landed_pins)?;
                    rec.id
                }
            };
            let reviewed_ms = now_ms();
            self.conn
                .execute(
                    "UPDATE showroom SET reviewed_by=?2, reviewed_ms=?3 WHERE id=?1",
                    params![item_id, reviewer, reviewed_ms],
                )
                .map_err(db_err)?;
            self.conn
                .execute(
                    "UPDATE submissions SET state='landed', landed_index_id=?2, landed_showroom_id=?3, reviewed_by=?4, reviewed_ms=?5, review_note='', land_error='' WHERE id=?1",
                    params![id, index_id, item_id, reviewer, reviewed_ms],
                )
                .map_err(db_err)?;
            Ok((index_id, item_id))
        };
        match attempt() {
            Ok(v) => {
                self.conn.execute_batch("COMMIT").map_err(db_err)?;
                Ok(v)
            }
            Err(e) => {
                let _ = self.conn.execute_batch("ROLLBACK");
                Err(e)
            }
        }
    }

    /// Permanently delete a unit and everything it owns (versions
    /// and their tags, renditions, page bindings) — the cascade
    /// `trash_purge` runs for kind "unit", exposed for callers
    /// that remove a unit which was never trashed: the staged
    /// import (line ⑤) drops the one-page wrapper units of pages
    /// a section (or a caller-supplied block) absorbs, and of
    /// pages no block covers. Page ENTITIES are not touched: a
    /// page outlives every wrapper that referenced it.
    /// Merge a single-page unit into another unit's draft
    /// (content-wall card drag onto a card, user ruling
    /// 2026-11-25): the source's one page is appended to the END
    /// of the target lead rendition's working page sequence, and
    /// the source unit then dissolves — permanently, NOT through
    /// the trash. What moves is the reference layer: the page
    /// entity stays in the page store untouched, exactly like an
    /// import fold absorbing its wrapper units. The write stays
    /// inside the version system: when the target's working
    /// version is a draft, the page is appended to that draft
    /// row itself (a draft is unpublished work — the in-place
    /// field updates `set_version_scripts` and
    /// `set_version_fragment_hash` are the precedent); when the
    /// working version is published, a NEW draft version (next
    /// minor number, status draft) is born carrying the extended
    /// sequence and the published version is never touched. A
    /// fresh draft also sorts first in the history list (highest
    /// number), ahead of the numbered versions newest-to-oldest.
    /// Guards, all checked before any write: the source must be
    /// a different unit whose lead working version holds exactly
    /// one already-split, entity-backed page; the target's
    /// working version must be fully entity-backed; and the
    /// source passes `delete_unit_permanent`'s publish-pin guard
    /// up front, so a merge can never half-land (page appended,
    /// source stuck) on a pinned source.
    pub fn merge_unit_page_into(
        &self,
        target_id: &str,
        source_id: &str,
        author: &str,
    ) -> SResult<VersionRec> {
        if target_id == source_id {
            return Err("不能把内容块并入它自己".into());
        }
        // --- source: one split page, entity-backed ---
        let src_unit = self.get_unit(source_id)?;
        let src_rends = self.list_renditions(&src_unit.id)?;
        let src_rend = src_rends.first().ok_or("源内容块暂无版本")?;
        let src_ver = self
            .latest_version(&src_rend.id)?
            .ok_or("源内容块暂无版本")?;
        if src_ver.pages.len() != 1 {
            return Err("只能把单页内容块并入其他内容块".into());
        }
        if src_ver.fragment_hash.is_empty() {
            return Err("源内容块尚未拆分完成，不能并入".into());
        }
        let src_page = src_ver.pages[0].clone();
        if src_page.id.is_empty() || self.get_page(&src_page.id)?.is_none() {
            return Err("源页面尚未实体化，不能并入".into());
        }
        // Publish-pin guard, up front (`delete_unit_permanent`
        // re-checks it at the dissolve step; the store's callers
        // are serialized, so the verdict cannot move between the
        // two reads).
        let pinned_live: i64 = self
            .conn
            .query_row(
                "SELECT COUNT(*) FROM showroom_pins p JOIN showroom s ON s.id = p.showroom_id JOIN versions v ON v.id = p.version_id JOIN renditions r ON r.id = v.rendition_id WHERE r.unit_id = ?1 AND s.state = 'live'",
                params![source_id],
                |r| r.get(0),
            )
            .map_err(db_err)?;
        if pinned_live > 0 {
            return Err(
                "该内容块的版本正被演示厅条目钉为正式版，不能并入；请先更新该条目的演示推送或将其下架"
                    .into(),
            );
        }
        // --- target: working version, entity-backed pages ---
        let tgt_unit = self.get_unit(target_id)?;
        let tgt_rends = self.list_renditions(&tgt_unit.id)?;
        let tgt_rend = tgt_rends.first().ok_or("目标内容块暂无版本")?;
        let working = self
            .latest_version(&tgt_rend.id)?
            .ok_or("目标内容块暂无版本")?;
        if working.fragment_hash.is_empty() {
            return Err("目标内容块尚未拆分完成，不能并入".into());
        }
        if working.pages.iter().any(|p| p.id.is_empty()) {
            return Err("目标内容块含未实体化的页面，不能并入".into());
        }
        // --- build the extended sequence ---
        let mut pages: Vec<PageInfo> = working.pages.clone();
        pages.push(src_page);
        let mut scripts: Vec<String> = working.scripts.clone();
        scripts.resize(working.pages.len(), String::new());
        scripts.push(src_ver.scripts.first().cloned().unwrap_or_default());
        // Fragment + per-page texts from the page packages (the
        // import fold's assembly, generalized): the working
        // version's packages in reference order, then the source
        // page's own. Per-page texts feed only the search index
        // (`text_all`); scripts come from the stored rows above
        // so hand-edited scripts survive.
        let mut pkgs = self.version_page_packages(&working)?;
        if pkgs.len() != working.pages.len() {
            return Err("目标内容块的页面组包不完整，不能并入".into());
        }
        let src_pkgs = self.version_page_packages(&src_ver)?;
        if src_pkgs.len() != 1 {
            return Err("源页面的组包不完整，不能并入".into());
        }
        pkgs.extend(src_pkgs);
        let mut texts: Vec<String> = Vec::with_capacity(pkgs.len());
        for pkg in &pkgs {
            let slide_refs = crate::pptx::slides(pkg)?;
            let first = slide_refs.first().ok_or("页面组包缺少幻灯片")?;
            let xml = pkg.get_text(&first.part)?;
            texts.push(slide_full_text(&xml));
        }
        let frag = crate::assemble::assemble(&pkgs)?;
        let frag_bytes = frag.to_bytes()?;
        // --- land it in the draft layer ---
        let draft = if working.status == "draft" {
            let hash = self.file_put(&frag_bytes)?;
            let n = self
                .conn
                .execute(
                    "UPDATE versions SET fragment_hash=?2, page_count=?3, pages_json=?4, text_all=?5, scripts_json=?6 WHERE id=?1 AND status='draft'",
                    params![
                        working.id,
                        hash,
                        pages.len() as i64,
                        serde_json::to_string(&pages).unwrap_or_default(),
                        texts.join("\n"),
                        serde_json::to_string(&scripts).unwrap_or_default()
                    ],
                )
                .map_err(db_err)?;
            if n == 0 {
                return Err("目标草稿状态已变化，请重试".into());
            }
            self.get_version(&working.id)?
        } else {
            let next = SemVer::parse(&working.semver)?.bumped(crate::semver::Increment::Minor);
            self.insert_version_from_pages(
                &tgt_rend.id,
                next,
                tgt_unit.current_revision,
                author,
                "并入页面",
                "draft",
                &pages,
                &texts,
                &scripts,
                Some(&frag_bytes),
            )?
        };
        // --- dissolve the source (its page entity stays) ---
        self.delete_unit_permanent(source_id)?;
        Ok(draft)
    }

    pub fn delete_unit_permanent(&self, unit_id: &str) -> SResult<()> {
        // Same publish-pin guard as delete_version (pin audit
        // N1): permanently deleting a unit whose version a live
        // showroom item pins would silently re-resolve the item
        // onto a substitute. The staged import's wrapper units
        // are never published, so they never trip this.
        let pinned_live: i64 = self
            .conn
            .query_row(
                "SELECT COUNT(*) FROM showroom_pins p JOIN showroom s ON s.id = p.showroom_id JOIN versions v ON v.id = p.version_id JOIN renditions r ON r.id = v.rendition_id WHERE r.unit_id = ?1 AND s.state = 'live'",
                params![unit_id],
                |r| r.get(0),
            )
            .map_err(db_err)?;
        if pinned_live > 0 {
            return Err(
                "该内容块的版本正被演示厅条目钉为正式版，不能彻底删除；请先更新该条目的演示推送或将其下架"
                    .into(),
            );
        }
        if self.conn.is_autocommit() {
            let tx = self.conn.unchecked_transaction().map_err(db_err)?;
            Self::delete_unit_rows(&tx, unit_id)?;
            tx.commit().map_err(db_err)
        } else {
            Self::delete_unit_rows(&self.conn, unit_id)
        }
    }

    /// The unit cascade itself, on a caller-provided
    /// connection handle so `trash_purge` can run it inside its
    /// own transaction. Attachments die with the unit (audit
    /// M3): their CAS files are content-addressed and shared,
    /// so only the rows go — a purged unit must not leave
    /// attachment rows pointing at nothing.
    fn delete_unit_rows(conn: &Connection, unit_id: &str) -> SResult<()> {
        conn.execute(
            "DELETE FROM version_tags WHERE version_id IN (SELECT v.id FROM versions v JOIN renditions r ON v.rendition_id = r.id WHERE r.unit_id = ?1)",
            params![unit_id],
        )
        .map_err(db_err)?;
        conn.execute(
            "DELETE FROM versions WHERE rendition_id IN (SELECT id FROM renditions WHERE unit_id = ?1)",
            params![unit_id],
        )
        .map_err(db_err)?;
        conn.execute("DELETE FROM renditions WHERE unit_id = ?1", params![unit_id])
            .map_err(db_err)?;
        conn.execute("DELETE FROM page_bindings WHERE unit_id = ?1", params![unit_id])
            .map_err(db_err)?;
        conn.execute(
            "DELETE FROM attachments WHERE parent_type = 'unit' AND parent_id = ?1",
            params![unit_id],
        )
        .map_err(db_err)?;
        conn.execute("DELETE FROM units WHERE id = ?1", params![unit_id])
            .map_err(db_err)?;
        Ok(())
    }

    pub fn trash_add(
        &self,
        kind: &str,
        object_id: &str,
        title: &str,
        origin: &str,
        deleted_by: &str,
    ) -> SResult<TrashRec> {
        if let Some(existing) = self.trash_find(kind, object_id)? {
            return Ok(existing);
        }
        if kind == "version" {
            // The delete-time face of delete_version's publish-pin
            // guard (pin audit N1, user ruling 2026-11-25): a
            // version a live showroom item pins as its formal
            // version is refused at the trash door itself, with
            // the guard's own wording — hiding it would leave the
            // item pinned to a version its owner believes deleted.
            // The guard lives here so both faces (local dispatch
            // and the server handler) inherit it from the one call
            // they share; delete_version's own check is untouched.
            let pinned_live: i64 = self
                .conn
                .query_row(
                    "SELECT COUNT(*) FROM showroom_pins p JOIN showroom s ON s.id = p.showroom_id WHERE p.version_id = ?1 AND s.state = 'live'",
                    params![object_id],
                    |r| r.get(0),
                )
                .map_err(db_err)?;
            if pinned_live > 0 {
                return Err(
                    "该版本正被演示厅条目钉为正式版，不能删除；请先更新该条目的演示推送或将其下架"
                        .into(),
                );
            }
        }
        let rec = TrashRec {
            id: new_id("trs"),
            kind: kind.into(),
            object_id: object_id.into(),
            title: title.into(),
            origin: origin.into(),
            deleted_by: deleted_by.into(),
            deleted_ms: now_ms(),
        };
        self.conn
            .execute(
                "INSERT INTO trash_items(id, kind, object_id, title, origin, deleted_by, deleted_ms) VALUES(?1,?2,?3,?4,?5,?6,?7)",
                params![
                    rec.id,
                    rec.kind,
                    rec.object_id,
                    rec.title,
                    rec.origin,
                    rec.deleted_by,
                    rec.deleted_ms
                ],
            )
            .map_err(db_err)?;
        Ok(rec)
    }

    pub fn trash_find(&self, kind: &str, object_id: &str) -> SResult<Option<TrashRec>> {
        Ok(self
            .trash_list()?
            .into_iter()
            .find(|t| t.kind == kind && t.object_id == object_id))
    }

    pub fn trash_list(&self) -> SResult<Vec<TrashRec>> {
        let mut stmt = self
            .conn
            .prepare("SELECT id, kind, object_id, title, origin, deleted_by, deleted_ms FROM trash_items ORDER BY deleted_ms DESC")
            .map_err(db_err)?;
        let rows = stmt
            .query_map([], |r| {
                Ok(TrashRec {
                    id: r.get(0)?,
                    kind: r.get(1)?,
                    object_id: r.get(2)?,
                    title: r.get(3)?,
                    origin: r.get(4)?,
                    deleted_by: r.get(5)?,
                    deleted_ms: r.get(6)?,
                })
            })
            .map_err(db_err)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(db_err)?;
        Ok(rows)
    }

    pub fn trash_get(&self, id: &str) -> SResult<TrashRec> {
        self.trash_list()?
            .into_iter()
            .find(|t| t.id == id)
            .ok_or_else(|| "trash item not found".to_string())
    }

    /// The object ids currently hidden by the trash, for one kind.
    pub fn trashed_ids(&self, kind: &str) -> SResult<std::collections::HashSet<String>> {
        Ok(self
            .trash_list()?
            .into_iter()
            .filter(|t| t.kind == kind)
            .map(|t| t.object_id)
            .collect())
    }

    /// Restore: drop the overlay row; the object reappears in its
    /// normal lists exactly as it was.
    pub fn trash_restore(&self, id: &str) -> SResult<TrashRec> {
        let rec = self.trash_get(id)?;
        self.conn
            .execute("DELETE FROM trash_items WHERE id=?1", params![rec.id])
            .map_err(db_err)?;
        Ok(rec)
    }

    /// Purge: permanently delete the object (cascade per kind), then
    /// drop the overlay row. Returns the purged trash record.
    pub fn trash_purge(&self, id: &str) -> SResult<TrashRec> {
        let rec = self.trash_get(id)?;
        let oid = rec.object_id.as_str();
        // One transaction for the whole cascade (audit M3): a
        // failure midway used to leave the object half-deleted
        // but still listed. Pin bookkeeping (pin audit N1) is
        // part of the cascade: a unit whose version a live item
        // pins is refused (the same guard delete_unit_permanent
        // carries — the baseline purge reached it through that
        // call; the row-level split must not lose the guard),
        // and a dying showroom item's pin rows die with it
        // (whole pin sets of a removed item have no positional
        // side effects on other items).
        let tx = self.conn.unchecked_transaction().map_err(db_err)?;
        match rec.kind.as_str() {
            "unit" => {
                let pinned_live: i64 = tx
                    .query_row(
                        "SELECT COUNT(*) FROM showroom_pins p JOIN showroom s ON s.id = p.showroom_id JOIN versions v ON v.id = p.version_id JOIN renditions r ON r.id = v.rendition_id WHERE r.unit_id = ?1 AND s.state = 'live'",
                        params![oid],
                        |r| r.get(0),
                    )
                    .map_err(db_err)?;
                if pinned_live > 0 {
                    return Err(
                        "该内容块的版本正被演示厅条目钉为正式版，不能彻底删除；请先更新该条目的演示推送或将其下架"
                            .into(),
                    );
                }
                Self::delete_unit_rows(&tx, oid)?;
            }
            "index" => {
                // Showroom items sourced from this index can
                // never resolve again; they die with it, along
                // with their library / share rows.
                tx.execute(
                    "DELETE FROM showroom_pins WHERE showroom_id IN (SELECT id FROM showroom WHERE source_index_id = ?1)",
                    params![oid],
                )
                .map_err(db_err)?;
                tx.execute(
                    "DELETE FROM library_entries WHERE item_kind = 'showroom' AND item_id IN (SELECT id FROM showroom WHERE source_index_id = ?1)",
                    params![oid],
                )
                .map_err(db_err)?;
                tx.execute(
                    "DELETE FROM shares WHERE item_kind = 'showroom' AND item_id IN (SELECT id FROM showroom WHERE source_index_id = ?1)",
                    params![oid],
                )
                .map_err(db_err)?;
                tx.execute(
                    "DELETE FROM showroom WHERE source_index_id = ?1",
                    params![oid],
                )
                .map_err(db_err)?;
                // Folder main-index pointers at the purged
                // index clear (the same cleanup
                // move_index_to_folder performs), its exports
                // and attachments go too.
                tx.execute(
                    "UPDATE folders SET main_index_id=NULL WHERE main_index_id = ?1",
                    params![oid],
                )
                .map_err(db_err)?;
                tx.execute("DELETE FROM exports WHERE index_id = ?1", params![oid])
                    .map_err(db_err)?;
                tx.execute(
                    "DELETE FROM attachments WHERE parent_type = 'index' AND parent_id = ?1",
                    params![oid],
                )
                .map_err(db_err)?;
                tx.execute(
                    "DELETE FROM index_branches WHERE index_id = ?1",
                    params![oid],
                )
                .map_err(db_err)?;
                tx.execute(
                    "DELETE FROM index_commits WHERE index_id = ?1",
                    params![oid],
                )
                .map_err(db_err)?;
                tx.execute("DELETE FROM index_tags WHERE index_id = ?1", params![oid])
                    .map_err(db_err)?;
                tx.execute("DELETE FROM indexes_tbl WHERE id = ?1", params![oid])
                    .map_err(db_err)?;
            }
            "showroom" => {
                tx.execute(
                    "DELETE FROM showroom_pins WHERE showroom_id = ?1",
                    params![oid],
                )
                .map_err(db_err)?;
                tx.execute(
                    "DELETE FROM library_entries WHERE item_kind = 'showroom' AND item_id = ?1",
                    params![oid],
                )
                .map_err(db_err)?;
                tx.execute(
                    "DELETE FROM shares WHERE item_kind = 'showroom' AND item_id = ?1",
                    params![oid],
                )
                .map_err(db_err)?;
                tx.execute("DELETE FROM showroom WHERE id = ?1", params![oid])
                    .map_err(db_err)?;
            }
            "version" => {
                // delete_version's cascade, inside this purge's
                // transaction, with its publish-pin guard first
                // (same query, same refusal): a version that went
                // to the trash unpinned can still have been pinned
                // since — by another item's re-push — and purging
                // it would silently re-resolve that item.
                let pinned_live: i64 = tx
                    .query_row(
                        "SELECT COUNT(*) FROM showroom_pins p JOIN showroom s ON s.id = p.showroom_id WHERE p.version_id = ?1 AND s.state = 'live'",
                        params![oid],
                        |r| r.get(0),
                    )
                    .map_err(db_err)?;
                if pinned_live > 0 {
                    return Err(
                        "该版本正被演示厅条目钉为正式版，不能彻底删除；请先更新该条目的演示推送或将其下架"
                            .into(),
                    );
                }
                tx.execute(
                    "DELETE FROM version_tags WHERE version_id=?1",
                    params![oid],
                )
                .map_err(db_err)?;
                tx.execute(
                    "DELETE FROM page_bindings WHERE from_version_id=?1 OR to_version_id=?1",
                    params![oid],
                )
                .map_err(db_err)?;
                let n = tx
                    .execute("DELETE FROM versions WHERE id=?1", params![oid])
                    .map_err(db_err)?;
                if n == 0 {
                    return Err("版本不存在".into());
                }
            }
            other => return Err(format!("unknown trash kind: {other}")),
        }
        tx.execute("DELETE FROM trash_items WHERE id=?1", params![rec.id])
            .map_err(db_err)?;
        tx.commit().map_err(db_err)?;
        Ok(rec)
    }

    // ---------- undo snapshots & restores (client Ctrl+Z) ----------
    //
    // The client's session undo stack holds pre-operation
    // snapshots taken through these faces; the restores here are
    // the ONLY writers that consume them. Every restore is one
    // transaction and checks its guards before writing: a
    // snapshot whose world has moved on (the target draft edited
    // again, an object purged from the trash, a folder id
    // re-created) is refused with a user-readable reason and
    // writes nothing — an undo must never half-land.

    const VERSION_SNAP_COLS: &'static str = "id, rendition_id, semver, major, minor, patch, basis_revision, author, note, created_ms, fragment_hash, page_count, pages_json, text_all, status, scripts_json";

    fn row_to_version_snap(r: &rusqlite::Row) -> rusqlite::Result<VersionRowSnap> {
        Ok(VersionRowSnap {
            id: r.get(0)?,
            rendition_id: r.get(1)?,
            semver: r.get(2)?,
            major: r.get(3)?,
            minor: r.get(4)?,
            patch: r.get(5)?,
            basis_revision: r.get(6)?,
            author: r.get(7)?,
            note: r.get(8)?,
            created_ms: r.get(9)?,
            fragment_hash: r.get(10)?,
            page_count: r.get(11)?,
            pages_json: r.get(12)?,
            text_all: r.get(13)?,
            status: r.get(14)?,
            scripts_json: r.get(15)?,
        })
    }

    fn version_snap_on(conn: &Connection, id: &str) -> SResult<Option<VersionRowSnap>> {
        conn.query_row(
            &format!(
                "SELECT {} FROM versions WHERE id=?1",
                Self::VERSION_SNAP_COLS
            ),
            params![id],
            Self::row_to_version_snap,
        )
        .optional()
        .map_err(db_err)
    }

    /// The content fields two snapshots of one version row are
    /// compared on: identity / provenance columns (author,
    /// created_ms, semver) cannot change in place, so equality
    /// of the mutable payload is what "unchanged since the
    /// snapshot" means.
    fn version_snap_same(a: &VersionRowSnap, b: &VersionRowSnap) -> bool {
        a.fragment_hash == b.fragment_hash
            && a.page_count == b.page_count
            && a.pages_json == b.pages_json
            && a.text_all == b.text_all
            && a.scripts_json == b.scripts_json
            && a.status == b.status
    }

    fn object_exists_on(conn: &Connection, kind: &str, id: &str) -> SResult<bool> {
        let table = match kind {
            "unit" => "units",
            "version" => "versions",
            "index" => "indexes_tbl",
            "showroom" => "showroom",
            other => return Err(format!("unknown trash kind: {other}")),
        };
        conn.query_row(
            &format!("SELECT EXISTS(SELECT 1 FROM {table} WHERE id=?1)"),
            params![id],
            |r| r.get(0),
        )
        .map_err(db_err)
    }

    /// Every unit's wall position (see UnitPosition).
    pub fn snapshot_unit_positions(&self) -> SResult<Vec<UnitPosition>> {
        let mut stmt = self
            .conn
            .prepare("SELECT id, folder_id, sort FROM units")
            .map_err(db_err)?;
        let rows = stmt
            .query_map([], |r| {
                Ok(UnitPosition {
                    id: r.get(0)?,
                    folder_id: r.get(1)?,
                    sort: r.get(2)?,
                })
            })
            .map_err(db_err)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(db_err)?;
        Ok(rows)
    }

    /// Write a unit-position snapshot back: each listed unit
    /// regains its recorded home and seat. Units created since
    /// the snapshot are not in it and keep their seats (their
    /// relative order against the restored set can shift — the
    /// snapshot predates them); units deleted since are skipped
    /// by the UPDATE itself.
    pub fn restore_unit_positions(&self, rows: &[UnitPosition]) -> SResult<()> {
        let tx = self.conn.unchecked_transaction().map_err(db_err)?;
        for row in rows {
            tx.execute(
                "UPDATE units SET folder_id=?2, sort=?3 WHERE id=?1",
                params![row.id, row.folder_id, row.sort],
            )
            .map_err(db_err)?;
        }
        tx.commit().map_err(db_err)?;
        Ok(())
    }

    /// One version row, verbatim (see VersionRowSnap).
    pub fn snapshot_version_row(&self, version_id: &str) -> SResult<VersionRowSnap> {
        Self::version_snap_on(&self.conn, version_id)?
            .ok_or_else(|| "版本不存在".to_string())
    }

    /// The version `merge_unit_page_into` would extend for this
    /// unit — the lead rendition's latest version — resolved by
    /// the merge's own rule, so a pre-merge snapshot names
    /// exactly the row the merge is about to touch.
    pub fn snapshot_working_version(
        &self,
        unit_id: &str,
    ) -> SResult<Option<VersionRowSnap>> {
        let rends = self.list_renditions(unit_id)?;
        let Some(rend) = rends.first() else {
            return Ok(None);
        };
        let Some(ver) = self.latest_version(&rend.id)? else {
            return Ok(None);
        };
        Ok(Some(self.snapshot_version_row(&ver.id)?))
    }

    /// One version's page-id sequence, in order (redo
    /// support): the capture a draft-reorder undo takes of the
    /// CURRENT sequence before re-running the reorder with the
    /// old one, so the redo entry can name the post-drag order
    /// it must restore. Read through `get_version`, the same
    /// parse the reorder itself validates against.
    pub fn snapshot_version_page_ids(&self, version_id: &str) -> SResult<Vec<String>> {
        let ver = self.get_version(version_id)?;
        Ok(ver.pages.iter().map(|p| p.id.clone()).collect())
    }

    /// The full pre-image of one unit (see UnitSnapshot).
    /// Versions are read raw per rendition — NOT through
    /// list_versions, whose trash-overlay filter would drop a
    /// trashed version the dissolve still removes (and the
    /// resurrection must therefore bring back).
    pub fn snapshot_unit(&self, unit_id: &str) -> SResult<UnitSnapshot> {
        let unit = self.get_unit(unit_id)?;
        let renditions = self.list_renditions(unit_id)?;
        let mut versions: Vec<VersionRowSnap> = Vec::new();
        for rend in &renditions {
            let mut stmt = self
                .conn
                .prepare(&format!(
                    "SELECT {} FROM versions WHERE rendition_id=?1 ORDER BY created_ms, rowid",
                    Self::VERSION_SNAP_COLS
                ))
                .map_err(db_err)?;
            let rows = stmt
                .query_map(params![rend.id], Self::row_to_version_snap)
                .map_err(db_err)?
                .collect::<Result<Vec<_>, _>>()
                .map_err(db_err)?;
            versions.extend(rows);
        }
        let mut version_tags: Vec<VerTagRec> = Vec::new();
        for ver in &versions {
            version_tags.extend(self.list_version_tags(&ver.id)?);
        }
        let bindings = self.list_page_bindings(unit_id)?;
        let attachments = self
            .list_attachments(unit_id)?
            .into_iter()
            .filter(|a| a.parent_type == "unit")
            .collect();
        let mut stmt = self
            .conn
            .prepare(
                "SELECT id, kind, object_id, title, origin, deleted_by, deleted_ms FROM trash_items WHERE (kind='unit' AND object_id=?1) OR (kind='version' AND object_id IN (SELECT v.id FROM versions v JOIN renditions r ON r.id = v.rendition_id WHERE r.unit_id=?1))",
            )
            .map_err(db_err)?;
        let trash_rows = stmt
            .query_map(params![unit_id], |r| {
                Ok(TrashRec {
                    id: r.get(0)?,
                    kind: r.get(1)?,
                    object_id: r.get(2)?,
                    title: r.get(3)?,
                    origin: r.get(4)?,
                    deleted_by: r.get(5)?,
                    deleted_ms: r.get(6)?,
                })
            })
            .map_err(db_err)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(db_err)?;
        Ok(UnitSnapshot {
            unit,
            renditions,
            versions,
            version_tags,
            bindings,
            attachments,
            trash_rows,
        })
    }

    /// Insert a unit snapshot's rows (the resurrection half of
    /// `undo_merge`), on a caller-provided handle inside its
    /// transaction. Callers verify absence first; the inserts
    /// are plain, so a colliding id fails the whole undo rather
    /// than overwriting a live row.
    fn insert_unit_snapshot(tx: &Connection, snap: &UnitSnapshot) -> SResult<()> {
        let u = &snap.unit;
        tx.execute(
            "INSERT INTO units(id, name, folder_id, group_id, maintainer, lead_lang, description, current_revision, created_ms, sort) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)",
            params![
                u.id,
                u.name,
                u.folder_id,
                u.group_id,
                u.maintainer,
                u.lead_lang,
                u.description,
                u.current_revision,
                u.created_ms,
                u.sort
            ],
        )
        .map_err(db_err)?;
        for r in &snap.renditions {
            tx.execute(
                "INSERT INTO renditions(id, unit_id, form, lang) VALUES(?1,?2,?3,?4)",
                params![r.id, r.unit_id, r.form, r.lang],
            )
            .map_err(db_err)?;
        }
        for v in &snap.versions {
            tx.execute(
                "INSERT INTO versions(id, rendition_id, semver, major, minor, patch, basis_revision, author, note, created_ms, fragment_hash, page_count, pages_json, text_all, status, scripts_json) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16)",
                params![
                    v.id,
                    v.rendition_id,
                    v.semver,
                    v.major,
                    v.minor,
                    v.patch,
                    v.basis_revision,
                    v.author,
                    v.note,
                    v.created_ms,
                    v.fragment_hash,
                    v.page_count,
                    v.pages_json,
                    v.text_all,
                    v.status,
                    v.scripts_json
                ],
            )
            .map_err(db_err)?;
        }
        for t in &snap.version_tags {
            tx.execute(
                "INSERT INTO version_tags(id, version_id, dim, value, strength, author) VALUES(?1,?2,?3,?4,?5,?6)",
                params![t.id, t.version_id, t.dim, t.value, t.strength, t.author],
            )
            .map_err(db_err)?;
        }
        for b in &snap.bindings {
            tx.execute(
                "INSERT INTO page_bindings(id, unit_id, from_version_id, from_slot, to_version_id, to_slot, kind, author, created_ms, from_page_id, to_page_id) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11)",
                params![
                    b.id,
                    b.unit_id,
                    b.from_version_id,
                    b.from_slot,
                    b.to_version_id,
                    b.to_slot,
                    b.kind,
                    b.author,
                    b.created_ms,
                    b.from_page_id,
                    b.to_page_id
                ],
            )
            .map_err(db_err)?;
        }
        for a in &snap.attachments {
            tx.execute(
                "INSERT INTO attachments(id, parent_type, parent_id, filename, hash, size, uploader, created_ms) VALUES(?1,?2,?3,?4,?5,?6,?7,?8)",
                params![
                    a.id,
                    a.parent_type,
                    a.parent_id,
                    a.filename,
                    a.hash,
                    a.size,
                    a.uploader,
                    a.created_ms
                ],
            )
            .map_err(db_err)?;
        }
        for t in &snap.trash_rows {
            // OR IGNORE mirrors trash_add's idempotence: a row
            // for the same object minted after the dissolve (the
            // object cannot have been re-trashed — it did not
            // exist — but a same-(kind, object) row is the one
            // collision that must never fail a resurrection).
            tx.execute(
                "INSERT OR IGNORE INTO trash_items(id, kind, object_id, title, origin, deleted_by, deleted_ms) VALUES(?1,?2,?3,?4,?5,?6,?7)",
                params![
                    t.id, t.kind, t.object_id, t.title, t.origin, t.deleted_by, t.deleted_ms
                ],
            )
            .map_err(db_err)?;
        }
        Ok(())
    }

    /// Undo a `merge_unit_page_into`: restore the target draft
    /// to its pre-merge row and resurrect the dissolved source
    /// unit from its snapshot. `pre` is the target's working
    /// version before the merge (the merge's own resolution,
    /// see snapshot_working_version); `post` is the draft the
    /// merge returned. When pre and post are the same row the
    /// merge extended a draft in place and the undo writes the
    /// pre-image back; when they differ the merge born a new
    /// draft and the undo deletes that row (the published
    /// working version was never touched). Guards, all before
    /// any write: the post row must still equal its snapshot
    /// (a draft edited after the merge refuses — restoring
    /// would silently discard that edit); a born draft must not
    /// have gained bindings or tags; an in-place draft must not
    /// have gained a binding onto the appended page; the source
    /// must not have been re-created meanwhile.
    pub fn undo_merge(
        &self,
        source: &UnitSnapshot,
        pre: &VersionRowSnap,
        post: &VersionRowSnap,
    ) -> SResult<()> {
        let tx = self.conn.unchecked_transaction().map_err(db_err)?;
        let cur_post = Self::version_snap_on(&tx, &post.id)?.ok_or(
            "并入生成的草稿已不存在，无法撤回并入".to_string(),
        )?;
        if !Self::version_snap_same(&cur_post, post) {
            return Err(
                "目标草稿在并入后又被修改，无法撤回并入；请先处理草稿的后续修改".into(),
            );
        }
        if pre.id == post.id {
            // In-place: bindings created after the merge that
            // address the appended page (slots at or beyond the
            // pre-merge page count) would dangle over a page the
            // restore removes.
            let dangling: i64 = tx
                .query_row(
                    "SELECT COUNT(*) FROM page_bindings WHERE (from_version_id=?1 AND from_slot>=?2) OR (to_version_id=?1 AND to_slot>=?2)",
                    params![post.id, pre.page_count],
                    |r| r.get(0),
                )
                .map_err(db_err)?;
            if dangling > 0 {
                return Err(
                    "草稿在并入后新增了指向被并入页的页面绑定，无法撤回并入".into(),
                );
            }
            let n = tx
                .execute(
                    "UPDATE versions SET fragment_hash=?2, page_count=?3, pages_json=?4, text_all=?5, scripts_json=?6 WHERE id=?1 AND status='draft'",
                    params![
                        pre.id,
                        pre.fragment_hash,
                        pre.page_count,
                        pre.pages_json,
                        pre.text_all,
                        pre.scripts_json
                    ],
                )
                .map_err(db_err)?;
            if n == 0 {
                return Err("目标草稿状态已变化，无法撤回并入".into());
            }
        } else {
            let cur_pre = Self::version_snap_on(&tx, &pre.id)?
                .ok_or("目标内容块的工作版本已不存在，无法撤回并入".to_string())?;
            if !Self::version_snap_same(&cur_pre, pre) {
                return Err("目标内容块的工作版本在并入后已变化，无法撤回并入".into());
            }
            let refs: i64 = tx
                .query_row(
                    "SELECT (SELECT COUNT(*) FROM page_bindings WHERE from_version_id=?1 OR to_version_id=?1) + (SELECT COUNT(*) FROM version_tags WHERE version_id=?1)",
                    params![post.id],
                    |r| r.get(0),
                )
                .map_err(db_err)?;
            if refs > 0 {
                return Err(
                    "并入生成的草稿已被引用（页面绑定或标签），无法撤回并入".into(),
                );
            }
            tx.execute("DELETE FROM versions WHERE id=?1", params![post.id])
                .map_err(db_err)?;
        }
        // Resurrect the source: every id it owned must still be
        // free — a collision means the world rebuilt something
        // under the same identity, and overwriting it would be
        // worse than refusing.
        if Self::object_exists_on(&tx, "unit", &source.unit.id)? {
            return Err("源内容块已重新存在，无法撤回并入".into());
        }
        for v in &source.versions {
            if Self::object_exists_on(&tx, "version", &v.id)? {
                return Err("源内容块的版本已重新存在，无法撤回并入".into());
            }
        }
        Self::insert_unit_snapshot(&tx, source)?;
        tx.commit().map_err(db_err)?;
        Ok(())
    }

    /// Whether the unit `snap` was taken of still matches the
    /// snapshot on every field a dissolve would destroy (redo
    /// support): the unit row's columns, the rendition set, each
    /// version's mutable payload (the `version_snap_same`
    /// basis), and the tag / binding / attachment / trash-row
    /// identity sets. A hand edit that only rewrites a tag's
    /// value in place (same id, same count) is below this
    /// comparison's resolution — declared; every edit path the
    /// client exposes replaces rows or changes a version
    /// payload, both of which register.
    pub fn unit_snapshot_intact(&self, snap: &UnitSnapshot) -> SResult<bool> {
        let cur = match self.snapshot_unit(&snap.unit.id) {
            Ok(c) => c,
            Err(_) => return Ok(false),
        };
        let a = &snap.unit;
        let b = &cur.unit;
        if a.name != b.name
            || a.folder_id != b.folder_id
            || a.group_id != b.group_id
            || a.maintainer != b.maintainer
            || a.lead_lang != b.lead_lang
            || a.description != b.description
            || a.current_revision != b.current_revision
            || a.created_ms != b.created_ms
            || a.sort != b.sort
        {
            return Ok(false);
        }
        if cur.renditions.len() != snap.renditions.len()
            || snap.renditions.iter().any(|r| {
                !cur.renditions
                    .iter()
                    .any(|c| c.id == r.id && c.form == r.form && c.lang == r.lang)
            })
        {
            return Ok(false);
        }
        if cur.versions.len() != snap.versions.len()
            || snap.versions.iter().any(|v| {
                !cur.versions
                    .iter()
                    .any(|c| c.id == v.id && Self::version_snap_same(c, v))
            })
        {
            return Ok(false);
        }
        let ids_same = |a: &[String], b: &[String]| {
            a.len() == b.len() && a.iter().all(|x| b.contains(x))
        };
        if !ids_same(
            &snap.version_tags.iter().map(|t| t.id.clone()).collect::<Vec<_>>(),
            &cur.version_tags.iter().map(|t| t.id.clone()).collect::<Vec<_>>(),
        ) || !ids_same(
            &snap.bindings.iter().map(|t| t.id.clone()).collect::<Vec<_>>(),
            &cur.bindings.iter().map(|t| t.id.clone()).collect::<Vec<_>>(),
        ) || !ids_same(
            &snap.attachments.iter().map(|t| t.id.clone()).collect::<Vec<_>>(),
            &cur.attachments.iter().map(|t| t.id.clone()).collect::<Vec<_>>(),
        ) || !ids_same(
            &snap.trash_rows.iter().map(|t| t.id.clone()).collect::<Vec<_>>(),
            &cur.trash_rows.iter().map(|t| t.id.clone()).collect::<Vec<_>>(),
        ) {
            return Ok(false);
        }
        Ok(true)
    }

    /// Redo a `merge_unit_page_into` that `undo_merge` reversed
    /// (Ctrl+Y): write the merge's product back — the target
    /// draft row returns to its `post` image (in place when the
    /// merge extended a draft, re-inserted verbatim when the
    /// merge born it) — and dissolve the source unit again.
    /// Guards, all before any write: the target must still sit
    /// at its `pre` image (a draft edited after the undo
    /// refuses — re-applying would silently discard that edit);
    /// a born draft's id must still be free; the resurrected
    /// source must still match its snapshot (an edit to it
    /// after the undo refuses — dissolving would destroy it);
    /// and the publish-pin guard the forward merge carries is
    /// re-checked, since a version could have been pinned while
    /// the source lived again.
    pub fn redo_merge(
        &self,
        source: &UnitSnapshot,
        pre: &VersionRowSnap,
        post: &VersionRowSnap,
    ) -> SResult<()> {
        if pre.id == post.id {
            let cur = Self::version_snap_on(&self.conn, &pre.id)?
                .ok_or("目标草稿已不存在，无法重做并入".to_string())?;
            if !Self::version_snap_same(&cur, pre) {
                return Err(
                    "目标草稿在撤回并入后又被修改，无法重做并入；请先处理草稿的后续修改"
                        .into(),
                );
            }
        } else {
            let cur_pre = Self::version_snap_on(&self.conn, &pre.id)?
                .ok_or("目标内容块的工作版本已不存在，无法重做并入".to_string())?;
            if !Self::version_snap_same(&cur_pre, pre) {
                return Err(
                    "目标内容块的工作版本在撤回并入后已变化，无法重做并入".into(),
                );
            }
            if Self::version_snap_on(&self.conn, &post.id)?.is_some() {
                return Err("并入生成的草稿已重新存在，无法重做并入".into());
            }
        }
        if !Self::object_exists_on(&self.conn, "unit", &source.unit.id)? {
            return Err("源内容块已不存在，无法重做并入".into());
        }
        if !self.unit_snapshot_intact(source)? {
            return Err(
                "源内容块在撤回并入后已被修改，无法重做并入；重做会丢失这些修改".into(),
            );
        }
        let pinned_live: i64 = self
            .conn
            .query_row(
                "SELECT COUNT(*) FROM showroom_pins p JOIN showroom s ON s.id = p.showroom_id JOIN versions v ON v.id = p.version_id JOIN renditions r ON r.id = v.rendition_id WHERE r.unit_id = ?1 AND s.state = 'live'",
                params![source.unit.id],
                |r| r.get(0),
            )
            .map_err(db_err)?;
        if pinned_live > 0 {
            return Err(
                "该内容块的版本正被演示厅条目钉为正式版，不能重做并入；请先更新该条目的演示推送或将其下架"
                    .into(),
            );
        }
        let tx = self.conn.unchecked_transaction().map_err(db_err)?;
        if pre.id == post.id {
            let n = tx
                .execute(
                    "UPDATE versions SET fragment_hash=?2, page_count=?3, pages_json=?4, text_all=?5, scripts_json=?6 WHERE id=?1 AND status='draft'",
                    params![
                        post.id,
                        post.fragment_hash,
                        post.page_count,
                        post.pages_json,
                        post.text_all,
                        post.scripts_json
                    ],
                )
                .map_err(db_err)?;
            if n == 0 {
                return Err("目标草稿状态已变化，无法重做并入".into());
            }
        } else {
            tx.execute(
                "INSERT INTO versions(id, rendition_id, semver, major, minor, patch, basis_revision, author, note, created_ms, fragment_hash, page_count, pages_json, text_all, status, scripts_json) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16)",
                params![
                    post.id,
                    post.rendition_id,
                    post.semver,
                    post.major,
                    post.minor,
                    post.patch,
                    post.basis_revision,
                    post.author,
                    post.note,
                    post.created_ms,
                    post.fragment_hash,
                    post.page_count,
                    post.pages_json,
                    post.text_all,
                    post.status,
                    post.scripts_json
                ],
            )
            .map_err(db_err)?;
        }
        Self::delete_unit_rows(&tx, &source.unit.id)?;
        tx.commit().map_err(db_err)?;
        Ok(())
    }

    /// Undo a batch of trash operations: drop the overlay rows
    /// the operations created, so the objects reappear exactly
    /// as they were. A row already gone with its object visible
    /// again was restored by hand in the meantime and counts as
    /// done; a row gone with its object ALSO gone means the
    /// object was purged — the whole batch refuses and nothing
    /// is restored (a partial undelete would misreport what the
    /// undo achieved). A row replaced by a NEWER trash row for
    /// the same object belongs to a later delete, not this one,
    /// and refuses likewise.
    pub fn undo_trash_restore(&self, rows: &[TrashRec]) -> SResult<()> {
        let tx = self.conn.unchecked_transaction().map_err(db_err)?;
        for row in rows {
            let by_id: i64 = tx
                .query_row(
                    "SELECT COUNT(*) FROM trash_items WHERE id=?1",
                    params![row.id],
                    |r| r.get(0),
                )
                .map_err(db_err)?;
            if by_id > 0 {
                tx.execute("DELETE FROM trash_items WHERE id=?1", params![row.id])
                    .map_err(db_err)?;
                continue;
            }
            if !Self::object_exists_on(&tx, &row.kind, &row.object_id)? {
                return Err(format!(
                    "「{}」已被彻底删除，无法撤回",
                    if row.title.is_empty() {
                        row.object_id.clone()
                    } else {
                        row.title.clone()
                    }
                ));
            }
            let replaced: i64 = tx
                .query_row(
                    "SELECT COUNT(*) FROM trash_items WHERE kind=?1 AND object_id=?2",
                    params![row.kind, row.object_id],
                    |r| r.get(0),
                )
                .map_err(db_err)?;
            if replaced > 0 {
                return Err(format!(
                    "「{}」已被再次移入回收站，无法撤回此前的删除",
                    if row.title.is_empty() {
                        row.object_id.clone()
                    } else {
                        row.title.clone()
                    }
                ));
            }
            // Object visible, no row: restored by hand already.
        }
        tx.commit().map_err(db_err)?;
        Ok(())
    }

    /// Redo a batch of trash operations `undo_trash_restore`
    /// reversed (Ctrl+Y): re-apply the overlay rows for the
    /// recorded (kind, object) pairs — fresh row ids and a
    /// fresh deletion time, title / origin / deleted_by carried
    /// from the rows the undo dropped — and return the new rows
    /// (the redo's own counter-entry on the undo stack names
    /// exactly them). Guards, all before any write, mirroring
    /// `trash_add` and the undo's whole-batch discipline: every
    /// object must still exist (a purged object refuses the
    /// whole batch rather than re-trashing a subset), no object
    /// may already sit behind the overlay (a later delete owns
    /// its state now), and a version a live showroom item pins
    /// refuses with `trash_add`'s own wording.
    pub fn redo_trash_apply(&self, rows: &[TrashRec]) -> SResult<Vec<TrashRec>> {
        let tx = self.conn.unchecked_transaction().map_err(db_err)?;
        for row in rows {
            let title = if row.title.is_empty() {
                row.object_id.clone()
            } else {
                row.title.clone()
            };
            if !Self::object_exists_on(&tx, &row.kind, &row.object_id)? {
                return Err(format!("「{title}」已被彻底删除，无法重做删除"));
            }
            let existing: i64 = tx
                .query_row(
                    "SELECT COUNT(*) FROM trash_items WHERE kind=?1 AND object_id=?2",
                    params![row.kind, row.object_id],
                    |r| r.get(0),
                )
                .map_err(db_err)?;
            if existing > 0 {
                return Err(format!(
                    "「{title}」已被再次移入回收站，无法重做此前的删除"
                ));
            }
            if row.kind == "version" {
                let pinned_live: i64 = tx
                    .query_row(
                        "SELECT COUNT(*) FROM showroom_pins p JOIN showroom s ON s.id = p.showroom_id WHERE p.version_id = ?1 AND s.state = 'live'",
                        params![row.object_id],
                        |r| r.get(0),
                    )
                    .map_err(db_err)?;
                if pinned_live > 0 {
                    return Err(
                        "该版本正被演示厅条目钉为正式版，不能删除；请先更新该条目的演示推送或将其下架"
                            .into(),
                    );
                }
            }
        }
        let mut fresh: Vec<TrashRec> = Vec::with_capacity(rows.len());
        for row in rows {
            let rec = TrashRec {
                id: new_id("trs"),
                kind: row.kind.clone(),
                object_id: row.object_id.clone(),
                title: row.title.clone(),
                origin: row.origin.clone(),
                deleted_by: row.deleted_by.clone(),
                deleted_ms: now_ms(),
            };
            tx.execute(
                "INSERT INTO trash_items(id, kind, object_id, title, origin, deleted_by, deleted_ms) VALUES(?1,?2,?3,?4,?5,?6,?7)",
                params![
                    rec.id,
                    rec.kind,
                    rec.object_id,
                    rec.title,
                    rec.origin,
                    rec.deleted_by,
                    rec.deleted_ms
                ],
            )
            .map_err(db_err)?;
            fresh.push(rec);
        }
        tx.commit().map_err(db_err)?;
        Ok(fresh)
    }

    /// Every folder's tree position within one sidebar scope
    /// (see FolderPosition).
    pub fn snapshot_folder_positions(&self, scope: &str) -> SResult<Vec<FolderPosition>> {
        Ok(self
            .list_folders_in(scope)?
            .into_iter()
            .map(|f| FolderPosition {
                id: f.id,
                parent_id: f.parent_id,
                sort: f.sort,
            })
            .collect())
    }

    /// Write a folder-position snapshot back (the inverse of a
    /// folder move): each listed folder regains its recorded
    /// parent and seat. Folders created since keep their rows;
    /// folders deleted since are skipped by the UPDATE itself.
    /// The snapshot is one consistent forest state, so writing
    /// a subset of it back cannot mint a cycle.
    pub fn restore_folder_positions(&self, rows: &[FolderPosition]) -> SResult<()> {
        let tx = self.conn.unchecked_transaction().map_err(db_err)?;
        for row in rows {
            tx.execute(
                "UPDATE folders SET parent_id=?2, sort=?3 WHERE id=?1",
                params![row.id, row.parent_id, row.sort],
            )
            .map_err(db_err)?;
        }
        tx.commit().map_err(db_err)?;
        Ok(())
    }

    /// Whether a folder still holds anything (child folders,
    /// units or indexes homed in it) — the create-undo guard:
    /// un-creating a folder that has since gained content would
    /// have to move that content somewhere, which is a different
    /// operation than the undo of its creation.
    pub fn folder_has_contents(&self, id: &str) -> SResult<bool> {
        let n: i64 = self
            .conn
            .query_row(
                "SELECT (SELECT COUNT(*) FROM folders WHERE parent_id=?1) + (SELECT COUNT(*) FROM units WHERE folder_id=?1) + (SELECT COUNT(*) FROM indexes_tbl WHERE folder_id=?1)",
                params![id],
                |r| r.get(0),
            )
            .map_err(db_err)?;
        Ok(n > 0)
    }

    /// One folder row, verbatim (redo support): the capture a
    /// folder-creation undo takes before the create is reversed,
    /// so the redo can re-insert the folder with its recorded
    /// identity and fields instead of minting a look-alike.
    /// None when the folder is already gone.
    pub fn snapshot_folder_row(&self, id: &str) -> SResult<Option<FolderRec>> {
        Ok(self.list_folders()?.into_iter().find(|f| f.id == id))
    }

    /// Re-insert one folder row (the redo of a folder creation
    /// the undo deleted): the recorded id must still be free
    /// and the recorded parent must still exist — a collision
    /// or a dangling parent refuses rather than resurrecting
    /// the folder into a different tree than the one it was
    /// created in.
    pub fn restore_folder_row(&self, f: &FolderRec) -> SResult<()> {
        let exists: i64 = self
            .conn
            .query_row(
                "SELECT COUNT(*) FROM folders WHERE id=?1",
                params![f.id],
                |r| r.get(0),
            )
            .map_err(db_err)?;
        if exists > 0 {
            return Err("该文件夹已被重新创建，无法重做新建".into());
        }
        if let Some(p) = &f.parent_id {
            let parent_exists: i64 = self
                .conn
                .query_row(
                    "SELECT COUNT(*) FROM folders WHERE id=?1",
                    params![p],
                    |r| r.get(0),
                )
                .map_err(db_err)?;
            if parent_exists == 0 {
                return Err("父文件夹已不存在，无法重做新建".into());
            }
        }
        self.conn
            .execute(
                "INSERT INTO folders(id, name, parent_id, sort, main_index_id, scope, color, note, origin) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9)",
                params![
                    f.id,
                    f.name,
                    f.parent_id,
                    f.sort,
                    f.main_index_id,
                    f.scope,
                    f.color,
                    f.note,
                    f.origin
                ],
            )
            .map_err(db_err)?;
        Ok(())
    }

    /// The pre-image of a folder delete (see
    /// FolderDeleteSnapshot), gathered with the same walks
    /// `delete_folder` performs — read-only, so taking the
    /// snapshot never disturbs the delete that follows it.
    pub fn snapshot_folder_delete(
        &self,
        id: &str,
        mode: FolderDeleteMode,
    ) -> SResult<FolderDeleteSnapshot> {
        let all = self.list_folders()?;
        let Some(root) = all.iter().find(|f| f.id == id) else {
            return Err("文件夹不存在".into());
        };
        let root = root.clone();
        match mode {
            FolderDeleteMode::Lift => {
                let lifted_children = all
                    .iter()
                    .filter(|f| f.parent_id.as_deref() == Some(id))
                    .map(|f| f.id.clone())
                    .collect();
                let mut stmt = self
                    .conn
                    .prepare("SELECT id FROM units WHERE folder_id=?1")
                    .map_err(db_err)?;
                let lifted_units = stmt
                    .query_map(params![id], |r| r.get::<_, String>(0))
                    .map_err(db_err)?
                    .collect::<Result<Vec<_>, _>>()
                    .map_err(db_err)?;
                drop(stmt);
                let mut stmt = self
                    .conn
                    .prepare("SELECT id FROM indexes_tbl WHERE folder_id=?1")
                    .map_err(db_err)?;
                let lifted_indexes = stmt
                    .query_map(params![id], |r| r.get::<_, String>(0))
                    .map_err(db_err)?
                    .collect::<Result<Vec<_>, _>>()
                    .map_err(db_err)?;
                Ok(FolderDeleteSnapshot {
                    trash_mode: false,
                    root,
                    subtree: Vec::new(),
                    lifted_children,
                    lifted_units,
                    lifted_indexes,
                    contents: Vec::new(),
                })
            }
            FolderDeleteMode::Trash => {
                // The subtree walk, delete_folder's own: this
                // folder plus every descendant, breadth by
                // repeated passes over the folder list.
                let mut subtree_ids: Vec<String> = vec![id.to_string()];
                let mut grew = true;
                while grew {
                    grew = false;
                    for f in &all {
                        if let Some(p) = f.parent_id.as_deref() {
                            if subtree_ids.iter().any(|x| x == p)
                                && !subtree_ids.iter().any(|x| x == &f.id)
                            {
                                subtree_ids.push(f.id.clone());
                                grew = true;
                            }
                        }
                    }
                }
                let subtree: Vec<FolderRec> = subtree_ids
                    .iter()
                    .skip(1)
                    .filter_map(|fid| all.iter().find(|f| &f.id == fid).cloned())
                    .collect();
                let mut contents: Vec<FolderTrashContent> = Vec::new();
                let mut index_ids: Vec<String> = Vec::new();
                for fid in &subtree_ids {
                    let mut stmt = self
                        .conn
                        .prepare("SELECT id, name FROM units WHERE folder_id=?1")
                        .map_err(db_err)?;
                    let units = stmt
                        .query_map(params![fid], |r| {
                            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
                        })
                        .map_err(db_err)?
                        .collect::<Result<Vec<_>, _>>()
                        .map_err(db_err)?;
                    drop(stmt);
                    for (uid, name) in units {
                        let had = self.trash_find("unit", &uid)?.is_some();
                        contents.push(FolderTrashContent {
                            kind: "unit".into(),
                            object_id: uid,
                            title: name,
                            had_trash_row: had,
                        });
                    }
                    let mut stmt = self
                        .conn
                        .prepare("SELECT id, name FROM indexes_tbl WHERE folder_id=?1")
                        .map_err(db_err)?;
                    let indexes = stmt
                        .query_map(params![fid], |r| {
                            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
                        })
                        .map_err(db_err)?
                        .collect::<Result<Vec<_>, _>>()
                        .map_err(db_err)?;
                    drop(stmt);
                    for (xid, name) in indexes {
                        let had = self.trash_find("index", &xid)?.is_some();
                        contents.push(FolderTrashContent {
                            kind: "index".into(),
                            object_id: xid.clone(),
                            title: name,
                            had_trash_row: had,
                        });
                        index_ids.push(xid);
                    }
                }
                for xid in &index_ids {
                    let mut stmt = self
                        .conn
                        .prepare("SELECT id, title FROM showroom WHERE source_index_id=?1")
                        .map_err(db_err)?;
                    let items = stmt
                        .query_map(params![xid], |r| {
                            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
                        })
                        .map_err(db_err)?
                        .collect::<Result<Vec<_>, _>>()
                        .map_err(db_err)?;
                    drop(stmt);
                    for (sid, title) in items {
                        let had = self.trash_find("showroom", &sid)?.is_some();
                        contents.push(FolderTrashContent {
                            kind: "showroom".into(),
                            object_id: sid,
                            title,
                            had_trash_row: had,
                        });
                    }
                }
                Ok(FolderDeleteSnapshot {
                    trash_mode: true,
                    root,
                    subtree,
                    lifted_children: Vec::new(),
                    lifted_units: Vec::new(),
                    lifted_indexes: Vec::new(),
                    contents,
                })
            }
        }
    }

    /// Undo a folder delete from its pre-image: re-insert the
    /// folder rows, then reverse the mode's content effect —
    /// trash mode drops the trash rows the delete created (rows
    /// an object already had stay: they are an earlier delete's,
    /// not this one's), lift mode homes the lifted children /
    /// units / indexes back, but only those still sitting at
    /// the recorded lift target (one re-homed since belongs to
    /// its newer home, and the undo leaves it there). Guards
    /// before any write: no folder row with a recorded id may
    /// exist again, and in trash mode every content object must
    /// still exist — a purged object refuses the whole restore
    /// rather than resurrecting a folder around a hole.
    pub fn restore_folder_delete(&self, snap: &FolderDeleteSnapshot) -> SResult<()> {
        let tx = self.conn.unchecked_transaction().map_err(db_err)?;
        for f in std::iter::once(&snap.root).chain(snap.subtree.iter()) {
            let exists: i64 = tx
                .query_row(
                    "SELECT COUNT(*) FROM folders WHERE id=?1",
                    params![f.id],
                    |r| r.get(0),
                )
                .map_err(db_err)?;
            if exists > 0 {
                return Err("该文件夹已被重新创建，无法撤回删除".into());
            }
        }
        if snap.trash_mode {
            for c in &snap.contents {
                if !Self::object_exists_on(&tx, &c.kind, &c.object_id)? {
                    return Err(format!(
                        "「{}」已被彻底删除，无法撤回删除文件夹",
                        if c.title.is_empty() {
                            c.object_id.clone()
                        } else {
                            c.title.clone()
                        }
                    ));
                }
            }
        }
        for f in std::iter::once(&snap.root).chain(snap.subtree.iter()) {
            tx.execute(
                "INSERT INTO folders(id, name, parent_id, sort, main_index_id, scope, color, note, origin) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9)",
                params![
                    f.id,
                    f.name,
                    f.parent_id,
                    f.sort,
                    f.main_index_id,
                    f.scope,
                    f.color,
                    f.note,
                    f.origin
                ],
            )
            .map_err(db_err)?;
        }
        if snap.trash_mode {
            for c in &snap.contents {
                if c.had_trash_row {
                    continue;
                }
                // A row already gone was restored by hand in the
                // meantime; the DELETE simply finds nothing.
                tx.execute(
                    "DELETE FROM trash_items WHERE kind=?1 AND object_id=?2",
                    params![c.kind, c.object_id],
                )
                .map_err(db_err)?;
            }
        } else {
            let lift_to = snap.root.parent_id.clone();
            for child in &snap.lifted_children {
                tx.execute(
                    "UPDATE folders SET parent_id=?2 WHERE id=?1 AND parent_id IS ?3",
                    params![child, snap.root.id, lift_to],
                )
                .map_err(db_err)?;
            }
            for uid in &snap.lifted_units {
                tx.execute(
                    "UPDATE units SET folder_id=?2 WHERE id=?1 AND folder_id IS ?3",
                    params![uid, snap.root.id, lift_to],
                )
                .map_err(db_err)?;
            }
            for xid in &snap.lifted_indexes {
                tx.execute(
                    "UPDATE indexes_tbl SET folder_id=?2 WHERE id=?1 AND folder_id IS ?3",
                    params![xid, snap.root.id, lift_to],
                )
                .map_err(db_err)?;
            }
        }
        tx.commit().map_err(db_err)?;
        Ok(())
    }

    /// Redo a folder delete `restore_folder_delete` reversed
    /// (Ctrl+Y): re-run the delete the snapshot describes —
    /// same root, same mode — against the CURRENT tree, and
    /// return the fresh pre-image the delete actually consumed
    /// (the redo's counter-entry on the undo stack names that
    /// one, so an undo of the redo restores what the redo
    /// really took, including anything homed into the folder
    /// after the original undo). The snapshot is re-taken with
    /// `snapshot_folder_delete`'s own walk and the delete runs
    /// through `delete_folder` itself, so the redo can never
    /// diverge from the forward operation's semantics. The
    /// root must still exist; a root deleted again by hand in
    /// the meantime refuses.
    pub fn redo_folder_delete(
        &self,
        snap: &FolderDeleteSnapshot,
        deleted_by: &str,
    ) -> SResult<FolderDeleteSnapshot> {
        if self.snapshot_folder_row(&snap.root.id)?.is_none() {
            return Err("文件夹已不存在，无法重做删除".into());
        }
        let mode = if snap.trash_mode {
            FolderDeleteMode::Trash
        } else {
            FolderDeleteMode::Lift
        };
        let fresh = self.snapshot_folder_delete(&snap.root.id, mode)?;
        self.delete_folder(&snap.root.id, mode, deleted_by)?;
        Ok(fresh)
    }

    pub fn save_assembly(
        &self,
        user_id: &str,
        name: &str,
        entries: &[IndexEntry],
    ) -> SResult<String> {
        let id = new_id("asm");
        self.conn
            .execute(
                "INSERT INTO assemblies(id, user_id, name, entries_json, created_ms) VALUES(?1,?2,?3,?4,?5)",
                params![
                    id,
                    user_id,
                    name,
                    serde_json::to_string(entries).unwrap_or_default(),
                    now_ms()
                ],
            )
            .map_err(db_err)?;
        Ok(id)
    }

    pub fn list_assemblies(&self, user_id: &str) -> SResult<Vec<serde_json::Value>> {
        let mut stmt = self
            .conn
            .prepare("SELECT id, name, entries_json, created_ms FROM assemblies WHERE user_id=?1 ORDER BY created_ms DESC")
            .map_err(db_err)?;
        let rows = stmt
            .query_map(params![user_id], |r| {
                let entries_json: String = r.get(2)?;
                let count = serde_json::from_str::<Vec<IndexEntry>>(&entries_json)
                    .map(|v| v.len())
                    .unwrap_or(0);
                Ok(serde_json::json!({
                    "id": r.get::<_, String>(0)?,
                    "name": r.get::<_, String>(1)?,
                    "entry_count": count,
                    "created_ms": r.get::<_, i64>(3)?,
                }))
            })
            .map_err(db_err)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(db_err)?;
        Ok(rows)
    }

    /// Who owns an assembly (audit H3): the export door checks
    /// ownership before resolving a single entry, so one member's
    /// saved assembly can never be exported under another's
    /// session. None when the assembly does not exist.
    pub fn assembly_owner(&self, id: &str) -> SResult<Option<String>> {
        let owner = self
            .conn
            .query_row(
                "SELECT user_id FROM assemblies WHERE id=?1",
                params![id],
                |r| r.get::<_, String>(0),
            )
            .optional()
            .map_err(db_err)?;
        Ok(owner)
    }

    pub fn assembly_entries(&self, id: &str) -> SResult<Vec<IndexEntry>> {
        let json: String = self
            .conn
            .query_row(
                "SELECT entries_json FROM assemblies WHERE id=?1",
                params![id],
                |r| r.get(0),
            )
            .map_err(db_err)?;
        Ok(serde_json::from_str(&json).unwrap_or_default())
    }

    // ---------- profiles & settings ----------

    // ---------- custom groups / shares / library (§3.17, §7.7) ----------

    /// Is `dept_id` the department `ancestor_id` itself or one of its
    /// descendants? (Department shares reach sub-departments and people
    /// who join them later.)
    pub fn dept_within(&self, dept_id: &str, ancestor_id: &str) -> SResult<bool> {
        let mut cur = Some(dept_id.to_string());
        for _ in 0..64 {
            match cur {
                None => return Ok(false),
                Some(ref d) if d == ancestor_id => return Ok(true),
                Some(d) => {
                    cur = self
                        .conn
                        .query_row(
                            "SELECT parent_id FROM departments WHERE id=?1",
                            params![d],
                            |r| r.get::<_, Option<String>>(0),
                        )
                        .optional()
                        .map_err(db_err)?
                        .flatten();
                }
            }
        }
        Ok(false)
    }

    pub fn create_custom_group(&self, owner_id: &str, name: &str) -> SResult<String> {
        let id = new_id("cgrp");
        self.conn
            .execute(
                "INSERT INTO custom_groups(id, name, owner_id, created_ms) VALUES(?1,?2,?3,?4)",
                params![id, name, owner_id, now_ms()],
            )
            .map_err(db_err)?;
        self.conn
            .execute(
                "INSERT OR IGNORE INTO custom_group_members(group_id, user_id) VALUES(?1,?2)",
                params![id, owner_id],
            )
            .map_err(db_err)?;
        Ok(id)
    }

    pub fn custom_group(&self, id: &str) -> SResult<Option<(String, String)>> {
        // (name, owner_id)
        let row = self
            .conn
            .query_row(
                "SELECT name, owner_id FROM custom_groups WHERE id=?1",
                params![id],
                |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)),
            )
            .optional()
            .map_err(db_err)?;
        Ok(row)
    }

    pub fn custom_group_members(&self, group_id: &str) -> SResult<Vec<String>> {
        let mut stmt = self
            .conn
            .prepare("SELECT user_id FROM custom_group_members WHERE group_id=?1")
            .map_err(db_err)?;
        let rows = stmt
            .query_map(params![group_id], |r| r.get::<_, String>(0))
            .map_err(db_err)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(db_err)?;
        Ok(rows)
    }

    pub fn add_custom_group_member(&self, group_id: &str, user_id: &str) -> SResult<()> {
        self.conn
            .execute(
                "INSERT OR IGNORE INTO custom_group_members(group_id, user_id) VALUES(?1,?2)",
                params![group_id, user_id],
            )
            .map_err(db_err)?;
        Ok(())
    }

    pub fn remove_custom_group_member(&self, group_id: &str, user_id: &str) -> SResult<()> {
        self.conn
            .execute(
                "DELETE FROM custom_group_members WHERE group_id=?1 AND user_id=?2",
                params![group_id, user_id],
            )
            .map_err(db_err)?;
        Ok(())
    }

    /// Custom groups the user owns or belongs to, as JSON with members.
    pub fn list_custom_groups_for(&self, user_id: &str) -> SResult<Vec<serde_json::Value>> {
        let users = self.list_users()?;
        let name_of = |uid: &str| {
            users
                .iter()
                .find(|u| u.id == uid)
                .map(|u| u.display_name.clone())
                .unwrap_or_else(|| uid.to_string())
        };
        let mut stmt = self
            .conn
            .prepare("SELECT id, name, owner_id, created_ms FROM custom_groups ORDER BY created_ms")
            .map_err(db_err)?;
        let rows = stmt
            .query_map([], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, i64>(3)?,
                ))
            })
            .map_err(db_err)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(db_err)?;
        let mut out = Vec::new();
        for (id, name, owner_id, created_ms) in rows {
            let members = self.custom_group_members(&id)?;
            if owner_id != user_id && !members.iter().any(|m| m == user_id) {
                continue;
            }
            out.push(serde_json::json!({
                "id": id, "name": name, "owner_id": owner_id,
                "owner_name": name_of(&owner_id), "created_ms": created_ms,
                "members": members.iter().map(|m| serde_json::json!({
                    "user_id": m, "name": name_of(m),
                })).collect::<Vec<_>>(),
            }));
        }
        Ok(out)
    }

    pub fn create_share(
        &self,
        item_kind: &str,
        item_id: &str,
        title: &str,
        from_user: &str,
        target_kind: &str,
        target_id: &str,
        note: &str,
    ) -> SResult<String> {
        let id = new_id("shr2");
        self.conn
            .execute(
                "INSERT INTO shares(id, item_kind, item_id, title, from_user, target_kind, target_id, note, created_ms) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9)",
                params![id, item_kind, item_id, title, from_user, target_kind, target_id, note, now_ms()],
            )
            .map_err(db_err)?;
        Ok(id)
    }

    pub fn list_shares(&self) -> SResult<Vec<serde_json::Value>> {
        let users = self.list_users()?;
        let name_of = |uid: &str| {
            users
                .iter()
                .find(|u| u.id == uid)
                .map(|u| u.display_name.clone())
                .unwrap_or_else(|| uid.to_string())
        };
        let mut stmt = self
            .conn
            .prepare("SELECT id, item_kind, item_id, title, from_user, target_kind, target_id, note, created_ms FROM shares ORDER BY created_ms DESC")
            .map_err(db_err)?;
        let rows = stmt
            .query_map([], |r| {
                Ok(serde_json::json!({
                    "id": r.get::<_, String>(0)?,
                    "item_kind": r.get::<_, String>(1)?,
                    "item_id": r.get::<_, String>(2)?,
                    "title": r.get::<_, String>(3)?,
                    "from_user": r.get::<_, String>(4)?,
                    "from_name": name_of(&r.get::<_, String>(4)?),
                    "target_kind": r.get::<_, String>(5)?,
                    "target_id": r.get::<_, String>(6)?,
                    "note": r.get::<_, String>(7)?,
                    "created_ms": r.get::<_, i64>(8)?,
                }))
            })
            .map_err(db_err)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(db_err)?;
        Ok(rows)
    }

    pub fn get_showroom(&self, id: &str) -> SResult<Option<ShowroomRec>> {
        Ok(self.list_showroom(None)?.into_iter().find(|r| r.id == id))
    }

    pub fn add_library_entry(
        &self,
        user_id: &str,
        item_kind: &str,
        item_id: &str,
        title: &str,
        folder: &str,
    ) -> SResult<String> {
        if let Some(existing) = self
            .list_library(user_id)?
            .iter()
            .find(|e| e["item_id"] == item_id && e["item_kind"] == item_kind)
        {
            return Ok(existing["id"].as_str().unwrap_or_default().to_string());
        }
        let id = new_id("lib");
        self.conn
            .execute(
                "INSERT INTO library_entries(id, user_id, item_kind, item_id, title, folder, added_ms) VALUES(?1,?2,?3,?4,?5,?6,?7)",
                params![id, user_id, item_kind, item_id, title, folder, now_ms()],
            )
            .map_err(db_err)?;
        Ok(id)
    }

    pub fn list_library(&self, user_id: &str) -> SResult<Vec<serde_json::Value>> {
        let mut stmt = self
            .conn
            .prepare("SELECT id, item_kind, item_id, title, folder, added_ms FROM library_entries WHERE user_id=?1 ORDER BY added_ms DESC")
            .map_err(db_err)?;
        let rows = stmt
            .query_map(params![user_id], |r| {
                Ok(serde_json::json!({
                    "id": r.get::<_, String>(0)?,
                    "item_kind": r.get::<_, String>(1)?,
                    "item_id": r.get::<_, String>(2)?,
                    "title": r.get::<_, String>(3)?,
                    "folder": r.get::<_, String>(4)?,
                    "added_ms": r.get::<_, i64>(5)?,
                }))
            })
            .map_err(db_err)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(db_err)?;
        Ok(rows)
    }

    pub fn remove_library_entry(&self, id: &str, user_id: &str) -> SResult<()> {
        self.conn
            .execute(
                "DELETE FROM library_entries WHERE id=?1 AND user_id=?2",
                params![id, user_id],
            )
            .map_err(db_err)?;
        Ok(())
    }

    /// Can this user access a showroom item at item level: it is live and
    /// (its source group is visible, or it was shared to them / their
    /// department / one of their custom groups), or they submitted it and
    /// still have source access. A library reference never grants access.
    pub fn showroom_item_access(&self, user: &UserRec, item: &ShowroomRec) -> SResult<bool> {
        if user.role == "super" {
            return Ok(true);
        }
        let source_visible = match &item.source_index_id {
            Some(id) => match self.get_index(id) {
                Ok(index) => self.can_see_group(user, &index.group_id)?,
                Err(_) => false,
            },
            None => false,
        };
        if item.submitted_by == user.id {
            return Ok(item.source_index_id.is_none() || source_visible);
        }
        if item.state != "live" {
            return Ok(false);
        }
        if source_visible {
            return Ok(true);
        }
        let my_depts = self.user_dept_ids(&user.id)?;
        let my_groups: Vec<String> = self
            .list_custom_groups_for(&user.id)?
            .iter()
            .filter_map(|g| g["id"].as_str().map(|s| s.to_string()))
            .collect();
        for s in self.list_shares()? {
            if s["item_kind"] != "showroom" || s["item_id"] != item.id {
                continue;
            }
            match s["target_kind"].as_str() {
                Some("user") if s["target_id"] == user.id => return Ok(true),
                Some("group")
                    if my_groups
                        .iter()
                        .any(|g| Some(g.as_str()) == s["target_id"].as_str()) =>
                {
                    return Ok(true)
                }
                Some("dept") => {
                    if let Some(t) = s["target_id"].as_str() {
                        for d in &my_depts {
                            if self.dept_within(d, t)? {
                                return Ok(true);
                            }
                        }
                    }
                }
                _ => {}
            }
        }
        Ok(false)
    }

    /// Guest (anonymous) visibility of a showroom item (org-model
    /// G2/G3): live, and carrying the org_public scope. That is the
    /// whole of the anonymous visible set this phase (DESIGN §2.3).
    pub fn guest_item_visible(&self, item: &ShowroomRec) -> bool {
        item.state == "live" && scope_allows_guest(&item.scope)
    }

    /// The commit a showroom item publishes: the one pinned at
    /// submit time, else the source index's current main head —
    /// the same resolution export_showroom performs.
    pub fn guest_item_commit(&self, item: &ShowroomRec) -> SResult<Option<CommitRec>> {
        match &item.commit_id {
            Some(cid) => Ok(self.get_commit(cid).ok()),
            None => match &item.source_index_id {
                Some(idx) => match self.branch_head(idx, "main")? {
                    Some(head) => Ok(self.get_commit(&head).ok()),
                    None => Ok(None),
                },
                None => Ok(None),
            },
        }
    }

    /// The newest live org_public item sourced from an index — the
    /// entry point of the guest source chain for that index.
    pub fn guest_source_item(&self, index_id: &str) -> SResult<Option<ShowroomRec>> {
        Ok(self.list_showroom(Some("live"))?.into_iter().find(|i| {
            i.source_index_id.as_deref() == Some(index_id) && scope_allows_guest(&i.scope)
        }))
    }

    /// Whether a version is among the published versions of some
    /// live org_public showroom item (org-model G2, the entry
    /// funnel): a guest session may reach a version's resources
    /// only through a published item's source chain, never by bare
    /// version id — an unpublished version simply does not
    /// resolve here.
    pub fn guest_published_version(&self, version_id: &str) -> SResult<bool> {
        for item in self.list_showroom(Some("live"))? {
            if !scope_allows_guest(&item.scope) {
                continue;
            }
            let Some(commit) = self.guest_item_commit(&item)? else {
                continue;
            };
            // The published version set of an item is its pinned
            // resolution: a newer block version the item does not
            // publish is not guest-reachable through it.
            let pin_rows = self.showroom_pin_rows(&item.id)?;
            // Pins bound to a different commit than the item's
            // current one are a write-side desync: the item
            // publishes nothing through them — never guess a
            // version set at the guest boundary.
            if pin_rows
                .iter()
                .any(|(_, c)| !c.is_empty() && c != &commit.id)
            {
                continue;
            }
            let pins: Vec<String> = pin_rows.into_iter().map(|(vid, _)| vid).collect();
            for (pos, e) in commit.entries.iter().enumerate() {
                let pin = pins.get(pos).map(|s| s.as_str());
                if let Some(v) = self.resolve_showroom_entry(e, pin)? {
                    // Published means formally published: a
                    // resolution landing on a draft (a legacy
                    // unpinned entry following latest onto an
                    // unreviewed draft) is not a publication and
                    // never enters the guest-reachable set.
                    if v.id == version_id && v.status == "normal" {
                        return Ok(true);
                    }
                }
            }
        }
        Ok(false)
    }

    pub fn set_profile_field(&self, user_id: &str, field: &str, value: &str) -> SResult<()> {
        self.conn
            .execute(
                "INSERT INTO profile_fields(user_id, field, value) VALUES(?1,?2,?3) ON CONFLICT(user_id, field) DO UPDATE SET value=excluded.value",
                params![user_id, field, value],
            )
            .map_err(db_err)?;
        Ok(())
    }

    pub fn profile(&self, user_id: &str) -> SResult<Vec<(String, String)>> {
        let mut stmt = self
            .conn
            .prepare("SELECT field, value FROM profile_fields WHERE user_id=?1 ORDER BY field")
            .map_err(db_err)?;
        let rows = stmt
            .query_map(params![user_id], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
            })
            .map_err(db_err)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(db_err)?;
        Ok(rows)
    }

    pub fn set_company_field(&self, field: &str, value: &str) -> SResult<()> {
        self.conn
            .execute(
                "INSERT INTO company_profile(field, value) VALUES(?1,?2) ON CONFLICT(field) DO UPDATE SET value=excluded.value",
                params![field, value],
            )
            .map_err(db_err)?;
        Ok(())
    }

    pub fn company_profile(&self) -> SResult<Vec<(String, String)>> {
        let mut stmt = self
            .conn
            .prepare("SELECT field, value FROM company_profile ORDER BY field")
            .map_err(db_err)?;
        let rows = stmt
            .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))
            .map_err(db_err)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(db_err)?;
        Ok(rows)
    }

    pub fn get_setting(&self, key: &str) -> SResult<Option<String>> {
        let v: Option<String> = self
            .conn
            .query_row(
                "SELECT value FROM settings WHERE key=?1",
                params![key],
                |r| r.get(0),
            )
            .ok();
        Ok(v)
    }

    pub fn set_setting(&self, key: &str, value: &str) -> SResult<()> {
        self.conn
            .execute(
                "INSERT INTO settings(key, value) VALUES(?1,?2) ON CONFLICT(key) DO UPDATE SET value=excluded.value",
                params![key, value],
            )
            .map_err(db_err)?;
        Ok(())
    }

    /// Create a consistent snapshot backup of `qimu.db` (via `VACUUM INTO`)
    /// plus all files in `files/` under `dest_dir`. Returns the number of
    /// copied files from `files/`.
    pub fn backup_to(&self, dest_dir: &Path) -> SResult<usize> {
        std::fs::create_dir_all(dest_dir).map_err(|e| format!("创建备份目录失败：{e}"))?;
        let dest_db = dest_dir.join("qimu.db");
        if dest_db.exists() {
            std::fs::remove_file(&dest_db).map_err(|e| format!("清理旧备份数据库失败：{e}"))?;
        }
        self.conn
            .execute(
                "VACUUM INTO ?1",
                params![dest_db.to_string_lossy().as_ref()],
            )
            .map_err(db_err)?;

        fn copy_dir_recursive(src: &Path, dst: &Path) -> std::io::Result<usize> {
            if !src.exists() {
                return Ok(0);
            }
            std::fs::create_dir_all(dst)?;
            let mut count = 0;
            for entry in std::fs::read_dir(src)? {
                let entry = entry?;
                let ty = entry.file_type()?;
                let to = dst.join(entry.file_name());
                if ty.is_dir() {
                    count += copy_dir_recursive(&entry.path(), &to)?;
                } else if ty.is_file() {
                    std::fs::copy(entry.path(), &to)?;
                    count += 1;
                }
            }
            Ok(count)
        }

        let files_src = self.root.join("files");
        let files_dst = dest_dir.join("files");
        copy_dir_recursive(&files_src, &files_dst).map_err(|e| format!("复制内容文件失败：{e}"))
    }

    /// First-run seed: super admin + one default content group + top folders.
    pub fn ensure_seed(&self, admin_password: &str) -> SResult<()> {
        if self.user_count()? > 0 {
            return Ok(());
        }
        let admin = self.create_user("admin", "超级管理员", admin_password, None, "super")?;
        let _ = admin;
        let g = self.create_group("公司介绍")?;
        self.create_group("技术")?;
        self.create_folder("公司介绍", None, "index")?;
        self.create_folder("公司技术", None, "index")?;
        self.create_folder("公司案例", None, "index")?;
        self.set_setting(
            "languages",
            &serde_json::json!(["中文", "英语", "日语"]).to_string(),
        )?;
        let _ = g;
        Ok(())
    }
}

#[cfg(test)]
mod version_management_tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn store() -> (Store, PathBuf) {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "qimu-version-management-{}-{stamp}",
            std::process::id()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let store = Store::open(&root).unwrap();
        store
            .conn
            .execute_batch(
                "INSERT INTO renditions(id, unit_id, form, lang) VALUES('r1', 'u1', '标准版', 'zh');",
            )
            .unwrap();
        (store, root)
    }

    fn insert_version(store: &Store, id: &str, rendition_id: &str, semver: &str, status: &str) {
        let parts = SemVer::parse(semver)
            .ok()
            .map(|parsed| {
                (
                    semver_part(parsed.major),
                    semver_part(parsed.minor),
                    semver_part(parsed.patch),
                )
            })
            .unwrap_or((-1, 0, 0));
        store
            .conn
            .execute(
                "INSERT INTO versions(id, rendition_id, semver, major, minor, patch, basis_revision, author, note, created_ms, fragment_hash, page_count, pages_json, status) VALUES(?1,?2,?3,?4,?5,?6,1,'tester','original',1,'',0,'[]',?7)",
                params![id, rendition_id, semver, parts.0, parts.1, parts.2, status],
            )
            .unwrap();
    }

    #[test]
    fn working_publish_uses_selected_segment_and_removes_the_draft() {
        for (bump, expected) in [
            ("major", "3.0.0"),
            ("minor", "2.4.0"),
            ("patch", "2.3.5"),
        ] {
            let (store, root) = store();
            insert_version(&store, "head", "r1", "2.3.4", "normal");
            insert_version(&store, "working", "r1", "", "working");
            let published = store
                .publish_working_draft("working", "", "tester", Some(bump))
                .unwrap();
            assert_eq!(published.id, "working");
            assert_eq!(published.semver, expected);
            assert_eq!(published.status, "normal");
            assert!(store.working_version("r1").unwrap().is_none());
            drop(store);
            std::fs::remove_dir_all(root).unwrap();
        }
    }

    #[test]
    fn numbered_draft_publishes_above_the_normal_head() {
        let (store, root) = store();
        insert_version(&store, "head", "r1", "2.3.4", "normal");
        insert_version(&store, "draft", "r1", "9.0.0", "draft");
        let published = store
            .publish_draft("draft", Some(""), "tester", Some("patch"))
            .unwrap();
        assert_eq!(published.id, "draft");
        assert_eq!(published.semver, "2.3.5");
        assert_eq!(published.status, "normal");
        let ledger = store.list_renumbers("draft").unwrap();
        assert_eq!(ledger.len(), 1);
        assert_eq!(ledger[0].from_semver, "9.0.0");
        assert_eq!(ledger[0].to_semver, "2.3.5");
        drop(store);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn drag_reassigns_numbers_without_changing_version_identity() {
        let (store, root) = store();
        insert_version(&store, "a", "r1", "3.0.0", "normal");
        insert_version(&store, "b", "r1", "2.0.0", "normal");
        insert_version(&store, "c", "r1", "1.0.0", "normal");
        let order = vec!["b".to_string(), "c".to_string(), "a".to_string()];
        let shifts = store
            .reorder_versions_recorded("r1", &order, "tester")
            .unwrap();
        assert_eq!(shifts.len(), 3);
        let line = store.version_line_order("r1").unwrap();
        assert_eq!(
            line.iter()
                .map(|version| (version.id.as_str(), version.semver.as_str()))
                .collect::<Vec<_>>(),
            vec![("b", "3.0.0"), ("c", "2.0.0"), ("a", "1.0.0")]
        );
        assert_eq!(store.get_version("a").unwrap().note, "original");
        assert_eq!(store.list_renumbers("a").unwrap().len(), 1);
        assert!(store
            .reorder_versions_recorded("r1", &["a".to_string()], "tester")
            .is_err());
        assert_eq!(store.version_line_order("r1").unwrap().len(), 3);
        drop(store);
        std::fs::remove_dir_all(root).unwrap();
    }
}

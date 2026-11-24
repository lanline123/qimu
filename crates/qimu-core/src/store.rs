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
use crate::text::{slide_full_text, slide_title};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashSet;
use std::path::{Path, PathBuf};

pub type SResult<T> = Result<T, String>;

fn db_err(e: rusqlite::Error) -> String {
    e.to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserRec {
    pub id: String,
    pub username: String,
    pub display_name: String,
    pub dept_id: Option<String>,
    pub role: String, // super | dept_admin | member
    pub disabled: bool,
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
    pub status: String, // normal | recalled | draft
    /// Per-page speaker scripts (讲稿), aligned with `pages`. Imported
    /// from the fragment's notes; editable without bumping the version.
    #[serde(default)]
    pub scripts: Vec<String>,
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
    pub state: String, // pending | landed | rejected
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
CREATE TABLE IF NOT EXISTS users(id TEXT PRIMARY KEY, username TEXT UNIQUE NOT NULL, display_name TEXT NOT NULL, password_hash TEXT NOT NULL, dept_id TEXT, role TEXT NOT NULL DEFAULT 'member', disabled INTEGER NOT NULL DEFAULT 0);
CREATE TABLE IF NOT EXISTS user_depts(user_id TEXT NOT NULL, dept_id TEXT NOT NULL, PRIMARY KEY(user_id, dept_id));
CREATE TABLE IF NOT EXISTS sessions(token TEXT PRIMARY KEY, user_id TEXT, created_ms INTEGER NOT NULL, kind TEXT NOT NULL DEFAULT 'user', client_ip TEXT, last_seen_ms INTEGER NOT NULL DEFAULT 0);
CREATE TABLE IF NOT EXISTS folders(id TEXT PRIMARY KEY, name TEXT NOT NULL, parent_id TEXT, sort INTEGER NOT NULL DEFAULT 0, main_index_id TEXT, scope TEXT NOT NULL DEFAULT '', color TEXT, note TEXT NOT NULL DEFAULT '');
CREATE TABLE IF NOT EXISTS groups_tbl(id TEXT PRIMARY KEY, name TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS group_grants(id TEXT PRIMARY KEY, group_id TEXT NOT NULL, subject_type TEXT NOT NULL, subject_id TEXT NOT NULL, capability TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS units(id TEXT PRIMARY KEY, name TEXT NOT NULL, folder_id TEXT, group_id TEXT NOT NULL, maintainer TEXT NOT NULL, lead_lang TEXT NOT NULL DEFAULT 'zh', description TEXT NOT NULL DEFAULT '', current_revision INTEGER NOT NULL DEFAULT 1, created_ms INTEGER NOT NULL);
CREATE TABLE IF NOT EXISTS renditions(id TEXT PRIMARY KEY, unit_id TEXT NOT NULL, form TEXT NOT NULL, lang TEXT NOT NULL, UNIQUE(unit_id, form, lang));
CREATE TABLE IF NOT EXISTS versions(id TEXT PRIMARY KEY, rendition_id TEXT NOT NULL, semver TEXT NOT NULL, major INTEGER NOT NULL, minor INTEGER NOT NULL, patch INTEGER NOT NULL, basis_revision INTEGER NOT NULL, author TEXT NOT NULL, note TEXT NOT NULL DEFAULT '', created_ms INTEGER NOT NULL, fragment_hash TEXT NOT NULL, page_count INTEGER NOT NULL, pages_json TEXT NOT NULL, text_all TEXT NOT NULL DEFAULT '', status TEXT NOT NULL DEFAULT 'normal', scripts_json TEXT NOT NULL DEFAULT '[]');
CREATE TABLE IF NOT EXISTS indexes_tbl(id TEXT PRIMARY KEY, name TEXT NOT NULL, folder_id TEXT, group_id TEXT NOT NULL, maintainer TEXT NOT NULL, lifecycle TEXT NOT NULL DEFAULT 'active', scope TEXT NOT NULL DEFAULT 'company', created_ms INTEGER NOT NULL);
CREATE TABLE IF NOT EXISTS index_branches(index_id TEXT NOT NULL, name TEXT NOT NULL, head_commit TEXT, PRIMARY KEY(index_id, name));
CREATE TABLE IF NOT EXISTS index_commits(id TEXT PRIMARY KEY, index_id TEXT NOT NULL, branch TEXT NOT NULL, parent_id TEXT, author TEXT NOT NULL, message TEXT NOT NULL, created_ms INTEGER NOT NULL, entries_json TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS index_tags(id TEXT PRIMARY KEY, index_id TEXT NOT NULL, branch TEXT NOT NULL, name TEXT NOT NULL, commit_id TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS exports(id TEXT PRIMARY KEY, index_id TEXT NOT NULL, branch TEXT NOT NULL, commit_id TEXT NOT NULL, tag TEXT, user_id TEXT NOT NULL, created_ms INTEGER NOT NULL, deck_hash TEXT NOT NULL, page_count INTEGER NOT NULL, kind TEXT NOT NULL DEFAULT 'personal', deck_id TEXT NOT NULL DEFAULT '', manifest_json TEXT NOT NULL DEFAULT '');
CREATE TABLE IF NOT EXISTS attachments(id TEXT PRIMARY KEY, parent_type TEXT NOT NULL, parent_id TEXT NOT NULL, filename TEXT NOT NULL, hash TEXT NOT NULL, size INTEGER NOT NULL, uploader TEXT NOT NULL, created_ms INTEGER NOT NULL);
CREATE TABLE IF NOT EXISTS version_tags(id TEXT PRIMARY KEY, version_id TEXT NOT NULL, dim TEXT NOT NULL, value TEXT NOT NULL, strength TEXT NOT NULL DEFAULT 'fits', author TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS showroom(id TEXT PRIMARY KEY, title TEXT NOT NULL, intro TEXT NOT NULL DEFAULT '', source_index_id TEXT, commit_id TEXT, submitted_by TEXT NOT NULL, state TEXT NOT NULL DEFAULT 'pending', created_ms INTEGER NOT NULL, scope TEXT NOT NULL DEFAULT 'org_public');
CREATE TABLE IF NOT EXISTS showroom_pins(showroom_id TEXT NOT NULL, pos INTEGER NOT NULL, version_id TEXT NOT NULL, PRIMARY KEY(showroom_id, pos));
CREATE TABLE IF NOT EXISTS submissions(id TEXT PRIMARY KEY, title TEXT NOT NULL, intro TEXT NOT NULL DEFAULT '', submitter_id TEXT NOT NULL, submitter_name TEXT NOT NULL, state TEXT NOT NULL DEFAULT 'pending', note TEXT NOT NULL DEFAULT '', review_note TEXT NOT NULL DEFAULT '', reviewed_by TEXT NOT NULL DEFAULT '', reviewed_ms INTEGER NOT NULL DEFAULT 0, created_ms INTEGER NOT NULL, supersedes TEXT, source_index_id TEXT, landed_index_id TEXT, landed_showroom_id TEXT, entry_count INTEGER NOT NULL DEFAULT 0, page_count INTEGER NOT NULL DEFAULT 0, land_error TEXT NOT NULL DEFAULT '', manifest_json TEXT NOT NULL DEFAULT '');
CREATE TABLE IF NOT EXISTS trash_items(id TEXT PRIMARY KEY, kind TEXT NOT NULL, object_id TEXT NOT NULL, title TEXT NOT NULL DEFAULT '', origin TEXT NOT NULL DEFAULT '', deleted_by TEXT NOT NULL DEFAULT '', deleted_ms INTEGER NOT NULL DEFAULT 0, UNIQUE(kind, object_id));
CREATE TABLE IF NOT EXISTS assemblies(id TEXT PRIMARY KEY, user_id TEXT NOT NULL, name TEXT NOT NULL, entries_json TEXT NOT NULL, created_ms INTEGER NOT NULL);
CREATE TABLE IF NOT EXISTS page_bindings(id TEXT PRIMARY KEY, unit_id TEXT NOT NULL, from_version_id TEXT NOT NULL, from_slot INTEGER NOT NULL, to_version_id TEXT NOT NULL, to_slot INTEGER NOT NULL, kind TEXT NOT NULL DEFAULT 'manual', author TEXT NOT NULL, created_ms INTEGER NOT NULL);
CREATE TABLE IF NOT EXISTS pages(id TEXT PRIMARY KEY, fp TEXT NOT NULL, hash TEXT NOT NULL, title TEXT NOT NULL DEFAULT '');
CREATE TABLE IF NOT EXISTS custom_groups(id TEXT PRIMARY KEY, name TEXT NOT NULL, owner_id TEXT NOT NULL, created_ms INTEGER NOT NULL);
CREATE TABLE IF NOT EXISTS custom_group_members(group_id TEXT NOT NULL, user_id TEXT NOT NULL, PRIMARY KEY(group_id, user_id));
CREATE TABLE IF NOT EXISTS shares(id TEXT PRIMARY KEY, item_kind TEXT NOT NULL, item_id TEXT NOT NULL, title TEXT NOT NULL, from_user TEXT NOT NULL, target_kind TEXT NOT NULL, target_id TEXT NOT NULL, note TEXT NOT NULL DEFAULT '', created_ms INTEGER NOT NULL);
CREATE TABLE IF NOT EXISTS library_entries(id TEXT PRIMARY KEY, user_id TEXT NOT NULL, item_kind TEXT NOT NULL, item_id TEXT NOT NULL, title TEXT NOT NULL, folder TEXT NOT NULL DEFAULT '', added_ms INTEGER NOT NULL);
CREATE TABLE IF NOT EXISTS profile_fields(user_id TEXT NOT NULL, field TEXT NOT NULL, value TEXT NOT NULL, PRIMARY KEY(user_id, field));
CREATE TABLE IF NOT EXISTS company_profile(field TEXT PRIMARY KEY, value TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS settings(key TEXT PRIMARY KEY, value TEXT NOT NULL);
";

pub struct Store {
    conn: Connection,
    root: PathBuf,
}

impl Store {
    pub fn open(root: &Path) -> SResult<Store> {
        std::fs::create_dir_all(root.join("files")).map_err(|e| e.to_string())?;
        let conn = Connection::open(root.join("qimu.db")).map_err(db_err)?;
        conn.execute_batch(SCHEMA).map_err(db_err)?;
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
                conn.execute_batch(
                    "CREATE TABLE sessions_new(token TEXT PRIMARY KEY, user_id TEXT, created_ms INTEGER NOT NULL, kind TEXT NOT NULL DEFAULT 'user', client_ip TEXT, last_seen_ms INTEGER NOT NULL DEFAULT 0);",
                )
                .map_err(db_err)?;
                conn.execute(
                    "INSERT INTO sessions_new(token, user_id, created_ms, kind, client_ip, last_seen_ms) SELECT token, user_id, created_ms, 'user', NULL, ?1 FROM sessions",
                    params![now_ms()],
                )
                .map_err(db_err)?;
                conn.execute_batch(
                    "DROP TABLE sessions; ALTER TABLE sessions_new RENAME TO sessions;",
                )
                .map_err(db_err)?;
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
        store.migrate_page_entities()?;
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
    pub fn register_page(&self, fp: &str, title: &str, single: &[u8]) -> SResult<PageRec> {
        let id = page_id_for_fp(fp);
        if let Some(rec) = self.get_page(&id)? {
            return Ok(rec);
        }
        let hash = self.file_put(single)?;
        self.conn
            .execute(
                "INSERT OR IGNORE INTO pages(id, fp, hash, title) VALUES(?1,?2,?3,?4)",
                params![id, fp, hash, title],
            )
            .map_err(db_err)?;
        self.get_page(&id)?.ok_or_else(|| "页实体登记失败".to_string())
    }

    pub fn get_page(&self, id: &str) -> SResult<Option<PageRec>> {
        self.conn
            .query_row(
                "SELECT id, fp, hash, title FROM pages WHERE id=?1",
                params![id],
                |r| {
                    Ok(PageRec {
                        id: r.get(0)?,
                        fp: r.get(1)?,
                        hash: r.get(2)?,
                        title: r.get(3)?,
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
        self.file_get(&version.fragment_hash)
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
        let pending: Vec<(String, String, String)> = {
            let mut stmt = self
                .conn
                .prepare("SELECT id, fragment_hash, pages_json FROM versions")
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
            let mut pages: Vec<PageInfo> =
                serde_json::from_str(pages_json).unwrap_or_default();
            if pages.iter().all(|p| !p.id.is_empty()) {
                continue;
            }
            let Ok(bytes) = self.file_get(fragment_hash) else {
                continue;
            };
            let Ok(pkg) = Package::load(&bytes) else {
                continue;
            };
            let Ok(singles) = crate::pptx::split(&pkg) else {
                continue;
            };
            if singles.len() != pages.len() {
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
                match self.register_page(&page.fp, &page.title, &single_bytes) {
                    Ok(rec) => page.id = rec.id,
                    Err(_) => {
                        filled = false;
                        break;
                    }
                }
            }
            if !filled {
                continue;
            }
            self.conn
                .execute(
                    "UPDATE versions SET pages_json=?2 WHERE id=?1",
                    params![
                        version_id,
                        serde_json::to_string(&pages).unwrap_or_default()
                    ],
                )
                .map_err(db_err)?;
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
            self.conn
                .execute(
                    "UPDATE page_bindings SET from_page_id=?2, to_page_id=?3 WHERE id=?1",
                    params![binding_id, from_pid, to_pid],
                )
                .map_err(db_err)?;
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
        })
    }

    pub fn user_by_id(&self, id: &str) -> SResult<UserRec> {
        self.conn
            .query_row(
                "SELECT id, username, display_name, dept_id, role, disabled FROM users WHERE id=?1",
                params![id],
                Self::row_to_user,
            )
            .map_err(db_err)
    }

    pub fn user_by_username(&self, username: &str) -> SResult<UserRec> {
        self.conn
            .query_row(
                "SELECT id, username, display_name, dept_id, role, disabled FROM users WHERE username=?1",
                params![username],
                Self::row_to_user,
            )
            .map_err(db_err)
    }

    pub fn list_users(&self) -> SResult<Vec<UserRec>> {
        let mut stmt = self
            .conn
            .prepare("SELECT id, username, display_name, dept_id, role, disabled FROM users ORDER BY username")
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
        let (user, hash) = self
            .conn
            .query_row(
                "SELECT id, username, display_name, dept_id, role, disabled, password_hash FROM users WHERE username=?1",
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
                        },
                        r.get::<_, String>(6)?,
                    ))
                },
            )
            .map_err(|_| "用户名或密码不正确".to_string())?;
        if user.disabled {
            return Err("账号已停用".into());
        }
        if !verify_password(password, &hash) {
            return Err("用户名或密码不正确".into());
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
                "SELECT id, username, display_name, dept_id, role, disabled FROM users WHERE role='super' ORDER BY rowid LIMIT 1",
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
        let scope = match parent_id {
            Some(pid) => self
                .list_folders()?
                .into_iter()
                .find(|f| f.id == pid)
                .map(|f| f.scope)
                .filter(|sc| !sc.is_empty())
                .unwrap_or_else(|| scope.to_string()),
            None => scope.to_string(),
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
        })
    }

    pub fn list_folders(&self) -> SResult<Vec<FolderRec>> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT id, name, parent_id, sort, main_index_id, scope, color, note FROM folders ORDER BY sort, name",
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

    pub fn rename_folder(&self, id: &str, name: &str) -> SResult<()> {
        self.conn
            .execute("UPDATE folders SET name=?2 WHERE id=?1", params![id, name])
            .map_err(db_err)?;
        Ok(())
    }

    /// Set (or clear, with an empty string) the folder's note
    /// （备注）. The content page's folder-flow header edits it in
    /// place; the sidebar tree does not surface it.
    pub fn set_folder_note(&self, id: &str, note: &str) -> SResult<()> {
        self.conn
            .execute("UPDATE folders SET note=?2 WHERE id=?1", params![id, note])
            .map_err(db_err)?;
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
        self.conn
            .execute(
                "UPDATE folders SET main_index_id=?2 WHERE id=?1",
                params![folder_id, index_id],
            )
            .map_err(db_err)?;
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
        self.conn
            .execute(
                "INSERT INTO units(id, name, folder_id, group_id, maintainer, lead_lang, description, current_revision, created_ms) VALUES(?1,?2,?3,?4,?5,?6,?7,1,?8)",
                params![id, name, folder_id, group_id, maintainer, lead_lang, description, now_ms()],
            )
            .map_err(db_err)?;
        self.get_unit(&id)
    }

    pub fn get_unit(&self, id: &str) -> SResult<UnitRec> {
        self.conn
            .query_row(
                "SELECT id, name, folder_id, group_id, maintainer, lead_lang, description, current_revision, created_ms FROM units WHERE id=?1",
                params![id],
                Self::row_to_unit,
            )
            .map_err(db_err)
    }

    pub fn list_units(&self, visible: Option<&HashSet<String>>) -> SResult<Vec<UnitRec>> {
        let all = {
            let mut stmt = self
                .conn
                .prepare("SELECT id, name, folder_id, group_id, maintainer, lead_lang, description, current_revision, created_ms FROM units ORDER BY name")
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
        })
    }

    const VERSION_COLS: &'static str = "id, rendition_id, semver, basis_revision, author, note, created_ms, fragment_hash, page_count, pages_json, status, scripts_json";

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
        if let Ok(singles) = crate::pptx::split(&pkg) {
            if singles.len() == pages.len() {
                for (i, page) in pages.iter_mut().enumerate() {
                    if let Ok(single_bytes) = singles[i].to_bytes() {
                        if let Ok(rec) =
                            self.register_page(&page.fp, &page.title, &single_bytes)
                        {
                            page.id = rec.id;
                        }
                    }
                }
            }
        }
        let hash = self.file_put(fragment)?;
        let id = new_id("ver");
        self.conn
            .execute(
                "INSERT INTO versions(id, rendition_id, semver, major, minor, patch, basis_revision, author, note, created_ms, fragment_hash, page_count, pages_json, text_all, status, scripts_json) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16)",
                params![
                    id,
                    rendition_id,
                    semver.to_string(),
                    semver.major as i64,
                    semver.minor as i64,
                    semver.patch as i64,
                    basis_revision,
                    author,
                    note,
                    now_ms(),
                    hash,
                    pages.len() as i64,
                    serde_json::to_string(&pages).unwrap_or_default(),
                    texts.join("\n"),
                    status,
                    serde_json::to_string(&scripts).unwrap_or_default()
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
    /// `fragment_hash`) require it to exist, so only a caller with
    /// nothing byte-facing may pass None.
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
        if scripts.len() != pages.len() {
            return Err("讲稿数量与页面数量不匹配".into());
        }
        if texts.len() != pages.len() {
            return Err("页面文本数量与页面数量不匹配".into());
        }
        let hash = match fragment {
            Some(bytes) => self.file_put(bytes)?,
            None => String::new(),
        };
        let id = new_id("ver");
        self.conn
            .execute(
                "INSERT INTO versions(id, rendition_id, semver, major, minor, patch, basis_revision, author, note, created_ms, fragment_hash, page_count, pages_json, text_all, status, scripts_json) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16)",
                params![
                    id,
                    rendition_id,
                    semver.to_string(),
                    semver.major as i64,
                    semver.minor as i64,
                    semver.patch as i64,
                    basis_revision,
                    author,
                    note,
                    now_ms(),
                    hash,
                    pages.len() as i64,
                    serde_json::to_string(&pages).unwrap_or_default(),
                    texts.join("\n"),
                    status,
                    serde_json::to_string(&scripts).unwrap_or_default()
                ],
            )
            .map_err(db_err)?;
        self.get_version(&id)
    }

    /// Replace a version's per-page scripts. Editing scripts never bumps
    /// the version or the content revision (§3.16).
    pub fn set_version_scripts(&self, version_id: &str, scripts: &[String]) -> SResult<()> {
        self.conn
            .execute(
                "UPDATE versions SET scripts_json=?2 WHERE id=?1",
                params![
                    version_id,
                    serde_json::to_string(scripts).unwrap_or_default()
                ],
            )
            .map_err(db_err)?;
        Ok(())
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
    pub fn list_versions(&self, rendition_id: &str) -> SResult<Vec<VersionRec>> {
        let mut stmt = self
            .conn
            .prepare(&format!(
                "SELECT {} FROM versions WHERE rendition_id=?1 ORDER BY major DESC, minor DESC, patch DESC",
                Self::VERSION_COLS
            ))
            .map_err(db_err)?;
        let rows = stmt
            .query_map(params![rendition_id], Self::row_to_version)
            .map_err(db_err)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(db_err)?;
        Ok(rows)
    }

    pub fn latest_version(&self, rendition_id: &str) -> SResult<Option<VersionRec>> {
        Ok(self
            .list_versions(rendition_id)?
            .into_iter()
            .find(|v| v.status != "recalled"))
    }

    pub fn set_version_status(&self, id: &str, status: &str) -> SResult<()> {
        self.conn
            .execute(
                "UPDATE versions SET status=?2 WHERE id=?1",
                params![id, status],
            )
            .map_err(db_err)?;
        Ok(())
    }

    pub fn delete_version(&self, id: &str) -> SResult<()> {
        self.conn
            .execute("DELETE FROM versions WHERE id=?1", params![id])
            .map_err(db_err)?;
        Ok(())
    }

    pub fn renumber_version(&self, id: &str, semver: SemVer) -> SResult<()> {
        self.conn
            .execute(
                "UPDATE versions SET semver=?2, major=?3, minor=?4, patch=?5 WHERE id=?1",
                params![
                    id,
                    semver.to_string(),
                    semver.major as i64,
                    semver.minor as i64,
                    semver.patch as i64
                ],
            )
            .map_err(db_err)?;
        Ok(())
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
            if from_slot < 0 || to_slot < 0 {
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
        self.conn
            .execute(
                "DELETE FROM page_bindings WHERE id=?1 AND unit_id=?2",
                params![binding_id, unit_id],
            )
            .map_err(db_err)?;
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
        })
    }

    const INDEX_COLS: &'static str =
        "id, name, folder_id, group_id, maintainer, lifecycle, scope, created_ms";

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
        self.conn
            .execute(
                "UPDATE indexes_tbl SET lifecycle=?2 WHERE id=?1",
                params![id, lifecycle],
            )
            .map_err(db_err)?;
        Ok(())
    }

    pub fn branch_head(&self, index_id: &str, branch: &str) -> SResult<Option<String>> {
        let head: Option<Option<String>> = self
            .conn
            .query_row(
                "SELECT head_commit FROM index_branches WHERE index_id=?1 AND name=?2",
                params![index_id, branch],
                |r| r.get(0),
            )
            .ok();
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
            let exists = tx
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM folders WHERE id=?1)",
                    params![folder_id],
                    |row| row.get::<_, bool>(0),
                )
                .map_err(db_err)?;
            if !exists {
                return Err("目标文件夹不存在".into());
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
        self.conn
            .execute(
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
        self.conn
            .execute(
                "UPDATE index_branches SET head_commit=?3 WHERE index_id=?1 AND name=?2",
                params![index_id, branch, id],
            )
            .map_err(db_err)?;
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
                break;
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
    /// Follow-latest → newest surviving version; locked → the locked one,
    /// with deletion substitution (nearest surviving version of the same
    /// rendition, searching upward from the locked number).
    pub fn resolve_entry_version(&self, entry: &IndexEntry) -> SResult<Option<VersionRec>> {
        let versions = self.list_versions(&entry.rendition_id.0)?;
        let surviving: Vec<VersionRec> = versions
            .into_iter()
            .filter(|v| v.status != "recalled")
            .collect();
        match &entry.policy {
            crate::model::EntryPolicy::FollowLatest => Ok(surviving.into_iter().next()),
            crate::model::EntryPolicy::Locked { version_id } => {
                if let Some(v) = surviving.iter().find(|v| v.id == version_id.0) {
                    return Ok(Some(v.clone()));
                }
                // deletion substitution: nearest surviving version at or
                // above the locked display number, else the latest.
                if let Some(ls) = &entry.locked_semver {
                    if let Ok(want) = SemVer::parse(ls) {
                        let mut above: Vec<(SemVer, &VersionRec)> = surviving
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
                Ok(surviving.into_iter().next())
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
    pub fn set_showroom_pins(&self, item_id: &str, version_ids: &[String]) -> SResult<()> {
        self.conn
            .execute(
                "DELETE FROM showroom_pins WHERE showroom_id=?1",
                params![item_id],
            )
            .map_err(db_err)?;
        for (pos, vid) in version_ids.iter().enumerate() {
            self.conn
                .execute(
                    "INSERT INTO showroom_pins(showroom_id, pos, version_id) VALUES(?1,?2,?3)",
                    params![item_id, pos as i64, vid],
                )
                .map_err(db_err)?;
        }
        Ok(())
    }

    /// An item's pins, ordered by entry position. Empty when the
    /// item predates publish pinning (legacy rows resolve exactly
    /// as they always did).
    pub fn showroom_pins(&self, item_id: &str) -> SResult<Vec<String>> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT version_id FROM showroom_pins WHERE showroom_id=?1 ORDER BY pos",
            )
            .map_err(db_err)?;
        let rows = stmt
            .query_map(params![item_id], |r| r.get::<_, String>(0))
            .map_err(db_err)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(db_err)?;
        Ok(rows)
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
        let sql = "SELECT id, index_id, branch, commit_id, tag, user_id, created_ms, deck_hash, page_count, kind, deck_id, manifest_json FROM exports";
        let mut stmt = self
            .conn
            .prepare(&format!("{sql} ORDER BY created_ms DESC LIMIT 500"))
            .map_err(db_err)?;
        let rows = stmt
            .query_map([], |r| {
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
            })
            .map_err(db_err)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(db_err)?;
        Ok(match index_id {
            None => rows,
            Some(iid) => rows.into_iter().filter(|e| e.index_id == iid).collect(),
        })
    }

    /// The export record for a manifest deck_id (editor-stripped deck
    /// recovery). Latest export wins.
    pub fn export_by_deck_id(&self, deck_id: &str) -> SResult<Option<ExportRec>> {
        let all = self.list_exports(None)?;
        Ok(all
            .into_iter()
            .find(|e| e.deck_id == deck_id && !e.manifest_json.is_empty()))
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
            filename: filename.into(),
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
        self.conn
            .execute(
                "UPDATE showroom SET state=?2, review_note=?3, reviewed_by=?4, reviewed_ms=?5 WHERE id=?1",
                params![id, state, note, reviewed_by, now_ms()],
            )
            .map_err(db_err)?;
        Ok(())
    }

    pub fn set_showroom_state(&self, id: &str, state: &str) -> SResult<()> {
        self.conn
            .execute(
                "UPDATE showroom SET state=?2 WHERE id=?1",
                params![id, state],
            )
            .map_err(db_err)?;
        Ok(())
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
        self.conn
            .execute(
                "UPDATE submissions SET state='rejected', review_note=?2, reviewed_by=?3, reviewed_ms=?4, land_error='' WHERE id=?1",
                params![id, note, by, now_ms()],
            )
            .map_err(db_err)?;
        Ok(())
    }

    /// Record why a landing attempt rolled back. The submission
    /// itself stays pending; only the error note changes.
    pub fn set_submission_land_error(&self, id: &str, err: &str) -> SResult<()> {
        self.conn
            .execute(
                "UPDATE submissions SET land_error=?2 WHERE id=?1",
                params![id, err],
            )
            .map_err(db_err)?;
        Ok(())
    }

    /// Delete a submission row and its staged package. Landed
    /// content (if any) is a separate object by then and is not
    /// touched — this removes the submission record only.
    pub fn delete_submission(&self, id: &str) -> SResult<()> {
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
    pub fn submission_staging_dir(&self, id: &str) -> PathBuf {
        self.root.join("staging").join(id)
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
        let sub = if hash.len() >= 2 { &hash[..2] } else { "00" };
        let path = self
            .submission_staging_dir(id)
            .join("files")
            .join(sub)
            .join(format!("{hash}.bin"));
        std::fs::read(&path).map_err(|e| format!("待审包文件 {hash}: {e}"))
    }

    pub fn clear_submission_staging(&self, id: &str) {
        let _ = std::fs::remove_dir_all(self.submission_staging_dir(id));
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
    /// single showroom entry — unpinned, so the entry follows
    /// the index's current main head — instead of minting a
    /// duplicate index and entry.
    pub fn land_submission(
        &self,
        id: &str,
        manifest: &SubmissionManifest,
        reviewer: &str,
    ) -> SResult<(String, String)> {
        let sub = self
            .get_submission(id)?
            .ok_or_else(|| "提交不存在".to_string())?;
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
            // entry deleted since) inserts the entry the same
            // way.
            let prior_entry = prior
                .as_ref()
                .and_then(|(_, entry)| entry.clone())
                .filter(|eid| self.get_showroom(eid).ok().flatten().is_some());
            let item_id = match &prior_entry {
                Some(entry_id) => {
                    self.conn
                        .execute(
                            "UPDATE showroom SET title=?2, intro=?3, commit_id=?4, state='live' WHERE id=?1",
                            params![entry_id.as_str(), sub.title, sub.intro, commit.id],
                        )
                        .map_err(db_err)?;
                    self.set_showroom_pins(entry_id, &landed_pins)?;
                    entry_id.clone()
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
                    self.set_showroom_pins(&rec.id, &landed_pins)?;
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
    pub fn delete_unit_permanent(&self, unit_id: &str) -> SResult<()> {
        self.conn
            .execute(
                "DELETE FROM version_tags WHERE version_id IN (SELECT v.id FROM versions v JOIN renditions r ON v.rendition_id = r.id WHERE r.unit_id = ?1)",
                params![unit_id],
            )
            .map_err(db_err)?;
        self.conn
            .execute(
                "DELETE FROM versions WHERE rendition_id IN (SELECT id FROM renditions WHERE unit_id = ?1)",
                params![unit_id],
            )
            .map_err(db_err)?;
        self.conn
            .execute("DELETE FROM renditions WHERE unit_id = ?1", params![unit_id])
            .map_err(db_err)?;
        self.conn
            .execute("DELETE FROM page_bindings WHERE unit_id = ?1", params![unit_id])
            .map_err(db_err)?;
        self.conn
            .execute("DELETE FROM units WHERE id = ?1", params![unit_id])
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
        match rec.kind.as_str() {
            "unit" => {
                self.delete_unit_permanent(oid)?;
            }
            "index" => {
                self.conn
                    .execute(
                        "DELETE FROM index_branches WHERE index_id = ?1",
                        params![oid],
                    )
                    .map_err(db_err)?;
                self.conn
                    .execute(
                        "DELETE FROM index_commits WHERE index_id = ?1",
                        params![oid],
                    )
                    .map_err(db_err)?;
                self.conn
                    .execute("DELETE FROM index_tags WHERE index_id = ?1", params![oid])
                    .map_err(db_err)?;
                self.conn
                    .execute("DELETE FROM indexes_tbl WHERE id = ?1", params![oid])
                    .map_err(db_err)?;
            }
            "showroom" => {
                self.conn
                    .execute(
                        "DELETE FROM library_entries WHERE item_kind = 'showroom' AND item_id = ?1",
                        params![oid],
                    )
                    .map_err(db_err)?;
                self.conn
                    .execute(
                        "DELETE FROM shares WHERE item_kind = 'showroom' AND item_id = ?1",
                        params![oid],
                    )
                    .map_err(db_err)?;
                self.conn
                    .execute("DELETE FROM showroom WHERE id = ?1", params![oid])
                    .map_err(db_err)?;
            }
            other => return Err(format!("unknown trash kind: {other}")),
        }
        self.conn
            .execute("DELETE FROM trash_items WHERE id=?1", params![rec.id])
            .map_err(db_err)?;
        Ok(rec)
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
            let pins = self.showroom_pins(&item.id)?;
            for (pos, e) in commit.entries.iter().enumerate() {
                let pin = pins.get(pos).map(|s| s.as_str());
                if let Some(v) = self.resolve_showroom_entry(e, pin)? {
                    if v.id == version_id {
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

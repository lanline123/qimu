use crate::semver::SemVer;
use serde::{Deserialize, Serialize};

/// Core domain model (spec §3). Identity is always by immutable ID;
/// display numbers (X.Y.Z) may be renumbered by drag-insert into history
/// without breaking any reference (spec §3.5, scenario 46).

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Id(pub String);

impl Id {
    pub fn new(s: impl Into<String>) -> Self {
        Id(s.into())
    }
}

/// A stable topic spot in the library (封面, 公司介绍, 合作伙伴介绍…).
/// Exactly one home folder; one maintainer (person or department).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContentUnit {
    pub id: Id,
    pub name: String,
    pub description: String,
    pub home_folder: String,
    pub maintainer: String,
    pub lead_language: String,
    /// Content-revision axis (r1, r2, …): increments on substantive
    /// fact/data/wording changes only, never on split/merge alone.
    pub current_revision: u64,
}

/// One form × language line of a unit (精简·中文·1页, 详细·英文·3页…).
/// Forms coexist long-term on one content line and are mutually
/// replaceable in place.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Rendition {
    pub id: Id,
    pub unit_id: Id,
    pub form: String,
    pub language: String,
    /// Content revision this rendition line is based on; a rendition on an
    /// older revision than the unit's current one is flagged stale.
    pub basis_revision: u64,
}

/// One immutable published version of a rendition: a small PPTX fragment
/// of 1..N slides. Versions are whole-group snapshots, but every page has
/// a stable slot and its own fingerprint so delivery can be surgical.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VersionRec {
    /// Immutable internal version ID — all references bind by this.
    pub id: Id,
    pub rendition_id: Id,
    /// Current display number; may change on cascading renumber.
    pub number: SemVer,
    pub basis_revision: u64,
    pub author: String,
    pub change_note: String,
    pub pages: Vec<PageSlot>,
    /// Whole-fragment fingerprint (content hash of the stored fragment).
    pub fingerprint: String,
    pub status: VersionStatus,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VersionStatus {
    Normal,
    /// 已撤回 — recall notices are a separate strong category on clients.
    Recalled,
    /// 草稿 / 占位 — not yet formally published; the only state whose
    /// number may be inherited by an in-place replacement (spec 补记三十一).
    Draft,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PageSlot {
    /// Stable per-page slot identity inside the rendition (页位).
    pub slot_id: String,
    pub fingerprint: String,
}

/// An index is a Git-like repository on its directory coordinate
/// (spec §3.10): edit = commit, release = immutable tag, derivation =
/// branch. Language counterparts are mirrors to another repo, never
/// branches.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IndexRepo {
    pub id: Id,
    /// Directory coordinate, e.g. "公司介绍 › 客户拜访" — the repo address.
    pub folder_path: String,
    pub main_branch: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IndexCommit {
    pub id: Id,
    pub parents: Vec<Id>,
    pub author: String,
    pub message: String,
}

/// How one index entry resolves its content block version.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EntryPolicy {
    /// 跟随最新 — resolve to the rendition's latest published version.
    FollowLatest,
    /// 版本锁定 — pinned to one version ID; later versions change nothing
    /// for this entry until a maintainer acts.
    Locked { version_id: Id },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IndexEntry {
    pub unit_id: Id,
    pub rendition_id: Id,
    pub policy: EntryPolicy,
    /// Excerpted-page bindings: when only a subset of a multi-page
    /// version's pages is used, the picked slot IDs are recorded here.
    pub excerpt_slots: Option<Vec<String>>,
}

impl VersionRec {
    pub fn is_latest_among<'a>(self_id: &Id, all: impl Iterator<Item = &'a VersionRec>) -> bool {
        all.filter(|v| v.status == VersionStatus::Normal)
            .max_by_key(|v| v.number)
            .map(|v| &v.id == self_id)
            .unwrap_or(false)
    }
}

//! The local library owned by the client process itself
//! (local-first P0–P4, design §6.2/§7).
//!
//! The client's working data lives in a `qimu_core::store::Store`
//! opened in-process at the local library root — the same Store
//! implementation and the same on-disk format as a server's data
//! root (`qimu.db` + `files/`), so a library previously served by
//! the bundled standalone server is taken over with zero
//! migration. Standalone use needs no server process at all
//! (P4): the client never spawns one, and organization servers
//! are an added layer reached over REST when the user connects
//! one — never the foundation the local library stands on.
//!
//! Scope: the store handle, the library-root resolution, the
//! seeded machine owner + default group, and the JSON adapter
//! layer that feeds the UI the same `Value` shapes the REST list
//! endpoints return. The `dispatch` router below serves the
//! working-data REST paths (import / units / indexes / folders /
//! trash / search / page editing / attachments / exports / index
//! graphs) from this library, so working data is written here,
//! in-process; organization surfaces fall through to REST.

use qimu_core::manifest::{Binding, Manifest, SourceRef};
use qimu_core::pptx::{self, Package};
use qimu_core::store::{FolderDeleteMode, Store, UserRec};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Mutex;

/// Library root resolution (design §6.2, same conventions as
/// `cache_dir()`): the `library_dir` client setting wins; then an
/// explicit `QIMU_DATA_DIR` (the data dir the bundled server would
/// use — naming the same store keeps takeover exact); then an
/// existing bundled-server library beside the executable (recognized
/// by its `qimu.db`, so an empty probe-created folder does not divert
/// the library into a possibly read-only install dir); finally
/// `~/.qimu/library`.
pub fn library_dir() -> PathBuf {
    if let Some(p) = crate::client_settings()
        .get("library_dir")
        .and_then(|v| v.as_str())
        .filter(|s| !s.trim().is_empty())
    {
        return PathBuf::from(p);
    }
    if let Ok(d) = std::env::var("QIMU_DATA_DIR") {
        if !d.trim().is_empty() {
            return PathBuf::from(d);
        }
    }
    let beside = crate::exe_dir().join("qimu-data");
    if beside.join("qimu.db").exists() {
        return beside;
    }
    crate::qimu_home().join("library")
}

/// Serialize a record list into the `Vec<Value>` shape the UI consumes
/// (the REST list endpoints serialize the same core records).
fn values_of<T: serde::Serialize>(recs: Vec<T>) -> Vec<Value> {
    serde_json::to_value(recs)
        .ok()
        .and_then(|v| v.as_array().cloned())
        .unwrap_or_default()
}

/// The in-process local library: one core Store, its seeded machine
/// owner and the default group local content belongs to.
pub struct LocalStore {
    store: Mutex<Store>,
    /// The machine owner seeded into the library (core's standalone
    /// owner). Data preparation only — no account UI is involved.
    pub owner: UserRec,
    /// The 「本机」 default group created on first open.
    pub default_group_id: String,
    /// The library root this store was opened at.
    pub root: PathBuf,
    lock_path: PathBuf,
}

/// Read the holder pid from a lock file (line 1). `None` when the
/// file is missing, unreadable, or carries no parsable pid — e.g. a
/// lock written by the P0 build, which stored only the pid line.
fn read_lock_pid(lock_path: &std::path::Path) -> Option<u32> {
    std::fs::read_to_string(lock_path)
        .ok()?
        .lines()
        .next()?
        .trim()
        .parse()
        .ok()
}

/// Is `pid` still a live process on this machine? Conservative `true`
/// when the check itself cannot run — a lock whose holder cannot be
/// proven dead is never stolen, so the worst case is the old
/// manual-delete error, never two live clients on one library.
#[cfg(target_os = "windows")]
fn pid_alive(pid: u32) -> bool {
    // tasklist ships in System32 on every Windows; /FO CSV /NH keeps
    // the output machine-readable across locales — a live holder
    // yields one CSV row whose second field is the pid, a dead one
    // yields only the localized INFO line. The `,"{pid}",` needle
    // cannot match any other field: image names hold no quotes, and
    // the memory-usage field always carries its " K" suffix.
    let mut cmd = std::process::Command::new("cmd");
    cmd.args([
        "/C",
        "tasklist",
        "/FI",
        &format!("PID eq {pid}"),
        "/FO",
        "CSV",
        "/NH",
    ])
    .stdout(std::process::Stdio::piped())
    .stderr(std::process::Stdio::null());
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    match cmd.output() {
        Ok(out) => {
            let text = String::from_utf8_lossy(&out.stdout);
            text.lines().any(|l| l.contains(&format!(",\"{pid}\",")))
        }
        Err(_) => true,
    }
}

/// Same contract as the Windows half; `kill -0` probes existence
/// without signalling.
#[cfg(not(target_os = "windows"))]
fn pid_alive(pid: u32) -> bool {
    std::process::Command::new("kill")
        .args(["-0", &pid.to_string()])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(true)
}

impl LocalStore {
    /// Open the local library (P4: unconditional — the local face
    /// is the client's own capability, not a gray rollout). The
    /// failure reason rides along so the settings page can show
    /// it instead of only logging to stderr (P0 fix: a lock-held
    /// failure used to surface only as "see the console log").
    /// On failure the client keeps running — organization
    /// connections still work — with the local face unavailable
    /// and the reason on display.
    pub fn open_library() -> (Option<LocalStore>, Option<String>) {
        let root = library_dir();
        match Self::open(&root) {
            Ok(s) => (Some(s), None),
            Err(e) => {
                eprintln!("[qimu] 本地库打开失败（{}）：{e}", root.display());
                (None, Some(e))
            }
        }
    }

    pub fn open(root: &std::path::Path) -> Result<LocalStore, String> {
        std::fs::create_dir_all(root).map_err(|e| e.to_string())?;
        // Advisory lock (design §9 R5, client-side half): one client
        // process owns a library root at a time. The client never
        // spawns a server against this root (P4), so the only
        // possible second opener is another client process — or a
        // bundled server left running from a pre-P4 install, which
        // does not know this lock; the open error names the holder
        // so that case stays visible instead of silent.
        let lock_path = root.join("qimu-local.lock");
        //
        // P0 fix: a lock left behind by a crashed process used to
        // block every later start until the user deleted it by hand.
        // The lock now carries the holder pid (line 1) + start time
        // (line 2); when the file already exists and the recorded
        // holder is provably dead — or is our own pid, which cannot
        // be the holder of a pre-existing lock — the stale lock is
        // removed and acquisition retried (bounded: a live holder
        // racing us back in still wins, and a stale lock that will
        // not go away after 3 tries errors out with its path).
        let mut attempts = 0;
        loop {
            attempts += 1;
            match std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&lock_path)
            {
                Ok(mut f) => {
                    use std::io::Write;
                    let started = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map(|d| d.as_secs())
                        .unwrap_or(0);
                    let _ = writeln!(f, "{}\n{started}", std::process::id());
                    break;
                }
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                    let holder = read_lock_pid(&lock_path);
                    let stale = matches!(holder, Some(pid) if pid == std::process::id() || !pid_alive(pid));
                    if stale {
                        if attempts < 3 {
                            eprintln!(
                                "[qimu] 本地库残留锁已清理（持有者 {:?} 已不在运行）：{}",
                                holder,
                                lock_path.display()
                            );
                            let _ = std::fs::remove_file(&lock_path);
                            continue;
                        }
                        return Err(format!(
                            "本地库锁文件残留（持有者已不在运行），但重试后仍未能取得锁（锁文件 {}）；请删除该锁文件后重试",
                            lock_path.display()
                        ));
                    }
                    return Err(match holder {
                        Some(pid) => format!(
                            "本地库已被另一个启幕进程占用（持有者 PID {pid} 仍在运行；锁文件 {}）；若该进程已不在运行，删除该锁文件后重试",
                            lock_path.display()
                        ),
                        None => format!(
                            "本地库已被另一个启幕进程占用（锁文件 {}，持有者信息不可读）；若确认没有其他启幕在运行，删除该锁文件后重试",
                            lock_path.display()
                        ),
                    });
                }
                Err(e) => return Err(e.to_string()),
            }
        }
        let opened = (|| -> Result<LocalStore, String> {
            let store = Store::open(root)?;
            if store.user_count()? == 0 {
                // First run on a fresh library: seed exactly like a
                // fresh standalone server would. The seed password is
                // a random token that is never displayed or used by
                // any sign-in flow — this is in-process data
                // preparation, not a login (design §3.3).
                store.ensure_seed(&qimu_core::auth::new_token())?;
            }
            let owner = store.seed_owner()?;
            // Default group 「本机」 with the owner holding every
            // capability on it; idempotent across restarts.
            let groups = store.list_groups()?;
            let default_group = match groups.iter().find(|g| g.name == "本机") {
                Some(g) => g.clone(),
                None => store.create_group("本机")?,
            };
            let grants = store.list_grants()?;
            for cap in ["view", "use", "publish", "manage"] {
                let has = grants.iter().any(|g| {
                    g.group_id == default_group.id
                        && g.subject_type == "user"
                        && g.subject_id == owner.id
                        && g.capability == cap
                });
                if !has {
                    store.add_grant(&default_group.id, "user", &owner.id, cap)?;
                }
            }
            Ok(LocalStore {
                store: Mutex::new(store),
                owner,
                default_group_id: default_group.id,
                root: root.to_path_buf(),
                lock_path: lock_path.clone(),
            })
        })();
        if opened.is_err() {
            let _ = std::fs::remove_file(&lock_path);
        }
        opened
    }

    /// GET /v1/folders shape: bare folder records.
    pub fn list_folders(&self) -> Vec<Value> {
        let store = self.store.lock().unwrap();
        store.list_folders().map(values_of).unwrap_or_default()
    }

    /// GET /v1/groups shape: bare group records (the local owner is
    /// the seeded super user, so no visibility filtering applies).
    pub fn list_groups(&self) -> Vec<Value> {
        let store = self.store.lock().unwrap();
        store.list_groups().map(values_of).unwrap_or_default()
    }

    /// GET /v1/indexes shape: bare index records.
    pub fn list_indexes(&self) -> Vec<Value> {
        let store = self.store.lock().unwrap();
        store.list_indexes(None).map(values_of).unwrap_or_default()
    }

    /// GET /v1/units shape: the server's `unit_json` wrapper —
    /// `{unit, renditions: [{rendition, latest, versions}]}`.
    /// (The REST list endpoint leaves `versions` empty and only
    /// its detail endpoint fills it; the local face fills it in
    /// both — P1 — because the version strips read it from the
    /// list data.)
    pub fn list_units(&self) -> Vec<Value> {
        let store = self.store.lock().unwrap();
        let units = store.list_units(None).unwrap_or_default();
        units.iter().map(|u| unit_json(&store, u)).collect()
    }

    /// The version's fragment PPTX bytes from the local store
    /// (the same read the server's thumb and edit-preview handlers
    /// perform). None when the version is not in this library.
    pub fn version_fragment(&self, version_id: &str) -> Option<Vec<u8>> {
        let store = self.store.lock().unwrap();
        let ver = store.get_version(version_id).ok()?;
        store.version_fragment_bytes(&ver).ok()
    }

    /// An attachment's bytes from the local CAS (the read the
    /// server's download handler performs). None when the
    /// attachment is not in this library — the PDF preview's
    /// worker thread then falls back to the server download,
    /// exactly like thumbnails do (T0 pattern).
    pub fn attachment_bytes(&self, att_id: &str) -> Option<Vec<u8>> {
        let store = self.store.lock().unwrap();
        let rec = store.get_attachment(att_id).ok()?;
        store.file_get(&rec.hash).ok()
    }

    /// Assemble one showroom item's deck from this library,
    /// mirroring the server's export funnel step for step (handler
    /// `export_showroom_pdf` + `build_export`): the item's commit
    /// (pinned `commit_id`, else the source index's main head)
    /// supplies the entries, with the item's publish pins applied;
    /// each entry resolves to a version whose
    /// pages are taken from its page-reference sequence and
    /// excerpted; speaker scripts are written
    /// back into the notes pages when `include_notes`; personal
    /// placeholders resolve against the company profile + the
    /// library owner's profile (kind "personal", the only kind the
    /// client requests) and a missing value blocks the export with
    /// the server's message; the manifest is embedded exactly as
    /// the server embeds it. Returns the deck bytes and the deck's
    /// page count.
    ///
    /// `None` when the item (or its source index) is not in this
    /// library at all — the deck is not local and the caller keeps
    /// the server path. `Some(Err)` is a local verdict on local
    /// data; the caller still falls back to the server path, whose
    /// answer on the same data is the same.
    pub fn assemble_showroom_deck(
        &self,
        item_id: &str,
        include_notes: bool,
    ) -> Option<Result<(Vec<u8>, i64), String>> {
        let store = self.store.lock().unwrap();
        let item = match store.get_showroom(item_id) {
            Ok(Some(it)) => it,
            Ok(None) => return None,
            Err(e) => return Some(Err(e)),
        };
        let idx_id = match &item.source_index_id {
            Some(id) => id.clone(),
            None => return None,
        };
        let run = || -> Result<(Vec<u8>, i64), String> {
            let commit = match &item.commit_id {
                Some(cid) => store.get_commit(cid)?,
                None => {
                    let head = store
                        .branch_head(&idx_id, "main")?
                        .ok_or_else(|| "来源索引尚无提交".to_string())?;
                    store.get_commit(&head)?
                }
            };
            // Publish pins applied, mirroring the server's
            // export funnel: the item assembles its confirmed
            // formal version, not today's follow-latest drift.
            let entries = store.pinned_commit_entries(&item, &commit)?;
            let mut pkgs: Vec<Package> = Vec::new();
            let mut bindings: Vec<Binding> = Vec::new();
            let mut binding_scripts: Vec<Vec<String>> = Vec::new();
            for e in &entries {
                let version = store
                    .resolve_entry_version(e)?
                    .ok_or_else(|| {
                        "有条目无法解析到版本（内容块尚无已发布版本）".to_string()
                    })?;
                let singles = store.version_page_packages(&version)?;
                let chosen: Vec<usize> = match &e.excerpt_slots {
                    Some(slots) => {
                        qimu_core::store::excerpt_positions(&version, slots)
                    }
                    None => (0..singles.len()).collect(),
                };
                let mut fps = Vec::new();
                let mut scripts = Vec::new();
                for i in &chosen {
                    if let Some(p) = version.pages.get(*i) {
                        fps.push(p.fp.clone());
                    }
                    scripts.push(version.scripts.get(*i).cloned().unwrap_or_default());
                    pkgs.push(singles[*i].clone());
                }
                binding_scripts.push(scripts);
                bindings.push(Binding {
                    unit_id: e.unit_id.0.clone(),
                    rendition_id: e.rendition_id.0.clone(),
                    version_id: version.id.clone(),
                    version_semver: version.semver.clone(),
                    page_fps: fps,
                });
            }
            if pkgs.is_empty() {
                return Err("没有可导出的页面".into());
            }
            let page_count = pkgs.len() as i64;
            let mut assembled = qimu_core::assemble::assemble(&pkgs)?;
            if include_notes {
                let mut cursor = 0usize;
                for scripts in &binding_scripts {
                    for script in scripts {
                        if !script.is_empty() {
                            qimu_core::pptx::set_notes_text(
                                &mut assembled,
                                cursor,
                                script,
                            );
                        }
                        cursor += 1;
                    }
                }
            }
            // Personal binding (§3.14), kind "personal" with the
            // library owner as the exporter (the server substitutes
            // the requesting user; in the local library that is the
            // owner). Company tokens are mapped exactly as the
            // server maps them; a placeholder with no value blocks
            // the export — never fill with someone else's data.
            let mut values: BTreeMap<String, String> = BTreeMap::new();
            for (k, v) in store.company_profile()? {
                let token = match k.as_str() {
                    "phone" => "company_phone",
                    "website" => "company_website",
                    "address" => "company_address",
                    "gongzhonghao" => "company_gongzhonghao",
                    _ => continue,
                };
                if !v.is_empty() {
                    values.insert(token.to_string(), v);
                }
            }
            values.insert("name".to_string(), self.owner.display_name.clone());
            for (k, v) in store.profile(&self.owner.id)? {
                if !v.is_empty() {
                    values.insert(k, v);
                }
            }
            let missing = qimu_core::personalize::substitute(&mut assembled, &values, &[]);
            if !missing.is_empty() {
                return Err(format!(
                    "导出已拦截：以下占位字段没有值（请先在设置里补全资料）：{}",
                    missing.join("、")
                ));
            }
            let deck = assembled.to_bytes()?;
            let manifest = Manifest {
                deck_id: qimu_core::auth::new_id("deck"),
                kind: "personal".to_string(),
                source: Some(SourceRef {
                    index_id: idx_id.clone(),
                    branch: "main".to_string(),
                    commit_id: commit.id.clone(),
                    tag: None,
                }),
                exported_ms: qimu_core::auth::now_ms(),
                bindings,
            };
            let deck = qimu_core::manifest::embed_manifest(&deck, &manifest)?;
            Ok((deck, page_count))
        };
        Some(run())
    }

    /// Assemble ONE content-block version's deck from this library
    /// (views unification, user ruling 2026-11-25): a version is a
    /// frozen page sequence — an index of one block — so opening it
    /// in the bottom-bar views resolves exactly the way
    /// `assemble_showroom_deck` resolves one entry: the version's
    /// page-reference sequence supplies the page packages in order
    /// and its speaker scripts are written back as notes (scripts
    /// live in the DB and are edited there; the stored fragment is
    /// never rewritten by a script save, so the notes must come
    /// from `version.scripts`, never from the fragment alone).
    ///
    /// Deliberately NOT an export: no manifest is embedded and no
    /// personalization runs — the views open the working-data block
    /// as it stands, placeholders and all, exactly what the page
    /// editor shows. `None` when the version is not in this
    /// library; the caller then takes the REST fragment path.
    pub fn assemble_version_deck(
        &self,
        version_id: &str,
    ) -> Option<Result<(Vec<u8>, i64), String>> {
        let store = self.store.lock().unwrap();
        let version = match store.get_version(version_id) {
            Ok(v) => v,
            Err(_) => return None,
        };
        let run = || -> Result<(Vec<u8>, i64), String> {
            let singles = store.version_page_packages(&version)?;
            if singles.is_empty() {
                return Err("没有可打开的页面".into());
            }
            let page_count = singles.len() as i64;
            let mut assembled = qimu_core::assemble::assemble(&singles)?;
            for (slot, script) in version.scripts.iter().enumerate() {
                if !script.is_empty() {
                    qimu_core::pptx::set_notes_text(&mut assembled, slot, script);
                }
            }
            let deck = assembled.to_bytes()?;
            Ok((deck, page_count))
        };
        Some(run())
    }

    /// Assemble one local index's deck for the bottom-bar views
    /// (views unification, user ruling 2026-11-25): the branch head
    /// commit's entries, resolved and assembled by the very same
    /// `build_deck_local` the index export dispatch arm runs —
    /// same follow/locked resolution, same personalization
    /// verdicts, kind "personal", notes included — but WITHOUT
    /// registering an export record: opening a view is not an
    /// export. `None` when the index is not in this library; the
    /// caller then takes the REST export path.
    pub fn assemble_index_deck(
        &self,
        index_id: &str,
        branch: &str,
    ) -> Option<Result<(Vec<u8>, i64), String>> {
        let store = self.store.lock().unwrap();
        if store.get_index(index_id).is_err() {
            return None;
        }
        let run = || -> Result<(Vec<u8>, i64), String> {
            let head = store
                .branch_head(index_id, branch)?
                .ok_or_else(|| "分支尚无提交".to_string())?;
            let commit = store.get_commit(&head)?;
            let source = SourceRef {
                index_id: index_id.to_string(),
                branch: branch.to_string(),
                commit_id: commit.id.clone(),
                tag: None,
            };
            let (deck, _manifest, page_count) = build_deck_local(
                &store,
                &self.owner,
                &commit.entries,
                Some(source),
                "personal",
                true,
            )?;
            Ok((deck, page_count))
        };
        Some(run())
    }

    /// Build the P3 submission package for one local index at the
    /// exact published tag shown by the push preview. The preview's
    /// entry/version pins keep follow-latest entries from drifting to
    /// a newer version between preview and submission. `None` means
    /// the index is organization-side and uses the legacy reference
    /// submission path.
    pub fn build_submission_package(
        &self,
        index_id: &str,
        published_tag_id: &str,
        pinned_versions: &[(qimu_core::model::IndexEntry, String)],
    ) -> Option<Result<Value, String>> {
        let store = self.store.lock().unwrap();
        let index = match store.get_index(index_id) {
            Ok(i) => i,
            Err(_) => return None,
        };
        let run = || -> Result<Value, String> {
            let tag = store
                .list_tags(index_id)?
                .into_iter()
                .find(|tag| tag.id == published_tag_id)
                .ok_or_else(|| "发布版本不存在或不属于该索引".to_string())?;
            if tag.branch != "main" {
                return Err("只能推送主分支的发布版本".into());
            }
            let commit = store.get_commit(&tag.commit_id)?;
            let mut remaining = pinned_versions.to_vec();
            let mut units: Vec<qimu_core::store::SubmissionUnit> = Vec::new();
            let mut renditions: Vec<qimu_core::store::SubmissionRendition> = Vec::new();
            let mut versions: Vec<qimu_core::store::SubmissionVersion> = Vec::new();
            let mut entries: Vec<qimu_core::store::SubmissionEntry> = Vec::new();
            let mut files = serde_json::Map::new();
            for e in &commit.entries {
                let position = remaining
                    .iter()
                    .position(|(entry, _)| entry == e)
                    .ok_or_else(|| "推送摘要已过期，请重新读取后再提交".to_string())?;
                let (_, version_id) = remaining.remove(position);
                let version = store.get_version(&version_id)?;
                if version.rendition_id != e.rendition_id.0 {
                    return Err("推送摘要与发布版本不匹配，请重新读取后再提交".into());
                }
                if version.status != "normal" {
                    return Err("推送摘要中的内容块版本已失效，请重新发布后再提交".into());
                }
                if !units.iter().any(|u| u.id == e.unit_id.0) {
                    let u = store.get_unit(&e.unit_id.0)?;
                    units.push(qimu_core::store::SubmissionUnit {
                        id: u.id.clone(),
                        name: u.name.clone(),
                        lead_lang: u.lead_lang.clone(),
                        description: u.description.clone(),
                        current_revision: u.current_revision,
                        maintainer: u.maintainer.clone(),
                    });
                }
                if !renditions.iter().any(|r| r.id == e.rendition_id.0) {
                    let r = store.get_rendition(&e.rendition_id.0)?;
                    renditions.push(qimu_core::store::SubmissionRendition {
                        id: r.id.clone(),
                        unit_id: r.unit_id.clone(),
                        form: r.form.clone(),
                        lang: r.lang.clone(),
                    });
                }
                if !versions.iter().any(|v| v.id == version.id) {
                    let bytes = store.version_fragment_bytes(&version)?;
                    files.insert(
                        version.fragment_hash.clone(),
                        Value::String(crate::b64e(&bytes)),
                    );
                    versions.push(qimu_core::store::SubmissionVersion {
                        id: version.id.clone(),
                        rendition_id: version.rendition_id.clone(),
                        semver: version.semver.clone(),
                        basis_revision: version.basis_revision,
                        author: version.author.clone(),
                        note: version.note.clone(),
                        status: version.status.clone(),
                        created_ms: version.created_ms,
                        fragment_hash: version.fragment_hash.clone(),
                        page_count: version.page_count,
                        scripts: version.scripts.clone(),
                    });
                }
                entries.push(qimu_core::store::SubmissionEntry {
                    entry: e.clone(),
                    version_id: version.id.clone(),
                });
            }
            if !remaining.is_empty() {
                return Err("推送摘要与发布版本不匹配，请重新读取后再提交".into());
            }
            let manifest = qimu_core::store::SubmissionManifest {
                format: 1,
                index_name: index.name.clone(),
                index_scope: index.scope.clone(),
                commit: qimu_core::store::SubmissionCommit {
                    branch: "main".to_string(),
                    message: commit.message.clone(),
                    entries,
                },
                units,
                renditions,
                versions,
            };
            Ok(json!({
                "manifest": serde_json::to_value(&manifest).unwrap_or_default(),
                "files": Value::Object(files),
            }))
        };
        Some(run())
    }

    /// Thumbnail cache directory for one cache key, in the
    /// shared layout (`<root>/thumbs/deckcraft-v2/<key>`). Since
    /// the staged-import line (⑤, analysis point on thumbnail
    /// keying) the key is the PAGE ENTITY id: one page renders
    /// once per library and is reused by every version, block and
    /// re-import that references it. Directories keyed by version
    /// id from before the change are simply never found again —
    /// the page re-renders once under its entity key and the old
    /// directory is orphaned (no migration, by design). Only
    /// legacy versions whose pages carry no entity id still key
    /// by version id, through `local_render`'s fallback.
    pub fn thumb_cache_dir(&self, key: &str) -> PathBuf {
        self.root.join("thumbs").join("deckcraft-v2").join(key)
    }

    /// The page entity id of one page of a version in this
    /// library, or None when the version is not local or the page
    /// carries no entity id (a legacy version the migration could
    /// not split).
    pub fn version_page_id(&self, version_id: &str, page: usize) -> Option<String> {
        let store = self.store.lock().unwrap();
        let ver = store.get_version(version_id).ok()?;
        let info = ver.pages.get(page)?;
        if info.id.is_empty() {
            None
        } else {
            Some(info.id.clone())
        }
    }

    /// The stored single-page package bytes of a page entity.
    pub fn page_package_bytes(&self, page_id: &str) -> Option<Vec<u8>> {
        let store = self.store.lock().unwrap();
        let rec = store.get_page(page_id).ok()??;
        store.file_get(&rec.hash).ok()
    }

    /// (units, folders, indexes) counts for the settings diagnostic.
    pub fn counts(&self) -> (usize, usize, usize) {
        let store = self.store.lock().unwrap();
        (
            store.list_units(None).map(|v| v.len()).unwrap_or(0),
            store.list_folders().map(|v| v.len()).unwrap_or(0),
            store.list_indexes(None).map(|v| v.len()).unwrap_or(0),
        )
    }

    /// The index detail the REST `GET /v1/indexes/{id}` returns for
    /// a named session — `{index, branches, tags, head,
    /// resolved_entries}` — assembled from the local store (the
    /// guest entry funnel does not exist locally: the library is
    /// the owner's own). None when the index is not in this
    /// library. Used by the refresh per-index pass and by the
    /// working-data dispatcher below.
    pub fn index_detail(&self, id: &str, branch: &str) -> Option<Value> {
        let store = self.store.lock().unwrap();
        let index = store.get_index(id).ok()?;
        let branches = store.list_branches(id).unwrap_or_default();
        let tags = store.list_tags(id).unwrap_or_default();
        let head = store
            .branch_head(id, branch)
            .ok()
            .flatten()
            .and_then(|cid| store.get_commit(&cid).ok());
        let entries = head.as_ref().map(|c| resolved_entries(&store, c));
        Some(json!({
            "index": serde_json::to_value(&index).unwrap_or(Value::Null),
            "branches": branches,
            "tags": serde_json::to_value(&tags).unwrap_or(Value::Null),
            "head": serde_json::to_value(&head).unwrap_or(Value::Null),
            "resolved_entries": entries,
        }))
    }

    pub fn push_preview(&self, id: &str, branch: &str) -> Option<Result<Value, String>> {
        let store = self.store.lock().unwrap();
        store.get_index(id).ok()?;
        if branch != "main" {
            return Some(Err("只能推送主分支的发布版本".into()));
        }
        let run = || -> Result<Value, String> {
            let mut tags = store
                .list_tags(id)?
                .into_iter()
                .filter(|tag| tag.branch == branch)
                .collect::<Vec<_>>();
            tags.sort_by(|a, b| {
                qimu_core::semver::SemVer::parse(&b.name)
                    .ok()
                    .cmp(&qimu_core::semver::SemVer::parse(&a.name).ok())
            });
            let published = tags.into_iter().next();
            let current = published
                .as_ref()
                .and_then(|tag| store.get_commit(&tag.commit_id).ok());
            Ok(json!({
                "published_version": published.as_ref().map(|tag| tag.name.clone()),
                "published_tag_id": published.as_ref().map(|tag| tag.id.clone()),
                "current_entries": current
                    .as_ref()
                    .map(|commit| resolved_entries(&store, commit))
                    .unwrap_or_default(),
                "previous_entries": [],
            }))
        };
        Some(run())
    }

    /// sdBlockPublish's candidate search (P3 §8 step 3): the
    /// local indexes whose main-branch head commit carries an
    /// entry for `unit_id`, as (index_id, index_name) pairs in
    /// library order. The submission package is built from this
    /// same store, so the search runs here rather than over the
    /// wire-shaped detail endpoints.
    pub fn indexes_with_unit_on_main(&self, unit_id: &str) -> Vec<(String, String)> {
        let store = self.store.lock().unwrap();
        let mut out = Vec::new();
        for index in store.list_indexes(None).unwrap_or_default() {
            let head = store
                .branch_head(&index.id, "main")
                .ok()
                .flatten()
                .and_then(|cid| store.get_commit(&cid).ok());
            if let Some(commit) = head {
                if commit.entries.iter().any(|e| e.unit_id.0 == unit_id) {
                    out.push((index.id.clone(), index.name.clone()));
                }
            }
        }
        out
    }

    /// Working-data dispatcher (local-first P1/P4): serves the REST
    /// paths whose data is working data — import, units, indexes,
    /// folders, trash, search, page editing (page-text / edit-pages
    /// / scripts), attachments, the export registry and index
    /// exports, index graphs — from this local library, with the
    /// exact response shapes and error strings the server handlers
    /// produce (they are thin shells over the same core calls; the
    /// capability checks they add are vacuous for the library's
    /// seeded super owner). Returns None for anything else —
    /// organization surfaces, and the few objects that live on the
    /// server even in local mode (showroom trash rows) — so the
    /// caller falls through to REST unchanged. The arms added in
    /// P4 additionally fall through when the target object is not
    /// in this library at all, so a stray organization object can
    /// never be answered from the wrong store.
    pub fn dispatch(
        &self,
        method: &str,
        path: &str,
        body: Option<&Value>,
    ) -> Option<Result<Value, String>> {
        let (p, q) = match path.split_once('?') {
            Some((p, q)) => (p, q),
            None => (path, ""),
        };
        let seg: Vec<&str> = p.split('/').filter(|s| !s.is_empty()).collect();
        let body = body.cloned().unwrap_or(Value::Null);
        // The route match runs in a closure so its arms can use `?`
        // over the core calls; routes this face does not own — and
        // the server-owned objects above — surface as the UNHANDLED
        // sentinel and become None (REST fallthrough) below.
        let r = (|| -> Result<Value, String> {
            match (method, seg.as_slice()) {
                // ---- import ----
                ("POST", ["v1", "import", "propose"]) => {
                    let bytes = b64_field(&body, "data_base64")?;
                    let pkg = qimu_core::pptx::Package::load(&bytes)?;
                    let report = qimu_core::propose::propose_blocks(&pkg)?;
                    Ok(serde_json::to_value(&report).unwrap_or_default())
                }
                // Staged import, stage 1 (line ⑤): open the deck —
                // pages land as one-page units and the wall can
                // refresh before grouping runs. Local face only:
                // the server's /v1/import stays the one-shot
                // composition of the same two core stages.
                ("POST", ["v1", "import", "open"]) => {
                    let bytes = b64_field(&body, "data_base64")?;
                    let blocks = import_block_specs(&body);
                    let store = self.store.lock().unwrap();
                    let opened = store.import_open(
                        &str_field(&body, "filename").unwrap_or_default(),
                        &bytes,
                        opt_str_field(&body, "folder_id"),
                        &str_field(&body, "group_id").unwrap_or_default(),
                        &self.owner.display_name,
                        blocks.as_deref(),
                    )?;
                    Ok(serde_json::to_value(&opened).unwrap_or_default())
                }
                // Staged import, stage 2: fold the opened deck's
                // wrappers into section / supplied blocks. The
                // opened state travels in the request body (it is
                // plain data; the store keeps no session state).
                ("POST", ["v1", "import", "group"]) => {
                    let opened: qimu_core::import::ImportOpened =
                        serde_json::from_value(body.get("opened").cloned().unwrap_or_default())
                            .map_err(|e| format!("打开状态无效：{e}"))?;
                    let blocks = import_block_specs(&body);
                    let store = self.store.lock().unwrap();
                    let outcome = store.import_group(
                        &opened,
                        opt_str_field(&body, "folder_id"),
                        &str_field(&body, "group_id").unwrap_or_default(),
                        &self.owner.display_name,
                        blocks,
                    )?;
                    Ok(json!({
                        "ok": true,
                        "slide_count": outcome.slide_count,
                        "fingerprints_match": outcome.fingerprints_match,
                        "units": outcome.units,
                    }))
                }
                ("POST", ["v1", "import"]) => {
                    let bytes = b64_field(&body, "data_base64")?;
                    let blocks = import_block_specs(&body);
                    let store = self.store.lock().unwrap();
                    let outcome = store.import_blocks(
                        &str_field(&body, "filename").unwrap_or_default(),
                        &bytes,
                        opt_str_field(&body, "folder_id"),
                        &str_field(&body, "group_id").unwrap_or_default(),
                        &self.owner.display_name,
                        blocks,
                    )?;
                    Ok(json!({
                        "ok": true,
                        "slide_count": outcome.slide_count,
                        "fingerprints_match": outcome.fingerprints_match,
                        "units": outcome.units,
                    }))
                }
                // ---- units ----
                ("POST", ["v1", "units"]) => {
                    let store = self.store.lock().unwrap();
                    let u = store.create_unit(
                        &str_field(&body, "name").unwrap_or_default(),
                        opt_str_field(&body, "folder_id"),
                        &str_field(&body, "group_id").unwrap_or_default(),
                        &str_field(&body, "maintainer")
                            .unwrap_or_else(|| self.owner.display_name.clone()),
                        &str_field(&body, "lead_lang").unwrap_or_else(|| "zh".to_string()),
                        &str_field(&body, "description").unwrap_or_default(),
                    )?;
                    Ok(serde_json::to_value(&u).unwrap_or(Value::Null))
                }
                ("GET", ["v1", "units", id]) => {
                    let store = self.store.lock().unwrap();
                    let unit = store.get_unit(id).map_err(|_| NOT_FOUND.to_string())?;
                    Ok(unit_json(&store, &unit))
                }
                ("PATCH", ["v1", "units", id]) => {
                    // Header meta edits (rename / intro): mirrors
                    // the server's patch_unit — absent fields keep
                    // their values; folder_id keeps the server's
                    // double-option semantics (absent = keep,
                    // null = clear, string = set).
                    let store = self.store.lock().unwrap();
                    let folder_id = match body.get("folder_id") {
                        None => None,
                        Some(Value::Null) => Some(None),
                        Some(v) => Some(v.as_str()),
                    };
                    let u = store.update_unit(
                        id,
                        str_field(&body, "name").as_deref(),
                        folder_id,
                        str_field(&body, "maintainer").as_deref(),
                        str_field(&body, "description").as_deref(),
                    )?;
                    Ok(serde_json::to_value(&u).unwrap_or(Value::Null))
                }
                ("POST", ["v1", "units", id, "publish"]) => {
                    let store = self.store.lock().unwrap();
                    let unit = store.get_unit(id).map_err(|_| NOT_FOUND.to_string())?;
                    let frag = b64_field(&body, "data_base64")?;
                    // sanity: it must be a valid PPTX fragment
                    qimu_core::pptx::Package::load(&frag)?;
                    let basis = if body
                        .get("new_revision")
                        .and_then(|v| v.as_bool())
                        .unwrap_or(false)
                    {
                        store.bump_unit_revision(id)?
                    } else {
                        unit.current_revision
                    };
                    let rend = store.get_or_create_rendition(
                        id,
                        &str_field(&body, "form").unwrap_or_else(|| "标准".to_string()),
                        &str_field(&body, "lang").unwrap_or_else(|| "zh".to_string()),
                    )?;
                    let latest = store.latest_version(&rend.id)?;
                    let inc = match str_field(&body, "bump").as_deref() {
                        Some("major") => qimu_core::Increment::Major,
                        Some("patch") => qimu_core::Increment::Patch,
                        _ => qimu_core::Increment::Minor,
                    };
                    let next = match latest {
                        Some(v) => qimu_core::SemVer::parse(&v.semver)?.bumped(inc),
                        None => qimu_core::SemVer::new(1, 0, 0),
                    };
                    let ver = store.insert_version(
                        &rend.id,
                        next,
                        basis,
                        &self.owner.display_name,
                        &str_field(&body, "note").unwrap_or_default(),
                        &frag,
                        &str_field(&body, "status").unwrap_or_else(|| "normal".to_string()),
                    )?;
                    Ok(serde_json::to_value(&ver).unwrap_or(Value::Null))
                }
                ("GET", ["v1", "units", id, "page-bindings"]) => {
                    let store = self.store.lock().unwrap();
                    store.get_unit(id).map_err(|_| NOT_FOUND.to_string())?;
                    let bindings = store.list_page_bindings(id)?;
                    Ok(json!({ "bindings": bindings }))
                }
                ("POST", ["v1", "units", id, "page-bindings"]) => {
                    let store = self.store.lock().unwrap();
                    store.get_unit(id).map_err(|_| NOT_FOUND.to_string())?;
                    let b = store.add_page_binding(
                        id,
                        &str_field(&body, "from_version_id").unwrap_or_default(),
                        body.get("from_slot").and_then(|v| v.as_i64()).unwrap_or(0),
                        &str_field(&body, "to_version_id").unwrap_or_default(),
                        body.get("to_slot").and_then(|v| v.as_i64()).unwrap_or(0),
                        &str_field(&body, "kind").unwrap_or_else(|| "manual".to_string()),
                        &self.owner.display_name,
                    )?;
                    Ok(serde_json::to_value(&b).unwrap_or(Value::Null))
                }
                ("POST", ["v1", "units", id, "page-bindings", bid, "delete"]) => {
                    let store = self.store.lock().unwrap();
                    store.get_unit(id).map_err(|_| NOT_FOUND.to_string())?;
                    store.delete_page_binding(id, bid)?;
                    Ok(json!({ "ok": true }))
                }
                // ---- versions (working-data reads/writes) ----
                ("GET", ["v1", "versions", id, "fragment"]) => {
                    let store = self.store.lock().unwrap();
                    let ver = store.get_version(id).map_err(|_| NOT_FOUND.to_string())?;
                    let bytes = store.version_fragment_bytes(&ver)?;
                    Ok(json!({
                        "version": serde_json::to_value(&ver).unwrap_or(Value::Null),
                        "data_base64": crate::b64e(&bytes),
                    }))
                }
                ("POST", ["v1", "versions", id, "recall"]) => {
                    let store = self.store.lock().unwrap();
                    store.get_version(id).map_err(|_| NOT_FOUND.to_string())?;
                    store.set_version_status(id, "recalled")?;
                    Ok(json!({ "ok": true }))
                }
                ("GET", ["v1", "versions", id, "page-text"]) => {
                    // P4: the page editor's read side (server
                    // `version_page_text`): the runs + element model
                    // of one page, parsed from the local fragment
                    // with the same core calls. A version this
                    // library does not hold falls through to REST.
                    let page: usize = query_param(q, "page")
                        .and_then(|p| p.parse().ok())
                        .unwrap_or(0);
                    let store = self.store.lock().unwrap();
                    let ver = match store.get_version(id) {
                        Ok(v) => v,
                        Err(_) => return Err(UNHANDLED.to_string()),
                    };
                    let bytes = store.version_fragment_bytes(&ver)?;
                    let pkg = Package::load(&bytes)?;
                    let runs = qimu_core::pptx::slide_text_runs(&pkg, page)?;
                    let (slide_cx, slide_cy) = qimu_core::pptx::slide_size(&pkg);
                    let elements =
                        qimu_core::pptx::slide_elements(&pkg, page).unwrap_or_default();
                    Ok(json!({
                        "page": page,
                        "runs": runs,
                        "slide_cx": slide_cx,
                        "slide_cy": slide_cy,
                        "elements": serde_json::to_value(&elements).unwrap_or(json!([])),
                    }))
                }
                ("PUT", ["v1", "versions", id, "scripts"]) => {
                    // P4: speaker-scripts save (server
                    // `put_version_scripts`); never bumps a version.
                    let store = self.store.lock().unwrap();
                    if store.get_version(id).is_err() {
                        return Err(UNHANDLED.to_string());
                    }
                    let scripts: Vec<String> = body
                        .get("scripts")
                        .and_then(|v| v.as_array())
                        .map(|arr| {
                            arr.iter()
                                .map(|x| x.as_str().unwrap_or("").to_string())
                                .collect()
                        })
                        .unwrap_or_default();
                    store.set_version_scripts(id, &scripts)?;
                    Ok(json!({ "ok": true }))
                }
                ("POST", ["v1", "versions", id, "edit-pages"]) => {
                    // P4: the page editor's save (server
                    // `edit_version_pages`, minus its preview_page
                    // branch — previews never travel through the
                    // dispatcher; page_editor's in-process preview
                    // owns them). Applies the request to the local
                    // fragment and inserts the bumped version, with
                    // the server's exact verdicts. A version this
                    // library does not hold falls through to REST.
                    if body.get("preview_page").is_some() {
                        return Err(UNHANDLED.to_string());
                    }
                    let edits_v = body.get("edits").and_then(|v| v.as_array());
                    let ops_v = body.get("ops").and_then(|v| v.as_array());
                    if edits_v.map(|a| a.len()).unwrap_or(0) > 20_000
                        || ops_v.map(|a| a.len()).unwrap_or(0) > 20_000
                    {
                        return Err("编辑操作过多".to_string());
                    }
                    let store = self.store.lock().unwrap();
                    let ver = match store.get_version(id) {
                        Ok(v) => v,
                        Err(_) => return Err(UNHANDLED.to_string()),
                    };
                    let rend = store.get_rendition(&ver.rendition_id)?;
                    let unit = store.get_unit(&rend.unit_id)?;
                    let bytes = store.version_fragment_bytes(&ver)?;
                    let added_pages = body
                        .get("added_pages")
                        .and_then(|v| v.as_u64())
                        .unwrap_or(0) as usize;
                    let mut pkg = Package::load(&bytes)?;
                    qimu_core::pptx::append_blank_slides(&mut pkg, added_pages)?;
                    let mut by_page: std::collections::BTreeMap<
                        usize,
                        Vec<(usize, String)>,
                    > = std::collections::BTreeMap::new();
                    if let Some(edits) = edits_v {
                        for e in edits {
                            let page =
                                e.get("page").and_then(|v| v.as_u64()).unwrap_or(0) as usize;
                            let run =
                                e.get("run").and_then(|v| v.as_u64()).unwrap_or(0) as usize;
                            let text = str_field(e, "text").unwrap_or_default();
                            by_page.entry(page).or_default().push((run, text));
                        }
                    }
                    let mut applied = added_pages;
                    for (page, edits) in &by_page {
                        applied +=
                            qimu_core::pptx::replace_slide_text_runs(&mut pkg, *page, edits)?;
                    }
                    let empty_ops: Vec<Value> = Vec::new();
                    let ops_by_page = crate::page_editor::slide_ops_from_json(
                        ops_v.unwrap_or(&empty_ops),
                    )?;
                    for (page, ops) in &ops_by_page {
                        applied += qimu_core::pptx::apply_slide_ops(&mut pkg, *page, ops)?;
                    }
                    if applied == 0 {
                        return Err("没有可应用的修改".to_string());
                    }
                    let pages = qimu_core::pptx::slides(&pkg)?.len();
                    let scripts: Vec<String> = match body.get("scripts") {
                        Some(sv) => {
                            let arr: Vec<String> = sv
                                .as_array()
                                .map(|a| {
                                    a.iter()
                                        .map(|x| x.as_str().unwrap_or("").to_string())
                                        .collect()
                                })
                                .unwrap_or_default();
                            if arr.len() != pages {
                                return Err("讲稿数量与页面数量不匹配".to_string());
                            }
                            arr
                        }
                        None => {
                            let mut sc = ver.scripts.clone();
                            sc.resize(pages, String::new());
                            sc
                        }
                    };
                    let frag = pkg.to_bytes()?;
                    let basis = if body
                        .get("new_revision")
                        .and_then(|v| v.as_bool())
                        .unwrap_or(false)
                    {
                        store.bump_unit_revision(&unit.id)?
                    } else {
                        unit.current_revision
                    };
                    let latest = store.latest_version(&rend.id)?;
                    let inc = match str_field(&body, "bump").as_deref() {
                        Some("major") => qimu_core::Increment::Major,
                        Some("patch") => qimu_core::Increment::Patch,
                        _ => qimu_core::Increment::Minor,
                    };
                    let next = match latest {
                        Some(v) => qimu_core::SemVer::parse(&v.semver)?.bumped(inc),
                        None => qimu_core::SemVer::new(1, 0, 0),
                    };
                    let new_ver = store.insert_version_with_scripts(
                        &rend.id,
                        next,
                        basis,
                        &self.owner.display_name,
                        &str_field(&body, "note").unwrap_or_default(),
                        &frag,
                        "normal",
                        Some(&scripts),
                    )?;
                    Ok(json!({
                        "version": serde_json::to_value(&new_ver).unwrap_or(Value::Null)
                    }))
                }
                // ---- folders ----
                ("GET", ["v1", "folders"]) => {
                    let store = self.store.lock().unwrap();
                    // ?scope= limits the list to one sidebar tree
                    // (sidebar isolation); without it the full
                    // list is served, every row carrying its scope.
                    let folders = match query_param(q, "scope") {
                        Some(scope) => store.list_folders_in(&scope),
                        None => store.list_folders(),
                    }
                    .map(values_of)
                    .unwrap_or_default();
                    Ok(json!({ "folders": folders }))
                }
                ("POST", ["v1", "folders"]) => {
                    let store = self.store.lock().unwrap();
                    let f = store.create_folder(
                        &str_field(&body, "name").unwrap_or_default(),
                        opt_str_field(&body, "parent_id"),
                        opt_str_field(&body, "scope").unwrap_or("showroom"),
                    )?;
                    Ok(serde_json::to_value(&f).unwrap_or(Value::Null))
                }
                ("POST", ["v1", "folders", id, "rename"]) => {
                    let store = self.store.lock().unwrap();
                    store.rename_folder(id, &str_field(&body, "name").unwrap_or_default())?;
                    Ok(json!({ "ok": true }))
                }
                ("POST", ["v1", "folders", id, "note"]) => {
                    let store = self.store.lock().unwrap();
                    store.set_folder_note(id, &str_field(&body, "note").unwrap_or_default())?;
                    Ok(json!({ "ok": true }))
                }
                ("POST", ["v1", "folders", id, "move"]) => {
                    let store = self.store.lock().unwrap();
                    store.move_folder(
                        id,
                        opt_str_field(&body, "parent_id"),
                        opt_str_field(&body, "before_id"),
                    )?;
                    Ok(json!({ "ok": true }))
                }
                ("DELETE", ["v1", "folders", id]) => {
                    // Compatibility face of POST .../delete with
                    // mode "lift" (see the server route).
                    let store = self.store.lock().unwrap();
                    store.delete_folder(id, FolderDeleteMode::Lift, &self.owner.display_name)?;
                    Ok(json!({ "ok": true }))
                }
                ("POST", ["v1", "folders", id, "delete"]) => {
                    // The delete dialog's two shapes (user ruling
                    // 2026-11-24): "lift" keeps the contents one
                    // level up; "trash" sends the subtree's
                    // contents to the trash. Trash rows record the
                    // owner the same way this face's single-item
                    // trash path does (display name).
                    let mode = match str_field(&body, "mode").as_deref() {
                        Some("lift") => FolderDeleteMode::Lift,
                        Some("trash") => FolderDeleteMode::Trash,
                        other => {
                            return Err(format!(
                                "unknown delete mode: {}",
                                other.unwrap_or_default()
                            ))
                        }
                    };
                    let store = self.store.lock().unwrap();
                    store.delete_folder(id, mode, &self.owner.display_name)?;
                    Ok(json!({ "ok": true }))
                }
                ("POST", ["v1", "folders", id, "color"]) => {
                    let store = self.store.lock().unwrap();
                    store.set_folder_color(id, opt_str_field(&body, "color"))?;
                    let folder = store
                        .list_folders()
                        .unwrap_or_default()
                        .into_iter()
                        .find(|f| f.id == *id);
                    Ok(serde_json::to_value(&folder).unwrap_or(Value::Null))
                }
                // ---- trash ----
                ("GET", ["v1", "trash"]) => {
                    let store = self.store.lock().unwrap();
                    let rows = store.trash_list()?;
                    let mut items = Vec::new();
                    for row in rows {
                        let (cover, pages) = trash_cover(&store, &row);
                        let mut v = serde_json::to_value(&row).unwrap_or(Value::Null);
                        v["cover_version_id"] = json!(cover);
                        v["page_count"] = json!(pages);
                        items.push(v);
                    }
                    Ok(json!({ "items": items }))
                }
                ("POST", ["v1", "trash"]) => {
                    let kind = str_field(&body, "kind").unwrap_or_default();
                    let object_id = str_field(&body, "object_id").unwrap_or_default();
                    if kind == "showroom" {
                        // Showroom items live on the organization
                        // server even in local mode; their trash
                        // rows are the server's, not this library's.
                        return Err(UNHANDLED.to_string());
                    }
                    let store = self.store.lock().unwrap();
                    let (title, origin) = match kind.as_str() {
                        "index" => {
                            let idx = store
                                .get_index(&object_id)
                                .map_err(|_| NOT_FOUND.to_string())?;
                            (
                                idx.name.clone(),
                                trash_origin(&store, idx.folder_id.as_deref(), "索引页"),
                            )
                        }
                        "unit" => {
                            let unit = store
                                .get_unit(&object_id)
                                .map_err(|_| NOT_FOUND.to_string())?;
                            (
                                unit.name.clone(),
                                trash_origin(&store, unit.folder_id.as_deref(), "页面"),
                            )
                        }
                        _ => return Err("unknown trash kind".to_string()),
                    };
                    let rec = store.trash_add(
                        &kind,
                        &object_id,
                        &title,
                        &origin,
                        &self.owner.display_name,
                    )?;
                    Ok(serde_json::to_value(&rec).unwrap_or(Value::Null))
                }
                ("POST", ["v1", "trash", "purge-all"]) => {
                    let store = self.store.lock().unwrap();
                    let rows = store.trash_list()?;
                    let mut purged = 0usize;
                    for row in rows {
                        if store.trash_purge(&row.id).is_ok() {
                            purged += 1;
                        }
                    }
                    Ok(json!({ "purged": purged }))
                }
                ("POST", ["v1", "trash", id, "restore"]) => {
                    let store = self.store.lock().unwrap();
                    if store.trash_get(id).is_err() {
                        // Not one of this library's rows — an
                        // organization row the server owns.
                        return Err(UNHANDLED.to_string());
                    }
                    let rec = store.trash_restore(id)?;
                    Ok(serde_json::to_value(&rec).unwrap_or(Value::Null))
                }
                ("POST", ["v1", "trash", id, "purge"]) => {
                    let store = self.store.lock().unwrap();
                    if store.trash_get(id).is_err() {
                        return Err(UNHANDLED.to_string());
                    }
                    let rec = store.trash_purge(id)?;
                    Ok(serde_json::to_value(&rec).unwrap_or(Value::Null))
                }
                // ---- search ----
                ("GET", ["v1", "search"]) => {
                    let query = query_param(q, "q").unwrap_or_default();
                    let store = self.store.lock().unwrap();
                    let hits = store.search(&query, None)?;
                    let out: Vec<Value> = hits
                        .iter()
                        .map(|(u, v)| json!({ "unit": u, "version": v }))
                        .collect();
                    Ok(json!({ "hits": out }))
                }
                // ---- indexes ----
                ("GET", ["v1", "indexes", id]) => {
                    let branch = query_param(q, "branch").unwrap_or_else(|| "main".to_string());
                    match self.index_detail(id, &branch) {
                        Some(d) => Ok(d),
                        None => Err(NOT_FOUND.to_string()),
                    }
                }
                ("GET", ["v1", "indexes", id, "push-preview"]) => {
                    let branch = query_param(q, "branch").unwrap_or_else(|| "main".to_string());
                    match self.push_preview(id, &branch) {
                        Some(preview) => preview,
                        None => Err(NOT_FOUND.to_string()),
                    }
                }
                ("GET", ["v1", "indexes", id, "commits"]) => {
                    let branch = query_param(q, "branch").unwrap_or_else(|| "main".to_string());
                    let store = self.store.lock().unwrap();
                    let commits = store.list_commits(id, &branch)?;
                    Ok(json!({ "commits": commits }))
                }
                ("POST", ["v1", "indexes"]) => {
                    let entries: Vec<qimu_core::model::IndexEntry> = body
                        .get("entries")
                        .and_then(|e| e.as_array())
                        .map(|arr| {
                            arr.iter()
                                .map(|e| qimu_core::model::IndexEntry {
                                    unit_id: qimu_core::model::Id(
                                        str_field(e, "unit_id").unwrap_or_default(),
                                    ),
                                    rendition_id: qimu_core::model::Id(
                                        str_field(e, "rendition_id").unwrap_or_default(),
                                    ),
                                    policy: qimu_core::model::EntryPolicy::FollowLatest,
                                    locked_semver: None,
                                    excerpt_slots: None,
                                })
                                .collect()
                        })
                        .unwrap_or_default();
                    let store = self.store.lock().unwrap();
                    let idx = store.create_index(
                        &str_field(&body, "name").unwrap_or_default(),
                        opt_str_field(&body, "folder_id"),
                        &str_field(&body, "group_id").unwrap_or_default(),
                        &self.owner.display_name,
                        &str_field(&body, "scope").unwrap_or_else(|| "company".to_string()),
                        entries,
                    )?;
                    Ok(serde_json::to_value(&idx).unwrap_or(Value::Null))
                }
                ("POST", ["v1", "indexes", id, "commits"]) => {
                    let entries: Vec<qimu_core::model::IndexEntry> = serde_json::from_value(
                        body.get("entries").cloned().unwrap_or(json!([])),
                    )
                    .map_err(|e| e.to_string())?;
                    let store = self.store.lock().unwrap();
                    let c = store.create_commit(
                        id,
                        &str_field(&body, "branch").unwrap_or_else(|| "main".to_string()),
                        &self.owner.display_name,
                        &str_field(&body, "message").unwrap_or_default(),
                        entries,
                    )?;
                    Ok(serde_json::to_value(&c).unwrap_or(Value::Null))
                }
                ("POST", ["v1", "indexes", id, "tags"]) => {
                    let store = self.store.lock().unwrap();
                    let t = store.create_tag(
                        id,
                        &str_field(&body, "branch").unwrap_or_else(|| "main".to_string()),
                        &str_field(&body, "name").unwrap_or_default(),
                    )?;
                    Ok(serde_json::to_value(&t).unwrap_or(Value::Null))
                }
                ("POST", ["v1", "indexes", id, "branches"]) => {
                    let store = self.store.lock().unwrap();
                    store.create_branch(
                        id,
                        &str_field(&body, "name").unwrap_or_default(),
                        &str_field(&body, "from").unwrap_or_else(|| "main".to_string()),
                    )?;
                    Ok(json!({ "ok": true }))
                }
                ("POST", ["v1", "indexes", id, "merge", "preview"]) => {
                    let store = self.store.lock().unwrap();
                    let p = store.preview_merge(
                        id,
                        &str_field(&body, "from").unwrap_or_default(),
                        &str_field(&body, "into").unwrap_or_else(|| "main".to_string()),
                    )?;
                    Ok(serde_json::to_value(&p).unwrap_or(Value::Null))
                }
                ("POST", ["v1", "indexes", id, "merge"]) => {
                    let resolutions: std::collections::HashMap<String, String> = body
                        .get("resolutions")
                        .and_then(|r| r.as_object())
                        .map(|m| {
                            m.iter()
                                .filter_map(|(k, v)| {
                                    v.as_str().map(|s| (k.clone(), s.to_string()))
                                })
                                .collect()
                        })
                        .unwrap_or_default();
                    let store = self.store.lock().unwrap();
                    let c = store.execute_merge(
                        id,
                        &str_field(&body, "from").unwrap_or_default(),
                        &str_field(&body, "into").unwrap_or_else(|| "main".to_string()),
                        &self.owner.display_name,
                        &resolutions,
                    )?;
                    Ok(json!({ "commit": c }))
                }
                ("POST", ["v1", "indexes", id, "folder"]) => {
                    let store = self.store.lock().unwrap();
                    store.move_index_to_folder(id, opt_str_field(&body, "folder_id"))?;
                    Ok(json!({ "ok": true }))
                }
                ("POST", ["v1", "indexes", id, "lifecycle"]) => {
                    let store = self.store.lock().unwrap();
                    store.set_index_lifecycle(
                        id,
                        &str_field(&body, "lifecycle").unwrap_or_default(),
                    )?;
                    Ok(json!({ "ok": true }))
                }
                ("POST", ["v1", "indexes", id, "english"]) => {
                    let store = self.store.lock().unwrap();
                    let index = store.get_index(id).map_err(|_| NOT_FOUND.to_string())?;
                    let name = str_field(&body, "name").unwrap_or_default();
                    let name = name.trim();
                    if name.is_empty() {
                        return Err("请输入英文版索引名称".to_string());
                    }
                    let branch =
                        str_field(&body, "branch").unwrap_or_else(|| "main".to_string());
                    let derived =
                        store.derive_english_index(&self.owner, &index, &branch, name)?;
                    Ok(serde_json::to_value(&derived).unwrap_or(Value::Null))
                }
                // ---- attachments (P4) ----
                ("POST", ["v1", "attachments"]) => {
                    // Attachments hang off units and indexes — this
                    // library's working data. A parent this library
                    // does not hold falls through to REST.
                    let parent_id = str_field(&body, "parent_id").unwrap_or_default();
                    let store = self.store.lock().unwrap();
                    if store.get_unit(&parent_id).is_err()
                        && store.get_index(&parent_id).is_err()
                    {
                        return Err(UNHANDLED.to_string());
                    }
                    let bytes = b64_field(&body, "data_base64")?;
                    let rec = store.insert_attachment(
                        &str_field(&body, "parent_type").unwrap_or_default(),
                        &parent_id,
                        &str_field(&body, "filename").unwrap_or_default(),
                        &bytes,
                        &self.owner.display_name,
                    )?;
                    Ok(serde_json::to_value(&rec).unwrap_or(Value::Null))
                }
                ("GET", ["v1", "attachments"]) => {
                    let parent = query_param(q, "parent_id").unwrap_or_default();
                    let store = self.store.lock().unwrap();
                    if store.get_unit(&parent).is_err()
                        && store.get_index(&parent).is_err()
                    {
                        return Err(UNHANDLED.to_string());
                    }
                    let rows = store.list_attachments(&parent)?;
                    Ok(json!({ "attachments": values_of(rows) }))
                }
                ("GET", ["v1", "attachments", id, "download"]) => {
                    let store = self.store.lock().unwrap();
                    let rec = match store.get_attachment(id) {
                        Ok(r) => r,
                        Err(_) => return Err(UNHANDLED.to_string()),
                    };
                    let bytes = store.file_get(&rec.hash)?;
                    Ok(json!({
                        "filename": rec.filename,
                        "data_base64": crate::b64e(&bytes),
                    }))
                }
                // ---- export registry + index export (P4) ----
                ("GET", ["v1", "exports"]) => {
                    let store = self.store.lock().unwrap();
                    let rows =
                        store.list_exports(query_param(q, "index_id").as_deref())?;
                    Ok(json!({ "exports": values_of(rows) }))
                }
                ("POST", ["v1", "indexes", id, "export"]) => {
                    // The index export the detail page's 导出 runs
                    // (server `export_index`): resolve the commit
                    // (a tag on the branch, else the branch head),
                    // assemble in-process, register the export and
                    // return the deck inline — same response shape,
                    // same personalization verdicts.
                    let store = self.store.lock().unwrap();
                    if store.get_index(id).is_err() {
                        return Err(UNHANDLED.to_string());
                    }
                    let branch =
                        str_field(&body, "branch").unwrap_or_else(|| "main".to_string());
                    let commit = if let Some(tag_name) = str_field(&body, "tag") {
                        let tags = store.list_tags(id)?;
                        let tag = tags
                            .iter()
                            .find(|t| t.name == tag_name && t.branch == branch)
                            .ok_or_else(|| "标签不存在".to_string())?;
                        store.get_commit(&tag.commit_id)?
                    } else {
                        let head = store
                            .branch_head(id, &branch)?
                            .ok_or_else(|| "分支尚无提交".to_string())?;
                        store.get_commit(&head)?
                    };
                    let kind =
                        str_field(&body, "kind").unwrap_or_else(|| "personal".to_string());
                    let include_notes = body
                        .get("include_notes")
                        .and_then(|v| v.as_bool())
                        .unwrap_or(false);
                    let source = SourceRef {
                        index_id: id.to_string(),
                        branch: branch.clone(),
                        commit_id: commit.id.clone(),
                        tag: str_field(&body, "tag"),
                    };
                    let (deck, manifest, page_count) = build_deck_local(
                        &store,
                        &self.owner,
                        &commit.entries,
                        Some(source),
                        &kind,
                        include_notes,
                    )?;
                    let export_id = qimu_core::auth::new_id("exp");
                    let deck_hash = store.file_put(&deck)?;
                    store.insert_export(&qimu_core::store::ExportRec {
                        id: export_id.clone(),
                        index_id: id.to_string(),
                        branch: branch.clone(),
                        commit_id: commit.id.clone(),
                        tag: str_field(&body, "tag"),
                        user_id: self.owner.id.clone(),
                        created_ms: qimu_core::auth::now_ms(),
                        deck_hash,
                        page_count,
                        kind: kind.clone(),
                        deck_id: manifest.deck_id.clone(),
                        manifest_json: serde_json::to_string(&manifest)
                            .unwrap_or_default(),
                    })?;
                    Ok(json!({
                        "export_id": export_id,
                        "page_count": page_count,
                        "manifest": serde_json::to_value(&manifest)
                            .unwrap_or(Value::Null),
                        "data_base64": crate::b64e(&deck),
                    }))
                }
                // ---- index graphs (P4) ----
                ("GET", ["v1", "index-graphs"]) => {
                    let store = self.store.lock().unwrap();
                    let mut graphs = std::collections::BTreeMap::new();
                    if let Some(id) = query_param(q, "index_id") {
                        if store.get_index(&id).is_err() {
                            return Err(UNHANDLED.to_string());
                        }
                        graphs.insert(id.clone(), store.index_graph(&id)?);
                    } else {
                        for index in store.list_indexes(None)? {
                            let graph = store.index_graph(&index.id)?;
                            graphs.insert(index.id.clone(), graph);
                        }
                    }
                    Ok(json!({
                        "graphs": serde_json::to_value(&graphs).unwrap_or_default()
                    }))
                }
                _ => Err(UNHANDLED.to_string()),
            }
        })();
        match r {
            Err(e) if e == UNHANDLED => None,
            other => Some(other),
        }
    }
}

/// Sentinel for "the local face does not own this call" inside
/// the dispatcher closure — a control-character prefix makes a
/// collision with a real core error message impossible.
const UNHANDLED: &str = "\u{0}qimu-local-unhandled";

/// The server's 404 message, verbatim — the local face surfaces
/// the same string the REST path puts in the `error` field.
const NOT_FOUND: &str = "不存在";

/// A string field of a request body (`None` when absent or not a
/// string).
fn str_field(body: &Value, key: &str) -> Option<String> {
    body.get(key).and_then(|v| v.as_str()).map(|s| s.to_string())
}

/// A nullable string field: JSON null and absence both read as
/// `None`, a string as `Some`.
fn opt_str_field<'a>(body: &'a Value, key: &str) -> Option<&'a str> {
    body.get(key).and_then(|v| v.as_str())
}

/// Decode a base64 body field, with the server handler's error
/// wording.
/// The `blocks` array of an import request body, in core's
/// `ImportBlockSpec` shape (shared by the one-shot import arm
/// and the staged open arm's plan hint).
fn import_block_specs(body: &Value) -> Option<Vec<qimu_core::import::ImportBlockSpec>> {
    body.get("blocks").and_then(|b| b.as_array()).map(|arr| {
        arr.iter()
            .map(|b| qimu_core::import::ImportBlockSpec {
                name: str_field(b, "name").unwrap_or_default(),
                pages: b
                    .get("pages")
                    .and_then(|v| v.as_array())
                    .map(|a| {
                        a.iter()
                            .filter_map(|x| x.as_u64().map(|n| n as usize))
                            .collect()
                    })
                    .unwrap_or_default(),
                form: str_field(b, "form"),
                lang: str_field(b, "lang"),
            })
            .collect()
    })
}

fn b64_field(body: &Value, key: &str) -> Result<Vec<u8>, String> {
    let s = str_field(body, key).unwrap_or_default();
    crate::b64d(&s).map_err(|e| format!("base64 解码失败: {e}"))
}

/// Assemble a deck from commit entries entirely in-process —
/// the server's `build_export` with the library owner as the
/// exporter (P4; the showroom face keeps its own copy in
/// `assemble_showroom_deck`, T1). Personalization resolves exactly
/// like the server's: company tokens always, the exporter's
/// profile for a personal export, personal fields scrubbed for a
/// company export, and a valueless placeholder blocks the export.
fn build_deck_local(
    store: &Store,
    exporter: &UserRec,
    entries: &[qimu_core::model::IndexEntry],
    source: Option<SourceRef>,
    kind: &str,
    include_notes: bool,
) -> Result<(Vec<u8>, Manifest, i64), String> {
    let mut pkgs: Vec<Package> = Vec::new();
    let mut bindings: Vec<Binding> = Vec::new();
    let mut binding_scripts: Vec<Vec<String>> = Vec::new();
    for e in entries {
        let version = store
            .resolve_entry_version(e)?
            .ok_or_else(|| "有条目无法解析到版本（内容块尚无已发布版本）".to_string())?;
        let singles = store.version_page_packages(&version)?;
        let chosen: Vec<usize> = match &e.excerpt_slots {
            Some(slots) => qimu_core::store::excerpt_positions(&version, slots),
            None => (0..singles.len()).collect(),
        };
        let mut fps = Vec::new();
        let mut scripts = Vec::new();
        for i in &chosen {
            if let Some(p) = version.pages.get(*i) {
                fps.push(p.fp.clone());
            }
            scripts.push(version.scripts.get(*i).cloned().unwrap_or_default());
            pkgs.push(singles[*i].clone());
        }
        binding_scripts.push(scripts);
        bindings.push(Binding {
            unit_id: e.unit_id.0.clone(),
            rendition_id: e.rendition_id.0.clone(),
            version_id: version.id.clone(),
            version_semver: version.semver.clone(),
            page_fps: fps,
        });
    }
    if pkgs.is_empty() {
        return Err("没有可导出的页面".into());
    }
    let page_count = pkgs.len() as i64;
    let mut assembled = qimu_core::assemble::assemble(&pkgs)?;
    if include_notes {
        let mut cursor = 0usize;
        for scripts in &binding_scripts {
            for script in scripts {
                if !script.is_empty() {
                    qimu_core::pptx::set_notes_text(&mut assembled, cursor, script);
                }
                cursor += 1;
            }
        }
    }
    {
        use qimu_core::personalize;
        let mut values: BTreeMap<String, String> = BTreeMap::new();
        for (k, v) in store.company_profile()? {
            let token = match k.as_str() {
                "phone" => "company_phone",
                "website" => "company_website",
                "address" => "company_address",
                "gongzhonghao" => "company_gongzhonghao",
                _ => continue,
            };
            if !v.is_empty() {
                values.insert(token.to_string(), v);
            }
        }
        let scrub: &[&str] = if kind == "company" {
            personalize::PERSONAL_FIELDS
        } else {
            values.insert("name".to_string(), exporter.display_name.clone());
            for (k, v) in store.profile(&exporter.id)? {
                if !v.is_empty() {
                    values.insert(k, v);
                }
            }
            &[]
        };
        let missing = personalize::substitute(&mut assembled, &values, scrub);
        if !missing.is_empty() {
            return Err(format!(
                "导出已拦截：以下占位字段没有值（请先在设置里补全资料）：{}",
                missing.join("、")
            ));
        }
    }
    let deck = assembled.to_bytes()?;
    let manifest = Manifest {
        deck_id: qimu_core::auth::new_id("deck"),
        kind: kind.to_string(),
        source,
        exported_ms: qimu_core::auth::now_ms(),
        bindings,
    };
    let deck = qimu_core::manifest::embed_manifest(&deck, &manifest)?;
    Ok((deck, manifest, page_count))
}

/// The `unit_json` wrapper of the server, over an already-locked
/// store: `{unit, renditions: [{rendition, latest, versions}]}`.
/// The REST list endpoint sends `versions: []` and only the detail
/// endpoint fills it; the local face fills it in both (P1 — the
/// wall and detail version strips read it from the list data).
fn unit_json(store: &Store, unit: &qimu_core::store::UnitRec) -> Value {
    let mut rends = Vec::new();
    if let Ok(list) = store.list_renditions(&unit.id) {
        for r in &list {
            let versions = store.list_versions(&r.id).unwrap_or_default();
            let latest = store.latest_version(&r.id).ok().flatten();
            rends.push(json!({
                "rendition": serde_json::to_value(r).unwrap_or(Value::Null),
                "latest": serde_json::to_value(latest).unwrap_or(Value::Null),
                "versions": serde_json::to_value(versions).unwrap_or(Value::Null),
            }));
        }
    }
    let mut obj = serde_json::Map::new();
    obj.insert(
        "unit".into(),
        serde_json::to_value(unit).unwrap_or(Value::Null),
    );
    obj.insert("renditions".into(), Value::Array(rends));
    Value::Object(obj)
}

/// One commit's entries with their resolved versions — the
/// server's `resolved_entries` helper, over an already-locked
/// store.
fn resolved_entries(store: &Store, commit: &qimu_core::store::CommitRec) -> Vec<Value> {
    commit
        .entries
        .iter()
        .map(|e| {
            let unit = store.get_unit(&e.unit_id.0).ok();
            let rend = store.get_rendition(&e.rendition_id.0).ok();
            let resolved = store.resolve_entry_version(e).ok().flatten();
            let locked_id = match &e.policy {
                qimu_core::model::EntryPolicy::Locked { version_id } => {
                    Some(version_id.0.clone())
                }
                _ => None,
            };
            let substituted = match (&locked_id, &resolved) {
                (Some(lid), Some(v)) => &v.id != lid,
                _ => false,
            };
            json!({
                "entry": e,
                "unit_name": unit.map(|u| u.name),
                "form": rend.as_ref().map(|r| r.form.clone()),
                "lang": rend.as_ref().map(|r| r.lang.clone()),
                "resolved": resolved,
                "substituted": substituted,
            })
        })
        .collect()
}

/// Origin label for a move-to-trash (the server's `trash_origin`):
/// the home folder's name, or the surface fallback.
fn trash_origin(store: &Store, folder_id: Option<&str>, fallback: &str) -> String {
    match folder_id {
        Some(fid) => store
            .list_folders()
            .ok()
            .and_then(|fs| fs.into_iter().find(|f| f.id == fid))
            .map(|f| f.name)
            .unwrap_or_else(|| fallback.to_string()),
        None => fallback.to_string(),
    }
}

/// Cover for a trash row (the server's `trash_cover`): the version
/// whose first page represents the object (unit → latest version
/// of its first rendition; index → first resolved entry of the
/// main branch head), plus its page count.
fn trash_cover(
    store: &Store,
    row: &qimu_core::store::TrashRec,
) -> (Option<String>, i64) {
    let ver = match row.kind.as_str() {
        "unit" => store
            .list_renditions(&row.object_id)
            .ok()
            .and_then(|rs| rs.into_iter().next())
            .and_then(|r| store.latest_version(&r.id).ok().flatten()),
        "index" => store
            .branch_head(&row.object_id, "main")
            .ok()
            .flatten()
            .and_then(|cid| store.get_commit(&cid).ok())
            .and_then(|c| c.entries.into_iter().next())
            .and_then(|e| store.resolve_entry_version(&e).ok().flatten()),
        _ => None,
    };
    match ver {
        Some(v) => (Some(v.id), v.page_count),
        None => (None, 0),
    }
}

/// One query parameter of a request path, percent-decoded (the
/// client's `query_component` encodes everything outside
/// `[A-Za-z0-9-._~]` as %XX, so a literal '+' is data, never a
/// space).
fn query_param(query: &str, key: &str) -> Option<String> {
    query.split('&').find_map(|pair| {
        let (k, v) = pair.split_once('=')?;
        (k == key).then(|| percent_decode(v))
    })
}

fn percent_decode(s: &str) -> String {
    fn hex(c: u8) -> Option<u8> {
        match c {
            b'0'..=b'9' => Some(c - b'0'),
            b'a'..=b'f' => Some(c - b'a' + 10),
            b'A'..=b'F' => Some(c - b'A' + 10),
            _ => None,
        }
    }
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            if let (Some(h), Some(l)) = (hex(b[i + 1]), hex(b[i + 2])) {
                out.push(h * 16 + l);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

impl Drop for LocalStore {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.lock_path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use qimu_core::model::{EntryPolicy, Id, IndexEntry};
    use qimu_core::semver::SemVer;

    struct TestDir(std::path::PathBuf);

    impl TestDir {
        fn new() -> Self {
            let nonce = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            Self(std::env::temp_dir().join(format!(
                "qimu-client-local-test-{}-{nonce}",
                std::process::id()
            )))
        }
    }

    impl Drop for TestDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn crc32(data: &[u8]) -> u32 {
        let mut crc = 0xFFFF_FFFFu32;
        for &byte in data {
            crc ^= u32::from(byte);
            for _ in 0..8 {
                let mask = (crc & 1).wrapping_neg();
                crc = (crc >> 1) ^ (0xEDB8_8320 & mask);
            }
        }
        !crc
    }

    fn fragment_bytes() -> Vec<u8> {
        let parts = [
            (
                "[Content_Types].xml",
                r#"<?xml version="1.0" encoding="UTF-8"?><Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Override PartName="/ppt/presentation.xml" ContentType="application/vnd.openxmlformats-officedocument.presentationml.presentation.main+xml"/><Override PartName="/ppt/slides/slide1.xml" ContentType="application/vnd.openxmlformats-officedocument.presentationml.slide+xml"/></Types>"#,
            ),
            (
                "_rels/.rels",
                r#"<?xml version="1.0" encoding="UTF-8"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="ppt/presentation.xml"/></Relationships>"#,
            ),
            (
                "ppt/presentation.xml",
                r#"<?xml version="1.0" encoding="UTF-8"?><p:presentation xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"><p:sldIdLst><p:sldId id="256" r:id="rId1"/></p:sldIdLst><p:sldSz cx="9144000" cy="5143500"/></p:presentation>"#,
            ),
            (
                "ppt/_rels/presentation.xml.rels",
                r#"<?xml version="1.0" encoding="UTF-8"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/slide" Target="slides/slide1.xml"/></Relationships>"#,
            ),
            (
                "ppt/slides/slide1.xml",
                r#"<?xml version="1.0" encoding="UTF-8"?><p:sld xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main" xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main"><p:cSld><p:spTree><p:sp><p:txBody><a:p><a:r><a:t>Preview pin</a:t></a:r></a:p></p:txBody></p:sp></p:spTree></p:cSld></p:sld>"#,
            ),
        ];
        let mut out = Vec::new();
        let mut central = Vec::new();
        for (name, text) in parts {
            let data = text.as_bytes();
            let crc = crc32(data);
            let offset = out.len() as u32;
            out.extend_from_slice(&0x0403_4b50u32.to_le_bytes());
            out.extend_from_slice(&20u16.to_le_bytes());
            out.extend_from_slice(&0u16.to_le_bytes());
            out.extend_from_slice(&0u16.to_le_bytes());
            out.extend_from_slice(&0u16.to_le_bytes());
            out.extend_from_slice(&0u16.to_le_bytes());
            out.extend_from_slice(&crc.to_le_bytes());
            out.extend_from_slice(&(data.len() as u32).to_le_bytes());
            out.extend_from_slice(&(data.len() as u32).to_le_bytes());
            out.extend_from_slice(&(name.len() as u16).to_le_bytes());
            out.extend_from_slice(&0u16.to_le_bytes());
            out.extend_from_slice(name.as_bytes());
            out.extend_from_slice(data);
            central.extend_from_slice(&0x0201_4b50u32.to_le_bytes());
            central.extend_from_slice(&20u16.to_le_bytes());
            central.extend_from_slice(&20u16.to_le_bytes());
            central.extend_from_slice(&0u16.to_le_bytes());
            central.extend_from_slice(&0u16.to_le_bytes());
            central.extend_from_slice(&0u16.to_le_bytes());
            central.extend_from_slice(&0u16.to_le_bytes());
            central.extend_from_slice(&crc.to_le_bytes());
            central.extend_from_slice(&(data.len() as u32).to_le_bytes());
            central.extend_from_slice(&(data.len() as u32).to_le_bytes());
            central.extend_from_slice(&(name.len() as u16).to_le_bytes());
            central.extend_from_slice(&0u16.to_le_bytes());
            central.extend_from_slice(&0u16.to_le_bytes());
            central.extend_from_slice(&0u16.to_le_bytes());
            central.extend_from_slice(&0u16.to_le_bytes());
            central.extend_from_slice(&0u32.to_le_bytes());
            central.extend_from_slice(&offset.to_le_bytes());
            central.extend_from_slice(name.as_bytes());
        }
        let central_start = out.len() as u32;
        out.extend_from_slice(&central);
        out.extend_from_slice(&0x0605_4b50u32.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(&(parts.len() as u16).to_le_bytes());
        out.extend_from_slice(&(parts.len() as u16).to_le_bytes());
        out.extend_from_slice(&(central.len() as u32).to_le_bytes());
        out.extend_from_slice(&central_start.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out
    }

    #[test]
    fn submission_uses_the_exact_versions_pinned_by_the_preview() {
        let dir = TestDir::new();
        let local = LocalStore::open(&dir.0).unwrap();
        let fragment = fragment_bytes();
        let (tag, first, index_id, rendition_id, entry) = {
            let store = local.store.lock().unwrap();
            let unit = store
                .create_unit(
                    "内容块",
                    None,
                    &local.default_group_id,
                    &local.owner.display_name,
                    "zh",
                    "",
                )
                .unwrap();
            let rendition = store
                .get_or_create_rendition(&unit.id, "deck", "zh")
                .unwrap();
            let first = store
                .insert_version(
                    &rendition.id,
                    SemVer::parse("1.0.0").unwrap(),
                    1,
                    "tester",
                    "",
                    &fragment,
                    "normal",
                )
                .unwrap();
            let entry = IndexEntry {
                unit_id: Id(unit.id.clone()),
                rendition_id: Id(rendition.id.clone()),
                policy: EntryPolicy::FollowLatest,
                locked_semver: None,
                excerpt_slots: None,
            };
            let index = store
                .create_index(
                    "索引",
                    None,
                    &local.default_group_id,
                    "tester",
                    "private",
                    vec![entry.clone()],
                )
                .unwrap();
            let tag = store.create_tag(&index.id, "main", "1.0.0").unwrap();
            (tag, first, index.id.clone(), rendition.id.clone(), entry)
        };
        let preview = local.push_preview(&index_id, "main").unwrap().unwrap();
        assert_eq!(preview["published_tag_id"], tag.id);
        assert_eq!(preview["current_entries"][0]["resolved"]["id"], first.id);
        assert!(local.push_preview(&index_id, "dev").unwrap().is_err());
        let pins = preview["current_entries"]
            .as_array()
            .unwrap()
            .iter()
            .map(|row| {
                (
                    serde_json::from_value::<IndexEntry>(row["entry"].clone()).unwrap(),
                    row["resolved"]["id"].as_str().unwrap().to_string(),
                )
            })
            .collect::<Vec<_>>();

        let store = local.store.lock().unwrap();
        let second = store
            .insert_version(
                &rendition_id,
                SemVer::parse("1.1.0").unwrap(),
                1,
                "tester",
                "",
                &fragment,
                "normal",
            )
            .unwrap();
        store
            .create_commit(&index_id, "main", "tester", "new head", vec![entry])
            .unwrap();
        assert_eq!(store.latest_version(&rendition_id).unwrap().unwrap().id, second.id);
        drop(store);

        let package = local
            .build_submission_package(&index_id, &tag.id, &pins)
            .unwrap()
            .unwrap();
        assert_eq!(package["manifest"]["versions"][0]["id"], first.id);
        assert_eq!(package["manifest"]["versions"][0]["semver"], "1.0.0");
        assert_eq!(package["files"][first.fragment_hash.as_str()], crate::b64e(&fragment));
        assert!(local
            .build_submission_package(&index_id, &tag.id, &[])
            .unwrap()
            .is_err());
    }
}

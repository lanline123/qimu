//! Local-first foundation (本地优先 P0, design §6.2/§7): the local
//! library owned by the client process itself.
//!
//! Today the client's working data lives in a `qimu_core::store::Store`
//! owned by a server process (the bundled standalone server on this
//! machine, or an organization server), and the client only ever talks
//! to it over REST. P0 moves the *ownership* back: when the gray switch
//! `QIMU_LOCAL_FIRST=1` is set, the client opens the very same Store
//! implementation in-process at the local library root — same on-disk
//! format as the server's data root (`qimu.db` + `files/`), so an
//! existing bundled-server `qimu-data` directory is taken over with
//! zero migration. With the switch off nothing here runs and the
//! client behaves exactly as before.
//!
//! Scope: the store handle, the library-root resolution, the
//! seeded machine owner + default group, and the JSON adapter
//! layer that feeds the UI the same `Value` shapes the REST list
//! endpoints return. P1 adds the working-data write face: the
//! `dispatch` router below serves the import / units / indexes /
//! folders / trash / search REST paths from this library, so in
//! local mode working data is written here, in-process.

use qimu_core::manifest::{Binding, Manifest, SourceRef};
use qimu_core::pptx::{self, Package};
use qimu_core::store::{Store, UserRec};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Mutex;

/// The P0 gray switch (design §7): local-first behavior is opt-in via
/// the environment until the migration phases flip the default.
pub fn local_first_enabled() -> bool {
    matches!(
        std::env::var("QIMU_LOCAL_FIRST").as_deref(),
        Ok("1") | Ok("true")
    )
}

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
    /// Open the local library when the gray switch is on; `None` when
    /// it is off (legacy path untouched) or when opening fails (the
    /// reason is printed to stderr; the client keeps running on the
    /// legacy path instead of dying with the library).
    pub fn open_if_enabled() -> Option<LocalStore> {
        Self::open_if_enabled_detailed().0
    }

    /// Like `open_if_enabled`, but also returns the failure reason so
    /// the settings diagnostic can show it instead of only logging
    /// to stderr (P0 fix: a lock-held failure used to surface only
    /// as "see the console log").
    pub fn open_if_enabled_detailed() -> (Option<LocalStore>, Option<String>) {
        if !local_first_enabled() {
            return (None, None);
        }
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
        // process owns a library root at a time. While the gray switch
        // is on the client also skips spawning the bundled server
        // against the same root (see restore_connections), so the two
        // processes never hold the same library open together.
        let lock_path = root.join("qimu-local.lock");
        // Advisory lock (design §9 R5, client-side half): one client
        // process owns a library root at a time. While the gray switch
        // is on the client also skips spawning the bundled server
        // against the same root (see restore_connections), so the two
        // processes never hold the same library open together.
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

    /// The version's fragment PPTX bytes from the local CAS (the
    /// same `file_get(fragment_hash)` read the server's thumb and
    /// edit-preview handlers perform). None when the version is not
    /// in this library.
    pub fn version_fragment(&self, version_id: &str) -> Option<Vec<u8>> {
        let store = self.store.lock().unwrap();
        let ver = store.get_version(version_id).ok()?;
        store.file_get(&ver.fragment_hash).ok()
    }

    /// Assemble one showroom item's deck from this library,
    /// mirroring the server's export funnel step for step (handler
    /// `export_showroom_pdf` + `build_export`): the item's commit
    /// (pinned `commit_id`, else the source index's main head)
    /// supplies the entries; each entry resolves to a version whose
    /// fragment is split and excerpted; speaker scripts are written
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
            let mut pkgs: Vec<Package> = Vec::new();
            let mut bindings: Vec<Binding> = Vec::new();
            let mut binding_scripts: Vec<Vec<String>> = Vec::new();
            for e in &commit.entries {
                let version = store
                    .resolve_entry_version(e)?
                    .ok_or_else(|| {
                        "有条目无法解析到版本（内容块尚无已发布版本）".to_string()
                    })?;
                let frag_bytes = store.file_get(&version.fragment_hash)?;
                let frag_pkg = Package::load(&frag_bytes)?;
                let singles = pptx::split(&frag_pkg)?;
                let chosen: Vec<usize> = match &e.excerpt_slots {
                    Some(slots) => slots
                        .iter()
                        .filter_map(|sl| sl.parse::<usize>().ok())
                        .filter(|i| *i < singles.len())
                        .collect(),
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

    /// Thumbnail cache directory for one version, in the server's
    /// exact layout (`<root>/thumbs/deckcraft-v2/<version id>`), so
    /// a library previously served by the bundled server hands its
    /// cache over verbatim — and the server fallback path keeps
    /// finding the pages this process rendered.
    pub fn thumb_cache_dir(&self, version_id: &str) -> PathBuf {
        self.root
            .join("thumbs")
            .join("deckcraft-v2")
            .join(version_id)
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

    /// Working-data dispatcher (local-first P1): serves the REST
    /// paths whose data is working data — import, units, indexes,
    /// folders, trash, search — from this local library, with the
    /// exact response shapes and error strings the server handlers
    /// produce (they are thin shells over the same core calls; the
    /// capability checks they add are vacuous for the library's
    /// seeded super owner). Returns None for anything else —
    /// organization surfaces, and the few objects that live on the
    /// server even in local mode (showroom trash rows) — so the
    /// caller falls through to REST unchanged.
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
                ("POST", ["v1", "import"]) => {
                    let bytes = b64_field(&body, "data_base64")?;
                    let blocks = body.get("blocks").and_then(|b| b.as_array()).map(|arr| {
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
                    });
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
                    let bytes = store.file_get(&ver.fragment_hash)?;
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
                // ---- folders ----
                ("GET", ["v1", "folders"]) => {
                    let store = self.store.lock().unwrap();
                    let folders = store.list_folders().map(values_of).unwrap_or_default();
                    Ok(json!({ "folders": folders }))
                }
                ("POST", ["v1", "folders"]) => {
                    let store = self.store.lock().unwrap();
                    let f = store.create_folder(
                        &str_field(&body, "name").unwrap_or_default(),
                        opt_str_field(&body, "parent_id"),
                    )?;
                    Ok(serde_json::to_value(&f).unwrap_or(Value::Null))
                }
                ("POST", ["v1", "folders", id, "rename"]) => {
                    let store = self.store.lock().unwrap();
                    store.rename_folder(id, &str_field(&body, "name").unwrap_or_default())?;
                    Ok(json!({ "ok": true }))
                }
                ("DELETE", ["v1", "folders", id]) => {
                    let store = self.store.lock().unwrap();
                    store.delete_folder(id)?;
                    Ok(json!({ "ok": true }))
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
                                trash_origin(&store, unit.folder_id.as_deref(), "内容页"),
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
fn b64_field(body: &Value, key: &str) -> Result<Vec<u8>, String> {
    let s = str_field(body, key).unwrap_or_default();
    crate::b64d(&s).map_err(|e| format!("base64 解码失败: {e}"))
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

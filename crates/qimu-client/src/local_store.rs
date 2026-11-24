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
//! Scope of this phase: the store handle, the library-root resolution,
//! the seeded machine owner + default group, and the JSON adapter
//! layer that feeds the UI the same `Value` shapes the REST list
//! endpoints return. Write paths still go over REST until P1/P2 flip
//! them over one by one.

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
                lock_path,
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
    /// `{unit, renditions: [{rendition, latest, versions: []}]}`.
    pub fn list_units(&self) -> Vec<Value> {
        let store = self.store.lock().unwrap();
        let units = store.list_units(None).unwrap_or_default();
        let mut out = Vec::new();
        for u in &units {
            let mut rends = Vec::new();
            if let Ok(list) = store.list_renditions(&u.id) {
                for r in &list {
                    let latest = store.latest_version(&r.id).ok().flatten();
                    rends.push(json!({
                        "rendition": serde_json::to_value(r).unwrap_or(Value::Null),
                        "latest": serde_json::to_value(latest).unwrap_or(Value::Null),
                        "versions": [],
                    }));
                }
            }
            let mut obj = serde_json::Map::new();
            obj.insert(
                "unit".into(),
                serde_json::to_value(u).unwrap_or(Value::Null),
            );
            obj.insert("renditions".into(), Value::Array(rends));
            out.push(Value::Object(obj));
        }
        out
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
}

impl Drop for LocalStore {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.lock_path);
    }
}

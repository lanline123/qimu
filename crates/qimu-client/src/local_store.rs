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

use qimu_core::store::{Store, UserRec};
use serde_json::{json, Value};
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

impl LocalStore {
    /// Open the local library when the gray switch is on; `None` when
    /// it is off (legacy path untouched) or when opening fails (the
    /// reason is printed to stderr; the client keeps running on the
    /// legacy path instead of dying with the library).
    pub fn open_if_enabled() -> Option<LocalStore> {
        if !local_first_enabled() {
            return None;
        }
        let root = library_dir();
        match Self::open(&root) {
            Ok(s) => Some(s),
            Err(e) => {
                eprintln!("[qimu] 本地库打开失败（{}）：{e}", root.display());
                None
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
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&lock_path)
        {
            Ok(mut f) => {
                use std::io::Write;
                let _ = writeln!(f, "{}", std::process::id());
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                return Err(format!(
                    "本地库已被另一个启幕进程占用（锁文件 {}）；若确认没有其他启幕在运行，删除该锁文件后重试",
                    lock_path.display()
                ));
            }
            Err(e) => return Err(e.to_string()),
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

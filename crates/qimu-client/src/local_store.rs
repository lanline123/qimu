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
use qimu_core::store::{
    FolderDeleteMode, FolderDeleteSnapshot, FolderPosition, FolderRec, Store, TrashRec,
    UnitPosition, UnitSnapshot, UserRec, VersionRowSnap,
};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Mutex;

/// Library root resolution (design §6.2, extended by the
/// library-discovery line, user ruling 2026-11-25): the bootstrap
/// config's `data_dir` wins (see `read_bootstrap`); then the legacy
/// `library_dir` client setting; then an explicit data-dir
/// environment variable (`QIMU_CLIENT_DATA_DIR`, falling back to
/// the legacy shared `QIMU_DATA_DIR` — audit2 F-N2); finally the standard directory
/// `~/.qimu/library`, with a first-run discovery step in front of
/// it: when the standard directory holds no content yet, libraries
/// left behind by older versions in their historical locations are
/// probed and the fullest one is relocated (copied, then verified)
/// into the standard directory, so an upgrade no longer opens onto
/// a blank library while the user's data sits in an old install
/// dir. The configured roots (bootstrap, legacy setting) carry the
/// same floor: one that reads *empty* runs the same discovery
/// before it is adopted, so a configured-but-empty library can
/// never silently short-circuit the scan (2026-11-25 rescan line).
/// The whole resolution runs once per process.
pub fn library_dir() -> PathBuf {
    library_resolution().dir
}

/// The standard library directory: the default root a fresh
/// install uses and the target old libraries are relocated into.
pub fn standard_library_dir() -> PathBuf {
    crate::qimu_home().join("library")
}

// ---------------------------------------------------------------------------
// Bootstrap config (user ruling 2026-11-25): a small standalone file
// at `~/.qimu/bootstrap.json` — inside the per-user qimu home, which
// no installer creates, moves or removes (the NSIS packages manage
// only their install dir + registry keys), so the file is found no
// matter where the app is installed or where the library itself
// lives. It records where the library is (`data_dir`) and what to
// do with a relocated old library (`old_library_policy`). A missing
// or corrupt file is not an error: it reads as "no config" and the
// discovery flow runs.

/// What happens to the source of a verified relocation once the new
/// library is live (settings-selectable; stored in the bootstrap
/// config because that file is what governs library placement).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum OldLibraryPolicy {
    /// Ask the user (the default when never set).
    Ask,
    /// Delete the source without asking.
    Delete,
    /// Keep the source untouched.
    Keep,
}

impl OldLibraryPolicy {
    pub fn as_str(&self) -> &'static str {
        match self {
            OldLibraryPolicy::Ask => "ask",
            OldLibraryPolicy::Delete => "delete",
            OldLibraryPolicy::Keep => "keep",
        }
    }

    fn parse(s: &str) -> OldLibraryPolicy {
        match s {
            "delete" => OldLibraryPolicy::Delete,
            "keep" => OldLibraryPolicy::Keep,
            _ => OldLibraryPolicy::Ask,
        }
    }
}

/// A settings-registered data-directory move (audit P1): the
/// settings page only *registers* the intent; the relocation
/// itself runs inside the next launch's library resolution,
/// before the store opens, when the source library is static.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct PendingRelocate {
    pub from: String,
    pub to: String,
}

/// The bootstrap config's contents; absent fields read as `None`.
#[derive(Clone, Default)]
pub struct Bootstrap {
    pub data_dir: Option<String>,
    pub old_library_policy: Option<OldLibraryPolicy>,
    /// A data-directory move registered in settings, awaiting
    /// execution at the next launch (see `PendingRelocate`).
    pub pending_relocate: Option<PendingRelocate>,
    /// One-shot outcome note of a startup relocation, written by
    /// library resolution and surfaced (then cleared) by the app
    /// so a failed or cancelled move is never silent.
    pub relocate_notice: Option<String>,
    /// The client version that last completed library resolution
    /// (2026-11-25 rescan line): a stamp that differs from the
    /// running build marks this launch as the first after an
    /// upgrade/reinstall. The stamp only *records* that fact —
    /// the discovery floor in `resolve_library` runs on every
    /// launch (its cost is one probe of the configured directory,
    /// plus candidate probes only when that directory reads
    /// empty), so an empty configured library can never silently
    /// short-circuit discovery the way it could before this
    /// field existed; the stamp never drives a relocation by
    /// itself (a relocation still requires the resolved library
    /// to be empty and a candidate to hold content).
    pub app_version: Option<String>,
}

impl Bootstrap {
    /// The effective disposition policy: `Ask` until set otherwise.
    pub fn policy(&self) -> OldLibraryPolicy {
        self.old_library_policy.unwrap_or(OldLibraryPolicy::Ask)
    }
}

pub fn bootstrap_path() -> PathBuf {
    crate::qimu_home().join("bootstrap.json")
}

pub fn read_bootstrap() -> Bootstrap {
    let Ok(text) = std::fs::read_to_string(bootstrap_path()) else {
        return Bootstrap::default();
    };
    let Ok(v) = serde_json::from_str::<Value>(&text) else {
        // Corrupt config: behave as if there were none (discovery
        // runs); the file is left in place until a relocation or a
        // settings change writes a fresh one.
        return Bootstrap::default();
    };
    Bootstrap {
        data_dir: v
            .get("data_dir")
            .and_then(|x| x.as_str())
            .map(|s| s.to_string()),
        old_library_policy: v
            .get("old_library_policy")
            .and_then(|x| x.as_str())
            .map(OldLibraryPolicy::parse),
        pending_relocate: v.get("pending_relocate").and_then(|p| {
            Some(PendingRelocate {
                from: p.get("from")?.as_str()?.to_string(),
                to: p.get("to")?.as_str()?.to_string(),
            })
        }),
        relocate_notice: v
            .get("relocate_notice")
            .and_then(|x| x.as_str())
            .map(|s| s.to_string()),
        app_version: v
            .get("app_version")
            .and_then(|x| x.as_str())
            .map(|s| s.to_string()),
    }
}

/// Take (and clear) the one-shot relocation notice, if one is
/// pending. Called once by the app at startup.
pub fn take_relocate_notice() -> Option<String> {
    let mut b = read_bootstrap();
    let notice = b.relocate_notice.take()?;
    write_bootstrap(&b);
    Some(notice)
}

pub fn write_bootstrap(b: &Bootstrap) {
    let mut obj = serde_json::Map::new();
    if let Some(d) = &b.data_dir {
        obj.insert("data_dir".to_string(), json!(d));
    }
    if let Some(p) = &b.old_library_policy {
        obj.insert("old_library_policy".to_string(), json!(p.as_str()));
    }
    if let Some(p) = &b.pending_relocate {
        obj.insert(
            "pending_relocate".to_string(),
            json!({ "from": p.from, "to": p.to }),
        );
    }
    if let Some(n) = &b.relocate_notice {
        obj.insert("relocate_notice".to_string(), json!(n));
    }
    if let Some(v) = &b.app_version {
        obj.insert("app_version".to_string(), json!(v));
    }
    let dir = crate::qimu_home();
    let _ = std::fs::create_dir_all(&dir);
    let _ = craft_atomic::atomic_write(
        &dir.join("bootstrap.json"),
        serde_json::to_string_pretty(&Value::Object(obj))
            .unwrap_or_default()
            .as_bytes(),
    );
}

// ---------------------------------------------------------------------------
// Library probing + relocation. Probing is strictly read-only: it
// opens `qimu.db` with SQLITE_OPEN_READ_ONLY and never goes through
// `Store::open` (which creates tables, seeds the owner and runs
// backfills — a probe must not mutate the library it inspects).

/// Core content counts of a library: (content blocks, versions,
/// indexes) plus the page-entity and submission counts (audit
/// P3: the verification must cover the tables a relocation can
/// silently lose, not only the original three). The verification
/// currency of every relocation.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct LibraryCounts {
    pub units: usize,
    pub versions: usize,
    pub indexes: usize,
    pub pages: usize,
    pub submissions: usize,
}

impl LibraryCounts {
    pub fn has_content(&self) -> bool {
        self.units + self.versions + self.indexes > 0
    }

    /// "Fullest library" ranking key: blocks first, then versions,
    /// then indexes; the caller breaks remaining ties by the
    /// database file's modification time.
    fn rank(&self) -> (usize, usize, usize) {
        (self.units, self.versions, self.indexes)
    }
}

/// Recognize a library at `dir` and count its core content. A
/// directory is a qimu library when it holds a `qimu.db` that opens
/// as SQLite and carries the core tables (`units`, `versions`,
/// `indexes_tbl`). `None` for anything else — including a database
/// a crashed writer left a hot journal on (a read-only open refuses
/// it); such a candidate is skipped, never forced.
pub fn probe_library(dir: &std::path::Path) -> Option<LibraryCounts> {
    let db = dir.join("qimu.db");
    if !db.is_file() {
        return None;
    }
    let conn = rusqlite::Connection::open_with_flags(
        &db,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .ok()?;
    // A library another process is mid-write on should read as
    // "busy", not "not a library": wait out short transactions
    // (audit2 X2, same timeout as Store::open).
    let _ = conn.busy_timeout(std::time::Duration::from_secs(5));
    let tables: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' \
             AND name IN ('units','versions','indexes_tbl')",
            [],
            |r| r.get(0),
        )
        .ok()?;
    if tables < 3 {
        return None;
    }
    let count = |t: &str| -> Option<i64> {
        conn.query_row(&format!("SELECT COUNT(*) FROM \"{t}\""), [], |r| r.get(0))
            .ok()
    };
    // The page-entity and submission tables postdate the core
    // three; a library from before them reads as 0 here (and so
    // does its copy — the comparison stays apples-to-apples).
    let count_or_zero = |t: &str| -> usize {
        conn.query_row(&format!("SELECT COUNT(*) FROM \"{t}\""), [], |r| {
            r.get::<_, i64>(0)
        })
        .map(|n| n as usize)
        .unwrap_or(0)
    };
    Some(LibraryCounts {
        units: count("units")? as usize,
        versions: count("versions")? as usize,
        indexes: count("indexes_tbl")? as usize,
        pages: count_or_zero("pages"),
        submissions: count_or_zero("submissions"),
    })
}

/// Files a relocation never copies: the client advisory lock (a
/// copied lock would point at a stale pid — and the target must
/// take its own) and the SQLite shared-memory sidecar (transient
/// index state, rebuilt on open). The write-ahead log and the
/// rollback journal DO travel (audit P3): `relocate_library`
/// checkpoints the source first, which normally folds them into
/// the `.db`; when checkpointing is impossible (read-only media,
/// a locked source), the copied sidecars are what let SQLite
/// recover the last committed state at the target instead of
/// silently presenting an older database.
const COPY_SKIP: [&str; 2] = ["qimu-local.lock", "qimu.db-shm"];

/// Best-effort fold of a library's write-ahead log (and rollback
/// of any hot journal) into its `qimu.db`. A plain open performs
/// journal recovery; the checkpoint then truncates the wal. Used
/// on relocation sources and targets so probes and copies see the
/// last committed state. Failures are ignored by design: a source
/// that cannot be opened read-write is still copied verbatim
/// (sidecars included) and the counts check arbitrates.
fn checkpoint_db(dir: &std::path::Path) {
    let db = dir.join("qimu.db");
    if !db.is_file() {
        return;
    }
    if let Ok(conn) = rusqlite::Connection::open(&db) {
        let _ = conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);");
    }
}

/// Copy the library at `src` into `dst` and verify the copy by
/// core counts. The source is never modified. On any failure the
/// partial copy is removed again (exactly the top-level entries
/// this call copied, plus `dst` itself when this call created it),
/// so a half-copied directory is never left behind to be mistaken
/// for a library.
pub fn relocate_library(
    src: &std::path::Path,
    dst: &std::path::Path,
) -> Result<LibraryCounts, String> {
    // Settle the source first (audit P3): an old version killed
    // mid-write can leave committed rows only in its wal — the
    // read-only probe alone could count (and the copy carry) an
    // older state and still "verify".
    checkpoint_db(src);
    let src_counts = probe_library(src)
        .ok_or_else(|| "源目录不是可识别的启幕库（缺少 qimu.db 或核心表）".to_string())?;
    if src == dst {
        return Ok(src_counts);
    }
    let dst_existed = dst.exists();
    std::fs::create_dir_all(dst).map_err(|e| format!("无法创建目标目录：{e}"))?;
    let mut copied: Vec<PathBuf> = Vec::new();
    let attempt = (|| -> Result<(), String> {
        let rd = std::fs::read_dir(src).map_err(|e| e.to_string())?;
        for entry in rd {
            let entry = entry.map_err(|e| e.to_string())?;
            let name = entry.file_name();
            if COPY_SKIP.iter().any(|s| *s == name.to_string_lossy().as_ref()) {
                continue;
            }
            let to = dst.join(&name);
            let ft = entry.file_type().map_err(|e| e.to_string())?;
            if ft.is_dir() {
                crate::copy_dir_recursive(&entry.path(), &to).map_err(|e| e.to_string())?;
            } else {
                std::fs::copy(entry.path(), &to).map_err(|e| e.to_string())?;
            }
            copied.push(to);
        }
        Ok(())
    })();
    // Settle the copy the same way before verifying it, so the
    // probe reads the recovered, checkpointed state.
    if attempt.is_ok() {
        checkpoint_db(dst);
    }
    let verified = attempt.and_then(|()| match probe_library(dst) {
        Some(c) if c == src_counts => Ok(c),
        Some(c) => Err(format!(
            "搬迁后计数不一致（源：{} 内容块 / {} 版本 / {} 索引 / {} 页面 / {} 投稿；目标：{} / {} / {} / {} / {}）",
            src_counts.units,
            src_counts.versions,
            src_counts.indexes,
            src_counts.pages,
            src_counts.submissions,
            c.units,
            c.versions,
            c.indexes,
            c.pages,
            c.submissions
        )),
        None => Err("搬迁后目标目录无法识别为启幕库".to_string()),
    });
    if verified.is_err() {
        for p in &copied {
            if p.is_dir() {
                let _ = std::fs::remove_dir_all(p);
            } else {
                let _ = std::fs::remove_file(p);
            }
        }
        if !dst_existed {
            let _ = std::fs::remove_dir(dst);
        }
    }
    verified
}

/// Historical install roots whose `qimu-data` may hold a library an
/// older version created — every entry is evidenced by this tree:
/// the current exe-side dir (the server's and the pre-P4 client's
/// data root: server `data_dir()` resolution + all three NSIS
/// scripts pre-creating `$INSTDIR\qimu-data`), and the NSIS default
/// install dirs with their `.onInit` D-drive rewrites —
/// `启幕-客户端.nsi` (`$PROGRAMFILES64\启幕客户端` /
/// `D:\Program Files\启幕客户端`), `启幕.nsi`
/// (`$PROGRAMFILES64\启幕` / `D:\Program Files\启幕`),
/// `启幕-服务器.nsi` (`$PROGRAMFILES64\启幕服务器` /
/// `D:\Program Files\启幕服务器`), plus the install roots the
/// three scripts recorded in the registry (`InstallDir`), which
/// cover custom install paths the fixed list cannot know. The
/// standard directory itself is excluded (it is the relocation
/// target, not a candidate). Note `D:\Qimu` is deliberately
/// absent: that root belongs to the offline *cache*
/// (`platform_cache_default`), no version ever rooted the
/// library there.
pub fn candidate_library_dirs() -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = Vec::new();
    let mut push = |p: PathBuf| {
        if !out.contains(&p) {
            out.push(p);
        }
    };
    push(crate::exe_dir().join("qimu-data"));
    #[cfg(target_os = "windows")]
    {
        let program_files = std::env::var("ProgramFiles")
            .ok()
            .filter(|s| !s.trim().is_empty())
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("C:\\Program Files"));
        for base in [PathBuf::from("D:\\Program Files"), program_files] {
            for app in ["启幕客户端", "启幕", "启幕服务器"] {
                push(base.join(app).join("qimu-data"));
            }
        }
        // Historical install roots the fixed list above cannot
        // know: wherever an older installer actually put the app
        // (a custom path chosen in the wizard). All three NSIS
        // scripts record it as `InstallDir` under their HKLM key
        // and their `.onInit` reads it back on upgrade — the
        // client now reads the same keys for discovery, so a
        // library left in a custom install dir is found too
        // (2026-11-25 rescan line; before this, such a library
        // was invisible to discovery no matter its content).
        for root in registry_install_dirs() {
            push(root.join("qimu-data"));
        }
    }
    let standard = standard_library_dir();
    out.retain(|p| *p != standard);
    out
}

/// Install roots recorded by the three NSIS packages
/// (`HKLM\Software\QimuClient` / `Qimu` / `QimuServer`, value
/// `InstallDir`). Read through `reg.exe` — the same
/// standard-tool subprocess pattern `pid_alive` uses for
/// tasklist; every failure (key absent, reg missing, unparsable
/// output) reads as "no recorded root", never as an error.
#[cfg(target_os = "windows")]
fn registry_install_dirs() -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = Vec::new();
    for key in ["QimuClient", "Qimu", "QimuServer"] {
        let mut cmd = std::process::Command::new("reg");
        cmd.args([
            "query",
            &format!("HKLM\\Software\\{key}"),
            "/v",
            "InstallDir",
        ])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null());
        {
            use std::os::windows::process::CommandExt;
            cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
        }
        let Ok(o) = cmd.output() else { continue };
        if !o.status.success() {
            continue;
        }
        // Output line shape: `    InstallDir    REG_SZ    D:\Apps\启幕`.
        // Split on the type token rather than on columns of
        // spaces (the path itself may contain spaces).
        let text = String::from_utf8_lossy(&o.stdout);
        for line in text.lines() {
            if let Some((_, path)) = line.split_once("REG_SZ") {
                let p = path.trim();
                if !p.is_empty() {
                    let pb = PathBuf::from(p);
                    if !out.contains(&pb) {
                        out.push(pb);
                    }
                }
            }
        }
    }
    out
}

/// Is a live client holding this library? A candidate whose lock
/// names a still-running pid is skipped by discovery: copying a
/// database another process is writing could tear the copy (its
/// counts check would likely catch it, but there is no reason to
/// race a live writer when the fallback — leaving it alone — is
/// always safe).
fn library_in_use(dir: &std::path::Path) -> bool {
    let lock = dir.join("qimu-local.lock");
    if !lock.exists() {
        return false;
    }
    match read_lock_pid(&lock) {
        Some(pid) => pid_alive(pid),
        None => false,
    }
}

/// How the resolved library dir was arrived at (settings display +
/// the startup disposition hook).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum LibraryOrigin {
    /// The bootstrap config's `data_dir`.
    Bootstrap,
    /// The legacy `library_dir` client setting.
    LegacySetting,
    /// A data-dir environment variable (`QIMU_CLIENT_DATA_DIR`,
    /// or the legacy `QIMU_DATA_DIR`).
    Env,
    /// The standard directory (already populated, or fresh).
    Standard,
    /// An old library was relocated into the standard directory.
    Migrated,
    /// Relocation failed; the old library is used in place.
    FallbackSource,
}

/// The outcome of the once-per-process library resolution.
#[derive(Clone)]
pub struct LibraryResolution {
    pub dir: PathBuf,
    pub origin: LibraryOrigin,
    /// The source of a verified relocation into the standard
    /// directory — the one directory the disposition policy may
    /// act on. Never set on the fallback path: there the source
    /// IS the live library and no policy may delete it.
    pub migrated_from: Option<PathBuf>,
}

pub fn library_resolution() -> LibraryResolution {
    static RES: std::sync::OnceLock<LibraryResolution> = std::sync::OnceLock::new();
    RES.get_or_init(resolve_library).clone()
}

/// The fullest content-bearing library waiting in a historical
/// location, if any: candidates another client is writing are
/// skipped (see `library_in_use`), content-free candidates never
/// qualify, and the fullest wins (ties broken by the database's
/// modification time). Shared by the standard-directory
/// discovery and the empty-configured-library floor.
fn discover_best_candidate() -> Option<PathBuf> {
    let mut best: Option<(LibraryCounts, std::time::SystemTime, PathBuf)> = None;
    for cand in candidate_library_dirs() {
        if library_in_use(&cand) {
            continue;
        }
        let Some(c) = probe_library(&cand) else {
            continue;
        };
        if !c.has_content() {
            continue;
        }
        let mtime = std::fs::metadata(cand.join("qimu.db"))
            .and_then(|m| m.modified())
            .unwrap_or(std::time::SystemTime::UNIX_EPOCH);
        let better = match &best {
            None => true,
            Some((bc, bm, _)) => (c.rank(), mtime) > (bc.rank(), *bm),
        };
        if better {
            best = Some((c, mtime, cand));
        }
    }
    best.map(|(_, _, src)| src)
}

/// The empty-configured-library floor (2026-11-25 rescan line):
/// relocate the fullest discovered old library into `target` —
/// an explicitly configured library root that currently reads
/// empty — and resolve to `target`. On relocation failure,
/// resolve to the old library *in place* instead (never to the
/// empty target), leaving a notice; the configured path is kept
/// unchanged so the next launch retries the move. `None` when
/// no candidate holds content — the caller then opens the empty
/// target as a fresh library, exactly as before. On success the
/// resolution carries `migrated_from`, so the stored
/// disposition policy applies to the source exactly as it does
/// for a standard-directory migration; because the trigger is
/// "target reads empty", a completed relocation can never
/// re-trigger (the target is non-empty from then on).
fn rescue_into_configured(
    target: &std::path::Path,
    boot: &Bootstrap,
    origin: LibraryOrigin,
) -> Option<LibraryResolution> {
    // Historical locations first (fullest wins); the standard
    // directory is the last-resort source — it is the relocation
    // *target* of the unconfigured flow, never a candidate, but
    // a configured root that reads empty while the standard
    // directory holds the user's library (a settings experiment
    // left `data_dir` pointing at a fresh path) must not strand
    // that library either.
    let src = discover_best_candidate().or_else(|| {
        let std_dir = standard_library_dir();
        if std_dir != target
            && !library_in_use(&std_dir)
            && probe_library(&std_dir).map(|c| c.has_content()).unwrap_or(false)
        {
            Some(std_dir)
        } else {
            None
        }
    })?;
    if src == target {
        return None;
    }
    match relocate_library(&src, target) {
        Ok(_) => Some(LibraryResolution {
            dir: target.to_path_buf(),
            origin,
            migrated_from: Some(src),
        }),
        Err(e) => {
            eprintln!(
                "[qimu] 旧库搬入配置目录失败（{e}），改按原目录打开：{}",
                src.display()
            );
            let mut nb = boot.clone();
            nb.relocate_notice = Some(format!(
                "在旧位置 {} 找到有内容的旧库，但搬入数据目录 {} 失败（{e}）；已按原目录打开，数据没有丢失，数据目录设置未变更，下次启动会重试搬迁",
                src.display(),
                target.display()
            ));
            write_bootstrap(&nb);
            Some(LibraryResolution {
                dir: src,
                origin: LibraryOrigin::FallbackSource,
                migrated_from: None,
            })
        }
    }
}

/// Record the running build's version in the bootstrap config
/// (see `Bootstrap::app_version`). Runs after resolution, as a
/// read-modify-write, so it preserves whatever the resolution
/// itself just wrote (a new `data_dir`, a notice).
fn stamp_app_version() {
    let cur = env!("CARGO_PKG_VERSION");
    let mut b = read_bootstrap();
    if b.app_version.as_deref() != Some(cur) {
        b.app_version = Some(cur.to_string());
        write_bootstrap(&b);
    }
}

fn resolve_library() -> LibraryResolution {
    let resolved = resolve_library_inner();
    stamp_app_version();
    resolved
}

fn resolve_library_inner() -> LibraryResolution {
    let res = |dir: PathBuf, origin: LibraryOrigin, migrated_from: Option<PathBuf>| {
        LibraryResolution {
            dir,
            origin,
            migrated_from,
        }
    };
    // 0. A settings-registered data-directory move executes
    //    first (audit P1): this is the one moment the source
    //    library is guaranteed static — no store is open yet —
    //    so the copy can never fork away from the running
    //    session's writes the way a live copy did. A move whose
    //    target meanwhile gained a library of its own is
    //    cancelled, never forced over it; a failed move keeps the
    //    old directory and leaves a notice for the user.
    let mut boot = read_bootstrap();
    if let Some(pend) = boot.pending_relocate.clone() {
        let from = PathBuf::from(&pend.from);
        let to = PathBuf::from(&pend.to);
        let target_taken = probe_library(&to)
            .map(|c| c.has_content())
            .unwrap_or(false);
        if target_taken {
            boot.pending_relocate = None;
            boot.relocate_notice = Some(format!(
                "数据目录搬迁已取消：目标目录 {} 里已有另一个启幕库，未做任何改动",
                to.display()
            ));
            write_bootstrap(&boot);
        } else {
            match relocate_library(&from, &to) {
                Ok(_) => {
                    boot.data_dir = Some(to.to_string_lossy().into_owned());
                    boot.pending_relocate = None;
                    boot.relocate_notice = Some(format!(
                        "数据目录已搬迁到 {}（内容核对通过）",
                        to.display()
                    ));
                    write_bootstrap(&boot);
                    return res(to, LibraryOrigin::Bootstrap, None);
                }
                Err(e) => {
                    boot.pending_relocate = None;
                    if boot.data_dir.is_none() {
                        boot.data_dir = Some(from.to_string_lossy().into_owned());
                    }
                    boot.relocate_notice = Some(format!(
                        "数据目录搬迁失败：{e}；仍在使用原目录 {}",
                        from.display()
                    ));
                    write_bootstrap(&boot);
                }
            }
        }
    }
    // 1. The bootstrap config wins — but with a floor under it
    //    (2026-11-25 rescan line). The pre-floor rule adopted the
    //    configured path whenever it was missing, empty, or held
    //    any `qimu.db` at all, and skipped discovery entirely:
    //    a configured directory an intermediate build had opened
    //    once (leaving a valid but content-free `qimu.db`), or a
    //    configured path whose directory was later moved away,
    //    therefore opened onto an empty library forever while
    //    the user's real library sat undiscovered in a historical
    //    location. Now the configured path is adopted outright
    //    only when it provably holds content — or when its
    //    database will not even probe (corrupt, hot journal):
    //    not provably empty, so it is adopted as before and
    //    Store::open surfaces whatever is wrong. When it reads
    //    as an *empty* library root — no directory yet, an empty
    //    directory, or a library with zero content — discovery
    //    runs first (`rescue_into_configured`), and a
    //    content-bearing old library is relocated into the
    //    configured place: the user's chosen home stays the
    //    home. A path that exists, is non-empty and holds no
    //    `qimu.db` is still NOT adopted (audit P4): it may be a
    //    stray folder sitting where the library used to live —
    //    discovery runs, and the configured path only gets its
    //    fresh library when discovery finds nothing anywhere.
    let mut configured_stray: Option<PathBuf> = None;
    if let Some(d) = boot.data_dir.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        let p = PathBuf::from(d);
        let probed = probe_library(&p);
        if p.exists() && !p.is_dir() {
            // a file: invalid, fall through
        } else if p.join("qimu.db").is_file() && probed.is_none() {
            return res(p, LibraryOrigin::Bootstrap, None);
        } else if probed.map(|c| c.has_content()).unwrap_or(false) {
            return res(p, LibraryOrigin::Bootstrap, None);
        } else if !p.exists() || dir_is_empty(&p) || p.join("qimu.db").is_file() {
            if let Some(r) = rescue_into_configured(&p, &boot, LibraryOrigin::Bootstrap) {
                return r;
            }
            return res(p, LibraryOrigin::Bootstrap, None);
        } else {
            configured_stray = Some(p);
        }
    }
    // 2. The legacy client setting (design §6.2 hook; predates the
    //    bootstrap file, kept so a hand-set value keeps working).
    if let Some(p) = crate::client_settings()
        .get("library_dir")
        .and_then(|v| v.as_str())
        .filter(|s| !s.trim().is_empty())
    {
        let p = PathBuf::from(p);
        // The same empty-library floor as the bootstrap path
        // (2026-11-25 rescan line): a legacy-configured root
        // that reads empty must not short-circuit discovery
        // either. Anything else about this compat hook is
        // unchanged.
        let reads_empty = match probe_library(&p) {
            Some(c) => !c.has_content(),
            None => !p.join("qimu.db").is_file() && (!p.exists() || dir_is_empty(&p)),
        };
        if reads_empty {
            if let Some(r) = rescue_into_configured(&p, &boot, LibraryOrigin::LegacySetting) {
                return r;
            }
        }
        return res(p, LibraryOrigin::LegacySetting, None);
    }
    // 3. An explicit data-dir environment variable: the
    //    client-specific QIMU_CLIENT_DATA_DIR first, the legacy
    //    QIMU_DATA_DIR as fallback (audit2 F-N2: the legacy name is
    //    shared with the server's data root, so a machine-wide
    //    setting could collide the two onto one directory; the
    //    specific names win whenever both are set).
    for var in ["QIMU_CLIENT_DATA_DIR", "QIMU_DATA_DIR"] {
        if let Ok(d) = std::env::var(var) {
            if !d.trim().is_empty() {
                return res(PathBuf::from(d), LibraryOrigin::Env, None);
            }
        }
    }
    // 4. The standard directory — unless it is still empty, in
    //    which case an older version's library may be waiting in a
    //    historical location: discover, relocate, verify.
    let standard = standard_library_dir();
    if probe_library(&standard).map(|c| c.has_content()).unwrap_or(false) {
        return res(standard, LibraryOrigin::Standard, None);
    }
    if let Some(src) = discover_best_candidate() {
        match relocate_library(&src, &standard) {
            Ok(_) => {
                let mut nb = boot.clone();
                nb.data_dir = Some(standard.to_string_lossy().into_owned());
                write_bootstrap(&nb);
                return res(standard, LibraryOrigin::Migrated, Some(src));
            }
            Err(e) => {
                // Fallback: claim the old library in place. It is
                // now the live library — `migrated_from` stays None
                // so no disposition policy can ever delete it.
                eprintln!(
                    "[qimu] 旧库搬迁失败（{e}），改按原目录打开：{}",
                    src.display()
                );
                let mut nb = boot.clone();
                nb.data_dir = Some(src.to_string_lossy().into_owned());
                write_bootstrap(&nb);
                return res(src, LibraryOrigin::FallbackSource, None);
            }
        }
    }
    if let Some(p) = configured_stray {
        // Discovery found nothing anywhere: the configured path
        // gets the fresh library after all (it held no library,
        // so nothing is orphaned by opening one there).
        return res(p, LibraryOrigin::Bootstrap, None);
    }
    res(standard, LibraryOrigin::Standard, None)
}

/// Whether a directory holds no entries at all. An unreadable
/// directory counts as non-empty (never adopted sight unseen).
fn dir_is_empty(dir: &std::path::Path) -> bool {
    std::fs::read_dir(dir)
        .map(|mut it| it.next().is_none())
        .unwrap_or(false)
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

/// The nine heavy dispatch arms of the phase-1 measurement plan
/// (design §4.1 M2): routes that hold the store lock through
/// second-scale CPU/IO work. Matched on the query-stripped path.
fn heavy_arm(method: &str, path: &str) -> bool {
    let p = path.split('?').next().unwrap_or(path);
    matches!(
        (method, p),
        ("POST", "/v1/import/open")
            | ("POST", "/v1/import/group")
            | ("POST", "/v1/import/lazy-open")
            | ("POST", "/v1/import")
            | ("POST", "/v1/attachments")
            | ("GET", "/v1/trash")
    ) || (method == "POST"
        && p.starts_with("/v1/versions/")
        && (p.ends_with("/edit-pages") || p.ends_with("/reorder-pages")))
        || (method == "POST"
            && p.starts_with("/v1/renditions/")
            && p.ends_with("/seed-working"))
        || (method == "POST" && p.starts_with("/v1/folders/") && p.ends_with("/delete"))
        || (method == "POST" && p.starts_with("/v1/indexes/") && p.ends_with("/export"))
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

    /// Phase-1 measurement M1 (design §4.1): the store Mutex is
    /// the client's single serialization point — time the wait
    /// at the measured acquisition sites (the four list methods,
    /// index_detail, and the dispatch heavy arms) and log waits
    /// over 5ms to the perf log. Zero cost when QIMU_PERF_LOG is
    /// off beyond the Instant read itself.
    fn lock_timed(&self, what: &str) -> std::sync::MutexGuard<'_, Store> {
        let t = std::time::Instant::now();
        let guard = self.store.lock().unwrap();
        let waited = t.elapsed().as_millis();
        if waited > 5 {
            crate::perf_log(&format!("lock_wait {what} {waited}ms"));
        }
        guard
    }

    /// GET /v1/folders shape: bare folder records.
    pub fn list_folders(&self) -> Vec<Value> {
        let store = self.lock_timed("list_folders");
        store.list_folders().map(values_of).unwrap_or_default()
    }

    /// Folder sync (user ruling 2026-11-25): copy the server
    /// folders the user picked in the sync dialog into the local
    /// library. Id-based reconciliation lives in the core store
    /// (existing ids are skipped, never overwritten); returns how
    /// many folders were newly inserted.
    pub fn sync_server_folders(&self, folders: &[FolderRec]) -> usize {
        let store = self.lock_timed("sync_server_folders");
        store.sync_server_folders(folders).unwrap_or(0)
    }

    /// GET /v1/groups shape: bare group records (the local owner is
    /// the seeded super user, so no visibility filtering applies).
    pub fn list_groups(&self) -> Vec<Value> {
        let store = self.lock_timed("list_groups");
        store.list_groups().map(values_of).unwrap_or_default()
    }

    /// GET /v1/indexes shape: bare index records.
    pub fn list_indexes(&self) -> Vec<Value> {
        let store = self.lock_timed("list_indexes");
        store.list_indexes(None).map(values_of).unwrap_or_default()
    }

    /// GET /v1/showroom shape for THIS library's own showroom
    /// rows (the local showroom surface): rows the bundled-server
    /// era wrote into this library's showroom table (reference
    /// submissions and approval landings against what was then a
    /// server data root), invisible to every client list until
    /// now — `refresh_local` never read the table and the local
    /// dispatch owned no showroom route. The server handler's
    /// shaping is mirrored field for field: bare record,
    /// `submitted_by_id` alongside the display name, and
    /// `resolved_entries` resolved through the item's publish
    /// pins, so the deck face consumes these items exactly like
    /// organization items. All states are returned (this is the
    /// owner's own library; the state field drives the card's
    /// status label, and the trash overlay in `list_showroom`
    /// already hides trashed rows).
    pub fn list_local_showroom(&self) -> Vec<Value> {
        let store = self.store.lock().unwrap();
        let items = store.list_showroom(None).unwrap_or_default();
        let submitter_names: std::collections::HashMap<String, String> = store
            .list_users()
            .map(|us| us.into_iter().map(|u| (u.id, u.display_name)).collect())
            .unwrap_or_default();
        items
            .into_iter()
            .map(|item| {
                let commit = match &item.commit_id {
                    Some(cid) => store.get_commit(cid).ok(),
                    None => item
                        .source_index_id
                        .as_ref()
                        .and_then(|id| store.branch_head(id, "main").ok().flatten())
                        .and_then(|head| store.get_commit(&head).ok()),
                };
                let entries = match &commit {
                    Some(c) => {
                        let pins = store.showroom_pins(&item.id).unwrap_or_default();
                        resolved_entries_pinned(&store, c, &pins)
                    }
                    None => Vec::new(),
                };
                let mut value = serde_json::to_value(&item).unwrap_or(Value::Null);
                value["submitted_by_id"] = json!(item.submitted_by.clone());
                if let Some(name) = submitter_names.get(&item.submitted_by) {
                    value["submitted_by"] = json!(name);
                }
                value["resolved_entries"] = json!(entries);
                value
            })
            .collect()
    }

    /// N4 personal reorder copy (user ruling 2026-11-25):
    /// save a reordered page sequence of an open deck as a NEW
    /// item of THIS library — the composition lives in core
    /// (`create_reordered_showroom_copy`: personal index +
    /// locked entries + pins + showroom row, one transaction);
    /// this wrapper supplies the library's owner/group and
    /// returns the created row in exactly the shape
    /// `list_local_showroom` feeds the wall, so the caller can
    /// offer a jump straight into the copy. A deck whose
    /// versions this library does not hold (an organization
    /// deck whose content lives only on its server) fails in
    /// core's read phase with the reason — the copy is a
    /// local-library object by design, and nothing is written.
    pub fn save_reordered_showroom_copy(
        &self,
        title: &str,
        intro: &str,
        pages: &[(String, usize)],
    ) -> Result<Value, String> {
        let item_id = {
            let store = self.store.lock().unwrap();
            store
                .create_reordered_showroom_copy(
                    title,
                    intro,
                    &self.owner.id,
                    &self.default_group_id,
                    &self.owner.display_name,
                    pages,
                )?
                .id
        };
        self.list_local_showroom()
            .into_iter()
            .find(|v| v.get("id").and_then(|x| x.as_str()) == Some(item_id.as_str()))
            .ok_or_else(|| "副本已保存，但在本地演示列表中未找到".to_string())
    }

    /// One-lock snapshot of the four working-data lists
    /// (folders / groups / units / indexes — the same shapes the
    /// individual list methods return), taken with `try_lock`:
    /// when the store is busy — an import worker holding it
    /// through a stage — this returns None instead of queueing
    /// behind the lock. The import `Opened` beat refreshes
    /// through here so the UI thread never parks on the import's
    /// lock (see `App::refresh_opened` in main.rs).
    pub fn try_snapshot(&self) -> Option<(Vec<Value>, Vec<Value>, Vec<Value>, Vec<Value>)> {
        let store = self.store.try_lock().ok()?;
        let folders = store.list_folders().map(values_of).unwrap_or_default();
        let groups = store.list_groups().map(values_of).unwrap_or_default();
        let units: Vec<Value> = store
            .list_units(None)
            .unwrap_or_default()
            .iter()
            .map(|u| unit_json(&store, u))
            .collect();
        let indexes = store.list_indexes(None).map(values_of).unwrap_or_default();
        Some((folders, groups, units, indexes))
    }

    /// GET /v1/units shape: the server's `unit_json` wrapper —
    /// `{unit, renditions: [{rendition, latest, versions}]}`.
    /// (The REST list endpoint leaves `versions` empty and only
    /// its detail endpoint fills it; the local face fills it in
    /// both — P1 — because the version strips read it from the
    /// list data.)
    pub fn list_units(&self) -> Vec<Value> {
        let store = self.lock_timed("list_units");
        let units = store.list_units(None).unwrap_or_default();
        units.iter().map(|u| unit_json(&store, u)).collect()
    }

    // ---------- undo snapshots & restores (client Ctrl+Z) ----------
    //
    // The typed faces of the core store's undo support (see
    // Store::snapshot_unit_positions and neighbours). The App's
    // session undo stack captures pre-operation snapshots
    // through the snapshot faces BEFORE an operation runs and
    // hands them to the restore faces when the user undoes it;
    // a restore's Err is the guard's refusal wording, shown to
    // the user verbatim.

    pub fn undo_snapshot_unit_positions(&self) -> Vec<UnitPosition> {
        let store = self.store.lock().unwrap();
        store.snapshot_unit_positions().unwrap_or_default()
    }

    pub fn undo_restore_unit_positions(&self, rows: &[UnitPosition]) -> Result<(), String> {
        let store = self.store.lock().unwrap();
        store.restore_unit_positions(rows)
    }

    pub fn undo_snapshot_unit(&self, unit_id: &str) -> Option<UnitSnapshot> {
        let store = self.store.lock().unwrap();
        store.snapshot_unit(unit_id).ok()
    }

    /// The version a merge into this unit would extend (the
    /// merge's own resolution), snapshotted verbatim.
    pub fn undo_snapshot_working_version(&self, unit_id: &str) -> Option<VersionRowSnap> {
        let store = self.store.lock().unwrap();
        store.snapshot_working_version(unit_id).ok().flatten()
    }

    pub fn undo_snapshot_version(&self, version_id: &str) -> Option<VersionRowSnap> {
        let store = self.store.lock().unwrap();
        store.snapshot_version_row(version_id).ok()
    }

    pub fn undo_merge(
        &self,
        source: &UnitSnapshot,
        pre: &VersionRowSnap,
        post: &VersionRowSnap,
    ) -> Result<(), String> {
        let store = self.store.lock().unwrap();
        store.undo_merge(source, pre, post)
    }

    pub fn undo_trash_find(&self, kind: &str, object_id: &str) -> Option<TrashRec> {
        let store = self.store.lock().unwrap();
        store.trash_find(kind, object_id).ok().flatten()
    }

    pub fn undo_trash_restore(&self, rows: &[TrashRec]) -> Result<(), String> {
        let store = self.store.lock().unwrap();
        store.undo_trash_restore(rows)
    }

    pub fn undo_snapshot_folder_positions(&self, scope: &str) -> Vec<FolderPosition> {
        let store = self.store.lock().unwrap();
        store.snapshot_folder_positions(scope).unwrap_or_default()
    }

    pub fn undo_restore_folder_positions(
        &self,
        rows: &[FolderPosition],
    ) -> Result<(), String> {
        let store = self.store.lock().unwrap();
        store.restore_folder_positions(rows)
    }

    pub fn undo_snapshot_folder_delete(
        &self,
        id: &str,
        trash_mode: bool,
    ) -> Option<FolderDeleteSnapshot> {
        let store = self.store.lock().unwrap();
        let mode = if trash_mode {
            FolderDeleteMode::Trash
        } else {
            FolderDeleteMode::Lift
        };
        store.snapshot_folder_delete(id, mode).ok()
    }

    pub fn undo_restore_folder_delete(
        &self,
        snap: &FolderDeleteSnapshot,
    ) -> Result<(), String> {
        let store = self.store.lock().unwrap();
        store.restore_folder_delete(snap)
    }

    pub fn undo_folder_has_contents(&self, id: &str) -> bool {
        let store = self.store.lock().unwrap();
        store.folder_has_contents(id).unwrap_or(true)
    }

    // ---------- redo captures & restores (client Ctrl+Y) ----------
    //
    // The symmetric faces of the undo support above: an undo
    // captures the CURRENT (post-operation) state through these
    // before it restores the pre-image, and a redo hands its
    // entry to the matching restore — which is direction-free,
    // so several undo faces serve both ways and only the
    // genuinely forward restores are new in core.

    pub fn redo_snapshot_folder_row(&self, id: &str) -> Option<FolderRec> {
        let store = self.store.lock().unwrap();
        store.snapshot_folder_row(id).ok().flatten()
    }

    pub fn redo_restore_folder_row(&self, f: &FolderRec) -> Result<(), String> {
        let store = self.store.lock().unwrap();
        store.restore_folder_row(f)
    }

    pub fn redo_snapshot_version_page_ids(&self, version_id: &str) -> Option<Vec<String>> {
        let store = self.store.lock().unwrap();
        store.snapshot_version_page_ids(version_id).ok()
    }

    pub fn redo_merge(
        &self,
        source: &UnitSnapshot,
        pre: &VersionRowSnap,
        post: &VersionRowSnap,
    ) -> Result<(), String> {
        let store = self.store.lock().unwrap();
        store.redo_merge(source, pre, post)
    }

    pub fn redo_trash_apply(&self, rows: &[TrashRec]) -> Result<Vec<TrashRec>, String> {
        let store = self.store.lock().unwrap();
        store.redo_trash_apply(rows)
    }

    /// Re-run a folder delete the undo reversed; `deleted_by`
    /// follows the local dispatch's own delete arm (the library
    /// owner's display name).
    pub fn redo_folder_delete(
        &self,
        snap: &FolderDeleteSnapshot,
    ) -> Result<FolderDeleteSnapshot, String> {
        let store = self.store.lock().unwrap();
        store.redo_folder_delete(snap, &self.owner.display_name)
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

    /// The whole-deck backing of a page whose lazy-import split
    /// has not landed yet: (deck file hash, slide index). None
    /// once the page is an entity (or was never lazily imported).
    pub fn lazy_backing_for_page(&self, page_id: &str) -> Option<(String, usize)> {
        let store = self.store.lock().unwrap();
        store.lazy_backing_for_page(page_id).ok().flatten()
    }

    /// Raw file-store bytes by content hash (the lazy backing's
    /// deck file, among other callers).
    pub fn file_bytes(&self, hash: &str) -> Option<Vec<u8>> {
        let store = self.store.lock().unwrap();
        store.file_get(hash).ok()
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

    /// Snapshot the local library database (`VACUUM INTO`) and all files
    /// under `files/` into `dest`.
    pub fn backup_to(&self, dest: &std::path::Path) -> Result<usize, String> {
        let store = self.lock_timed("backup_to");
        store.backup_to(dest)
    }

    /// The index detail the REST `GET /v1/indexes/{id}` returns for
    /// a named session — `{index, branches, tags, head,
    /// resolved_entries}` — assembled from the local store (the
    /// guest entry funnel does not exist locally: the library is
    /// the owner's own). None when the index is not in this
    /// library. Used by the refresh per-index pass and by the
    /// working-data dispatcher below.
    pub fn index_detail(&self, id: &str, branch: &str) -> Option<Value> {
        let store = self.lock_timed("index_detail");
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
    /// server even in local mode (organization showroom items) — so the
    /// caller falls through to REST unchanged. The arms added in
    /// P4 additionally fall through when the target object is not
    /// in this library at all, so a stray organization object can
    /// never be answered from the wrong store. The trash arm owns
    /// showroom rows this library itself holds (its own showroom
    /// table); organization showroom items fall through the same
    /// way, by the same ownership test.
    pub fn dispatch(
        &self,
        method: &str,
        path: &str,
        body: Option<&Value>,
    ) -> Option<Result<Value, String>> {
        // Phase-1 measurement M2 (design §4.1): end-to-end timing
        // of the nine heavy arms — the routes that hold the store
        // lock through CPU/IO work — logged over 20ms when
        // QIMU_PERF_LOG is on. All other routes pass through
        // untimed (a single env-flag read).
        if !crate::perf_enabled() {
            return self.dispatch_impl(method, path, body);
        }
        let t = std::time::Instant::now();
        let r = self.dispatch_impl(method, path, body);
        let ms = t.elapsed().as_millis();
        if ms > 20 && heavy_arm(method, path) {
            crate::perf_log(&format!("arm {method} {path} {ms}ms"));
        }
        r
    }

    fn dispatch_impl(
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
                    let store = self.lock_timed("POST /v1/import/open");
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
                    let store = self.lock_timed("POST /v1/import/group");
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
                // Lazy import (user ruling 2026-11-25): open the
                // deck WITHOUT splitting — wrappers land on the
                // wall immediately, backed by the whole deck — and
                // split it afterwards, one page per call. Local
                // face only; the server's import routes stay
                // one-shot this round.
                ("POST", ["v1", "import", "lazy-open"]) => {
                    let bytes = b64_field(&body, "data_base64")?;
                    let blocks = import_block_specs(&body);
                    let store = self.lock_timed("POST /v1/import/lazy-open");
                    let opened = store.lazy_open(
                        &str_field(&body, "filename").unwrap_or_default(),
                        &bytes,
                        opt_str_field(&body, "folder_id"),
                        &str_field(&body, "group_id").unwrap_or_default(),
                        &self.owner.display_name,
                        blocks.as_deref(),
                    )?;
                    Ok(serde_json::to_value(&opened).unwrap_or_default())
                }
                ("POST", ["v1", "import", "lazy-list"]) => {
                    let store = self.store.lock().unwrap();
                    let states = store.lazy_states()?;
                    Ok(serde_json::to_value(&states).unwrap_or_default())
                }
                // One page per call: the store lock is held for a
                // single page's split, never across a whole deck,
                // so the UI thread never parks behind the worker
                // (the freeze line's F3 discipline).
                ("POST", ["v1", "import", "lazy-split"]) => {
                    let index = body.get("index").and_then(|v| v.as_u64()).unwrap_or(0) as usize;
                    let store = self.store.lock().unwrap();
                    let outcome = store.lazy_split_page(
                        &str_field(&body, "pending_id").unwrap_or_default(),
                        index,
                    )?;
                    Ok(serde_json::to_value(&outcome).unwrap_or_default())
                }
                // The split worker's retry cap landed: mark the
                // deck failed with the failing step's own error
                // text, so a deterministically failing page ends
                // in the failed presentation instead of spinning
                // the queue forever.
                ("POST", ["v1", "import", "lazy-fail"]) => {
                    let store = self.store.lock().unwrap();
                    let state = store.lazy_fail(
                        &str_field(&body, "pending_id").unwrap_or_default(),
                        &str_field(&body, "error").unwrap_or_default(),
                    )?;
                    Ok(serde_json::to_value(&state).unwrap_or_default())
                }
                ("POST", ["v1", "import", "lazy-forget"]) => {
                    let store = self.store.lock().unwrap();
                    store.delete_pending_import(
                        &str_field(&body, "pending_id").unwrap_or_default(),
                    )?;
                    Ok(json!({ "ok": true }))
                }
                ("POST", ["v1", "import"]) => {
                    let bytes = b64_field(&body, "data_base64")?;
                    let blocks = import_block_specs(&body);
                    let store = self.lock_timed("POST /v1/import");
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
                ("POST", ["v1", "units", "move"]) => {
                    // Content-wall card drag-and-drop (server
                    // `move_units` handler): rehome + reseat a
                    // batch of units; order persists in the
                    // units.sort renumber inside the store.
                    let store = self.store.lock().unwrap();
                    let ids: Vec<String> = body
                        .get("ids")
                        .and_then(|v| v.as_array())
                        .map(|a| {
                            a.iter()
                                .filter_map(|x| x.as_str().map(|s| s.to_string()))
                                .collect()
                        })
                        .unwrap_or_default();
                    store.move_units(
                        &ids,
                        opt_str_field(&body, "folder_id"),
                        opt_str_field(&body, "before_id"),
                    )?;
                    Ok(json!({ "ok": true }))
                }
                ("GET", ["v1", "units", id]) => {
                    // A unit this library does not hold may be an
                    // organization unit (merged search hits, F7,
                    // open through here): per the P4 discipline
                    // the local face must not answer for objects
                    // it does not own — fall through to REST,
                    // which serves it, or 404s in its own words
                    // when the unit truly does not exist.
                    let store = self.store.lock().unwrap();
                    let unit = match store.get_unit(id) {
                        Ok(u) => u,
                        Err(_) => return Err(UNHANDLED.to_string()),
                    };
                    let mut payload = unit_json(&store, &unit);
                    unit_detail_extras(&store, &mut payload, &unit);
                    attach_children(&store, &mut payload, &unit);
                    Ok(payload)
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
                ("POST", ["v1", "units", id, "merge-page"]) => {
                    // Card drag onto a card (server
                    // `merge_unit_page` handler): append the
                    // source single-page unit's page to this
                    // unit's draft and dissolve the source. The
                    // store runs every guard before any write.
                    let store = self.store.lock().unwrap();
                    let ver = store.merge_unit_page_into(
                        id,
                        &str_field(&body, "source_id").unwrap_or_default(),
                        &self.owner.display_name,
                    )?;
                    Ok(json!({ "version": serde_json::to_value(&ver).unwrap_or(Value::Null) }))
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
                ("POST", ["v1", "versions", id, "publish-draft"]) => {
                    // #101 history card 「发布为最新版本」 (server
                    // publish_draft_version): drafts only; the
                    // card's inline publish input may carry a
                    // note that replaces the draft's stored one.
                    let store = self.store.lock().unwrap();
                    store
                        .get_version(id)
                        .map_err(|_| NOT_FOUND.to_string())?;
                    let note = body
                        .get("note")
                        .and_then(|v| v.as_str())
                        .filter(|n| !n.trim().is_empty());
                    let bump = body.get("bump").and_then(|v| v.as_str());
                    let ver = store.publish_draft(
                        id,
                        note,
                        &self.owner.display_name,
                        bump,
                    )?;
                    Ok(json!({
                        "version": serde_json::to_value(&ver).unwrap_or(Value::Null),
                    }))
                }
                ("POST", ["v1", "versions", id, "publish-working"]) => {
                    // Publish governance (server publish_working):
                    // the unnumbered working draft's own row
                    // takes the next minor number in place.
                    let store = self.store.lock().unwrap();
                    if store.get_version(id).is_err() {
                        return Err(UNHANDLED.to_string());
                    }
                    let note = body
                        .get("note")
                        .and_then(|v| v.as_str())
                        .unwrap_or("");
                    let bump = body.get("bump").and_then(|v| v.as_str());
                    let ver = store.publish_working_draft(
                        id,
                        note,
                        &self.owner.display_name,
                        bump,
                    )?;
                    Ok(json!({
                        "version": serde_json::to_value(&ver).unwrap_or(Value::Null),
                    }))
                }
                ("POST", ["v1", "versions", id, "insert-at"]) => {
                    // 定号插入 (server insert_working_at): number
                    // the working draft into the line after the
                    // base version, cascade-shifting newer
                    // numbers with ledger records.
                    let store = self.store.lock().unwrap();
                    if store.get_version(id).is_err() {
                        return Err(UNHANDLED.to_string());
                    }
                    let base_id = body
                        .get("base_version_id")
                        .and_then(|v| v.as_str())
                        .unwrap_or("");
                    let (ver, shifts) = store.insert_working_at(
                        id,
                        base_id,
                        &self.owner.display_name,
                    )?;
                    Ok(json!({
                        "version": serde_json::to_value(&ver).unwrap_or(Value::Null),
                        "shifts": shifts
                            .iter()
                            .map(|(from, to)| json!({ "from": from, "to": to }))
                            .collect::<Vec<_>>(),
                    }))
                }
                ("POST", ["v1", "versions", id, "renumber"]) => {
                    // Super-admin manual renumber (server
                    // renumber_version_h). The server face checks
                    // the role; the local face is the owner's own
                    // library and the page only renders the entry
                    // for super — same posture as the other local
                    // governance arms.
                    let store = self.store.lock().unwrap();
                    if store.get_version(id).is_err() {
                        return Err(UNHANDLED.to_string());
                    }
                    let semver = body
                        .get("semver")
                        .and_then(|v| v.as_str())
                        .unwrap_or("");
                    let semver = qimu_core::semver::SemVer::parse(semver)?;
                    store.renumber_version_recorded(
                        id,
                        semver,
                        &self.owner.display_name,
                        "超级管理员手动改号",
                    )?;
                    Ok(json!({ "ok": true }))
                }
                ("POST", ["v1", "versions", id, "reject"]) => {
                    // 未通过 (server reject_version_h): the
                    // not-passed state with its reason.
                    let store = self.store.lock().unwrap();
                    if store.get_version(id).is_err() {
                        return Err(UNHANDLED.to_string());
                    }
                    let reason = body
                        .get("reason")
                        .and_then(|v| v.as_str())
                        .unwrap_or("");
                    store.reject_version(id, reason, &self.owner.display_name)?;
                    Ok(json!({ "ok": true }))
                }
                ("POST", ["v1", "versions", id, "restore"]) => {
                    let store = self.store.lock().unwrap();
                    if store.get_version(id).is_err() {
                        return Err(UNHANDLED.to_string());
                    }
                    store.restore_version(id)?;
                    Ok(json!({ "ok": true }))
                }
                ("POST", ["v1", "renditions", id, "seed-working"]) => {
                    // Birth (or fetch) the rendition's unnumbered
                    // working draft (server seed_working).
                    let store = self.store.lock().unwrap();
                    if store.get_rendition(id).is_err() {
                        return Err(UNHANDLED.to_string());
                    }
                    let ver = store.seed_working_draft(id, &self.owner.display_name)?;
                    Ok(json!({
                        "version": serde_json::to_value(&ver).unwrap_or(Value::Null),
                    }))
                }
                ("POST", ["v1", "units", id, "use-sel"]) => {
                    let store = self.store.lock().unwrap();
                    store.get_unit(id).map_err(|_| NOT_FOUND.to_string())?;
                    let rendition_id = body
                        .get("rendition_id")
                        .and_then(|v| v.as_str())
                        .unwrap_or("");
                    let version_id = body.get("version_id").and_then(|v| v.as_str());
                    store.set_use_sel(id, rendition_id, version_id)?;
                    Ok(json!({ "ok": true }))
                }
                ("POST", ["v1", "units", id, "lang-mount"]) => {
                    let store = self.store.lock().unwrap();
                    store.get_unit(id).map_err(|_| NOT_FOUND.to_string())?;
                    let version_id = body
                        .get("version_id")
                        .and_then(|v| v.as_str())
                        .unwrap_or("");
                    let lang = body
                        .get("lang")
                        .and_then(|v| v.as_str())
                        .unwrap_or("");
                    let state = body
                        .get("state")
                        .and_then(|v| v.as_str())
                        .unwrap_or("");
                    store.set_lang_mount(id, version_id, lang, state)?;
                    Ok(json!({ "ok": true }))
                }
                ("POST", ["v1", "units", id, "line-order"]) => {
                    let store = self.store.lock().unwrap();
                    store.get_unit(id).map_err(|_| NOT_FOUND.to_string())?;
                    let rendition_id = body
                        .get("rendition_id")
                        .and_then(|v| v.as_str())
                        .unwrap_or("");
                    let version_ids: Vec<String> = body
                        .get("version_ids")
                        .and_then(|v| v.as_array())
                        .map(|arr| {
                            arr.iter()
                                .map(|x| x.as_str().unwrap_or("").to_string())
                                .collect()
                        })
                        .unwrap_or_default();
                    let rendition = store
                        .get_rendition(rendition_id)
                        .map_err(|_| NOT_FOUND.to_string())?;
                    if rendition.unit_id != *id {
                        return Err("版本线不属于当前内容块".to_string());
                    }
                    let shifts = store.reorder_versions_recorded(
                        rendition_id,
                        &version_ids,
                        &self.owner.display_name,
                    )?;
                    Ok(json!({
                        "ok": true,
                        "shifts": shifts
                            .into_iter()
                            .map(|(id, from, to)| json!({ "id": id, "from": from, "to": to }))
                            .collect::<Vec<_>>(),
                    }))
                }
                ("POST", ["v1", "versions", id, "reorder-pages"]) => {
                    // #101 wall drag reorder, governed (server
                    // reorder_version_pages): drafts and the
                    // working draft reorder in place; a published
                    // version's drag seeds the working draft and
                    // lands the order there. Like the scripts arm,
                    // a version this library does not hold falls
                    // through to REST.
                    let store = self.store.lock().unwrap();
                    if store.get_version(id).is_err() {
                        return Err(UNHANDLED.to_string());
                    }
                    let page_ids: Vec<String> = body
                        .get("page_ids")
                        .and_then(|v| v.as_array())
                        .map(|arr| {
                            arr.iter()
                                .map(|x| x.as_str().unwrap_or("").to_string())
                                .collect()
                        })
                        .unwrap_or_default();
                    let ver = store.reorder_version_pages(
                        id,
                        &page_ids,
                        &self.owner.display_name,
                    )?;
                    Ok(json!({
                        "version": serde_json::to_value(&ver).unwrap_or(Value::Null),
                    }))
                }
                ("POST", ["v1", "versions", id, "delete"]) => {
                    // #101 history card 「放弃」 on a draft (server
                    // delete_version): core's delete refuses
                    // versions pinned by live showroom entries.
                    let store = self.store.lock().unwrap();
                    store.get_version(id).map_err(|_| NOT_FOUND.to_string())?;
                    store.delete_version(id)?;
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
                    let store = self.lock_timed("POST /v1/versions/:id/edit-pages");
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
                    let store = self.lock_timed("POST /v1/folders/:id/delete");
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
                    let store = self.lock_timed("GET /v1/trash");
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
                        "version" => {
                            // A history version files under its
                            // owning block: the row's title names
                            // the block and the version, its origin
                            // is the block's home — the trash page
                            // groups it with the 页面 category.
                            // The pin guard rides along inside
                            // Store::trash_add.
                            let ver = store
                                .get_version(&object_id)
                                .map_err(|_| NOT_FOUND.to_string())?;
                            let rend = store
                                .get_rendition(&ver.rendition_id)
                                .map_err(|_| NOT_FOUND.to_string())?;
                            let unit = store
                                .get_unit(&rend.unit_id)
                                .map_err(|_| NOT_FOUND.to_string())?;
                            (
                                format!("{} · v{}", unit.name, ver.semver),
                                trash_origin(&store, unit.folder_id.as_deref(), "页面"),
                            )
                        }
                        "showroom" => {
                            // A showroom row THIS library holds
                            // (bundled-server era rows, local
                            // landings) trashes locally, with the
                            // server handler's title and origin
                            // (演示厅). A row this library does not
                            // hold is an organization item — its
                            // trash row is the server's, so fall
                            // through unchanged.
                            let item = match store.get_showroom(&object_id) {
                                Ok(Some(it)) => it,
                                Ok(None) => return Err(UNHANDLED.to_string()),
                                Err(e) => return Err(e),
                            };
                            (item.title.clone(), "演示厅".to_string())
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
                    let store = self.lock_timed("POST /v1/attachments");
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
                    let store = self.lock_timed("POST /v1/indexes/:id/export");
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
                // ---- message center (full version, 2026-11-25):
                // the local mirror's faces. Local-only paths —
                // the server has no /v1/local-messages; when no
                // local library is open these calls fall through
                // and fail there, which the App treats as "no
                // local messages" (declared degradation).
                ("GET", ["v1", "local-messages"]) => {
                    let user = query_param(q, "user").unwrap_or_default();
                    let store = self.store.lock().unwrap();
                    let items = store.list_local_messages(&user)?;
                    Ok(json!({ "items": serde_json::to_value(&items).unwrap_or_default() }))
                }
                ("GET", ["v1", "local-messages", "unread"]) => {
                    let user = query_param(q, "user").unwrap_or_default();
                    let store = self.store.lock().unwrap();
                    let count = store.count_local_unread(&user)?;
                    Ok(json!({ "count": count }))
                }
                // One of this machine's own operation-feedback
                // messages (kind `system`), recorded by the
                // toast path.
                ("POST", ["v1", "local-messages", "system"]) => {
                    let store = self.store.lock().unwrap();
                    let rec = store.insert_local_system_message(
                        &str_field(&body, "title").unwrap_or_default(),
                        &str_field(&body, "body").unwrap_or_default(),
                        &str_field(&body, "user_id").unwrap_or_default(),
                    )?;
                    Ok(serde_json::to_value(&rec).unwrap_or_default())
                }
                // Merge a pulled page of server notifications
                // into the mirror (tombstones + existing ids are
                // skipped inside core).
                ("POST", ["v1", "local-messages", "merge"]) => {
                    let items: Vec<qimu_core::store::NotificationRec> =
                        serde_json::from_value(
                            body.get("items").cloned().unwrap_or_default(),
                        )
                        .map_err(|e| format!("通知数据无效：{e}"))?;
                    let store = self.store.lock().unwrap();
                    let inserted = store.merge_server_notifications(&items)?;
                    Ok(json!({ "inserted": inserted }))
                }
                ("POST", ["v1", "local-messages", "read"]) => {
                    let store = self.store.lock().unwrap();
                    store.mark_local_message_read(
                        &str_field(&body, "id").unwrap_or_default(),
                    )?;
                    Ok(json!({ "ok": true }))
                }
                ("POST", ["v1", "local-messages", "delete"]) => {
                    let store = self.store.lock().unwrap();
                    store.delete_local_message(
                        &str_field(&body, "id").unwrap_or_default(),
                    )?;
                    Ok(json!({ "ok": true }))
                }
                // The clear dialog's three modes; core computes
                // the day cutoff itself. Returns the removed
                // count for the result toast.
                ("POST", ["v1", "local-messages", "clear"]) => {
                    let days = body
                        .get("days")
                        .and_then(|v| v.as_i64())
                        .unwrap_or(7);
                    let store = self.store.lock().unwrap();
                    let removed = store.clear_local_messages(
                        &str_field(&body, "user_id").unwrap_or_default(),
                        &str_field(&body, "mode").unwrap_or_else(|| "all".into()),
                        &str_field(&body, "kind").unwrap_or_default(),
                        days,
                    )?;
                    Ok(json!({ "removed": removed }))
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
            let all = store.list_versions(&r.id).unwrap_or_default();
            // The unnumbered working draft leaves the version
            // list and rides its own field (server unit_json
            // parity): the history line is numbered versions.
            let working = all.iter().find(|v| v.status == "working").cloned();
            let numbered: Vec<_> =
                all.into_iter().filter(|v| v.status != "working").collect();
            // `versions` itself keeps the payload (semver)
            // order every other consumer reads; the arranged
            // display order rides `line_order` alone.
            let line_order: Vec<String> = if numbered.iter().any(|v| v.line_pos.is_some()) {
                let mut arranged = numbered.clone();
                arranged.sort_by_key(|v| v.line_pos.unwrap_or(i64::MAX));
                arranged.iter().map(|v| v.id.clone()).collect()
            } else {
                numbered.iter().map(|v| v.id.clone()).collect()
            };
            let latest = store.latest_version(&r.id).ok().flatten();
            let use_sel = store.get_use_sel(&unit.id, &r.id).ok().flatten();
            rends.push(json!({
                "rendition": serde_json::to_value(r).unwrap_or(Value::Null),
                "latest": serde_json::to_value(latest).unwrap_or(Value::Null),
                "versions": serde_json::to_value(numbered).unwrap_or(Value::Null),
                "working": serde_json::to_value(working).unwrap_or(Value::Null),
                "line_order": line_order,
                "use_sel": use_sel,
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

/// The detail-face payload extras (server unit_json parity):
/// the derived header status badges and the explicit history
/// language mounts. Only the single-unit detail assembly pays
/// for them — list rows and kids rows carry empty arrays.
fn unit_detail_extras(store: &Store, payload: &mut Value, unit: &qimu_core::store::UnitRec) {
    let badges: Vec<Value> = store
        .unit_status_badges(&unit.id)
        .unwrap_or_default()
        .into_iter()
        .map(|(kind, text)| json!({ "kind": kind, "text": text }))
        .collect();
    let mounts = store.list_lang_mounts(&unit.id).unwrap_or_default();
    if let Some(obj) = payload.as_object_mut() {
        obj.insert("status_badges".into(), Value::Array(badges));
        obj.insert(
            "lang_mounts".into(),
            serde_json::to_value(mounts).unwrap_or(Value::Null),
        );
    }
}

/// Cap on the #101 「内嵌内容块」 child list — the server's
/// EMBEDDED_CHILDREN_LIMIT, mirrored (the two detail faces must
/// agree on the payload).
const EMBEDDED_CHILDREN_LIMIT: usize = 100;

/// Attach the `children` array to a unit detail payload (D4) —
/// the server's `attach_children`, over an already-locked store:
/// the units embedded in this one, shaped like list rows so the
/// kids zone renders them as standard cards; each child carries
/// its own embedded units as minimal `{unit}` rows (the zone
/// only counts them). The local library is the owner's own —
/// no visibility filter. No embedding → empty array (no zone).
fn attach_children(store: &Store, payload: &mut Value, unit: &qimu_core::store::UnitRec) {
    let kids = store
        .embedded_children(&unit.id, None, EMBEDDED_CHILDREN_LIMIT)
        .unwrap_or_default();
    let rows: Vec<Value> = kids
        .iter()
        .map(|kid| {
            let mut row = unit_json(store, &kid.unit);
            if let Some(obj) = row.as_object_mut() {
                obj.insert(
                    "children".into(),
                    Value::Array(
                        kid.grandchildren
                            .iter()
                            .map(|g| json!({ "unit": g }))
                            .collect(),
                    ),
                );
            }
            row
        })
        .collect();
    if let Some(obj) = payload.as_object_mut() {
        obj.insert("children".into(), Value::Array(rows));
    }
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

/// One commit's entries resolved through a showroom item's
/// publish pins — the server's `resolved_entries_showroom`,
/// over an already-locked store. The unpinned sibling above is
/// the index face's follow-latest resolution; a showroom item
/// publishes its pinned formal version.
fn resolved_entries_pinned(
    store: &Store,
    commit: &qimu_core::store::CommitRec,
    pins: &[String],
) -> Vec<Value> {
    commit
        .entries
        .iter()
        .enumerate()
        .map(|(pos, e)| {
            let unit = store.get_unit(&e.unit_id.0).ok();
            let rend = store.get_rendition(&e.rendition_id.0).ok();
            let resolved = store
                .resolve_showroom_entry(e, pins.get(pos).map(|s| s.as_str()))
                .ok()
                .flatten();
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
        // A version row's cover is the version itself (read
        // directly — list_versions hides trashed versions, which
        // is exactly what this row is).
        "version" => store.get_version(&row.object_id).ok(),
        "index" => store
            .branch_head(&row.object_id, "main")
            .ok()
            .flatten()
            .and_then(|cid| store.get_commit(&cid).ok())
            .and_then(|c| c.entries.into_iter().next())
            .and_then(|e| store.resolve_entry_version(&e).ok().flatten()),
        "showroom" => store
            .get_showroom_any(&row.object_id)
            .ok()
            .flatten()
            .and_then(|item| {
                let commit = match &item.commit_id {
                    Some(cid) => store.get_commit(cid).ok(),
                    None => item
                        .source_index_id
                        .as_ref()
                        .and_then(|id| store.branch_head(id, "main").ok().flatten())
                        .and_then(|head| store.get_commit(&head).ok()),
                }?;
                let pins = store.showroom_pins(&item.id).ok()?;
                let first = commit.entries.into_iter().next()?;
                store
                    .resolve_showroom_entry(&first, pins.first().map(|s| s.as_str()))
                    .ok()
                    .flatten()
            }),
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

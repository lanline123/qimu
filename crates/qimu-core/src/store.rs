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
    pub fp: String,
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
    let bmap: HashMap<&str, &IndexEntry> =
        bk.iter().map(|k| k.as_str()).zip(base.iter()).collect();
    let omap: HashMap<&str, &IndexEntry> =
        ok.iter().map(|k| k.as_str()).zip(ours.iter()).collect();
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
            match resolutions.and_then(|r| r.get(key.as_str())).map(|s| s.as_str()) {
                Some("ours") => o.cloned(),
                Some("theirs") => t.cloned(),
                _ => {
                    let (unit_id, rendition_id) = key
                        .split_once(':')
                        .map(|(u, rest)| {
                            (u.to_string(), rest.split('#').next().unwrap_or("").to_string())
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
}

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS departments(id TEXT PRIMARY KEY, name TEXT NOT NULL, parent_id TEXT);
CREATE TABLE IF NOT EXISTS users(id TEXT PRIMARY KEY, username TEXT UNIQUE NOT NULL, display_name TEXT NOT NULL, password_hash TEXT NOT NULL, dept_id TEXT, role TEXT NOT NULL DEFAULT 'member', disabled INTEGER NOT NULL DEFAULT 0);
CREATE TABLE IF NOT EXISTS user_depts(user_id TEXT NOT NULL, dept_id TEXT NOT NULL, PRIMARY KEY(user_id, dept_id));
CREATE TABLE IF NOT EXISTS sessions(token TEXT PRIMARY KEY, user_id TEXT NOT NULL, created_ms INTEGER NOT NULL);
CREATE TABLE IF NOT EXISTS folders(id TEXT PRIMARY KEY, name TEXT NOT NULL, parent_id TEXT, sort INTEGER NOT NULL DEFAULT 0, main_index_id TEXT);
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
CREATE TABLE IF NOT EXISTS showroom(id TEXT PRIMARY KEY, title TEXT NOT NULL, intro TEXT NOT NULL DEFAULT '', source_index_id TEXT, commit_id TEXT, submitted_by TEXT NOT NULL, state TEXT NOT NULL DEFAULT 'pending', created_ms INTEGER NOT NULL);
CREATE TABLE IF NOT EXISTS assemblies(id TEXT PRIMARY KEY, user_id TEXT NOT NULL, name TEXT NOT NULL, entries_json TEXT NOT NULL, created_ms INTEGER NOT NULL);
CREATE TABLE IF NOT EXISTS page_bindings(id TEXT PRIMARY KEY, unit_id TEXT NOT NULL, from_version_id TEXT NOT NULL, from_slot INTEGER NOT NULL, to_version_id TEXT NOT NULL, to_slot INTEGER NOT NULL, kind TEXT NOT NULL DEFAULT 'manual', author TEXT NOT NULL, created_ms INTEGER NOT NULL);
CREATE TABLE IF NOT EXISTS custom_groups(id TEXT PRIMARY KEY, name TEXT NOT NULL, owner_id TEXT NOT NULL, created_ms INTEGER NOT NULL);
CREATE TABLE IF NOT EXISTS custom_group_members(group_id TEXT NOT NULL, user_id TEXT NOT NULL, PRIMARY KEY(group_id, user_id));
CREATE TABLE IF NOT EXISTS shares(id TEXT PRIMARY KEY, item_kind TEXT NOT NULL, item_id TEXT NOT NULL, title TEXT NOT NULL, from_user TEXT NOT NULL, target_kind TEXT NOT NULL, target_id TEXT NOT NULL, created_ms INTEGER NOT NULL);
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
        // migrations for databases created before these columns existed
        for (table, col, decl) in [
            ("exports", "deck_id", "TEXT NOT NULL DEFAULT ''"),
            ("exports", "manifest_json", "TEXT NOT NULL DEFAULT ''"),
            ("versions", "scripts_json", "TEXT NOT NULL DEFAULT '[]'"),
            ("folders", "main_index_id", "TEXT"),
        ] {
            let has: bool = conn
                .prepare(&format!("PRAGMA table_info({table})"))
                .and_then(|mut s| {
                    let rows = s.query_map([], |r| r.get::<_, String>(1))?;
                    Ok(rows.collect::<Result<Vec<_>, _>>()?.iter().any(|n| n == col))
                })
                .unwrap_or(false);
            if !has {
                conn.execute_batch(&format!("ALTER TABLE {table} ADD COLUMN {col} {decl}"))
                    .map_err(db_err)?;
            }
        }
        Ok(Store {
            conn,
            root: root.to_path_buf(),
        })
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
        self.root.join("files").join(sub).join(format!("{hash}.bin"))
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
        self.conn
            .execute(
                "INSERT INTO sessions(token, user_id, created_ms) VALUES(?1,?2,?3)",
                params![token, user.id, now_ms()],
            )
            .map_err(db_err)?;
        Ok((token, user))
    }

    pub fn user_by_token(&self, token: &str) -> SResult<UserRec> {
        let uid: String = self
            .conn
            .query_row(
                "SELECT user_id FROM sessions WHERE token=?1",
                params![token],
                |r| r.get(0),
            )
            .map_err(|_| "未登录".to_string())?;
        self.user_by_id(&uid)
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
            match depts.iter().find(|d| d.id == cur).and_then(|d| d.parent_id.clone()) {
                Some(p) => {
                    chain.push(p.clone());
                    cur = p;
                }
                None => break,
            }
        }
        Ok(chain)
    }

    pub fn create_folder(&self, name: &str, parent_id: Option<&str>) -> SResult<FolderRec> {
        let id = new_id("fld");
        let sort: i64 = self
            .conn
            .query_row("SELECT COALESCE(MAX(sort),0)+1 FROM folders", [], |r| r.get(0))
            .map_err(db_err)?;
        self.conn
            .execute(
                "INSERT INTO folders(id, name, parent_id, sort) VALUES(?1,?2,?3,?4)",
                params![id, name, parent_id, sort],
            )
            .map_err(db_err)?;
        Ok(FolderRec {
            id,
            name: name.into(),
            parent_id: parent_id.map(|s| s.to_string()),
            sort,
            main_index_id: None,
        })
    }

    pub fn list_folders(&self) -> SResult<Vec<FolderRec>> {
        let mut stmt = self
            .conn
            .prepare("SELECT id, name, parent_id, sort, main_index_id FROM folders ORDER BY sort, name")
            .map_err(db_err)?;
        let rows = stmt
            .query_map([], |r| {
                Ok(FolderRec {
                    id: r.get(0)?,
                    name: r.get(1)?,
                    parent_id: r.get(2)?,
                    sort: r.get(3)?,
                    main_index_id: r.get(4)?,
                })
            })
            .map_err(db_err)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(db_err)?;
        Ok(rows)
    }

    pub fn rename_folder(&self, id: &str, name: &str) -> SResult<()> {
        self.conn
            .execute("UPDATE folders SET name=?2 WHERE id=?1", params![id, name])
            .map_err(db_err)?;
        Ok(())
    }

    /// Delete a folder only when it is completely empty: no child
    /// folders, no content units and no indexes homed in it.
    pub fn delete_folder(&self, id: &str) -> SResult<()> {
        let count = |sql: &str| -> SResult<i64> {
            self.conn
                .query_row(sql, params![id], |r| r.get(0))
                .map_err(db_err)
        };
        if count("SELECT COUNT(*) FROM folders WHERE parent_id=?1")? > 0 {
            return Err("文件夹下还有子文件夹，不能删除".into());
        }
        if count("SELECT COUNT(*) FROM units WHERE folder_id=?1")? > 0 {
            return Err("文件夹内还有内容块，不能删除".into());
        }
        if count("SELECT COUNT(*) FROM indexes_tbl WHERE folder_id=?1")? > 0 {
            return Err("文件夹内还有索引，不能删除".into());
        }
        let n = self
            .conn
            .execute("DELETE FROM folders WHERE id=?1", params![id])
            .map_err(db_err)?;
        if n == 0 {
            return Err("文件夹不存在".into());
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

    pub fn create_unit(
        &self,
        name: &str,
        folder_id: Option<&str>,
        group_id: &str,
        maintainer: &str,
        lead_lang: &str,
        description: &str,
    ) -> SResult<UnitRec> {
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
        let pkg = Package::load(fragment)?;
        let slide_refs = crate::pptx::slides(&pkg)?;
        let mut pages = Vec::new();
        let mut texts = Vec::new();
        let mut scripts = Vec::new();
        for s in &slide_refs {
            let xml = pkg.get_text(&s.part)?;
            pages.push(PageInfo {
                fp: s.fingerprint.clone(),
                title: slide_title(&xml),
            });
            texts.push(slide_full_text(&xml));
            scripts.push(crate::text::notes_text(&pkg, &s.part));
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

    /// Replace a version's per-page scripts. Editing scripts never bumps
    /// the version or the content revision (§3.16).
    pub fn set_version_scripts(&self, version_id: &str, scripts: &[String]) -> SResult<()> {
        self.conn
            .execute(
                "UPDATE versions SET scripts_json=?2 WHERE id=?1",
                params![version_id, serde_json::to_string(scripts).unwrap_or_default()],
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
        let existing = self
            .list_page_bindings(unit_id)?
            .into_iter()
            .find(|b| {
                b.from_version_id == from_version_id
                    && b.from_slot == from_slot
                    && b.to_version_id == to_version_id
                    && b.to_slot == to_slot
            });
        if let Some(b) = existing {
            return Ok(b);
        }
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
        };
        self.conn
            .execute(
                "INSERT INTO page_bindings(id, unit_id, from_version_id, from_slot, to_version_id, to_slot, kind, author, created_ms) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9)",
                params![
                    rec.id,
                    rec.unit_id,
                    rec.from_version_id,
                    rec.from_slot,
                    rec.to_version_id,
                    rec.to_slot,
                    rec.kind,
                    rec.author,
                    rec.created_ms
                ],
            )
            .map_err(db_err)?;
        Ok(rec)
    }

    pub fn list_page_bindings(&self, unit_id: &str) -> SResult<Vec<PageBindingRec>> {
        let mut stmt = self
            .conn
            .prepare("SELECT id, unit_id, from_version_id, from_slot, to_version_id, to_slot, kind, author, created_ms FROM page_bindings WHERE unit_id=?1 ORDER BY created_ms")
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

    pub fn create_index(
        &self,
        name: &str,
        folder_id: Option<&str>,
        group_id: &str,
        maintainer: &str,
        scope: &str,
        entries: Vec<IndexEntry>,
    ) -> SResult<IndexRec> {
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
        let head = self.branch_head(index_id, from_branch)?;
        self.conn
            .execute(
                "INSERT INTO index_branches(index_id, name, head_commit) VALUES(?1,?2,?3)",
                params![index_id, name, head],
            )
            .map_err(db_err)?;
        Ok(())
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
        let b_set: std::collections::HashSet<String> =
            self.ancestors(b)?.into_iter().collect();
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

    pub fn create_tag(
        &self,
        index_id: &str,
        branch: &str,
        name: &str,
    ) -> SResult<IndexTagRec> {
        let head = self
            .branch_head(index_id, branch)?
            .ok_or("分支尚无提交")?;
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
            .prepare("SELECT id, index_id, branch, name, commit_id FROM index_tags WHERE index_id=?1 ORDER BY name DESC")
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
        Ok(all.into_iter().find(|e| e.deck_id == deck_id && !e.manifest_json.is_empty()))
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
        };
        self.conn
            .execute(
                "INSERT INTO showroom(id, title, intro, source_index_id, commit_id, submitted_by, state, created_ms) VALUES(?1,?2,?3,?4,?5,?6,?7,?8)",
                params![
                    rec.id,
                    rec.title,
                    rec.intro,
                    rec.source_index_id,
                    rec.commit_id,
                    rec.submitted_by,
                    rec.state,
                    rec.created_ms
                ],
            )
            .map_err(db_err)?;
        Ok(rec)
    }

    pub fn list_showroom(&self, state: Option<&str>) -> SResult<Vec<ShowroomRec>> {
        let mut stmt = self
            .conn
            .prepare("SELECT id, title, intro, source_index_id, commit_id, submitted_by, state, created_ms FROM showroom ORDER BY created_ms DESC")
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
                })
            })
            .map_err(db_err)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(db_err)?;
        Ok(match state {
            None => rows,
            Some(s) => rows.into_iter().filter(|r| r.state == s).collect(),
        })
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
    ) -> SResult<String> {
        let id = new_id("shr2");
        self.conn
            .execute(
                "INSERT INTO shares(id, item_kind, item_id, title, from_user, target_kind, target_id, created_ms) VALUES(?1,?2,?3,?4,?5,?6,?7,?8)",
                params![id, item_kind, item_id, title, from_user, target_kind, target_id, now_ms()],
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
            .prepare("SELECT id, item_kind, item_id, title, from_user, target_kind, target_id, created_ms FROM shares ORDER BY created_ms DESC")
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
                    "created_ms": r.get::<_, i64>(7)?,
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
    /// department / one of their custom groups, or it sits in their
    /// library, or they submitted it)?
    pub fn showroom_item_access(&self, user: &UserRec, item: &ShowroomRec) -> SResult<bool> {
        if item.submitted_by == user.id
            || item.submitted_by == user.display_name
            || user.role == "super"
        {
            return Ok(true);
        }
        if item.state != "live" {
            return Ok(false);
        }
        if let Some(idx_id) = &item.source_index_id {
            if let Ok(idx) = self.get_index(idx_id) {
                if self.can_see_group(user, &idx.group_id)? {
                    return Ok(true);
                }
            }
        }
        if self
            .list_library(&user.id)?
            .iter()
            .any(|e| e["item_id"] == item.id)
        {
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
                Some("group") if my_groups.iter().any(|g| Some(g.as_str()) == s["target_id"].as_str()) => {
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
            .query_map([], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
            })
            .map_err(db_err)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(db_err)?;
        Ok(rows)
    }

    pub fn get_setting(&self, key: &str) -> SResult<Option<String>> {
        let v: Option<String> = self
            .conn
            .query_row("SELECT value FROM settings WHERE key=?1", params![key], |r| {
                r.get(0)
            })
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
        self.create_folder("公司介绍", None)?;
        self.create_folder("公司技术", None)?;
        self.create_folder("公司案例", None)?;
        self.set_setting(
            "languages",
            &serde_json::json!(["中文", "英语", "日语"]).to_string(),
        )?;
        let _ = g;
        Ok(())
    }
}

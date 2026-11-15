//! qimu CLI — functionally equivalent to the GUI across all operations
//! (spec §4.4): fully non-interactive, JSON in/out, stable IDs and exit
//! codes (0 ok · 1 error · 2 completed with conflicts/pending items).
//! It runs under a logged-in account with that account's group
//! permissions — it is not a bypass.

use base64::Engine;
use clap::{Parser, Subcommand};
use qimu_core::manifest::{read_deck_id, read_manifest, Manifest};
use qimu_core::pptx::{self, Package};
use serde_json::{json, Value};
use std::process::ExitCode;

#[derive(Parser)]
#[command(name = "qimu", version, about = "启幕 PPT 管理器 CLI")]
struct Cli {
    /// Server base URL (default: $QIMU_SERVER or http://127.0.0.1:8730).
    #[arg(long, global = true)]
    server: Option<String>,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Log in and store the session token in ~/.qimu/session.json.
    Login { username: String, password: String },
    Logout,
    Me,
    /// Operations on a single .pptx file (local + update flow).
    Deck {
        #[command(subcommand)]
        cmd: DeckCommand,
    },
    /// Import a deck into the library (split + verified, §5.8/§3.1).
    Import {
        path: String,
        #[arg(long)]
        group: String,
        #[arg(long)]
        folder: Option<String>,
        /// JSON array of {name, pages:[0-based], form, lang}; default: one
        /// block with all pages, named after the file.
        #[arg(long)]
        blocks: Option<String>,
        /// Use the server's block proposal (§5.8) instead of one block.
        #[arg(long)]
        propose: bool,
    },
    /// Propose content blocks for a deck without importing (§5.8).
    ImportPropose {
        path: String,
    },
    Units {
        #[command(subcommand)]
        cmd: UnitsCommand,
    },
    /// Publish a fragment as a new version of a unit's rendition.
    Publish {
        unit: String,
        #[arg(long)]
        file: String,
        #[arg(long)]
        form: Option<String>,
        #[arg(long)]
        lang: Option<String>,
        /// major | minor | patch (default minor)
        #[arg(long)]
        bump: Option<String>,
        #[arg(long)]
        note: Option<String>,
        #[arg(long)]
        new_revision: bool,
        #[arg(long)]
        draft: bool,
    },
    Versions {
        #[command(subcommand)]
        cmd: VersionsCommand,
    },
    Index {
        #[command(subcommand)]
        cmd: IndexCommand,
    },
    Search { query: String },
    Org {
        #[command(subcommand)]
        cmd: OrgCommand,
    },
    Groups,
    Grants {
        #[command(subcommand)]
        cmd: GrantsCommand,
    },
    Folders {
        #[command(subcommand)]
        cmd: FoldersCommand,
    },
    Attachments {
        #[command(subcommand)]
        cmd: AttachmentsCommand,
    },
    Showroom {
        #[command(subcommand)]
        cmd: ShowroomCommand,
    },
    /// Custom groups (custom teams, §7.7).
    Teams {
        #[command(subcommand)]
        cmd: TeamsCommand,
    },
    /// Share a showroom item or assembly to a user / department / team.
    Share {
        /// showroom | assembly
        item_kind: String,
        item_id: String,
        #[arg(long)]
        to_user: Option<String>,
        #[arg(long)]
        to_dept: Option<String>,
        #[arg(long)]
        to_team: Option<String>,
    },
    /// Shares addressed to me (direct + my departments).
    Inbox {
        #[command(subcommand)]
        cmd: Option<InboxCommand>,
    },
    /// My library (references I accepted).
    Library {
        #[command(subcommand)]
        cmd: Option<LibraryCommand>,
    },
    /// Group spaces of my custom teams.
    GroupSpace,
    Assemblies {
        #[command(subcommand)]
        cmd: AssembliesCommand,
    },
    /// Show or set the offline-cache policy (set: super admin).
    CachePolicy {
        #[arg(long)]
        allowed: Option<bool>,
        #[arg(long)]
        max_mb: Option<i64>,
    },
    /// Show the AI-translation glossary, or replace it from a JSON file
    /// (array of {"src","dst"}).
    Glossary {
        #[arg(long)]
        set: Option<String>,
    },
    /// Set personal profile fields (key=value, repeatable).
    Profile {
        #[arg(long = "set")]
        set: Vec<String>,
    },
    /// Set company public-profile fields (super admin; key=value).
    CompanyProfile {
        #[arg(long = "set")]
        set: Vec<String>,
    },
    /// PowerPoint COM bridge (Windows; §4.3).
    Bridge {
        #[command(subcommand)]
        cmd: BridgeCommand,
    },
}

#[derive(Subcommand)]
enum BridgeCommand {
    /// Report whether PowerPoint is running and which deck is open.
    Status,
}

#[derive(Subcommand)]
enum DeckCommand {
    Info { path: String },
    SplitVerify { path: String },
    /// Print the embedded manifest of a managed deck.
    Manifest { path: String },
    /// Ask the server what changed for this deck (the colleague flow).
    Check { path: String },
    /// Accept content updates IN PLACE: the same file at the same path
    /// is updated surgically; structural changes are reported as pending
    /// (they are release-gated, §5.7). Exit 2 if conflicts were found.
    Apply { path: String },
}

#[derive(Subcommand)]
enum UnitsCommand {
    List,
    Get { id: String },
    Create {
        #[arg(long)]
        name: String,
        #[arg(long)]
        group: String,
        #[arg(long)]
        folder: Option<String>,
        #[arg(long)]
        lang: Option<String>,
    },
    /// List a unit's per-page language bindings.
    Bindings { id: String },
    /// Bind one page of one version to a page of another version (same unit).
    Bind {
        id: String,
        #[arg(long)]
        from_version: String,
        #[arg(long)]
        from_slot: i64,
        #[arg(long)]
        to_version: String,
        #[arg(long)]
        to_slot: i64,
    },
    Unbind {
        id: String,
        #[arg(long)]
        binding: String,
    },
    /// AI-translate one version into a target language (draft rendition).
    Translate {
        id: String,
        #[arg(long)]
        version: String,
        #[arg(long)]
        to: String,
    },
}

#[derive(Subcommand)]
enum VersionsCommand {
    Recall { id: String },
    Delete { id: String },
    /// Download a version's fragment PPTX.
    Fragment {
        id: String,
        #[arg(short, long)]
        out: String,
    },
    Tag {
        #[command(subcommand)]
        cmd: TagCommand,
    },
    /// Show a version (incl. per-page speaker scripts).
    Get {
        id: String,
    },
    /// Replace a version's per-page scripts from a JSON file (array of
    /// strings). Never bumps the version.
    SetScripts {
        id: String,
        #[arg(long)]
        file: String,
    },
}

#[derive(Subcommand)]
enum TagCommand {
    Add {
        version: String,
        #[arg(long)]
        dim: String,
        #[arg(long)]
        value: String,
        #[arg(long)]
        strength: Option<String>,
    },
    List { version: String },
}

#[derive(Subcommand)]
enum IndexCommand {
    List,
    Get {
        id: String,
        #[arg(long)]
        branch: Option<String>,
    },
    Create {
        #[arg(long)]
        name: String,
        #[arg(long)]
        group: String,
        #[arg(long)]
        folder: Option<String>,
        #[arg(long)]
        scope: Option<String>,
        /// Comma-separated unit_id:rendition_id pairs.
        #[arg(long)]
        entries: String,
    },
    /// Create a commit with a full entry list (JSON array of entries).
    Commit {
        id: String,
        #[arg(long)]
        message: String,
        #[arg(long)]
        branch: Option<String>,
        #[arg(long)]
        entries_json: String,
    },
    Commits {
        id: String,
        #[arg(long)]
        branch: Option<String>,
    },
    Branch {
        id: String,
        #[arg(long)]
        name: String,
        #[arg(long)]
        from: Option<String>,
    },
    Tag {
        id: String,
        #[arg(long)]
        name: String,
        #[arg(long)]
        branch: Option<String>,
    },
    Lifecycle {
        id: String,
        #[arg(long)]
        state: String,
    },
    /// Preview an entry-level three-way merge of --from into --into.
    MergePreview {
        id: String,
        #[arg(long)]
        from: String,
        #[arg(long)]
        into: Option<String>,
    },
    /// Execute the merge; unresolved conflicts fail. --resolve key=ours|theirs (repeatable).
    Merge {
        id: String,
        #[arg(long)]
        from: String,
        #[arg(long)]
        into: Option<String>,
        #[arg(long = "resolve")]
        resolve: Vec<String>,
    },
    Export {
        id: String,
        #[arg(long)]
        branch: Option<String>,
        #[arg(long)]
        tag: Option<String>,
        #[arg(long)]
        kind: Option<String>,
        /// Write per-page speaker scripts into the notes pages.
        #[arg(long)]
        notes: bool,
        #[arg(short, long)]
        out: String,
    },
    Exports,
}

#[derive(Subcommand)]
enum OrgCommand {
    Show,
    AddDept {
        #[arg(long)]
        name: String,
        #[arg(long)]
        parent: Option<String>,
    },
    AddUser {
        #[arg(long)]
        username: String,
        #[arg(long)]
        display_name: String,
        #[arg(long)]
        password: String,
        #[arg(long)]
        dept: Option<String>,
        #[arg(long)]
        role: Option<String>,
    },
    Disable {
        id: String,
        #[arg(long)]
        enable: bool,
    },
}

#[derive(Subcommand)]
enum GrantsCommand {
    List,
    Add {
        #[arg(long)]
        group: String,
        #[arg(long)]
        subject_type: String,
        #[arg(long)]
        subject_id: String,
        #[arg(long)]
        capability: String,
    },
}

#[derive(Subcommand)]
enum FoldersCommand {
    List,
    Add {
        #[arg(long)]
        name: String,
        #[arg(long)]
        parent: Option<String>,
    },
    Rename {
        id: String,
        #[arg(long)]
        name: String,
    },
    /// Designate the folder's main index (omit --index to clear).
    SetMain {
        id: String,
        #[arg(long)]
        index: Option<String>,
    },
}

#[derive(Subcommand)]
enum AttachmentsCommand {
    Add {
        #[arg(long)]
        parent_type: String,
        #[arg(long)]
        parent_id: String,
        #[arg(long)]
        file: String,
    },
    List {
        #[arg(long)]
        parent_id: String,
    },
    Get {
        id: String,
        #[arg(short, long)]
        out: String,
    },
}

#[derive(Subcommand)]
enum ShowroomCommand {
    List {
        #[arg(long)]
        state: Option<String>,
    },
    Submit {
        #[arg(long)]
        title: String,
        #[arg(long)]
        intro: Option<String>,
        #[arg(long)]
        index: Option<String>,
    },
    Review {
        id: String,
        #[arg(long)]
        state: String,
    },
    Export {
        id: String,
        #[arg(long, default_value = "personal")]
        kind: String,
        #[arg(short, long)]
        out: String,
    },
}

#[derive(Subcommand)]
enum TeamsCommand {
    List,
    Create {
        name: String,
    },
    AddMember {
        id: String,
        user_id: String,
    },
    RemoveMember {
        id: String,
        user_id: String,
    },
}

#[derive(Subcommand)]
enum InboxCommand {
    /// Save a received share into my library.
    Accept {
        share_id: String,
        #[arg(long)]
        folder: Option<String>,
    },
}

#[derive(Subcommand)]
enum LibraryCommand {
    Remove {
        id: String,
    },
}

#[derive(Subcommand)]
enum AssembliesCommand {
    List,
    /// Create a personal assembly from entry JSON (array of index entries).
    Create {
        #[arg(long)]
        name: String,
        #[arg(long)]
        entries_json: String,
    },
    Export {
        id: String,
        #[arg(long)]
        kind: Option<String>,
        #[arg(short, long)]
        out: String,
    },
}

// ---------------- session / HTTP ----------------

fn session_path() -> std::path::PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
    std::path::PathBuf::from(home).join(".qimu").join("session.json")
}

fn load_session() -> Value {
    std::fs::read_to_string(session_path())
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_else(|| json!({}))
}

fn save_session(v: &Value) {
    let p = session_path();
    if let Some(parent) = p.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = std::fs::write(&p, serde_json::to_string_pretty(v).unwrap());
}

struct Ctx {
    server: String,
    token: Option<String>,
}

impl Ctx {
    fn new(flag: &Option<String>) -> Ctx {
        let sess = load_session();
        let server = flag
            .clone()
            .or_else(|| std::env::var("QIMU_SERVER").ok())
            .or_else(|| sess.get("server").and_then(|v| v.as_str()).map(|s| s.to_string()))
            .unwrap_or_else(|| "http://127.0.0.1:8730".to_string());
        let token = sess.get("token").and_then(|v| v.as_str()).map(|s| s.to_string());
        Ctx { server, token }
    }

    fn call(&self, method: &str, path: &str, body: Option<Value>) -> Result<Value, String> {
        let url = format!("{}{}", self.server, path);
        let mut req = match method {
            "GET" => ureq::get(&url),
            "PUT" => ureq::put(&url),
            "PATCH" => ureq::patch(&url),
            _ => ureq::post(&url),
        };
        if let Some(t) = &self.token {
            req = req.set("Authorization", &format!("Bearer {t}"));
        }
        let resp = match body {
            Some(b) => req.send_json(b),
            None => req.call(),
        };
        match resp {
            Ok(r) => r
                .into_json::<Value>()
                .map_err(|e| format!("响应解析失败: {e}")),
            Err(ureq::Error::Status(_, r)) => {
                let text = r.into_string().unwrap_or_default();
                let msg = serde_json::from_str::<Value>(&text)
                    .ok()
                    .and_then(|v| v.get("error").and_then(|e| e.as_str()).map(|s| s.to_string()))
                    .unwrap_or(text);
                Err(msg)
            }
            Err(e) => Err(format!("请求失败: {e}")),
        }
    }
}

fn print(v: &Value) {
    println!("{}", serde_json::to_string_pretty(v).unwrap());
}

fn b64e(bytes: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

fn b64d(s: &str) -> Result<Vec<u8>, String> {
    base64::engine::general_purpose::STANDARD
        .decode(s)
        .map_err(|e| e.to_string())
}

fn read_deck(path: &str) -> Result<Vec<u8>, String> {
    std::fs::read(path).map_err(|e| format!("read {path}: {e}"))
}

/// Local embedded manifest first; if an editor stripped it on save, fall
/// back to the deck-id token (core keywords) + the server's export
/// registry.
fn deck_manifest(ctx: &Ctx, path: &str) -> Result<Manifest, String> {
    let bytes = read_deck(path)?;
    if let Some(m) = read_manifest(&bytes) {
        return Ok(m);
    }
    if let Some(deck_id) = read_deck_id(&bytes) {
        let resp = ctx.call("GET", &format!("/v1/decks/{deck_id}/manifest"), None)?;
        if let Some(mv) = resp.get("manifest") {
            let m: Manifest = serde_json::from_value(mv.clone()).map_err(|e| e.to_string())?;
            eprintln!("（内嵌清单已被编辑器剥离，已凭文件中的 deck 标识从服务器导出台账恢复清单）");
            return Ok(m);
        }
    }
    Err("该文件没有内嵌清单（不是受管文件，或清单已被编辑器剥离且服务器无此稿记录）".to_string())
}

// ---------------- deck apply (in-place update acceptance) ----------------

/// Accept content updates in place. Returns the process exit code:
/// 0 clean, 2 finished with conflicts / pending structural changes.
fn deck_apply(ctx: &Ctx, path: &str) -> Result<i32, String> {
    let bytes = read_deck(path)?;
    let manifest = deck_manifest(ctx, path)?;
    let _ = bytes;
    let resp = ctx.call(
        "POST",
        "/v1/deck/update-package",
        Some(json!({ "manifest": manifest })),
    )?;
    let diff = resp.get("diff").cloned().unwrap_or(json!({}));
    let updates = diff
        .get("updates")
        .and_then(|u| u.as_array())
        .cloned()
        .unwrap_or_default();
    let recalled: Vec<&Value> = updates
        .iter()
        .filter(|u| u.get("type").and_then(|t| t.as_str()) == Some("recalled"))
        .collect();
    let structural: Vec<&Value> = updates
        .iter()
        .filter(|u| {
            matches!(
                u.get("type").and_then(|t| t.as_str()),
                Some("added") | Some("removed")
            )
        })
        .collect();
    let pages = resp
        .get("pages")
        .and_then(|p| p.as_array())
        .cloned()
        .unwrap_or_default();
    let content_pages: Vec<&Value> = pages
        .iter()
        .filter(|p| p.get("type").and_then(|t| t.as_str()) == Some("content_update"))
        .collect();

    if content_pages.is_empty() {
        print(&json!({
            "applied": [],
            "conflicts": [],
            "recalled": recalled,
            "structural_pending": structural,
            "note": if updates.is_empty() { "已是最新" } else { "没有可直接接受的内容更新" },
        }));
        return Ok(if structural.is_empty() && recalled.is_empty() { 0 } else { 2 });
    }

    // Map new fragments by rendition.
    let mut new_frag: std::collections::HashMap<String, (String, String, Vec<u8>)> =
        Default::default();
    for p in &content_pages {
        let rid = p
            .get("rendition_id")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let frag = b64d(p.get("fragment_base64").and_then(|v| v.as_str()).unwrap_or(""))?;
        new_frag.insert(
            rid,
            (
                p.get("version_id").and_then(|v| v.as_str()).unwrap_or("").to_string(),
                p.get("semver").and_then(|v| v.as_str()).unwrap_or("").to_string(),
                frag,
            ),
        );
    }

    let pkg = Package::load(&bytes)?;
    let deck_slides = pptx::slides(&pkg)?;
    let singles = pptx::split(&pkg)?;
    let cur_fps: Vec<String> = deck_slides.iter().map(|s| s.fingerprint.clone()).collect();

    let mut out_pkgs: Vec<Package> = Vec::new();
    let mut new_bindings = manifest.bindings.clone();
    let mut applied: Vec<Value> = Vec::new();
    let mut conflicts: Vec<Value> = Vec::new();
    let mut pos = 0usize;

    for (bi, b) in manifest.bindings.iter().enumerate() {
        // Skip unmanaged slides inserted locally before this binding:
        // scan forward for the binding's first page fingerprint.
        if pos < cur_fps.len() && cur_fps[pos] != b.page_fps.first().cloned().unwrap_or_default() {
            let mut scan = pos;
            let first = b.page_fps.first().cloned().unwrap_or_default();
            while scan < cur_fps.len() && scan < pos + 32 && cur_fps[scan] != first {
                scan += 1;
            }
            if scan < cur_fps.len() && cur_fps[scan] == first {
                for k in pos..scan {
                    out_pkgs.push(singles[k].clone());
                }
                pos = scan;
            }
        }
        let n = b.page_fps.len();
        let local_ok = pos + n <= cur_fps.len()
            && (0..n).all(|k| cur_fps[pos + k] == b.page_fps[k]);
        if let Some((vid, semver, frag_bytes)) = new_frag.get(&b.rendition_id) {
            if !local_ok {
                conflicts.push(json!({
                    "rendition_id": b.rendition_id,
                    "reason": "本地已修改该块页面，未覆盖（保留本地）",
                }));
                for k in 0..n.min(cur_fps.len().saturating_sub(pos)) {
                    out_pkgs.push(singles[pos + k].clone());
                }
                pos += n;
                continue;
            }
            let frag_pkg = Package::load(frag_bytes)?;
            let frag_slides = pptx::slides(&frag_pkg)?;
            let frag_singles = pptx::split(&frag_pkg)?;
            // A page-count change inside one binding is a normal
            // accept (§5.3 页数/形态变化): running apply is the user's
            // confirmation, so the range is replaced with the new
            // version's pages however many there are. Only index-level
            // added/removed entries stay pending (release-gated, §5.7).
            for s in &frag_singles {
                out_pkgs.push(s.clone());
            }
            new_bindings[bi].version_id = vid.clone();
            new_bindings[bi].version_semver = semver.clone();
            new_bindings[bi].page_fps =
                frag_slides.iter().map(|s| s.fingerprint.clone()).collect();
            applied.push(json!({
                "rendition_id": b.rendition_id,
                "from": b.version_semver,
                "to": semver,
            }));
            pos += n;
        } else {
            for k in 0..n.min(cur_fps.len().saturating_sub(pos)) {
                out_pkgs.push(singles[pos + k].clone());
            }
            if !local_ok {
                conflicts.push(json!({
                    "rendition_id": b.rendition_id,
                    "reason": "本地已修改（无待接受更新，已保留本地）",
                }));
            }
            pos += n;
        }
    }
    for k in pos..singles.len() {
        out_pkgs.push(singles[k].clone());
    }

    if applied.is_empty() {
        print(&json!({
            "applied": [],
            "conflicts": conflicts,
            "recalled": recalled,
            "structural_pending": structural,
        }));
        return Ok(2);
    }

    let merged = qimu_core::assemble::assemble(&out_pkgs)?;
    let mut new_manifest = manifest.clone();
    new_manifest.bindings = new_bindings;
    let out_bytes = qimu_core::manifest::embed_manifest(&merged.to_bytes()?, &new_manifest)?;
    // Atomic in-place write: same path, via temp file + rename.
    let tmp = format!("{path}.qimu-tmp");
    std::fs::write(&tmp, &out_bytes).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, path).map_err(|e| e.to_string())?;

    print(&json!({
        "applied": applied,
        "conflicts": conflicts,
        "recalled": recalled,
        "structural_pending": structural,
        "path": path,
    }));
    Ok(if conflicts.is_empty() && structural.is_empty() { 0 } else { 2 })
}

// ---------------- run ----------------

fn run() -> Result<i32, String> {
    let cli = Cli::parse();
    let ctx = Ctx::new(&cli.server);
    match cli.command {
        Command::Login { username, password } => {
            let ctx2 = Ctx {
                server: ctx.server.clone(),
                token: None,
            };
            let r = ctx2.call(
                "POST",
                "/v1/auth/login",
                Some(json!({ "username": username, "password": password })),
            )?;
            let mut sess = load_session();
            sess["server"] = json!(ctx.server);
            sess["token"] = r.get("token").cloned().unwrap_or(Value::Null);
            save_session(&sess);
            print(&json!({ "ok": true, "user": r.get("user") }));
            Ok(0)
        }
        Command::Logout => {
            let _ = ctx.call("POST", "/v1/auth/logout", None);
            save_session(&json!({}));
            print(&json!({ "ok": true }));
            Ok(0)
        }
        Command::Me => {
            print(&ctx.call("GET", "/v1/me", None)?);
            Ok(0)
        }
        Command::Deck { cmd } => match cmd {
            DeckCommand::Info { path } => {
                let bytes = read_deck(&path)?;
                let pkg = Package::load(&bytes)?;
                let slides = pptx::slides(&pkg)?;
                print(&json!({
                    "path": path,
                    "bytes": bytes.len(),
                    "slide_count": slides.len(),
                    "package_fingerprint": pptx::package_fingerprint(&pkg),
                    "slides": slides,
                    "managed": read_manifest(&bytes).is_some(),
                    "deck_id": read_deck_id(&bytes),
                }));
                Ok(0)
            }
            DeckCommand::SplitVerify { path } => {
                let bytes = read_deck(&path)?;
                let pkg = Package::load(&bytes)?;
                let report = pptx::split_reassemble_verify(&pkg)?;
                print(&serde_json::to_value(&report).unwrap());
                if report.ok {
                    Ok(0)
                } else {
                    Err("verification failed".into())
                }
            }
            DeckCommand::Manifest { path } => {
                let m = deck_manifest(&ctx, &path)?;
                print(&serde_json::to_value(&m).unwrap());
                Ok(0)
            }
            DeckCommand::Check { path } => {
                let m = deck_manifest(&ctx, &path)?;
                let r = ctx.call("POST", "/v1/check-updates", Some(json!({ "manifest": m })))?;
                print(&r);
                Ok(0)
            }
            DeckCommand::Apply { path } => deck_apply(&ctx, &path),
        },
        Command::ImportPropose { path } => {
            let bytes = read_deck(&path)?;
            print(&ctx.call(
                "POST",
                "/v1/import/propose",
                Some(json!({ "data_base64": b64e(&bytes) })),
            )?);
            Ok(0)
        }
        Command::Import {
            path,
            group,
            folder,
            blocks,
            propose,
        } => {
            let bytes = read_deck(&path)?;
            let blocks_v: Option<Value> = match (blocks, propose) {
                (Some(s), _) => Some(serde_json::from_str(&s).map_err(|e| e.to_string())?),
                (None, true) => {
                    let p = ctx.call(
                        "POST",
                        "/v1/import/propose",
                        Some(json!({ "data_base64": b64e(&bytes) })),
                    )?;
                    Some(
                        p.get("blocks")
                            .cloned()
                            .ok_or("提块结果缺少 blocks")?,
                    )
                }
                (None, false) => None,
            };
            let r = ctx.call(
                "POST",
                "/v1/import",
                Some(json!({
                    "filename": path.rsplit('/').next().unwrap_or(&path),
                    "data_base64": b64e(&bytes),
                    "folder_id": folder,
                    "group_id": group,
                    "blocks": blocks_v,
                })),
            )?;
            print(&r);
            Ok(0)
        }
        Command::Units { cmd } => {
            match cmd {
                UnitsCommand::List => print(&ctx.call("GET", "/v1/units", None)?),
                UnitsCommand::Get { id } => {
                    print(&ctx.call("GET", &format!("/v1/units/{id}"), None)?)
                }
                UnitsCommand::Create {
                    name,
                    group,
                    folder,
                    lang,
                } => print(&ctx.call(
                    "POST",
                    "/v1/units",
                    Some(json!({
                        "name": name, "group_id": group, "folder_id": folder, "lead_lang": lang
                    })),
                )?),
                UnitsCommand::Bindings { id } => print(
                    &ctx.call("GET", &format!("/v1/units/{id}/page-bindings"), None)?,
                ),
                UnitsCommand::Bind {
                    id,
                    from_version,
                    from_slot,
                    to_version,
                    to_slot,
                } => print(&ctx.call(
                    "POST",
                    &format!("/v1/units/{id}/page-bindings"),
                    Some(json!({
                        "from_version_id": from_version, "from_slot": from_slot,
                        "to_version_id": to_version, "to_slot": to_slot
                    })),
                )?),
                UnitsCommand::Unbind { id, binding } => print(&ctx.call(
                    "POST",
                    &format!("/v1/units/{id}/page-bindings/{binding}/delete"),
                    None,
                )?),
                UnitsCommand::Translate { id, version, to } => print(&ctx.call(
                    "POST",
                    &format!("/v1/units/{id}/translate"),
                    Some(json!({ "version_id": version, "target_lang": to })),
                )?),
            }
            Ok(0)
        }
        Command::CachePolicy { allowed, max_mb } => {
            if allowed.is_none() && max_mb.is_none() {
                print(&ctx.call("GET", "/v1/settings/cache-policy", None)?);
            } else {
                let cur = ctx.call("GET", "/v1/settings/cache-policy", None)?;
                print(&ctx.call(
                    "PUT",
                    "/v1/settings/cache-policy",
                    Some(json!({
                        "allowed": allowed.unwrap_or_else(|| cur.get("allowed").and_then(|v| v.as_bool()).unwrap_or(true)),
                        "max_mb": max_mb.unwrap_or_else(|| cur.get("max_mb").and_then(|v| v.as_i64()).unwrap_or(500)),
                    })),
                )?);
            }
            Ok(0)
        }
        Command::Glossary { set } => {
            if let Some(file) = set {
                let text = std::fs::read_to_string(&file).map_err(|e| e.to_string())?;
                let pairs: Value = serde_json::from_str(&text).map_err(|e| e.to_string())?;
                print(&ctx.call(
                    "PUT",
                    "/v1/settings/glossary",
                    Some(json!({ "pairs": pairs })),
                )?);
            } else {
                print(&ctx.call("GET", "/v1/settings/glossary", None)?);
            }
            Ok(0)
        }
        Command::Publish {
            unit,
            file,
            form,
            lang,
            bump,
            note,
            new_revision,
            draft,
        } => {
            let bytes = read_deck(&file)?;
            let r = ctx.call(
                "POST",
                &format!("/v1/units/{unit}/publish"),
                Some(json!({
                    "form": form,
                    "lang": lang,
                    "data_base64": b64e(&bytes),
                    "bump": bump,
                    "note": note,
                    "status": if draft { "draft" } else { "normal" },
                    "new_revision": new_revision,
                })),
            )?;
            print(&r);
            Ok(0)
        }
        Command::Versions { cmd } => {
            match cmd {
                VersionsCommand::Recall { id } => {
                    print(&ctx.call("POST", &format!("/v1/versions/{id}/recall"), None)?)
                }
                VersionsCommand::Delete { id } => {
                    print(&ctx.call("POST", &format!("/v1/versions/{id}/delete"), None)?)
                }
                VersionsCommand::Fragment { id, out } => {
                    let r = ctx.call("GET", &format!("/v1/versions/{id}/fragment"), None)?;
                    let data = r
                        .get("data_base64")
                        .and_then(|v| v.as_str())
                        .ok_or("响应缺少片段数据")?;
                    std::fs::write(&out, b64d(data)?).map_err(|e| e.to_string())?;
                    print(&json!({ "ok": true, "out": out }));
                }
                VersionsCommand::Tag { cmd } => match cmd {
                    TagCommand::Add {
                        version,
                        dim,
                        value,
                        strength,
                    } => print(&ctx.call(
                        "POST",
                        &format!("/v1/versions/{version}/tags"),
                        Some(json!({ "dim": dim, "value": value, "strength": strength })),
                    )?),
                    TagCommand::List { version } => {
                        print(&ctx.call("GET", &format!("/v1/versions/{version}/tags"), None)?)
                    }
                },
                VersionsCommand::Get { id } => {
                    print(&ctx.call("GET", &format!("/v1/versions/{id}"), None)?)
                }
                VersionsCommand::SetScripts { id, file } => {
                    let text = std::fs::read_to_string(&file).map_err(|e| e.to_string())?;
                    let scripts: Value =
                        serde_json::from_str(&text).map_err(|e| e.to_string())?;
                    print(&ctx.call(
                        "PUT",
                        &format!("/v1/versions/{id}/scripts"),
                        Some(json!({ "scripts": scripts })),
                    )?)
                }
            }
            Ok(0)
        }
        Command::Index { cmd } => {
            match cmd {
                IndexCommand::List => print(&ctx.call("GET", "/v1/indexes", None)?),
                IndexCommand::Get { id, branch } => {
                    let q = branch
                        .map(|b| format!("?branch={b}"))
                        .unwrap_or_default();
                    print(&ctx.call("GET", &format!("/v1/indexes/{id}{q}"), None)?)
                }
                IndexCommand::Create {
                    name,
                    group,
                    folder,
                    scope,
                    entries,
                } => {
                    let pairs: Vec<Value> = entries
                        .split(',')
                        .filter(|s| !s.trim().is_empty())
                        .map(|pair| {
                            let mut it = pair.splitn(2, ':');
                            json!({
                                "unit_id": it.next().unwrap_or("").trim(),
                                "rendition_id": it.next().unwrap_or("").trim(),
                            })
                        })
                        .collect();
                    print(&ctx.call(
                        "POST",
                        "/v1/indexes",
                        Some(json!({
                            "name": name, "group_id": group, "folder_id": folder,
                            "scope": scope, "entries": pairs
                        })),
                    )?);
                }
                IndexCommand::Commit {
                    id,
                    message,
                    branch,
                    entries_json,
                } => {
                    let entries: Value =
                        serde_json::from_str(&entries_json).map_err(|e| e.to_string())?;
                    print(&ctx.call(
                        "POST",
                        &format!("/v1/indexes/{id}/commits"),
                        Some(json!({ "message": message, "branch": branch, "entries": entries })),
                    )?);
                }
                IndexCommand::Commits { id, branch } => {
                    let q = branch
                        .map(|b| format!("?branch={b}"))
                        .unwrap_or_default();
                    print(&ctx.call("GET", &format!("/v1/indexes/{id}/commits{q}"), None)?)
                }
                IndexCommand::Branch { id, name, from } => print(&ctx.call(
                    "POST",
                    &format!("/v1/indexes/{id}/branches"),
                    Some(json!({ "name": name, "from": from })),
                )?),
                IndexCommand::Tag { id, name, branch } => print(&ctx.call(
                    "POST",
                    &format!("/v1/indexes/{id}/tags"),
                    Some(json!({ "name": name, "branch": branch })),
                )?),
                IndexCommand::Lifecycle { id, state } => print(&ctx.call(
                    "POST",
                    &format!("/v1/indexes/{id}/lifecycle"),
                    Some(json!({ "lifecycle": state })),
                )?),
                IndexCommand::MergePreview { id, from, into } => print(&ctx.call(
                    "POST",
                    &format!("/v1/indexes/{id}/merge/preview"),
                    Some(json!({ "from": from, "into": into })),
                )?),
                IndexCommand::Merge {
                    id,
                    from,
                    into,
                    resolve,
                } => {
                    let mut resolutions = serde_json::Map::new();
                    for kv in &resolve {
                        let mut it = kv.splitn(2, '=');
                        resolutions.insert(
                            it.next().unwrap_or("").to_string(),
                            json!(it.next().unwrap_or("")),
                        );
                    }
                    print(&ctx.call(
                        "POST",
                        &format!("/v1/indexes/{id}/merge"),
                        Some(json!({ "from": from, "into": into, "resolutions": resolutions })),
                    )?)
                }
                IndexCommand::Export {
                    id,
                    branch,
                    tag,
                    kind,
                    notes,
                    out,
                } => {
                    let r = ctx.call(
                        "POST",
                        &format!("/v1/indexes/{id}/export"),
                        Some(json!({ "branch": branch, "tag": tag, "kind": kind, "include_notes": notes })),
                    )?;
                    let data = r
                        .get("data_base64")
                        .and_then(|v| v.as_str())
                        .ok_or("响应缺少文件数据")?;
                    std::fs::write(&out, b64d(data)?).map_err(|e| e.to_string())?;
                    print(&json!({
                        "ok": true,
                        "out": out,
                        "export_id": r.get("export_id"),
                        "page_count": r.get("page_count"),
                    }));
                }
                IndexCommand::Exports => print(&ctx.call("GET", "/v1/exports", None)?),
            }
            Ok(0)
        }
        Command::Search { query } => {
            print(&ctx.call("GET", &format!("/v1/search?q={query}"), None)?);
            Ok(0)
        }
        Command::Org { cmd } => {
            match cmd {
                OrgCommand::Show => print(&ctx.call("GET", "/v1/org", None)?),
                OrgCommand::AddDept { name, parent } => print(&ctx.call(
                    "POST",
                    "/v1/org/departments",
                    Some(json!({ "name": name, "parent_id": parent })),
                )?),
                OrgCommand::AddUser {
                    username,
                    display_name,
                    password,
                    dept,
                    role,
                } => print(&ctx.call(
                    "POST",
                    "/v1/org/users",
                    Some(json!({
                        "username": username, "display_name": display_name,
                        "password": password, "dept_id": dept, "role": role
                    })),
                )?),
                OrgCommand::Disable { id, enable } => print(&ctx.call(
                    "POST",
                    &format!("/v1/org/users/{id}/disable"),
                    Some(json!({ "disabled": !enable })),
                )?),
            }
            Ok(0)
        }
        Command::Groups => {
            print(&ctx.call("GET", "/v1/groups", None)?);
            Ok(0)
        }
        Command::Grants { cmd } => {
            match cmd {
                GrantsCommand::List => print(&ctx.call("GET", "/v1/grants", None)?),
                GrantsCommand::Add {
                    group,
                    subject_type,
                    subject_id,
                    capability,
                } => print(&ctx.call(
                    "POST",
                    "/v1/grants",
                    Some(json!({
                        "group_id": group, "subject_type": subject_type,
                        "subject_id": subject_id, "capability": capability
                    })),
                )?),
            }
            Ok(0)
        }
        Command::Folders { cmd } => {
            match cmd {
                FoldersCommand::List => print(&ctx.call("GET", "/v1/folders", None)?),
                FoldersCommand::Add { name, parent } => print(&ctx.call(
                    "POST",
                    "/v1/folders",
                    Some(json!({ "name": name, "parent_id": parent })),
                )?),
                FoldersCommand::Rename { id, name } => print(&ctx.call(
                    "POST",
                    &format!("/v1/folders/{id}/rename"),
                    Some(json!({ "name": name })),
                )?),
                FoldersCommand::SetMain { id, index } => print(&ctx.call(
                    "POST",
                    &format!("/v1/folders/{id}/main-index"),
                    Some(json!({ "index_id": index })),
                )?),
            }
            Ok(0)
        }
        Command::Attachments { cmd } => {
            match cmd {
                AttachmentsCommand::Add {
                    parent_type,
                    parent_id,
                    file,
                } => {
                    let bytes = read_deck(&file)?;
                    print(&ctx.call(
                        "POST",
                        "/v1/attachments",
                        Some(json!({
                            "parent_type": parent_type,
                            "parent_id": parent_id,
                            "filename": file.rsplit('/').next().unwrap_or(&file),
                            "data_base64": b64e(&bytes),
                        })),
                    )?);
                }
                AttachmentsCommand::List { parent_id } => {
                    print(&ctx.call("GET", &format!("/v1/attachments?parent_id={parent_id}"), None)?)
                }
                AttachmentsCommand::Get { id, out } => {
                    let r = ctx.call("GET", &format!("/v1/attachments/{id}/download"), None)?;
                    let data = r
                        .get("data_base64")
                        .and_then(|v| v.as_str())
                        .ok_or("响应缺少文件数据")?;
                    std::fs::write(&out, b64d(data)?).map_err(|e| e.to_string())?;
                    print(&json!({ "ok": true, "out": out }));
                }
            }
            Ok(0)
        }
        Command::Showroom { cmd } => {
            match cmd {
                ShowroomCommand::List { state } => {
                    let q = state.map(|s| format!("?state={s}")).unwrap_or_default();
                    print(&ctx.call("GET", &format!("/v1/showroom{q}"), None)?)
                }
                ShowroomCommand::Submit { title, intro, index } => print(&ctx.call(
                    "POST",
                    "/v1/showroom",
                    Some(json!({ "title": title, "intro": intro, "source_index_id": index })),
                )?),
                ShowroomCommand::Review { id, state } => print(&ctx.call(
                    "POST",
                    &format!("/v1/showroom/{id}/review"),
                    Some(json!({ "state": state })),
                )?),
                ShowroomCommand::Export { id, kind, out } => {
                    let r = ctx.call(
                        "POST",
                        &format!("/v1/showroom/{id}/export"),
                        Some(json!({ "kind": kind })),
                    )?;
                    let data = r
                        .get("data_base64")
                        .and_then(|v| v.as_str())
                        .ok_or("响应缺少文件数据")?;
                    std::fs::write(&out, b64d(data)?).map_err(|e| e.to_string())?;
                    print(&json!({ "ok": true, "out": out, "page_count": r.get("page_count") }));
                }
            }
            Ok(0)
        }
        Command::Teams { cmd } => {
            match cmd {
                TeamsCommand::List => print(&ctx.call("GET", "/v1/custom-groups", None)?),
                TeamsCommand::Create { name } => {
                    print(&ctx.call("POST", "/v1/custom-groups", Some(json!({ "name": name })))?)
                }
                TeamsCommand::AddMember { id, user_id } => print(&ctx.call(
                    "POST",
                    &format!("/v1/custom-groups/{id}/members"),
                    Some(json!({ "user_id": user_id })),
                )?),
                TeamsCommand::RemoveMember { id, user_id } => print(&ctx.call(
                    "POST",
                    &format!("/v1/custom-groups/{id}/members/remove"),
                    Some(json!({ "user_id": user_id })),
                )?),
            }
            Ok(0)
        }
        Command::Share {
            item_kind,
            item_id,
            to_user,
            to_dept,
            to_team,
        } => {
            let (target_kind, target_id) = match (to_user, to_dept, to_team) {
                (Some(u), None, None) => ("user", u),
                (None, Some(d), None) => ("dept", d),
                (None, None, Some(g)) => ("group", g),
                _ => return Err("必须且只能给出一个分享目标：--to-user / --to-dept / --to-team".into()),
            };
            print(&ctx.call(
                "POST",
                "/v1/shares",
                Some(json!({
                    "item_kind": item_kind, "item_id": item_id,
                    "target_kind": target_kind, "target_id": target_id,
                })),
            )?);
            Ok(0)
        }
        Command::Inbox { cmd } => {
            match cmd {
                None => print(&ctx.call("GET", "/v1/shares/inbox", None)?),
                Some(InboxCommand::Accept { share_id, folder }) => print(&ctx.call(
                    "POST",
                    &format!("/v1/shares/{share_id}/accept"),
                    Some(json!({ "folder": folder })),
                )?),
            }
            Ok(0)
        }
        Command::Library { cmd } => {
            match cmd {
                None => print(&ctx.call("GET", "/v1/library", None)?),
                Some(LibraryCommand::Remove { id }) => {
                    print(&ctx.call("POST", &format!("/v1/library/{id}/delete"), Some(json!({})))?)
                }
            }
            Ok(0)
        }
        Command::GroupSpace => {
            print(&ctx.call("GET", "/v1/group-space", None)?);
            Ok(0)
        }
        Command::Assemblies { cmd } => {
            match cmd {
                AssembliesCommand::List => print(&ctx.call("GET", "/v1/assemblies", None)?),
                AssembliesCommand::Create { name, entries_json } => {
                    let entries: Value =
                        serde_json::from_str(&entries_json).map_err(|e| e.to_string())?;
                    print(&ctx.call(
                        "POST",
                        "/v1/assemblies",
                        Some(json!({ "name": name, "entries": entries })),
                    )?)
                }
                AssembliesCommand::Export { id, kind, out } => {
                    let r = ctx.call(
                        "POST",
                        &format!("/v1/assemblies/{id}/export"),
                        Some(json!({ "kind": kind })),
                    )?;
                    let data = r
                        .get("data_base64")
                        .and_then(|v| v.as_str())
                        .ok_or("响应缺少文件数据")?;
                    std::fs::write(&out, b64d(data)?).map_err(|e| e.to_string())?;
                    print(&json!({
                        "ok": true,
                        "out": out,
                        "page_count": r.get("page_count"),
                    }));
                }
            }
            Ok(0)
        }
        Command::Profile { set } => {
            let mut fields = serde_json::Map::new();
            for kv in &set {
                let mut it = kv.splitn(2, '=');
                fields.insert(
                    it.next().unwrap_or("").to_string(),
                    json!(it.next().unwrap_or("")),
                );
            }
            print(&ctx.call("PUT", "/v1/me/profile", Some(json!({ "fields": fields })))?);
            Ok(0)
        }
        Command::CompanyProfile { set } => {
            let mut fields = serde_json::Map::new();
            for kv in &set {
                let mut it = kv.splitn(2, '=');
                fields.insert(
                    it.next().unwrap_or("").to_string(),
                    json!(it.next().unwrap_or("")),
                );
            }
            print(&ctx.call("PUT", "/v1/company-profile", Some(json!({ "fields": fields })))?);
            Ok(0)
        }
        Command::Bridge { cmd } => match cmd {
            BridgeCommand::Status => {
                let bridge = qimu_bridge::platform_bridge();
                print(&serde_json::to_value(bridge.status()).map_err(|e| e.to_string())?);
                Ok(0)
            }
        },
    }
}

fn main() -> ExitCode {
    match run() {
        Ok(code) => ExitCode::from(code as u8),
        Err(e) => {
            eprintln!("{}", json!({ "error": e }));
            ExitCode::from(1)
        }
    }
}

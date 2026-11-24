//! qimu-server — the LAN host service (spec §4). One portable data
//! directory (SQLite + content-hash file store). All authorization is
//! enforced here, server-side: content in groups a user cannot see does
//! not exist in any response (spec §7).

use axum::{
    extract::{DefaultBodyLimit, Path, Query, State},
    http::{HeaderMap, StatusCode},
    routing::{delete, get, post},
    Json, Router,
};
use base64::Engine;
use qimu_core::manifest::{Binding, Manifest, SourceRef};
use qimu_core::model::{EntryPolicy, Id, IndexEntry};
use qimu_core::pptx::{self, Package};
use qimu_core::semver::{Increment, SemVer};
use qimu_core::store::*;
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

struct AppState {
    store: Mutex<Store>,
    /// Background AI-translation jobs by id (long decks: clients start
    /// a job and poll progress instead of blocking on one request).
    translate_jobs: Mutex<HashMap<String, TranslateJobRec>>,
    data_dir: std::path::PathBuf,
    /// Standalone local-server mode (org-model standalone gap):
    /// tokenless loopback requests act as the seed owner. On only
    /// for the bundled single-machine install — the QIMU_STANDALONE
    /// env wins; unset falls back to a standalone.on marker in the
    /// data dir. An organization server never enables it.
    standalone: bool,
}

type St = Arc<AppState>;
type Api<T> = Result<Json<T>, (StatusCode, Json<Value>)>;

fn err(status: StatusCode, msg: impl Into<String>) -> (StatusCode, Json<Value>) {
    (status, Json(json!({ "error": msg.into() })))
}

fn bad(msg: impl Into<String>) -> (StatusCode, Json<Value>) {
    err(StatusCode::BAD_REQUEST, msg)
}

fn forbidden() -> (StatusCode, Json<Value>) {
    err(StatusCode::FORBIDDEN, "没有权限")
}

fn not_found() -> (StatusCode, Json<Value>) {
    err(StatusCode::NOT_FOUND, "不存在")
}

fn bearer_token(headers: &HeaderMap) -> Option<String> {
    headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .map(|v| v.trim().to_string())
}

/// Peer IP as stamped by the peer_ip_layer middleware (the value
/// is written by the server itself from the TCP peer; a
/// client-sent header of the same name is overwritten in transit,
/// never read).
fn peer_ip_str(headers: &HeaderMap) -> Option<String> {
    headers
        .get("x-qimu-peer-ip")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string())
}

fn peer_is_loopback(headers: &HeaderMap) -> bool {
    peer_ip_str(headers)
        .and_then(|s| s.parse::<std::net::IpAddr>().ok())
        .map(|ip| ip.is_loopback())
        .unwrap_or(false)
}

/// Resolve the session behind the Authorization header:
/// Ok(Some(user)) for a named session; Ok(None) when there is no
/// header, or the header carries a valid *guest* session
/// (org-model G1); Err(401) when a header is present but the
/// session is unknown or expired — a stale token surfaces as 401
/// instead of silently downgrading (entry-fix ruling, kept).
fn session_user(
    st: &St,
    headers: &HeaderMap,
) -> Result<Option<UserRec>, (StatusCode, Json<Value>)> {
    let Some(token) = bearer_token(headers) else {
        return Ok(None);
    };
    let store = st.store.lock().unwrap();
    match store.session_by_token(&token) {
        Ok(Some(s)) => match s.user_id {
            Some(uid) => store
                .user_by_id(&uid)
                .map(Some)
                .map_err(|e| err(StatusCode::UNAUTHORIZED, e)),
            None => Ok(None),
        },
        _ => Err(err(
            StatusCode::UNAUTHORIZED,
            "会话无效或已过期，请重新登录",
        )),
    }
}

/// The standalone local-server mapping (standalone-fix §4 option
/// 1, org-model G1 phase): with standalone mode on, a *tokenless*
/// request from the loopback peer is the machine's owner — the
/// bundled server is the user's own data plane, so "installed and
/// usable with zero accounts" holds. Any presented token, and any
/// non-loopback peer, bypasses the mapping entirely; an
/// organization server runs with the mode off.
fn standalone_owner(st: &St, headers: &HeaderMap) -> Option<UserRec> {
    if !st.standalone || bearer_token(headers).is_some() || !peer_is_loopback(headers) {
        return None;
    }
    let store = st.store.lock().unwrap();
    store
        .standalone_owner_session(peer_ip_str(headers).as_deref())
        .ok()
}

fn auth(st: &St, headers: &HeaderMap) -> Result<UserRec, (StatusCode, Json<Value>)> {
    if let Some(user) = session_user(st, headers)? {
        return Ok(user);
    }
    if bearer_token(headers).is_none() {
        if let Some(owner) = standalone_owner(st, headers) {
            return Ok(owner);
        }
        return Err(err(StatusCode::UNAUTHORIZED, "未登录"));
    }
    // A valid guest session was presented at a named-identity door:
    // guests read public content, they do not act under a name.
    Err(err(StatusCode::UNAUTHORIZED, "该操作需要实名登录"))
}

/// Optional auth for the anonymous guest read surface (user ruling
/// 2026-11-23: the client opens without login). Returns the user
/// when a valid named session is present, None for guests (no
/// header, or a valid guest session) — and rejects a *present but
/// invalid* token, so a stale session still surfaces as 401
/// instead of silently downgrading. In standalone mode a tokenless
/// loopback request resolves to the owner, as in auth().
fn auth_opt(st: &St, headers: &HeaderMap) -> Result<Option<UserRec>, (StatusCode, Json<Value>)> {
    match session_user(st, headers)? {
        Some(user) => Ok(Some(user)),
        None => Ok(standalone_owner(st, headers)),
    }
}

/// Whether the request carries a valid guest session (org-model
/// G1/G2): the key that opens the entry-funneled guest reads.
fn has_guest_session(st: &St, headers: &HeaderMap) -> bool {
    let Some(token) = bearer_token(headers) else {
        return false;
    };
    let store = st.store.lock().unwrap();
    matches!(store.session_by_token(&token), Ok(Some(s)) if s.kind == "guest")
}

/// Read access for the version resource endpoints (org-model G2):
/// a named user passes the group ACL as before; failing that, a
/// guest session passes only when a live org_public showroom item
/// publishes this version — the entry funnel (DESIGN §2.3), never
/// the bare version id.
fn version_read_allowed(
    st: &St,
    headers: &HeaderMap,
    auth_res: &Result<UserRec, (StatusCode, Json<Value>)>,
    version_id: &str,
    group_id: &str,
) -> Result<bool, (StatusCode, Json<Value>)> {
    match auth_res {
        Ok(user) => {
            let store = st.store.lock().unwrap();
            store.can_see_group(user, group_id).map_err(bad)
        }
        Err(_) => {
            if !has_guest_session(st, headers) {
                return Ok(false);
            }
            let store = st.store.lock().unwrap();
            store.guest_published_version(version_id).map_err(bad)
        }
    }
}

fn need_cap(
    st: &St,
    user: &UserRec,
    group_id: &str,
    cap: &str,
) -> Result<(), (StatusCode, Json<Value>)> {
    let store = st.store.lock().unwrap();
    let caps = store.capabilities(user, group_id).map_err(bad)?;
    if caps.contains(cap) || caps.contains("manage") {
        Ok(())
    } else {
        Err(forbidden())
    }
}

fn b64e(bytes: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

fn b64d(s: &str) -> Result<Vec<u8>, (StatusCode, Json<Value>)> {
    base64::engine::general_purpose::STANDARD
        .decode(s)
        .map_err(|e| bad(format!("base64 解码失败: {e}")))
}

fn visible_set(
    st: &St,
    user: &UserRec,
) -> Result<Option<std::collections::HashSet<String>>, (StatusCode, Json<Value>)> {
    let store = st.store.lock().unwrap();
    store.visible_groups(user).map_err(bad)
}

// ---------------- auth & profile ----------------

#[derive(Deserialize)]
struct LoginReq {
    username: String,
    password: String,
}

async fn login(State(st): State<St>, headers: HeaderMap, Json(req): Json<LoginReq>) -> Api<Value> {
    let store = st.store.lock().unwrap();
    let (token, user) = store
        .login_with_ip(
            &req.username,
            &req.password,
            peer_ip_str(&headers).as_deref(),
        )
        .map_err(bad)?;
    Ok(Json(json!({ "token": token, "user": user })))
}

/// Anonymous guest session issuance (org-model G1): the client
/// presents nothing but the server address it is connecting to;
/// the server returns an opaque guest token (24h, revocable,
/// IP-stamped — the row doubles as the minimal audit record for
/// anonymous reads, G4). A guest session reads org_public content
/// through the entry funnel only; auth() rejects it at every
/// named-identity door.
async fn guest_auth(State(st): State<St>, headers: HeaderMap) -> Api<Value> {
    let store = st.store.lock().unwrap();
    let token = store
        .create_guest_session(peer_ip_str(&headers).as_deref())
        .map_err(bad)?;
    Ok(Json(json!({
        "token": token,
        "kind": "guest",
        "ttl_ms": GUEST_TTL_MS,
    })))
}

async fn logout(State(st): State<St>, headers: HeaderMap) -> Api<Value> {
    // Any live session may end itself — named or guest (dropping
    // its guest session is how an anonymous client disconnects
    // cleanly).
    let token = bearer_token(&headers).ok_or_else(|| err(StatusCode::UNAUTHORIZED, "未登录"))?;
    let store = st.store.lock().unwrap();
    match store.session_by_token(&token) {
        Ok(Some(_)) => {}
        _ => return Err(err(StatusCode::UNAUTHORIZED, "未登录")),
    }
    store.logout(&token).map_err(bad)?;
    Ok(Json(json!({ "ok": true })))
}

async fn me(State(st): State<St>, headers: HeaderMap) -> Api<Value> {
    let user = auth(&st, &headers)?;
    let store = st.store.lock().unwrap();
    let profile: HashMap<String, String> =
        store.profile(&user.id).map_err(bad)?.into_iter().collect();
    Ok(Json(json!({ "user": user, "profile": profile })))
}

#[derive(Deserialize)]
struct ProfileReq {
    fields: HashMap<String, String>,
}

async fn put_profile(
    State(st): State<St>,
    headers: HeaderMap,
    Json(req): Json<ProfileReq>,
) -> Api<Value> {
    let user = auth(&st, &headers)?;
    let store = st.store.lock().unwrap();
    for (k, v) in &req.fields {
        store.set_profile_field(&user.id, k, v).map_err(bad)?;
    }
    Ok(Json(json!({ "ok": true })))
}

// ---------------- org / groups / settings ----------------

async fn org(State(st): State<St>, headers: HeaderMap) -> Api<Value> {
    let user = auth(&st, &headers)?;
    let store = st.store.lock().unwrap();
    let departments = store.list_departments().map_err(bad)?;
    let users = if user.role == "member" {
        Value::Array(vec![])
    } else {
        json!(store.list_users().map_err(bad)?)
    };
    Ok(Json(json!({ "departments": departments, "users": users })))
}

fn contact_rows(
    role: &str,
    departments: &[DeptRec],
    mut users: Vec<UserRec>,
    query: &str,
) -> Vec<Value> {
    if role == "member" {
        return Vec::new();
    }
    users.retain(|user| {
        !user.disabled
            && (query.is_empty()
                || user.display_name.to_lowercase().contains(query)
                || user.username.to_lowercase().contains(query))
    });
    users.truncate(50);
    users
        .into_iter()
        .map(|user| {
            let dept_name = user
                .dept_id
                .as_deref()
                .and_then(|id| departments.iter().find(|dept| dept.id == id))
                .map(|dept| dept.name.clone())
                .unwrap_or_default();
            json!({
                "id": user.id,
                "display_name": user.display_name,
                "username": user.username,
                "dept_id": user.dept_id,
                "dept_name": dept_name,
            })
        })
        .collect()
}

async fn org_contacts(
    State(st): State<St>,
    headers: HeaderMap,
    Query(q): Query<HashMap<String, String>>,
) -> Api<Value> {
    let user = auth(&st, &headers)?;
    let store = st.store.lock().unwrap();
    let departments = store.list_departments().map_err(bad)?;
    let query = q
        .get("q")
        .map(|q| q.trim().to_lowercase())
        .unwrap_or_default();
    let users = contact_rows(
        &user.role,
        &departments,
        store.list_users().map_err(bad)?,
        &query,
    );
    Ok(Json(json!({ "users": users })))
}

#[derive(Deserialize)]
struct DeptReq {
    name: String,
    parent_id: Option<String>,
}

async fn create_dept(
    State(st): State<St>,
    headers: HeaderMap,
    Json(req): Json<DeptReq>,
) -> Api<Value> {
    let user = auth(&st, &headers)?;
    if user.role == "member" {
        return Err(forbidden());
    }
    let store = st.store.lock().unwrap();
    let d = store
        .create_department(&req.name, req.parent_id.as_deref())
        .map_err(bad)?;
    Ok(Json(json!(d)))
}

#[derive(Deserialize)]
struct CreateUserReq {
    username: String,
    display_name: String,
    password: String,
    dept_id: Option<String>,
    role: Option<String>,
}

async fn create_user(
    State(st): State<St>,
    headers: HeaderMap,
    Json(req): Json<CreateUserReq>,
) -> Api<Value> {
    let user = auth(&st, &headers)?;
    if user.role == "member" {
        return Err(forbidden());
    }
    let store = st.store.lock().unwrap();
    let u = store
        .create_user(
            &req.username,
            &req.display_name,
            &req.password,
            req.dept_id.as_deref(),
            req.role.as_deref().unwrap_or("member"),
        )
        .map_err(bad)?;
    Ok(Json(json!(u)))
}

#[derive(Deserialize)]
struct DisableReq {
    disabled: bool,
}

async fn disable_user(
    State(st): State<St>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(req): Json<DisableReq>,
) -> Api<Value> {
    let user = auth(&st, &headers)?;
    if user.role != "super" {
        return Err(forbidden());
    }
    let store = st.store.lock().unwrap();
    store.set_user_disabled(&id, req.disabled).map_err(bad)?;
    Ok(Json(json!({ "ok": true })))
}

async fn groups(State(st): State<St>, headers: HeaderMap) -> Api<Value> {
    let user = auth(&st, &headers)?;
    let store = st.store.lock().unwrap();
    let visible = store.visible_groups(&user).map_err(bad)?;
    let all = store.list_groups().map_err(bad)?;
    let list: Vec<GroupRec> = match visible {
        None => all,
        Some(set) => all.into_iter().filter(|g| set.contains(&g.id)).collect(),
    };
    Ok(Json(json!({ "groups": list })))
}

#[derive(Deserialize)]
struct GroupReq {
    name: String,
}

async fn create_group(
    State(st): State<St>,
    headers: HeaderMap,
    Json(req): Json<GroupReq>,
) -> Api<Value> {
    let user = auth(&st, &headers)?;
    if user.role != "super" {
        return Err(forbidden());
    }
    let store = st.store.lock().unwrap();
    Ok(Json(json!(store.create_group(&req.name).map_err(bad)?)))
}

async fn grants(State(st): State<St>, headers: HeaderMap) -> Api<Value> {
    let user = auth(&st, &headers)?;
    if user.role == "member" {
        return Err(forbidden());
    }
    let store = st.store.lock().unwrap();
    Ok(Json(json!({ "grants": store.list_grants().map_err(bad)? })))
}

#[derive(Deserialize)]
struct GrantReq {
    group_id: String,
    subject_type: String,
    subject_id: String,
    capability: String,
}

async fn add_grant(
    State(st): State<St>,
    headers: HeaderMap,
    Json(req): Json<GrantReq>,
) -> Api<Value> {
    let user = auth(&st, &headers)?;
    if user.role != "super" {
        return Err(forbidden());
    }
    let store = st.store.lock().unwrap();
    let g = store
        .add_grant(
            &req.group_id,
            &req.subject_type,
            &req.subject_id,
            &req.capability,
        )
        .map_err(bad)?;
    Ok(Json(json!(g)))
}

async fn company_profile(State(st): State<St>, headers: HeaderMap) -> Api<Value> {
    let _ = auth(&st, &headers)?;
    let store = st.store.lock().unwrap();
    let fields: HashMap<String, String> =
        store.company_profile().map_err(bad)?.into_iter().collect();
    Ok(Json(json!({ "fields": fields })))
}

async fn put_company_profile(
    State(st): State<St>,
    headers: HeaderMap,
    Json(req): Json<ProfileReq>,
) -> Api<Value> {
    let user = auth(&st, &headers)?;
    if user.role != "super" {
        return Err(forbidden());
    }
    let store = st.store.lock().unwrap();
    for (k, v) in &req.fields {
        store.set_company_field(k, v).map_err(bad)?;
    }
    Ok(Json(json!({ "ok": true })))
}

async fn languages(State(st): State<St>, headers: HeaderMap) -> Api<Value> {
    let _ = auth(&st, &headers)?;
    let store = st.store.lock().unwrap();
    let raw = store.get_setting("languages").map_err(bad)?;
    let langs: Value = raw
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_else(|| json!(["中文", "英语", "日语"]));
    Ok(Json(json!({
        "languages": langs,
        "codes": load_language_codes(&store),
    })))
}

#[derive(Deserialize)]
struct LanguagesReq {
    languages: Vec<String>,
}

async fn put_languages(
    State(st): State<St>,
    headers: HeaderMap,
    Json(req): Json<LanguagesReq>,
) -> Api<Value> {
    let user = auth(&st, &headers)?;
    if user.role != "super" {
        return Err(forbidden());
    }
    let store = st.store.lock().unwrap();
    store
        .set_setting("languages", &json!(req.languages).to_string())
        .map_err(bad)?;
    Ok(Json(json!({ "ok": true })))
}

// ---------------- AI translation (§3.13) ----------------

use qimu_core::translate::{GlossaryPair, Translator};

fn load_glossary(store: &qimu_core::store::Store) -> Vec<GlossaryPair> {
    store
        .get_setting("glossary")
        .ok()
        .flatten()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

async fn get_glossary(State(st): State<St>, headers: HeaderMap) -> Api<Value> {
    let _ = auth(&st, &headers)?;
    let store = st.store.lock().unwrap();
    Ok(Json(json!({ "pairs": load_glossary(&store) })))
}

#[derive(Deserialize)]
struct GlossaryReq {
    pairs: Vec<GlossaryPair>,
}

async fn put_glossary(
    State(st): State<St>,
    headers: HeaderMap,
    Json(req): Json<GlossaryReq>,
) -> Api<Value> {
    let user = auth(&st, &headers)?;
    if user.role != "super" {
        return Err(forbidden());
    }
    let store = st.store.lock().unwrap();
    store
        .set_setting(
            "glossary",
            &serde_json::to_string(&req.pairs).unwrap_or_default(),
        )
        .map_err(bad)?;
    Ok(Json(json!({ "ok": true })))
}

async fn get_ai_settings(State(st): State<St>, headers: HeaderMap) -> Api<Value> {
    let user = auth(&st, &headers)?;
    if user.role != "super" {
        return Err(forbidden());
    }
    let store = st.store.lock().unwrap();
    let get = |k: &str| store.get_setting(k).ok().flatten().unwrap_or_default();
    let p = get("ai_provider");
    Ok(Json(json!({
        "provider": if p.is_empty() { "stub".to_string() } else { p },
        "endpoint": get("ai_endpoint"),
        "model": get("ai_model"),
        "has_key": !get("ai_api_key").is_empty(),
    })))
}

#[derive(Deserialize)]
struct AiSettingsReq {
    provider: String,
    endpoint: Option<String>,
    model: Option<String>,
    api_key: Option<String>,
}

async fn put_ai_settings(
    State(st): State<St>,
    headers: HeaderMap,
    Json(req): Json<AiSettingsReq>,
) -> Api<Value> {
    let user = auth(&st, &headers)?;
    if user.role != "super" {
        return Err(forbidden());
    }
    let store = st.store.lock().unwrap();
    store
        .set_setting("ai_provider", &req.provider)
        .map_err(bad)?;
    if let Some(e) = &req.endpoint {
        store.set_setting("ai_endpoint", e).map_err(bad)?;
    }
    if let Some(m) = &req.model {
        store.set_setting("ai_model", m).map_err(bad)?;
    }
    if let Some(k) = &req.api_key {
        store.set_setting("ai_api_key", k).map_err(bad)?;
    }
    Ok(Json(json!({ "ok": true })))
}

/// OpenAI-compatible chat-completions provider, driven through the
/// craft-ai-integrations adapter layer (storytold/craft-ai-integrations
/// @db30646, MIT OR Apache-2.0; step 4 of design/组件接入台账.md): the
/// provider owns the HTTP call, request shaping and job lifecycle; this
/// struct only maps qimu's settings + translation prompt contract onto
/// its `text.complete` capability. The settings keys and the prompt /
/// JSON-array contract are unchanged from the previous hand-rolled ureq
/// implementation. (The ecosystem's AiClient/ToolDesk orchestration is
/// a desktop-launcher layer — shared store + OS keychain + consent —
/// and does not fit a server process; the adapter layer is the part
/// that replaces our provider code.)
struct OpenAiCompat {
    endpoint: String, // base URL, e.g. https://api.openai.com
    api_key: String,
    model: String,
}

impl OpenAiCompat {
    fn complete(&self, system: &str, user: &str) -> Result<String, String> {
        use craft_ai_core::{
            CapabilityId, CapabilitySpec, Features, InputRole, JobState, ModelId, ModelInfo,
            Payload, Provider, Request, SecretString,
        };
        // The configured model may be any OpenAI-compatible id, known to
        // the adapter's catalog or not: declare it as a chat model with
        // the same text.complete spec the catalog builds for known ids.
        let cap = CapabilityId::parse("text.complete").map_err(|e| e.to_string())?;
        let spec = CapabilitySpec::new(cap.clone())
            .accepting(InputRole::Prompt, 64)
            .requiring(InputRole::Prompt)
            .accepting(InputRole::Context, 64)
            .accepting(InputRole::System, 1)
            .with_features(Features {
                streaming: true,
                tools: true,
                structured_output: true,
            });
        let model_id =
            ModelId::new(self.model.clone()).map_err(|e| format!("模型名不合法：{e}"))?;
        let mut info = ModelInfo::new(model_id.clone(), self.model.clone());
        info.capabilities = vec![spec];
        // The settings field holds the service base (e.g.
        // https://api.openai.com); tolerate an admin pasting the
        // /v1-suffixed form instead of failing with /v1/v1 requests.
        let base = self.endpoint.trim().trim_end_matches('/');
        let base = base.strip_suffix("/v1").unwrap_or(base);
        let config = craft_ai_openai::OpenAiConfig::new(format!("{base}/v1"))
            .with_key(SecretString::new(self.api_key.clone()))
            .with_models(vec![info]);
        let provider = craft_ai_openai::OpenAiProvider::new(config)
            .map_err(|e| format!("翻译服务配置不合法：{e}"))?;
        let request = Request::new(cap)
            .with_input(InputRole::System, Payload::text(system))
            .with_input(InputRole::Prompt, Payload::text(user))
            .with_param("temperature", json!(0.2));
        let job = provider
            .submit(&model_id, &request)
            .map_err(|e| format!("翻译服务请求失败：{e}"))?;
        // The adapter runs the request on its own worker; poll to a
        // terminal state (its request timeout bounds the wait). Poll
        // events are incremental: a plain chat reply arrives as
        // TextDelta events (a structured one as a JSON output), so the
        // text is accumulated across polls.
        let mut text = String::new();
        for _ in 0..6000 {
            let snap = provider
                .poll(&job)
                .map_err(|e| format!("翻译服务请求失败：{e}"))?;
            for ev in &snap.events {
                match ev {
                    craft_ai_core::Event::TextDelta { text: delta } => text.push_str(delta),
                    craft_ai_core::Event::Output(o) => match &o.payload {
                        Payload::Text { text: t } => text.push_str(t),
                        Payload::Inline { data, .. } => {
                            if let Ok(s) = std::str::from_utf8(data) {
                                text.push_str(s);
                            }
                        }
                        _ => {}
                    },
                    _ => {}
                }
            }
            match snap.state {
                JobState::Succeeded => {
                    if text.is_empty() {
                        return Err("翻译服务返回格式异常".into());
                    }
                    return Ok(text);
                }
                JobState::Failed { error } => {
                    return Err(format!("翻译服务请求失败：{error}"));
                }
                JobState::Cancelled => {
                    return Err("翻译服务请求失败：任务被取消".into());
                }
                _ => std::thread::sleep(std::time::Duration::from_millis(200)),
            }
        }
        Err("翻译服务请求失败：等待结果超时".into())
    }
}

impl Translator for OpenAiCompat {
    fn translate(
        &self,
        texts: &[String],
        target_lang: &str,
        glossary: &[GlossaryPair],
    ) -> Result<Vec<String>, String> {
        let gloss = glossary
            .iter()
            .map(|g| format!("{} -> {}", g.src, g.dst))
            .collect::<Vec<_>>()
            .join("; ");
        let system = format!(
            "You translate presentation slide text into {target_lang}. \
             Return ONLY a JSON array of strings with exactly the same length and order as the input array. \
             Preserve {{{{placeholders}}}}, numbers, units and line breaks. \
             Never translate these terms differently: {gloss}. \
             If a string is empty or whitespace, return it unchanged."
        );
        let user = serde_json::to_string(texts).unwrap_or_default();
        let content = self.complete(&system, &user)?;
        let content = content
            .trim()
            .trim_start_matches("```json")
            .trim_start_matches("```")
            .trim_end_matches("```")
            .trim();
        let out: Vec<String> =
            serde_json::from_str(content).map_err(|e| format!("翻译服务未返回 JSON 数组：{e}"))?;
        Ok(out)
    }
}

#[derive(Deserialize)]
struct TranslateReq {
    version_id: String,
    target_lang: String,
}

/// Map a `languages` setting entry to the rendition language code.
/// The setting is edited as display names （中文 / 英语 / 日语）; entries
/// that already are codes pass through unchanged.
fn lang_code_of(raw: &str) -> String {
    match raw.trim() {
        "中文" | "简体中文" => "zh".to_string(),
        "英语" | "英文" => "en".to_string(),
        "日语" | "日文" => "ja".to_string(),
        other => other.to_string(),
    }
}

/// The server's configured content languages as rendition codes —
/// the only legal translation targets.
fn load_language_codes(store: &qimu_core::store::Store) -> Vec<String> {
    let raw = store
        .get_setting("languages")
        .ok()
        .flatten()
        .and_then(|s| serde_json::from_str::<Vec<String>>(&s).ok())
        .unwrap_or_else(|| vec!["中文".into(), "英语".into(), "日语".into()]);
    let mut codes: Vec<String> = Vec::new();
    for name in &raw {
        let code = lang_code_of(name);
        if !code.is_empty() && !codes.contains(&code) {
            codes.push(code);
        }
    }
    codes
}

/// Everything a translation run needs, loaded and validated up front
/// (capability, language whitelist, provider construction), so the
/// synchronous endpoint and the background job endpoint share one
/// pipeline.
struct TranslatePrep {
    unit_id: String,
    src_version: qimu_core::store::VersionRec,
    src_rendition: qimu_core::store::RenditionRec,
    frag_bytes: Vec<u8>,
    glossary: Vec<GlossaryPair>,
    translator: Box<dyn Translator + Send>,
    provider_name: String,
    target_lang: String,
    author: String,
}

fn prepare_translate(
    st: &St,
    user: &UserRec,
    unit_id: &str,
    req: &TranslateReq,
) -> Result<TranslatePrep, (StatusCode, Json<Value>)> {
    let unit = {
        let store = st.store.lock().unwrap();
        store.get_unit(unit_id).map_err(|_| not_found())?
    };
    need_cap(st, user, &unit.group_id, "publish")?;
    let (src_version, src_rendition, frag_bytes, glossary, provider_cfg, codes) = {
        let store = st.store.lock().unwrap();
        let v = store
            .get_version(&req.version_id)
            .map_err(|_| not_found())?;
        let r = store.get_rendition(&v.rendition_id).map_err(bad)?;
        if r.unit_id != unit_id {
            return Err(bad("版本不属于该内容块"));
        }
        let bytes = store.file_get(&v.fragment_hash).map_err(bad)?;
        let get = |k: &str| store.get_setting(k).ok().flatten().unwrap_or_default();
        (
            v,
            r,
            bytes,
            load_glossary(&store),
            (
                get("ai_provider"),
                get("ai_endpoint"),
                get("ai_model"),
                get("ai_api_key"),
            ),
            load_language_codes(&store),
        )
    };
    let target_lang = lang_code_of(&req.target_lang);
    if !codes.contains(&target_lang) {
        return Err(bad(format!(
            "目标语言「{}」不在内容语言清单中（当前清单：{}），请先在设置中维护内容语言",
            req.target_lang,
            codes.join("、")
        )));
    }
    let (provider_raw, endpoint, model, api_key) = provider_cfg;
    let translator: Box<dyn Translator + Send> = match provider_raw.as_str() {
        "openai-compat" => {
            if endpoint.is_empty() || api_key.is_empty() || model.is_empty() {
                return Err(bad("AI 翻译未配置完整（endpoint / model / api_key），请先在设置中配置，或使用 stub 提供方"));
            }
            Box::new(OpenAiCompat {
                endpoint,
                api_key,
                model,
            })
        }
        "" | "stub" => Box::new(qimu_core::translate::StubTranslator),
        other => {
            return Err(bad(format!(
                "未知的 AI 提供方「{other}」，请在设置中选择 stub 或 openai-compat"
            )))
        }
    };
    Ok(TranslatePrep {
        unit_id: unit_id.to_string(),
        src_version,
        src_rendition,
        frag_bytes,
        glossary,
        translator,
        provider_name: if provider_raw.is_empty() {
            "stub".to_string()
        } else {
            provider_raw
        },
        target_lang,
        author: user.display_name.clone(),
    })
}

/// Run a prepared translation: chunked slide-run translation with
/// glossary enforcement, speaker scripts alongside, then the target
/// rendition + DRAFT 1.0.0 + `ai` page bindings. Progress is reported
/// as (done, total) over slide runs + non-empty scripts combined.
fn run_translate(
    st: &St,
    prep: TranslatePrep,
    on_progress: &mut dyn FnMut(usize, usize),
) -> Result<Value, (StatusCode, Json<Value>)> {
    let pkg = Package::load(&prep.frag_bytes).map_err(bad)?;
    let src_scripts: Vec<String> = prep.src_version.scripts.clone();
    let non_empty: Vec<String> = src_scripts
        .iter()
        .filter(|s| !s.trim().is_empty())
        .cloned()
        .collect();
    let pkg_total = qimu_core::translate::count_translatable_runs(&pkg).map_err(bad)?;
    let grand_total = pkg_total + non_empty.len();
    let (mut new_pkg, run_count, mut gloss) = {
        let mut cb = |d: usize, _t: usize| on_progress(d, grand_total);
        qimu_core::translate::translate_package_chunked(
            &pkg,
            prep.translator.as_ref(),
            &prep.target_lang,
            &prep.glossary,
            &mut cb,
        )
        .map_err(bad)?
    };
    // translate the speaker scripts too, and write them into the notes
    let mut translated_scripts = src_scripts.clone();
    if !non_empty.is_empty() {
        let (out, rep) = {
            let mut cb = |d: usize, _t: usize| on_progress(pkg_total + d, grand_total);
            qimu_core::translate::translate_texts_chunked(
                prep.translator.as_ref(),
                &non_empty,
                &prep.target_lang,
                &prep.glossary,
                &mut cb,
            )
            .map_err(bad)?
        };
        gloss.merge(rep);
        let mut it = out.into_iter();
        for s in translated_scripts.iter_mut() {
            if !s.trim().is_empty() {
                *s = it.next().unwrap_or_default();
            }
        }
        for (i, s) in translated_scripts.iter().enumerate() {
            if !s.is_empty() {
                qimu_core::pptx::set_notes_text(&mut new_pkg, i, s);
            }
        }
    }
    let new_bytes = new_pkg.to_bytes().map_err(bad)?;
    let store = st.store.lock().unwrap();
    let rend = store
        .get_or_create_rendition(&prep.unit_id, &prep.src_rendition.form, &prep.target_lang)
        .map_err(bad)?;
    let version = store
        .insert_version(
            &rend.id,
            SemVer::new(1, 0, 0),
            prep.src_version.basis_revision,
            &prep.author,
            &format!(
                "AI 翻译草稿（待校对）· 源 {} v{} · {} 个文本run",
                prep.src_rendition.lang, prep.src_version.semver, run_count
            ),
            &new_bytes,
            "draft",
        )
        .map_err(bad)?;
    for i in 0..prep.src_version.page_count {
        let _ = store.add_page_binding(
            &prep.unit_id,
            &prep.src_version.id,
            i,
            &version.id,
            i,
            "ai",
            &prep.author,
        );
    }
    Ok(json!({
        "version": version,
        "rendition_id": rend.id,
        "provider": prep.provider_name,
        "translated_runs": run_count,
        "glossary": {
            "checked": gloss.checked,
            "repaired": gloss.repaired,
            "unresolved": gloss.unresolved,
        },
    }))
}

/// Translate one version into a target language: creates (or reuses)
/// the target rendition and publishes a DRAFT 1.0.0 whose provenance
/// is recorded in the note and in `ai` page bindings (§3.13/§3.15).
async fn translate_unit(
    State(st): State<St>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(req): Json<TranslateReq>,
) -> Api<Value> {
    let user = auth(&st, &headers)?;
    let prep = prepare_translate(&st, &user, &id, &req)?;
    let result = run_translate(&st, prep, &mut |_, _| {})?;
    Ok(Json(result))
}

/// One background translation job, polled by the client for progress.
struct TranslateJobRec {
    owner: String,
    state: String, // running | succeeded | failed
    done: usize,
    total: usize,
    error: Option<String>,
    result: Option<Value>,
}

async fn start_translate_job(
    State(st): State<St>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(req): Json<TranslateReq>,
) -> Api<Value> {
    let user = auth(&st, &headers)?;
    let prep = prepare_translate(&st, &user, &id, &req)?;
    static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let seq = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let millis = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    let job_id = format!("tj-{millis}-{seq}");
    {
        let mut jobs = st.translate_jobs.lock().unwrap();
        // Keep only a bounded tail of finished jobs for polling clients.
        if jobs.len() > 50 {
            jobs.retain(|_, j| j.state == "running");
        }
        jobs.insert(
            job_id.clone(),
            TranslateJobRec {
                owner: user.id.clone(),
                state: "running".into(),
                done: 0,
                total: 0,
                error: None,
                result: None,
            },
        );
    }
    let st2 = st.clone();
    let jid = job_id.clone();
    tokio::task::spawn_blocking(move || {
        let st3 = st2.clone();
        let jid2 = jid.clone();
        let mut cb = move |done: usize, total: usize| {
            if let Ok(mut jobs) = st3.translate_jobs.lock() {
                if let Some(j) = jobs.get_mut(&jid2) {
                    j.done = done;
                    j.total = total;
                }
            }
        };
        let outcome = run_translate(&st2, prep, &mut cb);
        if let Ok(mut jobs) = st2.translate_jobs.lock() {
            if let Some(j) = jobs.get_mut(&jid) {
                match outcome {
                    Ok(v) => {
                        j.state = "succeeded".into();
                        j.done = j.total;
                        j.result = Some(v);
                    }
                    Err((_, body)) => {
                        j.state = "failed".into();
                        j.error = Some(
                            body.0
                                .get("error")
                                .and_then(|e| e.as_str())
                                .unwrap_or("翻译失败")
                                .to_string(),
                        );
                    }
                }
            }
        }
    });
    Ok(Json(json!({ "job_id": job_id, "state": "running" })))
}

async fn get_translate_job(
    State(st): State<St>,
    headers: HeaderMap,
    Path(job_id): Path<String>,
) -> Api<Value> {
    let user = auth(&st, &headers)?;
    let jobs = st.translate_jobs.lock().unwrap();
    let j = jobs.get(&job_id).ok_or_else(not_found)?;
    if j.owner != user.id && user.role != "super" {
        return Err(forbidden());
    }
    Ok(Json(json!({
        "job_id": job_id,
        "state": j.state,
        "done": j.done,
        "total": j.total,
        "error": j.error,
        "result": j.result,
    })))
}

/// Non-privileged AI configuration signal for clients: which provider
/// is effective and whether a real one is fully configured. Never
/// returns the endpoint or the key (those stay behind the super-only
/// settings endpoint).
async fn get_ai_status(State(st): State<St>, headers: HeaderMap) -> Api<Value> {
    let _ = auth(&st, &headers)?;
    let store = st.store.lock().unwrap();
    let get = |k: &str| store.get_setting(k).ok().flatten().unwrap_or_default();
    let p = get("ai_provider");
    let provider = if p.is_empty() { "stub".to_string() } else { p };
    let configured = provider == "openai-compat"
        && !get("ai_endpoint").is_empty()
        && !get("ai_model").is_empty()
        && !get("ai_api_key").is_empty();
    Ok(Json(json!({
        "provider": provider,
        "configured": configured,
        "model": get("ai_model"),
    })))
}

async fn get_cache_policy(State(st): State<St>) -> Api<Value> {
    // Anonymous-readable (user ruling 2026-11-23): the value is a
    // device policy (allowed / max_mb) the client applies to its
    // local offline cache; it carries no user data. Writing it
    // stays super-only (put_cache_policy below).
    let store = st.store.lock().unwrap();
    let raw = store.get_setting("cache_policy").map_err(bad)?;
    let policy: Value = raw
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_else(|| json!({ "allowed": true, "max_mb": 500 }));
    Ok(Json(policy))
}

#[derive(Deserialize)]
struct CachePolicyReq {
    allowed: bool,
    max_mb: i64,
}

async fn put_cache_policy(
    State(st): State<St>,
    headers: HeaderMap,
    Json(req): Json<CachePolicyReq>,
) -> Api<Value> {
    let user = auth(&st, &headers)?;
    if user.role != "super" {
        return Err(forbidden());
    }
    let store = st.store.lock().unwrap();
    store
        .set_setting(
            "cache_policy",
            &json!({ "allowed": req.allowed, "max_mb": req.max_mb }).to_string(),
        )
        .map_err(bad)?;
    Ok(Json(json!({ "ok": true })))
}

// ---------------- folders ----------------

async fn folders(State(st): State<St>, headers: HeaderMap) -> Api<Value> {
    let _ = auth(&st, &headers)?;
    let store = st.store.lock().unwrap();
    Ok(Json(
        json!({ "folders": store.list_folders().map_err(bad)? }),
    ))
}

#[derive(Deserialize)]
struct FolderReq {
    name: String,
    parent_id: Option<String>,
}

async fn create_folder(
    State(st): State<St>,
    headers: HeaderMap,
    Json(req): Json<FolderReq>,
) -> Api<Value> {
    let _ = auth(&st, &headers)?;
    let store = st.store.lock().unwrap();
    Ok(Json(json!(store
        .create_folder(&req.name, req.parent_id.as_deref())
        .map_err(bad)?)))
}

#[derive(Deserialize)]
struct RenameReq {
    name: String,
}

async fn rename_folder(
    State(st): State<St>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(req): Json<RenameReq>,
) -> Api<Value> {
    let _ = auth(&st, &headers)?;
    let store = st.store.lock().unwrap();
    store.rename_folder(&id, &req.name).map_err(bad)?;
    Ok(Json(json!({ "ok": true })))
}

/// DELETE /v1/folders/:id — empty folders only (the store refuses
/// non-empty ones with a specific reason).
async fn delete_folder(
    State(st): State<St>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Api<Value> {
    let _ = auth(&st, &headers)?;
    let store = st.store.lock().unwrap();
    store.delete_folder(&id).map_err(bad)?;
    Ok(Json(json!({ "ok": true })))
}

#[derive(Deserialize)]
struct MainIndexReq {
    index_id: Option<String>,
}

/// Designate the folder's main index (主索引, spec §3.10). The index
/// must live in this folder; pass index_id=null to clear.
async fn set_folder_main_index(
    State(st): State<St>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(req): Json<MainIndexReq>,
) -> Api<Value> {
    let _ = auth(&st, &headers)?;
    let store = st.store.lock().unwrap();
    if let Some(xid) = req.index_id.as_deref() {
        let idx = store.get_index(xid).map_err(bad)?;
        if idx.folder_id.as_deref() != Some(id.as_str()) {
            return Err(bad("主索引必须属于该文件夹".to_string()));
        }
    }
    store
        .set_folder_main_index(&id, req.index_id.as_deref())
        .map_err(bad)?;
    let folder = store
        .list_folders()
        .map_err(bad)?
        .into_iter()
        .find(|f| f.id == id);
    Ok(Json(json!(folder)))
}

// ---------------- content library ----------------

fn unit_json(st: &St, unit: &UnitRec, with_versions: bool) -> Value {
    let store = st.store.lock().unwrap();
    let mut rends = Vec::new();
    if let Ok(list) = store.list_renditions(&unit.id) {
        for r in list {
            let versions = if with_versions {
                store.list_versions(&r.id).unwrap_or_default()
            } else {
                vec![]
            };
            let latest = store.latest_version(&r.id).ok().flatten();
            rends.push(json!({
                "rendition": r,
                "latest": latest,
                "versions": versions,
            }));
        }
    }
    json!({ "unit": unit, "renditions": rends })
}

async fn list_units(State(st): State<St>, headers: HeaderMap) -> Api<Value> {
    let user = auth(&st, &headers)?;
    let visible = visible_set(&st, &user)?;
    let units = {
        let store = st.store.lock().unwrap();
        store.list_units(visible.as_ref()).map_err(bad)?
    };
    let out: Vec<Value> = units.iter().map(|u| unit_json(&st, u, false)).collect();
    Ok(Json(json!({ "units": out })))
}

async fn get_unit(State(st): State<St>, headers: HeaderMap, Path(id): Path<String>) -> Api<Value> {
    let user = auth(&st, &headers)?;
    let unit = {
        let store = st.store.lock().unwrap();
        store.get_unit(&id).map_err(|_| not_found())?
    };
    if !{
        let store = st.store.lock().unwrap();
        store.can_see_group(&user, &unit.group_id).map_err(bad)?
    } {
        return Err(not_found());
    }
    Ok(Json(unit_json(&st, &unit, true)))
}

#[derive(Deserialize)]
struct UnitReq {
    name: String,
    folder_id: Option<String>,
    group_id: String,
    maintainer: Option<String>,
    lead_lang: Option<String>,
    description: Option<String>,
}

async fn create_unit(
    State(st): State<St>,
    headers: HeaderMap,
    Json(req): Json<UnitReq>,
) -> Api<Value> {
    let user = auth(&st, &headers)?;
    need_cap(&st, &user, &req.group_id, "publish")?;
    let store = st.store.lock().unwrap();
    let u = store
        .create_unit(
            &req.name,
            req.folder_id.as_deref(),
            &req.group_id,
            req.maintainer.as_deref().unwrap_or(&user.display_name),
            req.lead_lang.as_deref().unwrap_or("zh"),
            req.description.as_deref().unwrap_or(""),
        )
        .map_err(bad)?;
    Ok(Json(json!(u)))
}

#[derive(Deserialize)]
struct PatchUnitReq {
    name: Option<String>,
    folder_id: Option<Option<String>>,
    maintainer: Option<String>,
    description: Option<String>,
}

async fn patch_unit(
    State(st): State<St>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(req): Json<PatchUnitReq>,
) -> Api<Value> {
    let user = auth(&st, &headers)?;
    let unit = {
        let store = st.store.lock().unwrap();
        store.get_unit(&id).map_err(|_| not_found())?
    };
    need_cap(&st, &user, &unit.group_id, "publish")?;
    let store = st.store.lock().unwrap();
    let u = store
        .update_unit(
            &id,
            req.name.as_deref(),
            req.folder_id.as_ref().map(|o| o.as_deref()),
            req.maintainer.as_deref(),
            req.description.as_deref(),
        )
        .map_err(bad)?;
    Ok(Json(json!(u)))
}

#[derive(Deserialize)]
struct ImportBlock {
    name: String,
    pages: Vec<usize>,
    form: Option<String>,
    lang: Option<String>,
}

#[derive(Deserialize)]
struct ImportReq {
    filename: String,
    data_base64: String,
    folder_id: Option<String>,
    group_id: String,
    blocks: Option<Vec<ImportBlock>>,
}

#[derive(Deserialize)]
struct ProposeReq {
    data_base64: String,
}

/// Propose content blocks for an import (§5.8 step 1). Nothing is
/// stored; the importer confirms or adjusts the proposal.
async fn import_propose(
    State(st): State<St>,
    headers: HeaderMap,
    Json(req): Json<ProposeReq>,
) -> Api<Value> {
    let _ = auth(&st, &headers)?;
    let _ = &st;
    let bytes = b64d(&req.data_base64)?;
    let pkg = Package::load(&bytes).map_err(bad)?;
    let report = qimu_core::propose::propose_blocks(&pkg).map_err(bad)?;
    Ok(Json(serde_json::to_value(&report).unwrap_or_default()))
}

/// Import (§5.8 + §3.1): the deck is split, reassembled and verified
/// page-by-page BEFORE anything is committed; a failed check imports
/// nothing. Each block becomes one content unit with one rendition and
/// version 1.0.0; the original whole file is never archived.
async fn import_deck(
    State(st): State<St>,
    headers: HeaderMap,
    Json(req): Json<ImportReq>,
) -> Api<Value> {
    let user = auth(&st, &headers)?;
    need_cap(&st, &user, &req.group_id, "publish")?;
    let bytes = b64d(&req.data_base64)?;
    // The orchestration lives in core (Store::import_blocks) since
    // local-first P1: the desktop client's in-process local library
    // runs the identical split / verify / commit sequence, so it
    // exists exactly once. This handler keeps the auth, capability
    // and transport shape it always had.
    let blocks = req.blocks.map(|bs| {
        bs.into_iter()
            .map(|b| qimu_core::import::ImportBlockSpec {
                name: b.name,
                pages: b.pages,
                form: b.form,
                lang: b.lang,
            })
            .collect()
    });
    let store = st.store.lock().unwrap();
    let outcome = store
        .import_blocks(
            &req.filename,
            &bytes,
            req.folder_id.as_deref(),
            &req.group_id,
            &user.display_name,
            blocks,
        )
        .map_err(bad)?;
    Ok(Json(json!({
        "ok": true,
        "slide_count": outcome.slide_count,
        "fingerprints_match": outcome.fingerprints_match,
        "units": outcome.units,
    })))
}

#[derive(Deserialize)]
struct PublishReq {
    form: Option<String>,
    lang: Option<String>,
    data_base64: String,
    bump: Option<String>, // major | minor | patch
    note: Option<String>,
    status: Option<String>, // normal | draft
    new_revision: Option<bool>,
}

/// Publish a new version of a unit's rendition (clipboard publish and the
/// external-editor loop both land here). The version number is a
/// three-way click increment (§3.5); the new version becomes the latest
/// automatically — there is no manual "set as latest".
async fn publish(
    State(st): State<St>,
    headers: HeaderMap,
    Path(unit_id): Path<String>,
    Json(req): Json<PublishReq>,
) -> Api<Value> {
    let user = auth(&st, &headers)?;
    let unit = {
        let store = st.store.lock().unwrap();
        store.get_unit(&unit_id).map_err(|_| not_found())?
    };
    need_cap(&st, &user, &unit.group_id, "publish")?;
    let frag = b64d(&req.data_base64)?;
    // sanity: it must be a valid PPTX fragment
    Package::load(&frag).map_err(bad)?;
    let store = st.store.lock().unwrap();
    let basis = if req.new_revision.unwrap_or(false) {
        store.bump_unit_revision(&unit_id).map_err(bad)?
    } else {
        unit.current_revision
    };
    let rend = store
        .get_or_create_rendition(
            &unit_id,
            req.form.as_deref().unwrap_or("标准"),
            req.lang.as_deref().unwrap_or("zh"),
        )
        .map_err(bad)?;
    let latest = store.latest_version(&rend.id).map_err(bad)?;
    let inc = match req.bump.as_deref() {
        Some("major") => Increment::Major,
        Some("patch") => Increment::Patch,
        _ => Increment::Minor,
    };
    let next = match latest {
        Some(v) => SemVer::parse(&v.semver).map_err(bad)?.bumped(inc),
        None => SemVer::new(1, 0, 0),
    };
    let ver = store
        .insert_version(
            &rend.id,
            next,
            basis,
            &user.display_name,
            req.note.as_deref().unwrap_or(""),
            &frag,
            req.status.as_deref().unwrap_or("normal"),
        )
        .map_err(bad)?;
    Ok(Json(json!(ver)))
}

async fn recall_version(
    State(st): State<St>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Api<Value> {
    let user = auth(&st, &headers)?;
    let store = st.store.lock().unwrap();
    let ver = store.get_version(&id).map_err(|_| not_found())?;
    let rend = store.get_rendition(&ver.rendition_id).map_err(bad)?;
    let unit = store.get_unit(&rend.unit_id).map_err(bad)?;
    drop(store);
    need_cap(&st, &user, &unit.group_id, "publish")?;
    let store = st.store.lock().unwrap();
    store.set_version_status(&id, "recalled").map_err(bad)?;
    Ok(Json(json!({ "ok": true })))
}

async fn delete_version(
    State(st): State<St>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Api<Value> {
    let user = auth(&st, &headers)?;
    let store = st.store.lock().unwrap();
    let ver = store.get_version(&id).map_err(|_| not_found())?;
    let rend = store.get_rendition(&ver.rendition_id).map_err(bad)?;
    let unit = store.get_unit(&rend.unit_id).map_err(bad)?;
    drop(store);
    need_cap(&st, &user, &unit.group_id, "manage")?;
    let store = st.store.lock().unwrap();
    store.delete_version(&id).map_err(bad)?;
    Ok(Json(json!({ "ok": true })))
}

async fn version_fragment(
    State(st): State<St>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Api<Value> {
    let auth_res = auth(&st, &headers);
    let (ver, group_id) = {
        let store = st.store.lock().unwrap();
        let ver = store.get_version(&id).map_err(|_| not_found())?;
        let rend = store.get_rendition(&ver.rendition_id).map_err(bad)?;
        let unit = store.get_unit(&rend.unit_id).map_err(bad)?;
        (ver, unit.group_id)
    };
    if !version_read_allowed(&st, &headers, &auth_res, &id, &group_id)? {
        return Err(match auth_res {
            Ok(_) => not_found(),
            Err(e) => e,
        });
    }
    let store = st.store.lock().unwrap();
    let bytes = store.file_get(&ver.fragment_hash).map_err(bad)?;
    Ok(Json(json!({ "version": ver, "data_base64": b64e(&bytes) })))
}

/// Resolve the deckcraft-cli binary: $QIMU_DECKCRAFT_CLI, then a
/// deckcraft-cli sitting next to this server executable, then PATH.
/// Shared by the thumbnail renderer and the PDF exporter (Q21).
fn deckcraft_cli_bin() -> String {
    std::env::var("QIMU_DECKCRAFT_CLI")
        .ok()
        .filter(|s| !s.is_empty())
        .or_else(|| {
            let name = format!("deckcraft-cli{}", std::env::consts::EXE_SUFFIX);
            std::env::current_exe()
                .ok()
                .map(|exe| exe.with_file_name(name))
                .filter(|p| p.exists())
                .map(|p| p.display().to_string())
        })
        .unwrap_or_else(|| "deckcraft-cli".to_string())
}

/// Render a fragment PPTX with DeckCraft. Each request gets its own work
/// directory on the cache filesystem, and only complete PNGs are published.
fn render_thumbs_deckcraft(cache_dir: &std::path::Path, frag: &[u8]) -> Result<(), String> {
    let bin = deckcraft_cli_bin();
    std::fs::create_dir_all(cache_dir).map_err(|e| e.to_string())?;
    let work = tempfile::tempdir_in(cache_dir).map_err(|e| e.to_string())?;
    let pptx = work.path().join("frag.pptx");
    std::fs::write(&pptx, frag).map_err(|e| e.to_string())?;
    let outdir = work.path().join("rendered");
    let out = std::process::Command::new(&bin)
        .arg("render")
        .arg(&pptx)
        .arg("--all")
        .arg(&outdir)
        .args(["--scale", "0.75"])
        .output()
        .map_err(|e| format!("启动 deckcraft-cli 失败（{bin}）: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "deckcraft-cli 渲染失败: {}",
            String::from_utf8_lossy(&out.stderr)
        ));
    }

    let mut pages = Vec::new();
    for entry in
        std::fs::read_dir(&outdir).map_err(|e| format!("读取 DeckCraft 渲染结果失败: {e}"))?
    {
        let entry = entry.map_err(|e| e.to_string())?;
        let name = entry.file_name();
        if let Some(number) = name
            .to_str()
            .and_then(|s| s.strip_prefix("slide-"))
            .and_then(|s| s.strip_suffix(".png"))
            .and_then(|s| s.parse::<u32>().ok())
        {
            pages.push((number, entry.path()));
        }
    }
    if pages.is_empty() {
        return Err("DeckCraft 渲染未产出任何页面".into());
    }

    pages.sort_by_key(|(number, _)| *number);
    for (page, (_, rendered)) in pages.into_iter().enumerate() {
        let cached = cache_dir.join(format!("{page}.png"));
        if !cached.exists() {
            if let Err(error) = std::fs::rename(rendered, &cached) {
                // Another render may have published this page already.
                if !cached.exists() {
                    return Err(error.to_string());
                }
            }
        }
    }
    Ok(())
}

/// GET /v1/versions/:id/thumb/:page — DeckCraft PNG, returned as base64.
/// Use a fresh cache namespace because older DeckCraft caches can contain
/// thumbnails produced by the legacy LibreOffice fallback.
async fn version_thumb(
    State(st): State<St>,
    headers: HeaderMap,
    Path((id, page)): Path<(String, usize)>,
) -> Api<Value> {
    let auth_res = auth(&st, &headers);
    let (root, frag) = {
        let (ver, group_id) = {
            let store = st.store.lock().unwrap();
            let ver = store.get_version(&id).map_err(|_| not_found())?;
            let rend = store.get_rendition(&ver.rendition_id).map_err(bad)?;
            let unit = store.get_unit(&rend.unit_id).map_err(bad)?;
            (ver, unit.group_id)
        };
        if !version_read_allowed(&st, &headers, &auth_res, &id, &group_id)? {
            return Err(match auth_res {
                Ok(_) => not_found(),
                Err(e) => e,
            });
        }
        let store = st.store.lock().unwrap();
        let frag = store.file_get(&ver.fragment_hash).map_err(bad)?;
        (store.root().to_path_buf(), frag)
    };
    let cache_dir = root.join("thumbs").join("deckcraft-v2").join(&id);
    let cache_file = cache_dir.join(format!("{page}.png"));
    if !cache_file.exists() {
        tokio::task::spawn_blocking(move || render_thumbs_deckcraft(&cache_dir, &frag))
            .await
            .map_err(|e| bad(e.to_string()))?
            .map_err(bad)?;
    }
    let bytes = std::fs::read(&cache_file).map_err(|_| not_found())?;
    Ok(Json(json!({ "page": page, "image_base64": b64e(&bytes) })))
}

#[derive(Deserialize)]
struct VerTagReq {
    dim: String,
    value: String,
    strength: Option<String>,
}

#[derive(Deserialize)]
struct PageBindingReq {
    from_version_id: String,
    from_slot: i64,
    to_version_id: String,
    to_slot: i64,
    kind: Option<String>,
}

async fn list_page_bindings(
    State(st): State<St>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Api<Value> {
    let user = auth(&st, &headers)?;
    let store = st.store.lock().unwrap();
    let unit = store.get_unit(&id).map_err(|_| not_found())?;
    if !store.can_see_group(&user, &unit.group_id).map_err(bad)? {
        return Err(not_found());
    }
    let bindings = store.list_page_bindings(&id).map_err(bad)?;
    Ok(Json(json!({ "bindings": bindings })))
}

async fn add_page_binding(
    State(st): State<St>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(req): Json<PageBindingReq>,
) -> Api<Value> {
    let user = auth(&st, &headers)?;
    let unit = {
        let store = st.store.lock().unwrap();
        store.get_unit(&id).map_err(|_| not_found())?
    };
    need_cap(&st, &user, &unit.group_id, "publish")?;
    let store = st.store.lock().unwrap();
    let b = store
        .add_page_binding(
            &id,
            &req.from_version_id,
            req.from_slot,
            &req.to_version_id,
            req.to_slot,
            req.kind.as_deref().unwrap_or("manual"),
            &user.display_name,
        )
        .map_err(bad)?;
    Ok(Json(json!(b)))
}

async fn delete_page_binding(
    State(st): State<St>,
    headers: HeaderMap,
    Path((id, bid)): Path<(String, String)>,
) -> Api<Value> {
    let user = auth(&st, &headers)?;
    let unit = {
        let store = st.store.lock().unwrap();
        store.get_unit(&id).map_err(|_| not_found())?
    };
    need_cap(&st, &user, &unit.group_id, "publish")?;
    let store = st.store.lock().unwrap();
    store.delete_page_binding(&id, &bid).map_err(bad)?;
    Ok(Json(json!({ "ok": true })))
}

async fn get_version(
    State(st): State<St>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Api<Value> {
    let auth_res = auth(&st, &headers);
    let (v, group_id) = {
        let store = st.store.lock().unwrap();
        let v = store.get_version(&id).map_err(|_| not_found())?;
        let rend = store.get_rendition(&v.rendition_id).map_err(bad)?;
        let unit = store.get_unit(&rend.unit_id).map_err(bad)?;
        (v, unit.group_id)
    };
    if !version_read_allowed(&st, &headers, &auth_res, &id, &group_id)? {
        return Err(match auth_res {
            Ok(_) => not_found(),
            Err(e) => e,
        });
    }
    Ok(Json(json!({ "version": v })))
}

#[derive(Deserialize)]
struct ScriptsReq {
    scripts: Vec<String>,
}

/// Edit a version's per-page speaker scripts. Never bumps the version
/// (§3.16); requires publish capability on the unit's group.
async fn put_version_scripts(
    State(st): State<St>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(req): Json<ScriptsReq>,
) -> Api<Value> {
    let user = auth(&st, &headers)?;
    let unit = {
        let store = st.store.lock().unwrap();
        let v = store.get_version(&id).map_err(|_| not_found())?;
        let rend = store.get_rendition(&v.rendition_id).map_err(bad)?;
        store.get_unit(&rend.unit_id).map_err(bad)?
    };
    need_cap(&st, &user, &unit.group_id, "publish")?;
    let store = st.store.lock().unwrap();
    store.set_version_scripts(&id, &req.scripts).map_err(bad)?;
    Ok(Json(json!({ "ok": true })))
}

async fn add_version_tag(
    State(st): State<St>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(req): Json<VerTagReq>,
) -> Api<Value> {
    let user = auth(&st, &headers)?;
    let store = st.store.lock().unwrap();
    store.get_version(&id).map_err(|_| not_found())?;
    let t = store
        .add_version_tag(
            &id,
            &req.dim,
            &req.value,
            req.strength.as_deref().unwrap_or("fits"),
            &user.display_name,
        )
        .map_err(bad)?;
    Ok(Json(json!(t)))
}

async fn list_version_tags(
    State(st): State<St>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Api<Value> {
    let _ = auth(&st, &headers)?;
    let store = st.store.lock().unwrap();
    Ok(Json(
        json!({ "tags": store.list_version_tags(&id).map_err(bad)? }),
    ))
}

#[derive(Deserialize)]
struct SearchQuery {
    q: String,
}

async fn search(
    State(st): State<St>,
    headers: HeaderMap,
    Query(qp): Query<SearchQuery>,
) -> Api<Value> {
    let user = auth(&st, &headers)?;
    let visible = visible_set(&st, &user)?;
    let store = st.store.lock().unwrap();
    let hits = store.search(&qp.q, visible.as_ref()).map_err(bad)?;
    let out: Vec<Value> = hits
        .iter()
        .map(|(u, v)| json!({ "unit": u, "version": v }))
        .collect();
    Ok(Json(json!({ "hits": out })))
}

// ---------------- indexes ----------------

async fn list_indexes(State(st): State<St>, headers: HeaderMap) -> Api<Value> {
    let user = auth(&st, &headers)?;
    let visible = visible_set(&st, &user)?;
    let store = st.store.lock().unwrap();
    Ok(Json(json!({
        "indexes": store.list_indexes(visible.as_ref()).map_err(bad)?
    })))
}

#[derive(Deserialize)]
struct EntryReq {
    unit_id: String,
    rendition_id: String,
}

#[derive(Deserialize)]
struct IndexReq {
    name: String,
    folder_id: Option<String>,
    group_id: String,
    scope: Option<String>,
    entries: Vec<EntryReq>,
}

fn entry_req_to_entry(e: &EntryReq) -> IndexEntry {
    IndexEntry {
        unit_id: Id(e.unit_id.clone()),
        rendition_id: Id(e.rendition_id.clone()),
        policy: EntryPolicy::FollowLatest,
        locked_semver: None,
        excerpt_slots: None,
    }
}

async fn create_index(
    State(st): State<St>,
    headers: HeaderMap,
    Json(req): Json<IndexReq>,
) -> Api<Value> {
    let user = auth(&st, &headers)?;
    need_cap(&st, &user, &req.group_id, "publish")?;
    let entries: Vec<IndexEntry> = req.entries.iter().map(entry_req_to_entry).collect();
    let store = st.store.lock().unwrap();
    let idx = store
        .create_index(
            &req.name,
            req.folder_id.as_deref(),
            &req.group_id,
            &user.display_name,
            req.scope.as_deref().unwrap_or("company"),
            entries,
        )
        .map_err(bad)?;
    Ok(Json(json!(idx)))
}

#[derive(Deserialize)]
struct BranchQuery {
    branch: Option<String>,
}

fn resolved_entries(st: &St, commit: &CommitRec) -> Vec<Value> {
    resolved_entries_for(st, &commit.entries)
}

fn resolved_entries_for(st: &St, entries: &[IndexEntry]) -> Vec<Value> {
    let store = st.store.lock().unwrap();
    entries
        .iter()
        .map(|e| {
            let unit = store.get_unit(&e.unit_id.0).ok();
            let rend = store.get_rendition(&e.rendition_id.0).ok();
            let resolved = store.resolve_entry_version(e).ok().flatten();
            let locked_id = match &e.policy {
                EntryPolicy::Locked { version_id } => Some(version_id.0.clone()),
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

fn showroom_commit(st: &St, item: &ShowroomRec) -> Option<CommitRec> {
    let store = st.store.lock().unwrap();
    let commit_id = match &item.commit_id {
        Some(id) => Some(id.clone()),
        None => item
            .source_index_id
            .as_ref()
            .and_then(|id| store.branch_head(id, "main").ok().flatten()),
    }?;
    store.get_commit(&commit_id).ok()
}

fn latest_published_tag(
    store: &Store,
    index_id: &str,
    branch: &str,
) -> SResult<Option<IndexTagRec>> {
    let mut tags = store
        .list_tags(index_id)?
        .into_iter()
        .filter(|tag| tag.branch == branch)
        .collect::<Vec<_>>();
    tags.sort_by(|a, b| {
        SemVer::parse(&b.name)
            .ok()
            .cmp(&SemVer::parse(&a.name).ok())
    });
    Ok(tags.into_iter().next())
}

async fn get_index(
    State(st): State<St>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Query(q): Query<BranchQuery>,
) -> Api<Value> {
    let branch = q.branch.unwrap_or_else(|| "main".to_string());
    let (index, branches, tags, head) = match auth(&st, &headers) {
        Ok(user) => {
            let store = st.store.lock().unwrap();
            let index = store.get_index(&id).map_err(|_| not_found())?;
            if !store.can_see_group(&user, &index.group_id).map_err(bad)? {
                return Err(not_found());
            }
            let branches = store.list_branches(&id).map_err(bad)?;
            let tags = store.list_tags(&id).map_err(bad)?;
            let head = match store.branch_head(&id, &branch).map_err(bad)? {
                Some(cid) => Some(store.get_commit(&cid).map_err(bad)?),
                None => None,
            };
            (index, branches, tags, head)
        }
        Err(e) => {
            // Guest entry funnel (org-model G2): a guest session
            // may resolve an index only as the source chain of a
            // live org_public showroom item, and only at the item's
            // pinned commit — branches, tags and the moving head
            // stay named-session territory. This is what lets
            // anonymous plaza cards resolve covers and pages.
            if !has_guest_session(&st, &headers) {
                return Err(e);
            }
            let store = st.store.lock().unwrap();
            let item = store
                .guest_source_item(&id)
                .map_err(bad)?
                .ok_or_else(|| not_found())?;
            let index = store.get_index(&id).map_err(|_| not_found())?;
            let head = store.guest_item_commit(&item).map_err(bad)?;
            (index, Vec::new(), Vec::new(), head)
        }
    };
    let entries = head.as_ref().map(|c| resolved_entries(&st, c));
    Ok(Json(json!({
        "index": index,
        "branches": branches,
        "tags": tags,
        "head": head,
        "resolved_entries": entries,
    })))
}

async fn index_push_preview(
    State(st): State<St>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Query(q): Query<BranchQuery>,
) -> Api<Value> {
    let user = auth(&st, &headers)?;
    let branch = q.branch.unwrap_or_else(|| "main".to_string());
    if branch != "main" {
        return Err(bad("只能推送主分支的发布版本"));
    }
    let store = st.store.lock().unwrap();
    let index = store.get_index(&id).map_err(|_| not_found())?;
    if !store.can_see_group(&user, &index.group_id).map_err(bad)? {
        return Err(not_found());
    }
    let published = latest_published_tag(&store, &id, &branch).map_err(bad)?;
    let current = published
        .as_ref()
        .and_then(|tag| store.get_commit(&tag.commit_id).ok());
    let previous = store
        .list_showroom(Some("live"))
        .map_err(bad)?
        .into_iter()
        .find(|item| item.source_index_id.as_deref() == Some(id.as_str()));
    drop(store);
    let current_entries = current
        .as_ref()
        .map(|commit| resolved_entries(&st, commit))
        .unwrap_or_default();
    let previous_entries = previous
        .as_ref()
        .and_then(|item| showroom_commit(&st, item))
        .map(|commit| resolved_entries(&st, &commit))
        .unwrap_or_default();
    Ok(Json(json!({
        "published_version": published.as_ref().map(|tag| tag.name.clone()),
        "published_tag_id": published.as_ref().map(|tag| tag.id.clone()),
        "current_entries": current_entries,
        "previous_entries": previous_entries,
    })))
}

async fn list_commits(
    State(st): State<St>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Query(q): Query<BranchQuery>,
) -> Api<Value> {
    let _ = auth(&st, &headers)?;
    let branch = q.branch.unwrap_or_else(|| "main".to_string());
    let store = st.store.lock().unwrap();
    Ok(Json(json!({
        "commits": store.list_commits(&id, &branch).map_err(bad)?
    })))
}

#[derive(Deserialize)]
struct CommitReq {
    branch: Option<String>,
    message: String,
    entries: Vec<IndexEntry>,
}

async fn post_commit(
    State(st): State<St>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(req): Json<CommitReq>,
) -> Api<Value> {
    let user = auth(&st, &headers)?;
    let index = {
        let store = st.store.lock().unwrap();
        store.get_index(&id).map_err(|_| not_found())?
    };
    need_cap(&st, &user, &index.group_id, "publish")?;
    let store = st.store.lock().unwrap();
    let c = store
        .create_commit(
            &id,
            req.branch.as_deref().unwrap_or("main"),
            &user.display_name,
            &req.message,
            req.entries,
        )
        .map_err(bad)?;
    Ok(Json(json!(c)))
}

#[derive(Deserialize)]
struct BranchReq {
    name: String,
    from: Option<String>,
}

async fn create_branch(
    State(st): State<St>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(req): Json<BranchReq>,
) -> Api<Value> {
    let user = auth(&st, &headers)?;
    let index = {
        let store = st.store.lock().unwrap();
        store.get_index(&id).map_err(|_| not_found())?
    };
    need_cap(&st, &user, &index.group_id, "publish")?;
    let store = st.store.lock().unwrap();
    store
        .create_branch(&id, &req.name, req.from.as_deref().unwrap_or("main"))
        .map_err(bad)?;
    Ok(Json(json!({ "ok": true })))
}

#[derive(Deserialize)]
struct MergeReq {
    from: String,
    into: Option<String>,
    resolutions: Option<std::collections::HashMap<String, String>>,
}

async fn merge_preview(
    State(st): State<St>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(req): Json<MergeReq>,
) -> Api<Value> {
    let user = auth(&st, &headers)?;
    let index = {
        let store = st.store.lock().unwrap();
        store.get_index(&id).map_err(|_| not_found())?
    };
    need_cap(&st, &user, &index.group_id, "publish")?;
    let store = st.store.lock().unwrap();
    let p = store
        .preview_merge(&id, &req.from, req.into.as_deref().unwrap_or("main"))
        .map_err(bad)?;
    Ok(Json(json!(p)))
}

async fn merge_execute(
    State(st): State<St>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(req): Json<MergeReq>,
) -> Api<Value> {
    let user = auth(&st, &headers)?;
    let index = {
        let store = st.store.lock().unwrap();
        store.get_index(&id).map_err(|_| not_found())?
    };
    need_cap(&st, &user, &index.group_id, "publish")?;
    let store = st.store.lock().unwrap();
    let resolutions = req.resolutions.unwrap_or_default();
    let c = store
        .execute_merge(
            &id,
            &req.from,
            req.into.as_deref().unwrap_or("main"),
            &user.display_name,
            &resolutions,
        )
        .map_err(bad)?;
    Ok(Json(json!({ "commit": c })))
}

#[derive(Deserialize)]
struct TagReq {
    branch: Option<String>,
    name: String,
}

async fn create_tag(
    State(st): State<St>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(req): Json<TagReq>,
) -> Api<Value> {
    let user = auth(&st, &headers)?;
    let index = {
        let store = st.store.lock().unwrap();
        store.get_index(&id).map_err(|_| not_found())?
    };
    need_cap(&st, &user, &index.group_id, "publish")?;
    let store = st.store.lock().unwrap();
    let t = store
        .create_tag(&id, req.branch.as_deref().unwrap_or("main"), &req.name)
        .map_err(bad)?;
    Ok(Json(json!(t)))
}

#[derive(Deserialize)]
struct LifecycleReq {
    lifecycle: String,
}

async fn set_lifecycle(
    State(st): State<St>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(req): Json<LifecycleReq>,
) -> Api<Value> {
    let user = auth(&st, &headers)?;
    let index = {
        let store = st.store.lock().unwrap();
        store.get_index(&id).map_err(|_| not_found())?
    };
    need_cap(&st, &user, &index.group_id, "manage")?;
    let store = st.store.lock().unwrap();
    store
        .set_index_lifecycle(&id, &req.lifecycle)
        .map_err(bad)?;
    Ok(Json(json!({ "ok": true })))
}

#[derive(Deserialize)]
struct IndexGraphQuery {
    index_id: Option<String>,
}

async fn index_graphs(
    State(st): State<St>,
    headers: HeaderMap,
    Query(query): Query<IndexGraphQuery>,
) -> Api<Value> {
    let user = auth(&st, &headers)?;
    let visible = visible_set(&st, &user)?;
    let store = st.store.lock().unwrap();
    let mut graphs = std::collections::BTreeMap::new();
    if let Some(id) = query.index_id {
        let index = store.get_index(&id).map_err(|_| not_found())?;
        if !store.can_see_group(&user, &index.group_id).map_err(bad)? {
            return Err(not_found());
        }
        graphs.insert(id.clone(), store.index_graph(&id).map_err(bad)?);
    } else {
        for index in store.list_indexes(visible.as_ref()).map_err(bad)? {
            let graph = store.index_graph(&index.id).map_err(bad)?;
            graphs.insert(index.id, graph);
        }
    }
    Ok(Json(json!({ "graphs": graphs })))
}

#[derive(Deserialize)]
struct MoveIndexReq {
    folder_id: Option<String>,
}

async fn move_index_to_folder(
    State(st): State<St>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(req): Json<MoveIndexReq>,
) -> Api<Value> {
    let user = auth(&st, &headers)?;
    let index = st
        .store
        .lock()
        .unwrap()
        .get_index(&id)
        .map_err(|_| not_found())?;
    need_cap(&st, &user, &index.group_id, "manage")?;
    st.store
        .lock()
        .unwrap()
        .move_index_to_folder(&id, req.folder_id.as_deref())
        .map_err(bad)?;
    Ok(Json(json!({ "ok": true })))
}

#[derive(Deserialize)]
struct EnglishIndexReq {
    branch: String,
    name: String,
}

async fn create_english_index(
    State(st): State<St>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(req): Json<EnglishIndexReq>,
) -> Api<Value> {
    let user = auth(&st, &headers)?;
    let index = st
        .store
        .lock()
        .unwrap()
        .get_index(&id)
        .map_err(|_| not_found())?;
    need_cap(&st, &user, &index.group_id, "publish")?;
    let name = req.name.trim();
    if name.is_empty() {
        return Err(bad("请输入英文版索引名称"));
    }
    // The derivation lives in core (Store::derive_english_index)
    // since local-first P1: the desktop client's in-process local
    // library runs the identical English-version selection, so it
    // exists exactly once. This handler keeps the auth, capability
    // and name validation it always had.
    let store = st.store.lock().unwrap();
    let derived = store
        .derive_english_index(&user, &index, &req.branch, name)
        .map_err(bad)?;
    Ok(Json(json!(derived)))
}

// ---------------- export engine ----------------

/// Resolve entries to pages and assemble a deck with an embedded
/// manifest. Returns (deck bytes, manifest, page count).
fn build_export(
    st: &St,
    user: Option<&UserRec>,
    entries: &[IndexEntry],
    source: Option<SourceRef>,
    kind: &str,
    include_notes: bool,
) -> Result<(Vec<u8>, Manifest, i64), (StatusCode, Json<Value>)> {
    let store = st.store.lock().unwrap();
    let mut pkgs: Vec<Package> = Vec::new();
    let mut bindings: Vec<Binding> = Vec::new();
    let mut binding_scripts: Vec<Vec<String>> = Vec::new();
    for e in entries {
        let version = store
            .resolve_entry_version(e)
            .map_err(bad)?
            .ok_or_else(|| bad("有条目无法解析到版本（内容块尚无已发布版本）"))?;
        let frag_bytes = store.file_get(&version.fragment_hash).map_err(bad)?;
        let frag_pkg = Package::load(&frag_bytes).map_err(bad)?;
        let singles = pptx::split(&frag_pkg).map_err(bad)?;
        let chosen: Vec<usize> = match &e.excerpt_slots {
            Some(slots) => slots
                .iter()
                .filter_map(|s| s.parse::<usize>().ok())
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
        return Err(bad("没有可导出的页面"));
    }
    let page_count = pkgs.len() as i64;
    let mut assembled = qimu_core::assemble::assemble(&pkgs).map_err(bad)?;
    // Speaker scripts (§3.16): optionally write each version's per-page
    // 讲稿 back into the deck's notes pages.
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
    // Personal binding (§3.14): resolve {{placeholders}} per recipient.
    // 个人版 = exporter's own profile + company data; 公用版 = company
    // data only, personal placeholders scrubbed blank. A placeholder with
    // no value blocks the export — never fill with someone else's data.
    {
        use qimu_core::personalize;
        use std::collections::BTreeMap;
        let mut values: BTreeMap<String, String> = BTreeMap::new();
        for (k, v) in store.company_profile().map_err(bad)? {
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
        // No named exporter (a guest export through the showroom
        // funnel) takes the company path: personal placeholders
        // are scrubbed, never filled from anyone's profile.
        let scrub: &[&str] = if kind == "company" {
            personalize::PERSONAL_FIELDS
        } else if let Some(user) = user {
            values.insert("name".to_string(), user.display_name.clone());
            for (k, v) in store.profile(&user.id).map_err(bad)? {
                if !v.is_empty() {
                    values.insert(k, v);
                }
            }
            &[]
        } else {
            personalize::PERSONAL_FIELDS
        };
        let missing = personalize::substitute(&mut assembled, &values, scrub);
        if !missing.is_empty() {
            return Err(bad(format!(
                "导出已拦截：以下占位字段没有值（请先在设置里补全资料）：{}",
                missing.join("、")
            )));
        }
    }
    let deck = assembled.to_bytes().map_err(bad)?;
    let manifest = Manifest {
        deck_id: qimu_core::auth::new_id("deck"),
        kind: kind.to_string(),
        source,
        exported_ms: qimu_core::auth::now_ms(),
        bindings,
    };
    let deck = qimu_core::manifest::embed_manifest(&deck, &manifest).map_err(bad)?;
    Ok((deck, manifest, page_count))
}

#[derive(Deserialize)]
struct ExportReq {
    branch: Option<String>,
    tag: Option<String>,
    kind: Option<String>,
    include_notes: Option<bool>,
}

async fn export_index(
    State(st): State<St>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(req): Json<ExportReq>,
) -> Api<Value> {
    let user = auth(&st, &headers)?;
    let branch = req.branch.clone().unwrap_or_else(|| "main".to_string());
    let (index, commit) = {
        let store = st.store.lock().unwrap();
        let index = store.get_index(&id).map_err(|_| not_found())?;
        if !store.can_see_group(&user, &index.group_id).map_err(bad)? {
            return Err(not_found());
        }
        let commit = if let Some(tag_name) = &req.tag {
            let tags = store.list_tags(&id).map_err(bad)?;
            let tag = tags
                .iter()
                .find(|t| &t.name == tag_name && t.branch == branch)
                .ok_or_else(|| bad("标签不存在"))?;
            store.get_commit(&tag.commit_id).map_err(bad)?
        } else {
            let head = store
                .branch_head(&id, &branch)
                .map_err(bad)?
                .ok_or_else(|| bad("分支尚无提交"))?;
            store.get_commit(&head).map_err(bad)?
        };
        (index, commit)
    };
    let kind = req.kind.as_deref().unwrap_or("personal");
    let source = SourceRef {
        index_id: id.clone(),
        branch: branch.clone(),
        commit_id: commit.id.clone(),
        tag: req.tag.clone(),
    };
    let (deck, manifest, page_count) = build_export(
        &st,
        Some(&user),
        &commit.entries,
        Some(source),
        kind,
        req.include_notes.unwrap_or(false),
    )?;
    let export_id = qimu_core::auth::new_id("exp");
    {
        let store = st.store.lock().unwrap();
        let deck_hash = store.file_put(&deck).map_err(bad)?;
        store
            .insert_export(&ExportRec {
                id: export_id.clone(),
                index_id: id.clone(),
                branch: branch.clone(),
                commit_id: commit.id.clone(),
                tag: req.tag.clone(),
                user_id: user.id.clone(),
                created_ms: qimu_core::auth::now_ms(),
                deck_hash,
                page_count,
                kind: kind.to_string(),
                deck_id: manifest.deck_id.clone(),
                manifest_json: serde_json::to_string(&manifest).unwrap_or_default(),
            })
            .map_err(bad)?;
    }
    let _ = index;
    Ok(Json(json!({
        "export_id": export_id,
        "page_count": page_count,
        "manifest": manifest,
        "data_base64": b64e(&deck),
    })))
}

/// Recover a deck's manifest from the export registry when an editor
/// stripped the embedded copies; the caller presents only the deck_id
/// that survived in the file's core keywords. Access: the exporter, or
/// anyone who can see the source index's group.
async fn deck_manifest_by_id(
    State(st): State<St>,
    headers: HeaderMap,
    Path(deck_id): Path<String>,
) -> Api<Value> {
    let user = auth(&st, &headers)?;
    let store = st.store.lock().unwrap();
    let rec = store
        .export_by_deck_id(&deck_id)
        .map_err(bad)?
        .ok_or_else(|| not_found())?;
    let allowed = rec.user_id == user.id
        || store
            .get_index(&rec.index_id)
            .map(|idx| store.can_see_group(&user, &idx.group_id).unwrap_or(false))
            .unwrap_or(false);
    if !allowed {
        return Err(not_found());
    }
    let manifest: Value =
        serde_json::from_str(&rec.manifest_json).map_err(|e| bad(e.to_string()))?;
    Ok(Json(json!({ "manifest": manifest })))
}

async fn list_exports(
    State(st): State<St>,
    headers: HeaderMap,
    Query(q): Query<HashMap<String, String>>,
) -> Api<Value> {
    let _ = auth(&st, &headers)?;
    let store = st.store.lock().unwrap();
    Ok(Json(json!({
        "exports": store.list_exports(q.get("index_id").map(|s| s.as_str())).map_err(bad)?
    })))
}

// ---------------- update check (the colleague flow) ----------------

#[derive(Deserialize)]
struct CheckReq {
    manifest: Manifest,
}

fn diff_updates(st: &St, manifest: &Manifest) -> Result<Value, (StatusCode, Json<Value>)> {
    let src = manifest
        .source
        .clone()
        .ok_or_else(|| bad("该文件不是由索引导出的受管文件"))?;
    let store = st.store.lock().unwrap();
    let index = match store.get_index(&src.index_id) {
        Ok(i) => i,
        Err(_) => {
            return Ok(json!({
                "deck_id": manifest.deck_id, "status": "source_gone", "updates": []
            }))
        }
    };
    if index.lifecycle == "void" {
        return Ok(json!({
            "deck_id": manifest.deck_id, "status": "void", "updates": []
        }));
    }
    let head_id = store
        .branch_head(&src.index_id, &src.branch)
        .map_err(bad)?
        .ok_or_else(|| bad("来源分支不存在"))?;
    let head = store.get_commit(&head_id).map_err(bad)?;
    let mut updates: Vec<Value> = Vec::new();

    // recalls are a strong, separate category (§5.3)
    for b in &manifest.bindings {
        if let Ok(v) = store.get_version(&b.version_id) {
            if v.status == "recalled" {
                updates.push(json!({
                    "type": "recalled",
                    "unit_id": b.unit_id,
                    "rendition_id": b.rendition_id,
                    "version": b.version_semver,
                }));
            }
        }
    }

    let binding_by_rend: HashMap<&str, &Binding> = manifest
        .bindings
        .iter()
        .map(|b| (b.rendition_id.as_str(), b))
        .collect();

    for e in &head.entries {
        if matches!(e.policy, EntryPolicy::Locked { .. }) {
            continue; // locked entries are silently skipped (§5.3)
        }
        let resolved = store.resolve_entry_version(e).map_err(bad)?;
        let unit_name = store
            .get_unit(&e.unit_id.0)
            .map(|u| u.name)
            .unwrap_or_default();
        match binding_by_rend.get(e.rendition_id.0.as_str()) {
            None => updates.push(json!({
                "type": "added",
                "unit_id": e.unit_id.0,
                "unit_name": unit_name,
                "rendition_id": e.rendition_id.0,
                "to_version": resolved.as_ref().map(|v| v.semver.clone()),
                "to_version_id": resolved.as_ref().map(|v| v.id.clone()),
            })),
            Some(b) => {
                if let Some(v) = &resolved {
                    if v.id != b.version_id {
                        // per-page diff when the shape allows it
                        let slots: Vec<usize> = match &e.excerpt_slots {
                            Some(s) => s.iter().filter_map(|x| x.parse::<usize>().ok()).collect(),
                            None => (0..v.pages.len()).collect(),
                        };
                        let mut changed: Vec<usize> = Vec::new();
                        if slots.len() == b.page_fps.len() {
                            for (pos, slot) in slots.iter().enumerate() {
                                let new_fp = v.pages.get(*slot).map(|p| p.fp.as_str());
                                if new_fp != Some(b.page_fps[pos].as_str()) {
                                    changed.push(*slot);
                                }
                            }
                        }
                        updates.push(json!({
                            "type": "content_update",
                            "unit_id": e.unit_id.0,
                            "unit_name": unit_name,
                            "rendition_id": e.rendition_id.0,
                            "from_version": b.version_semver,
                            "to_version": v.semver,
                            "to_version_id": v.id,
                            "changed_pages": changed,
                            "page_count_from": b.page_fps.len(),
                            "page_count_to": slots.len(),
                        }));
                    }
                }
            }
        }
    }
    let head_rends: std::collections::HashSet<&str> = head
        .entries
        .iter()
        .map(|e| e.rendition_id.0.as_str())
        .collect();
    for b in &manifest.bindings {
        if !head_rends.contains(b.rendition_id.as_str()) {
            updates.push(json!({
                "type": "removed",
                "unit_id": b.unit_id,
                "rendition_id": b.rendition_id,
                "version": b.version_semver,
            }));
        }
    }

    Ok(json!({
        "deck_id": manifest.deck_id,
        "status": "ok",
        "index_id": src.index_id,
        "branch": src.branch,
        "exported_commit": src.commit_id,
        "head_commit": head_id,
        "up_to_date": updates.is_empty(),
        "updates": updates,
    }))
}

async fn check_updates(
    State(st): State<St>,
    headers: HeaderMap,
    Json(req): Json<CheckReq>,
) -> Api<Value> {
    let _ = auth(&st, &headers)?;
    Ok(Json(diff_updates(&st, &req.manifest)?))
}

/// The server half of in-place acceptance: for a manifest, return the new
/// version fragments for every content update / addition. The client (or
/// the PowerPoint COM bridge) performs the surgical in-place replacement
/// with these fragments.
async fn update_package(
    State(st): State<St>,
    headers: HeaderMap,
    Json(req): Json<CheckReq>,
) -> Api<Value> {
    let _ = auth(&st, &headers)?;
    let diff = diff_updates(&st, &req.manifest)?;
    let store = st.store.lock().unwrap();
    let mut pages: Vec<Value> = Vec::new();
    if let Some(list) = diff.get("updates").and_then(|u| u.as_array()) {
        for u in list {
            let t = u.get("type").and_then(|v| v.as_str()).unwrap_or("");
            if t != "content_update" && t != "added" {
                continue;
            }
            if let Some(vid) = u.get("to_version_id").and_then(|v| v.as_str()) {
                if let Ok(v) = store.get_version(vid) {
                    if let Ok(bytes) = store.file_get(&v.fragment_hash) {
                        pages.push(json!({
                            "type": t,
                            "rendition_id": u.get("rendition_id"),
                            "version_id": v.id,
                            "semver": v.semver,
                            "fragment_base64": b64e(&bytes),
                        }));
                    }
                }
            }
        }
    }
    Ok(Json(json!({ "diff": diff, "pages": pages })))
}

// ---------------- LAN update mirror (craft-update local-mirror) ----------------
// The server host doubles as the LAN update mirror: drop release files
// under <data>/update-mirror/ — a releases.json in the GitHub /releases
// list shape, plus one folder per tag holding that release's assets.
// Clients poll it with craft-update's protocol layer and only *prompt*
// (no silent download/install). Read-only and unauthenticated, like a
// static file host; the JSON is inert metadata and assets are installers
// the user still runs manually.

async fn update_releases(State(st): State<St>) -> axum::response::Response {
    use axum::response::IntoResponse;
    let path = st.data_dir.join("update-mirror").join("releases.json");
    let bytes = std::fs::read(&path).unwrap_or_else(|_| b"[]".to_vec());
    (
        [(axum::http::header::CONTENT_TYPE, "application/json")],
        bytes,
    )
        .into_response()
}

async fn update_asset(
    State(st): State<St>,
    Path((tag, name)): Path<(String, String)>,
) -> axum::response::Response {
    use axum::response::IntoResponse;
    // Path segments cannot contain '/', but refuse traversal spellings.
    if tag.contains("..")
        || name.contains("..")
        || name.contains('\\')
        || tag.is_empty()
        || name.is_empty()
    {
        return StatusCode::NOT_FOUND.into_response();
    }
    let path = st.data_dir.join("update-mirror").join(&tag).join(&name);
    match std::fs::read(&path) {
        Ok(bytes) => (
            [(axum::http::header::CONTENT_TYPE, "application/octet-stream")],
            bytes,
        )
            .into_response(),
        Err(_) => StatusCode::NOT_FOUND.into_response(),
    }
}

// ---------------- attachments / showroom / assemblies ----------------

#[derive(Deserialize)]
struct AttachmentReq {
    parent_type: String,
    parent_id: String,
    filename: String,
    data_base64: String,
}

async fn add_attachment(
    State(st): State<St>,
    headers: HeaderMap,
    Json(req): Json<AttachmentReq>,
) -> Api<Value> {
    let user = auth(&st, &headers)?;
    let bytes = b64d(&req.data_base64)?;
    let store = st.store.lock().unwrap();
    let rec = store
        .insert_attachment(
            &req.parent_type,
            &req.parent_id,
            &req.filename,
            &bytes,
            &user.display_name,
        )
        .map_err(bad)?;
    Ok(Json(json!(rec)))
}

async fn list_attachments(
    State(st): State<St>,
    headers: HeaderMap,
    Query(q): Query<HashMap<String, String>>,
) -> Api<Value> {
    let _ = auth(&st, &headers)?;
    let parent = q.get("parent_id").cloned().unwrap_or_default();
    let store = st.store.lock().unwrap();
    Ok(Json(json!({
        "attachments": store.list_attachments(&parent).map_err(bad)?
    })))
}

async fn download_attachment(
    State(st): State<St>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Api<Value> {
    let _ = auth(&st, &headers)?;
    let store = st.store.lock().unwrap();
    let rec = store.get_attachment(&id).map_err(|_| not_found())?;
    let bytes = store.file_get(&rec.hash).map_err(bad)?;
    Ok(Json(json!({
        "filename": rec.filename,
        "data_base64": b64e(&bytes),
    })))
}

#[derive(Deserialize)]
struct ShowroomReq {
    title: String,
    intro: Option<String>,
    source_index_id: Option<String>,
    published_tag_id: Option<String>,
}

fn showroom_source_commit(
    store: &Store,
    index_id: &str,
    published_tag_id: Option<&str>,
) -> SResult<String> {
    let tag = match published_tag_id {
        Some(tag_id) => store
            .list_tags(index_id)?
            .into_iter()
            .find(|tag| tag.id == tag_id)
            .ok_or("发布版本不存在或不属于该索引")?,
        None => latest_published_tag(store, index_id, "main")?
            .ok_or("请先发布索引版本，再推送到演示厅")?,
    };
    if tag.branch != "main" {
        return Err("只能推送主分支的发布版本".into());
    }
    Ok(store.get_commit(&tag.commit_id)?.id)
}

async fn submit_showroom(
    State(st): State<St>,
    headers: HeaderMap,
    Json(req): Json<ShowroomReq>,
) -> Api<Value> {
    let user = auth(&st, &headers)?;
    let source_group = match &req.source_index_id {
        Some(id) => {
            let store = st.store.lock().unwrap();
            Some(store.get_index(id).map_err(|_| not_found())?.group_id)
        }
        None => None,
    };
    if let Some(group_id) = &source_group {
        need_cap(&st, &user, group_id, "publish")?;
    }
    let store = st.store.lock().unwrap();
    let commit_id = match &req.source_index_id {
        Some(iid) => {
            showroom_source_commit(&store, iid, req.published_tag_id.as_deref()).map_err(bad)?
        }
        None => String::new(),
    };
    let rec = store
        .insert_showroom(
            &req.title,
            req.intro.as_deref().unwrap_or(""),
            req.source_index_id.as_deref(),
            (!commit_id.is_empty()).then_some(commit_id.as_str()),
            &user.display_name,
            "pending",
        )
        .map_err(bad)?;
    Ok(Json(json!(rec)))
}

async fn list_showroom(
    State(st): State<St>,
    headers: HeaderMap,
    Query(q): Query<HashMap<String, String>>,
) -> Api<Value> {
    // Anonymous-readable (user ruling 2026-11-23): guests only
    // ever see published (live) items; the own-submissions widening
    // for non-super users applies to authenticated users only.
    let user = auth_opt(&st, &headers)?;
    let store = st.store.lock().unwrap();
    let state = q.get("state").map(|s| s.as_str());
    let mut items = store.list_showroom(state).map_err(bad)?;
    match &user {
        None => items.retain(|item| store.guest_item_visible(item)),
        Some(u) if u.role != "super" => {
            items.retain(|item| store.showroom_item_access(u, item).unwrap_or(false));
        }
        _ => {}
    }
    drop(store);
    let items = items
        .into_iter()
        .map(|item| {
            let entries = showroom_commit(&st, &item)
                .map(|commit| resolved_entries(&st, &commit))
                .unwrap_or_default();
            let mut value = serde_json::to_value(&item).unwrap_or(Value::Null);
            value["resolved_entries"] = json!(entries);
            value
        })
        .collect::<Vec<_>>();
    Ok(Json(json!({ "items": items })))
}

#[derive(Deserialize)]
struct ReviewReq {
    state: String, // live | rejected
    /// Rejection reason (required when rejecting — the prototype
    /// sends it back to the submitter verbatim). Approvals may
    /// carry an optional note too.
    note: Option<String>,
}

async fn review_showroom(
    State(st): State<St>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(req): Json<ReviewReq>,
) -> Api<Value> {
    let user = auth(&st, &headers)?;
    if user.role != "super" {
        return Err(forbidden());
    }
    let note = req.note.as_deref().unwrap_or("");
    if req.state == "rejected" && note.trim().is_empty() {
        return Err(bad("驳回必须填写理由（会原样回到提交人）"));
    }
    let store = st.store.lock().unwrap();
    store
        .review_showroom(&id, &req.state, note, &user.display_name)
        .map_err(bad)?;
    Ok(Json(json!({ "ok": true })))
}

// ---------------- submissions (local-first P3, design §6.3) ----------------
// The real-byte submission chain, replacing reference-style
// showroom submission for content that lives in a submitter's
// local library: the client uploads the content package itself
// (manifest + fragment bytes), the package waits in the isolated
// staging zone (submissions table + `<data root>/staging/<id>/` —
// never the landed store's tables or its CAS), and only a super's
// approval runs the landing transaction that rebuilds it as
// organization content with a live showroom entry. The
// reference-style POST /v1/showroom above stays for one cycle for
// content that already lives on this server (design §8.3); the
// approval face distinguishes the two sources.

#[derive(Deserialize)]
struct SubmissionReq {
    title: String,
    intro: Option<String>,
    /// The submitter's note riding with the package.
    note: Option<String>,
    /// A rejected submission of the caller's that this one
    /// replaces (resubmission link).
    supersedes: Option<String>,
    /// The submitter-side local index id (provenance for the
    /// submitter's own client; never resolved server-side).
    source_index_id: Option<String>,
    manifest: SubmissionManifest,
    /// Fragment bytes by SHA-256 hash, base64 — the manifest's
    /// versions name the hashes they need.
    files: HashMap<String, String>,
}

async fn create_submission(
    State(st): State<St>,
    headers: HeaderMap,
    Json(req): Json<SubmissionReq>,
) -> Api<Value> {
    let user = auth(&st, &headers)?;
    let manifest = &req.manifest;
    if manifest.format != 1 {
        return Err(bad("提交包格式版本不受支持"));
    }
    if manifest.commit.entries.is_empty() {
        return Err(bad("提交包没有任何条目"));
    }
    // The file set must cover exactly the fragments the manifest's
    // versions name — no missing pages, no undeclared bytes.
    let mut want: std::collections::HashSet<&str> = std::collections::HashSet::new();
    for v in &manifest.versions {
        want.insert(v.fragment_hash.as_str());
    }
    for h in req.files.keys() {
        if !want.contains(h.as_str()) {
            return Err(bad("提交包包含清单外文件"));
        }
    }
    for h in &want {
        if !req.files.contains_key(*h) {
            return Err(bad("提交包缺少片段文件"));
        }
    }
    let store = st.store.lock().unwrap();
    if let Some(prev_id) = &req.supersedes {
        let prev = store
            .get_submission(prev_id)
            .map_err(bad)?
            .ok_or_else(|| bad("重提指向的原提交不存在"))?;
        if prev.submitter_id != user.id {
            return Err(bad("重提必须指向自己的提交"));
        }
        if prev.state != "rejected" {
            return Err(bad("只有被驳回的提交可以被重提取代"));
        }
    }
    let id = qimu_core::auth::new_id("sub");
    let page_count: i64 = manifest.versions.iter().map(|v| v.page_count).sum();
    let rec = SubmissionRec {
        id: id.clone(),
        title: req.title.clone(),
        intro: req.intro.clone().unwrap_or_default(),
        submitter_id: user.id.clone(),
        submitter_name: user.display_name.clone(),
        state: "pending".into(),
        note: req.note.clone().unwrap_or_default(),
        review_note: String::new(),
        reviewed_by: String::new(),
        reviewed_ms: 0,
        created_ms: qimu_core::auth::now_ms(),
        supersedes: req.supersedes.clone(),
        source_index_id: req.source_index_id.clone(),
        landed_index_id: None,
        landed_showroom_id: None,
        entry_count: manifest.commit.entries.len() as i64,
        page_count,
        land_error: String::new(),
    };
    let manifest_json =
        serde_json::to_string(manifest).map_err(|e| bad(e.to_string()))?;
    // Stage the bytes first (hash-verified inside core); the row
    // insert is the commit point — a failure before it leaves no
    // submission behind, and the error path removes the staging
    // directory again.
    let staged = (|| -> Result<(), (StatusCode, Json<Value>)> {
        for (hash, b64) in &req.files {
            let bytes = b64d(b64)?;
            store.stage_submission_file(&id, hash, &bytes).map_err(bad)?;
        }
        store.insert_submission(&rec, &manifest_json).map_err(bad)?;
        Ok(())
    })();
    if let Err(e) = staged {
        store.clear_submission_staging(&id);
        return Err(e);
    }
    Ok(Json(json!(rec)))
}

async fn list_submissions(
    State(st): State<St>,
    headers: HeaderMap,
    Query(q): Query<HashMap<String, String>>,
) -> Api<Value> {
    let user = auth(&st, &headers)?;
    let store = st.store.lock().unwrap();
    let items = if q.get("mine").map(|v| v == "1").unwrap_or(false) {
        store
            .list_submissions_by_submitter(&user.id)
            .map_err(bad)?
    } else {
        // The review queue face: supers only, mirroring the
        // showroom review gate.
        if user.role != "super" {
            return Err(forbidden());
        }
        store
            .list_submissions(q.get("state").map(|s| s.as_str()))
            .map_err(bad)?
    };
    Ok(Json(json!({ "items": items })))
}

async fn review_submission(
    State(st): State<St>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(req): Json<ReviewReq>,
) -> Api<Value> {
    let user = auth(&st, &headers)?;
    if user.role != "super" {
        return Err(forbidden());
    }
    let note = req.note.as_deref().unwrap_or("");
    let store = st.store.lock().unwrap();
    let sub = store
        .get_submission(&id)
        .map_err(bad)?
        .ok_or_else(not_found)?;
    if sub.state != "pending" {
        return Err(bad("该提交已处理"));
    }
    match req.state.as_str() {
        "live" => {
            let manifest_json = store.submission_manifest(&id).map_err(bad)?;
            let manifest: SubmissionManifest = serde_json::from_str(&manifest_json)
                .map_err(|e| bad(format!("提交包清单无法解析：{e}")))?;
            match store.land_submission(&id, &manifest, &user.display_name) {
                Ok((index_id, showroom_id)) => Ok(Json(json!({
                    "ok": true,
                    "landed_index_id": index_id,
                    "landed_showroom_id": showroom_id
                }))),
                Err(e) => {
                    // The landing rolled back: the submission
                    // stays pending with the reason recorded.
                    let _ = store.set_submission_land_error(&id, &e);
                    Err(bad(format!("落地失败（提交仍在待审区）：{e}")))
                }
            }
        }
        "rejected" => {
            if note.trim().is_empty() {
                return Err(bad("驳回必须填写理由（会原样回到提交人）"));
            }
            store
                .set_submission_rejected(&id, note, &user.display_name)
                .map_err(bad)?;
            Ok(Json(json!({ "ok": true })))
        }
        _ => Err(bad("未知审批结果")),
    }
}

async fn delete_submission(
    State(st): State<St>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Api<Value> {
    let user = auth(&st, &headers)?;
    let store = st.store.lock().unwrap();
    let sub = store
        .get_submission(&id)
        .map_err(bad)?
        .ok_or_else(not_found)?;
    // Withdraw (pending) and cleanup (rejected) belong to the
    // submitter; a landed submission's record is provenance and
    // only a super may remove it (the landed content itself is a
    // separate object and is never touched here).
    let allowed = user.role == "super"
        || (sub.state != "landed" && sub.submitter_id == user.id);
    if !allowed {
        return Err(forbidden());
    }
    store.delete_submission(&id).map_err(bad)?;
    Ok(Json(json!({ "ok": true })))
}

// ---------------- trash (soft delete, prototype #trashView) ----------------
// Deleting moves an object behind the store's trash overlay: it
// disappears from the normal lists but keeps every version and
// binding, so restore is lossless. Purge is the only destructive
// step. Indexes and units are gated on `manage` for their content
// group (the lifecycle endpoint's bar); showroom items belong to
// their submitter, with super acting for anyone.

#[derive(Deserialize)]
struct TrashAddReq {
    kind: String, // index | unit | showroom
    object_id: String,
}

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

/// Resolve (title, origin) for a move-to-trash, enforcing the
/// caller's right to delete the object.
fn trash_check_cap(
    st: &St,
    user: &UserRec,
    kind: &str,
    object_id: &str,
) -> Result<(String, String), (StatusCode, Json<Value>)> {
    let (title, origin, gid) = {
        let store = st.store.lock().unwrap();
        match kind {
            "index" => {
                let idx = store.get_index(object_id).map_err(|_| not_found())?;
                (
                    idx.name.clone(),
                    trash_origin(&store, idx.folder_id.as_deref(), "索引页"),
                    Some(idx.group_id.clone()),
                )
            }
            "unit" => {
                let unit = store.get_unit(object_id).map_err(|_| not_found())?;
                (
                    unit.name.clone(),
                    trash_origin(&store, unit.folder_id.as_deref(), "内容页"),
                    Some(unit.group_id.clone()),
                )
            }
            "showroom" => {
                let item = store
                    .get_showroom(object_id)
                    .map_err(bad)?
                    .ok_or_else(not_found)?;
                if user.role != "super" && item.submitted_by != user.display_name {
                    return Err(forbidden());
                }
                (item.title.clone(), "演示厅".to_string(), None)
            }
            _ => return Err(bad("unknown trash kind")),
        }
    };
    if let Some(gid) = gid {
        need_cap(st, user, &gid, "manage")?;
    }
    Ok((title, origin))
}

/// Restore / purge gate on an existing trash row: super, the person
/// who deleted it, or a `manage` holder on the object's group.
fn trash_row_cap(st: &St, user: &UserRec, row: &TrashRec) -> Result<(), (StatusCode, Json<Value>)> {
    if user.role == "super" || row.deleted_by == user.display_name {
        return Ok(());
    }
    let gid = {
        let store = st.store.lock().unwrap();
        match row.kind.as_str() {
            "unit" => store
                .get_unit(&row.object_id)
                .map(|u| u.group_id)
                .map_err(|_| not_found())?,
            "index" => store
                .get_index(&row.object_id)
                .map(|x| x.group_id)
                .map_err(|_| not_found())?,
            _ => return Err(forbidden()),
        }
    };
    need_cap(st, user, &gid, "manage")
}

/// Cover for a trash row: the version whose first page represents
/// the object (unit → latest version of its first rendition; index
/// → first resolved entry of the main branch head), plus its page
/// count for the preview frame.
fn trash_cover(store: &Store, row: &TrashRec) -> (Option<String>, i64) {
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

async fn list_trash(State(st): State<St>, headers: HeaderMap) -> Api<Value> {
    let user = auth(&st, &headers)?;
    let visible = visible_set(&st, &user)?;
    let store = st.store.lock().unwrap();
    let rows = store.trash_list().map_err(bad)?;
    let mut items = Vec::new();
    for row in rows {
        let see = if user.role == "super" || row.deleted_by == user.display_name {
            true
        } else {
            let in_groups = |gid: &str| {
                visible
                    .as_ref()
                    .map(|set| set.contains(gid))
                    .unwrap_or(true)
            };
            match row.kind.as_str() {
                "unit" => store
                    .get_unit(&row.object_id)
                    .map(|u| in_groups(&u.group_id))
                    .unwrap_or(false),
                "index" => store
                    .get_index(&row.object_id)
                    .map(|x| in_groups(&x.group_id))
                    .unwrap_or(false),
                _ => false,
            }
        };
        if !see {
            continue;
        }
        let (cover, pages) = trash_cover(&store, &row);
        let mut v = json!(row);
        v["cover_version_id"] = json!(cover);
        v["page_count"] = json!(pages);
        items.push(v);
    }
    Ok(Json(json!({ "items": items })))
}

async fn add_trash(
    State(st): State<St>,
    headers: HeaderMap,
    Json(req): Json<TrashAddReq>,
) -> Api<Value> {
    let user = auth(&st, &headers)?;
    let (title, origin) = trash_check_cap(&st, &user, &req.kind, &req.object_id)?;
    let store = st.store.lock().unwrap();
    let rec = store
        .trash_add(
            &req.kind,
            &req.object_id,
            &title,
            &origin,
            &user.display_name,
        )
        .map_err(bad)?;
    Ok(Json(json!(rec)))
}

async fn restore_trash(
    State(st): State<St>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Api<Value> {
    let user = auth(&st, &headers)?;
    let row = {
        let store = st.store.lock().unwrap();
        store.trash_get(&id).map_err(|_| not_found())?
    };
    trash_row_cap(&st, &user, &row)?;
    let store = st.store.lock().unwrap();
    let rec = store.trash_restore(&id).map_err(bad)?;
    Ok(Json(json!(rec)))
}

async fn purge_trash(
    State(st): State<St>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Api<Value> {
    let user = auth(&st, &headers)?;
    let row = {
        let store = st.store.lock().unwrap();
        store.trash_get(&id).map_err(|_| not_found())?
    };
    trash_row_cap(&st, &user, &row)?;
    let store = st.store.lock().unwrap();
    let rec = store.trash_purge(&id).map_err(bad)?;
    Ok(Json(json!(rec)))
}

async fn purge_all_trash(State(st): State<St>, headers: HeaderMap) -> Api<Value> {
    let user = auth(&st, &headers)?;
    let rows = {
        let store = st.store.lock().unwrap();
        store.trash_list().map_err(bad)?
    };
    let mut purged = 0usize;
    for row in rows {
        if trash_row_cap(&st, &user, &row).is_ok() {
            let store = st.store.lock().unwrap();
            if store.trash_purge(&row.id).is_ok() {
                purged += 1;
            }
        }
    }
    Ok(Json(json!({ "purged": purged })))
}

// ---------------- page-level visual editing ----------------

/// The editable text runs of one page of a version (visual editor
/// read side). Same visibility bar as `get_version`.
async fn version_page_text(
    State(st): State<St>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Query(q): Query<std::collections::HashMap<String, String>>,
) -> Api<Value> {
    let user = auth(&st, &headers)?;
    let (ver, bytes) = {
        let store = st.store.lock().unwrap();
        let ver = store.get_version(&id).map_err(|_| not_found())?;
        let rend = store.get_rendition(&ver.rendition_id).map_err(bad)?;
        let unit = store.get_unit(&rend.unit_id).map_err(bad)?;
        if !store.can_see_group(&user, &unit.group_id).map_err(bad)? {
            return Err(not_found());
        }
        let bytes = store.file_get(&ver.fragment_hash).map_err(bad)?;
        (ver, bytes)
    };
    let _ = ver;
    let page: usize = q.get("page").and_then(|p| p.parse().ok()).unwrap_or(0);
    let pkg = Package::load(&bytes).map_err(bad)?;
    let runs = pptx::slide_text_runs(&pkg, page).map_err(bad)?;
    // v2 editor read side: the slide's element model. A parse failure
    // degrades to an empty element list (the flat run list above keeps
    // the v1 editing path alive), never to an error.
    let (slide_cx, slide_cy) = pptx::slide_size(&pkg);
    let elements = pptx::slide_elements(&pkg, page).unwrap_or_default();
    Ok(Json(json!({
        "page": page,
        "runs": runs,
        "slide_cx": slide_cx,
        "slide_cy": slide_cy,
        "elements": serde_json::to_value(&elements).unwrap_or(json!([])),
    })))
}

#[derive(Deserialize)]
struct PageRunEdit {
    page: usize,
    run: usize,
    text: String,
}

#[derive(Deserialize)]
struct EditPagesReq {
    #[serde(default)]
    edits: Vec<PageRunEdit>,
    #[serde(default)]
    added_pages: usize,
    preview_page: Option<usize>,
    scripts: Option<Vec<String>>,
    /// v2 element operations (visual editor): text/style per run,
    /// paragraph & run insertion/deletion, element move/resize and
    /// picture replacement. Applied after the legacy run edits.
    #[serde(default)]
    ops: Vec<PageOpReq>,
    bump: Option<String>, // major | minor | patch (default minor)
    note: Option<String>,
    new_revision: Option<bool>,
}

/// One v2 element operation. `kind` selects the operation; the rest
/// are its parameters (see pptx::SlideOp for semantics). Picture
/// replacement carries the new image as base64 — the same upload
/// shape as fragment publishing — and the server stores it through
/// the content-addressed file layer before it enters the package.
#[derive(Deserialize)]
struct PageOpReq {
    kind: String,
    page: usize,
    el: usize,
    #[serde(default)]
    parents: Vec<usize>,
    preset: Option<String>,
    element_kind: Option<String>,
    para: Option<usize>,
    run: Option<usize>,
    text: Option<String>,
    sz: Option<f32>,
    b: Option<bool>,
    color: Option<String>,
    x: Option<i64>,
    y: Option<i64>,
    cx: Option<i64>,
    cy: Option<i64>,
    data_base64: Option<String>,
    ext: Option<String>,
}

/// Sniff an image format from magic bytes (None = unrecognized).
fn sniff_image_ext(bytes: &[u8]) -> Option<&'static str> {
    if bytes.starts_with(&[0x89, 0x50, 0x4E, 0x47]) {
        Some("png")
    } else if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
        Some("jpg")
    } else if bytes.starts_with(b"GIF8") {
        Some("gif")
    } else if bytes.starts_with(b"BM") {
        Some("bmp")
    } else {
        None
    }
}

/// Save page-level text edits as a NEW version of the same rendition
/// (the §8 rule: saving an edit follows the existing bump dialog —
/// the client sends the chosen bump level). The fragment base is the
/// opened version; the new version becomes the rendition's latest.
async fn edit_version_pages(
    State(st): State<St>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(req): Json<EditPagesReq>,
) -> Api<Value> {
    let user = auth(&st, &headers)?;
    let (ver, rend, unit, bytes) = {
        let store = st.store.lock().unwrap();
        let ver = store.get_version(&id).map_err(|_| not_found())?;
        let rend = store.get_rendition(&ver.rendition_id).map_err(bad)?;
        let unit = store.get_unit(&rend.unit_id).map_err(bad)?;
        let bytes = store.file_get(&ver.fragment_hash).map_err(bad)?;
        (ver, rend, unit, bytes)
    };
    need_cap(&st, &user, &unit.group_id, "publish")?;
    if req.ops.len() > 20_000 || req.edits.len() > 20_000 {
        return Err(bad("编辑操作过多"));
    }
    let mut pic_bytes: std::collections::HashMap<usize, (Vec<u8>, String)> =
        std::collections::HashMap::new();
    for (i, op) in req.ops.iter().enumerate() {
        let needs_picture = op.kind == "replace_pic"
            || (op.kind == "add_element" && op.element_kind.as_deref() == Some("picture"));
        if !needs_picture {
            continue;
        }
        let b64 = op
            .data_base64
            .as_deref()
            .ok_or_else(|| bad("替换图片缺少图片数据"))?;
        let bytes = b64d(b64)?;
        let sniffed =
            sniff_image_ext(&bytes).ok_or_else(|| bad("只支持 PNG / JPEG / GIF / BMP 图片"))?;
        let ext = match op.ext.as_deref().map(|e| e.to_ascii_lowercase()) {
            Some(e) if e == sniffed || (e == "jpeg" && sniffed == "jpg") => sniffed.to_string(),
            Some(_) => return Err(bad("图片扩展名与实际格式不一致")),
            None => sniffed.to_string(),
        };
        pic_bytes.insert(i, (bytes, ext));
    }
    let mut pkg = Package::load(&bytes).map_err(bad)?;
    pptx::append_blank_slides(&mut pkg, req.added_pages).map_err(bad)?;
    // Group the flat edit list by page and apply run replacements.
    let mut by_page: std::collections::BTreeMap<usize, Vec<(usize, String)>> =
        std::collections::BTreeMap::new();
    for e in &req.edits {
        by_page
            .entry(e.page)
            .or_default()
            .push((e.run, e.text.clone()));
    }
    let mut applied = req.added_pages;
    for (page, edits) in &by_page {
        applied += pptx::replace_slide_text_runs(&mut pkg, *page, edits).map_err(bad)?;
    }
    // v2 element operations, grouped by page, in request order (the
    // client emits them in an index-safe order per element).
    let mut ops_by_page: std::collections::BTreeMap<usize, Vec<pptx::SlideOp>> =
        std::collections::BTreeMap::new();
    for (i, op) in req.ops.iter().enumerate() {
        let sop = match op.kind.as_str() {
            "set_text" => pptx::SlideOp::SetText {
                el: op.el,
                para: op.para.unwrap_or(0),
                run: op.run.unwrap_or(0),
                text: op.text.clone().unwrap_or_default(),
            },
            "set_style" => pptx::SlideOp::SetStyle {
                el: op.el,
                para: op.para.unwrap_or(0),
                run: op.run.unwrap_or(0),
                sz: op.sz,
                b: op.b,
                color: op.color.clone(),
            },
            "add_para" => pptx::SlideOp::AddPara {
                el: op.el,
                after: op.para,
                text: op.text.clone().unwrap_or_default(),
            },
            "del_para" => pptx::SlideOp::DeletePara {
                el: op.el,
                para: op.para.unwrap_or(0),
            },
            "add_run" => pptx::SlideOp::AddRun {
                el: op.el,
                para: op.para.unwrap_or(0),
                text: op.text.clone().unwrap_or_default(),
            },
            "del_run" => pptx::SlideOp::DeleteRun {
                el: op.el,
                para: op.para.unwrap_or(0),
                run: op.run.unwrap_or(0),
            },
            "move" => pptx::SlideOp::Move {
                el: op.el,
                x: op.x.unwrap_or(0),
                y: op.y.unwrap_or(0),
            },
            "resize" => pptx::SlideOp::Resize {
                el: op.el,
                cx: op.cx.unwrap_or(0),
                cy: op.cy.unwrap_or(0),
            },
            "reset_style" => pptx::SlideOp::ResetStyle {
                el: op.el,
                para: op.para.unwrap_or(0),
                run: op.run.unwrap_or(0),
            },
            "line_color" => pptx::SlideOp::LineColor {
                el: op.el,
                color: op.color.clone().ok_or_else(|| bad("缺少线条颜色"))?,
            },
            "preset" => {
                let preset = op
                    .preset
                    .clone()
                    .filter(|preset| pptx::editor_preset_allowed(preset))
                    .ok_or_else(|| bad("未知的形状预设"))?;
                pptx::SlideOp::Preset { el: op.el, preset }
            }
            "add_element" => pptx::SlideOp::AddElement(pptx::NewSlideElement {
                kind: op.element_kind.clone().ok_or_else(|| bad("缺少元素类型"))?,
                preset: op.preset.clone(),
                text: op.text.clone().unwrap_or_default(),
                x: op.x.unwrap_or(0),
                y: op.y.unwrap_or(0),
                cx: op.cx.unwrap_or(0),
                cy: op.cy.unwrap_or(0),
                picture: pic_bytes.remove(&i),
            }),
            "replace_pic" => {
                let (bytes, ext) = pic_bytes
                    .get(&i)
                    .ok_or_else(|| bad("替换图片数据丢失"))?
                    .clone();
                pptx::SlideOp::ReplacePic {
                    el: op.el,
                    bytes,
                    ext,
                }
            }
            other => return Err(bad(format!("未知编辑操作 {other}"))),
        };
        if op.parents.len() > 32 {
            return Err(bad("组合层级过深"));
        }
        let sop = if op.parents.is_empty() {
            sop
        } else {
            pptx::SlideOp::InGroup {
                parents: op.parents.clone(),
                op: Box::new(sop),
            }
        };
        ops_by_page.entry(op.page).or_default().push(sop);
    }
    for (page, ops) in &ops_by_page {
        applied += pptx::apply_slide_ops(&mut pkg, *page, ops).map_err(bad)?;
    }
    if let Some(page) = req.preview_page {
        if page >= pptx::slides(&pkg).map_err(bad)?.len() {
            return Err(bad("预览页超出范围"));
        }
        let rendered = tokio::task::spawn_blocking(move || render_page_edit_preview(&pkg, page))
            .await
            .map_err(|error| bad(error.to_string()))?
            .map_err(bad)?;
        return Ok(Json(json!({ "data_base64": b64e(&rendered) })));
    }
    if applied == 0 {
        return Err(bad("没有可应用的修改"));
    }
    let pages = pptx::slides(&pkg).map_err(bad)?.len();
    let scripts = match req.scripts {
        Some(scripts) if scripts.len() != pages => return Err(bad("讲稿数量与页面数量不匹配")),
        Some(scripts) => scripts,
        None => {
            let mut scripts = ver.scripts;
            scripts.resize(pages, String::new());
            scripts
        }
    };
    let frag = pkg.to_bytes().map_err(bad)?;
    let store = st.store.lock().unwrap();
    let basis = if req.new_revision.unwrap_or(false) {
        store.bump_unit_revision(&unit.id).map_err(bad)?
    } else {
        unit.current_revision
    };
    let latest = store.latest_version(&rend.id).map_err(bad)?;
    let inc = match req.bump.as_deref() {
        Some("major") => Increment::Major,
        Some("patch") => Increment::Patch,
        _ => Increment::Minor,
    };
    let next = match latest {
        Some(v) => SemVer::parse(&v.semver).map_err(bad)?.bumped(inc),
        None => SemVer::new(1, 0, 0),
    };
    let new_ver = store
        .insert_version_with_scripts(
            &rend.id,
            next,
            basis,
            &user.display_name,
            req.note.as_deref().unwrap_or(""),
            &frag,
            "normal",
            Some(&scripts),
        )
        .map_err(bad)?;
    Ok(Json(json!({ "version": new_ver })))
}

fn render_page_edit_preview(pkg: &Package, page: usize) -> Result<Vec<u8>, String> {
    let work = tempfile::tempdir().map_err(|error| error.to_string())?;
    let input = work.path().join("edit.pptx");
    let image = work.path().join("stage.png");
    std::fs::write(&input, pkg.to_bytes()?).map_err(|error| error.to_string())?;
    let output = std::process::Command::new(deckcraft_cli_bin())
        .arg("render")
        .arg(&input)
        .args(["--slide", &page.to_string(), "--scale", "1", "--edit"])
        .arg(&image)
        .output()
        .map_err(|error| format!("启动 DeckCraft 预览失败：{error}"))?;
    if !output.status.success() {
        return Err(format!(
            "DeckCraft 预览失败：{}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    std::fs::read(image).map_err(|error| error.to_string())
}

// ---------------- custom groups / shares / library (§3.17, §7.7) ----------------

#[derive(Deserialize)]
struct CGroupReq {
    name: String,
}

async fn create_custom_group(
    State(st): State<St>,
    headers: HeaderMap,
    Json(req): Json<CGroupReq>,
) -> Api<Value> {
    let user = auth(&st, &headers)?;
    let store = st.store.lock().unwrap();
    let id = store
        .create_custom_group(&user.id, &req.name)
        .map_err(bad)?;
    Ok(Json(json!({ "id": id })))
}

async fn list_custom_groups(State(st): State<St>, headers: HeaderMap) -> Api<Value> {
    let user = auth(&st, &headers)?;
    let store = st.store.lock().unwrap();
    let groups = store.list_custom_groups_for(&user.id).map_err(bad)?;
    Ok(Json(json!({ "groups": groups })))
}

#[derive(Deserialize)]
struct MemberReq {
    user_id: String,
}

async fn add_custom_group_member(
    State(st): State<St>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(req): Json<MemberReq>,
) -> Api<Value> {
    let user = auth(&st, &headers)?;
    let store = st.store.lock().unwrap();
    let (_, owner) = store
        .custom_group(&id)
        .map_err(bad)?
        .ok_or_else(|| not_found())?;
    if owner != user.id && user.role != "super" {
        return Err(forbidden());
    }
    store
        .add_custom_group_member(&id, &req.user_id)
        .map_err(bad)?;
    Ok(Json(json!({ "ok": true })))
}

async fn remove_custom_group_member(
    State(st): State<St>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(req): Json<MemberReq>,
) -> Api<Value> {
    let user = auth(&st, &headers)?;
    let store = st.store.lock().unwrap();
    let (_, owner) = store
        .custom_group(&id)
        .map_err(bad)?
        .ok_or_else(|| not_found())?;
    if owner != user.id && user.id != req.user_id && user.role != "super" {
        return Err(forbidden());
    }
    store
        .remove_custom_group_member(&id, &req.user_id)
        .map_err(bad)?;
    Ok(Json(json!({ "ok": true })))
}

#[derive(Deserialize)]
struct ShareReq {
    item_kind: String, // showroom | assembly
    item_id: String,
    target_kind: String, // user | dept | group
    target_id: String,
    note: Option<String>,
}

fn validate_share_target(
    store: &Store,
    user: &UserRec,
    target_kind: &str,
    target_id: &str,
) -> Result<(), (StatusCode, Json<Value>)> {
    match target_kind {
        "user" => {
            if !store
                .list_users()
                .map_err(bad)?
                .iter()
                .any(|u| u.id == target_id)
            {
                return Err(bad("目标用户不存在"));
            }
        }
        "dept" => {
            if !store
                .list_departments()
                .map_err(bad)?
                .iter()
                .any(|d| d.id == target_id)
            {
                return Err(bad("目标部门不存在"));
            }
        }
        "group" => {
            let members = store.custom_group_members(target_id).map_err(bad)?;
            if !members.iter().any(|m| m == &user.id) {
                return Err(forbidden());
            }
        }
        _ => return Err(bad("target_kind 只能是 user、dept 或 group")),
    }
    Ok(())
}

async fn create_share(
    State(st): State<St>,
    headers: HeaderMap,
    Json(req): Json<ShareReq>,
) -> Api<Value> {
    let user = auth(&st, &headers)?;
    let store = st.store.lock().unwrap();
    // the sharer must actually hold the item
    let title = match req.item_kind.as_str() {
        "showroom" => {
            let item = store
                .get_showroom(&req.item_id)
                .map_err(bad)?
                .ok_or_else(|| not_found())?;
            if !store.showroom_item_access(&user, &item).map_err(bad)? {
                return Err(forbidden());
            }
            item.title.clone()
        }
        "assembly" => {
            let mine = store.list_assemblies(&user.id).map_err(bad)?;
            let a = mine
                .iter()
                .find(|a| a["id"] == req.item_id)
                .ok_or_else(|| not_found())?;
            a["name"].as_str().unwrap_or("组装").to_string()
        }
        _ => return Err(bad("item_kind 只能是 showroom 或 assembly")),
    };
    validate_share_target(&store, &user, &req.target_kind, &req.target_id)?;
    let id = store
        .create_share(
            &req.item_kind,
            &req.item_id,
            &title,
            &user.id,
            &req.target_kind,
            &req.target_id,
            req.note.as_deref().unwrap_or("").trim(),
        )
        .map_err(bad)?;
    Ok(Json(json!({ "share_id": id })))
}

/// Revalidate a share's payload at read / accept time: the share
/// record alone is not proof the content is still there. Showroom
/// items must still exist and pass the same access funnel the read
/// surfaces use (`showroom_item_access` — a rejected, deleted or
/// no-longer-reachable item fails it); assemblies must still exist
/// in their owner's list (the share itself is the access grant).
fn share_item_still_valid(store: &Store, user: &UserRec, share: &Value) -> SResult<bool> {
    let item_id = share["item_id"].as_str().unwrap_or("");
    match share["item_kind"].as_str() {
        Some("assembly") => {
            let from = share["from_user"].as_str().unwrap_or("");
            Ok(store
                .list_assemblies(from)?
                .iter()
                .any(|a| a["id"] == item_id))
        }
        _ => match store.get_showroom(item_id)? {
            Some(item) => store.showroom_item_access(user, &item),
            None => Ok(false),
        },
    }
}

/// Shares landing in my inbox: targeted at me directly, or at a
/// department I belong to (incl. sub-department reach). Group shares
/// live in the group space instead.
async fn list_inbox(State(st): State<St>, headers: HeaderMap) -> Api<Value> {
    let user = auth(&st, &headers)?;
    let store = st.store.lock().unwrap();
    let my_depts = store.user_dept_ids(&user.id).map_err(bad)?;
    let library = store.list_library(&user.id).map_err(bad)?;
    let mut out = Vec::new();
    for s in store.list_shares().map_err(bad)? {
        if s["from_user"] == user.id {
            continue;
        }
        let visible = match s["target_kind"].as_str() {
            Some("user") => s["target_id"] == user.id,
            Some("dept") => {
                let t = s["target_id"].as_str().unwrap_or("");
                let mut v = false;
                for d in &my_depts {
                    if store.dept_within(d, t).map_err(bad)? {
                        v = true;
                        break;
                    }
                }
                v
            }
            _ => false,
        };
        if visible && share_item_still_valid(&store, &user, &s).map_err(bad)? {
            let mut s = s;
            s["in_library"] = json!(library.iter().any(|e| e["item_id"] == s["item_id"]));
            out.push(s);
        }
    }
    Ok(Json(json!({ "shares": out })))
}

fn group_space_items(store: &Store, user: &UserRec) -> SResult<Vec<Value>> {
    let groups = store.list_custom_groups_for(&user.id)?;
    let shares = store.list_shares()?;
    let mut out = Vec::new();
    for g in &groups {
        let mut items = Vec::new();
        for s in &shares {
            if s["target_kind"] == "group"
                && s["target_id"] == g["id"]
                && share_item_still_valid(store, user, s)?
            {
                items.push(s.clone());
            }
        }
        out.push(json!({ "group": g, "items": items }));
    }
    Ok(out)
}

/// Group space: shares targeted at custom groups I belong to, grouped.
async fn group_space(State(st): State<St>, headers: HeaderMap) -> Api<Value> {
    let user = auth(&st, &headers)?;
    let store = st.store.lock().unwrap();
    let out = group_space_items(&store, &user).map_err(bad)?;
    Ok(Json(json!({ "spaces": out })))
}

#[derive(Deserialize)]
struct AcceptReq {
    folder: Option<String>,
}

async fn accept_share(
    State(st): State<St>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(req): Json<AcceptReq>,
) -> Api<Value> {
    let user = auth(&st, &headers)?;
    let store = st.store.lock().unwrap();
    let share = store
        .list_shares()
        .map_err(bad)?
        .into_iter()
        .find(|s| s["id"] == id)
        .ok_or_else(|| not_found())?;
    // must be a legitimate recipient (same rules as inbox / group space)
    let my_depts = store.user_dept_ids(&user.id).map_err(bad)?;
    let recipient = match share["target_kind"].as_str() {
        Some("user") => share["target_id"] == user.id,
        Some("dept") => {
            let t = share["target_id"].as_str().unwrap_or("");
            let mut v = false;
            for d in &my_depts {
                if store.dept_within(d, t).map_err(bad)? {
                    v = true;
                    break;
                }
            }
            v
        }
        Some("group") => store
            .custom_group_members(share["target_id"].as_str().unwrap_or(""))
            .map_err(bad)?
            .iter()
            .any(|m| m == &user.id),
        _ => false,
    };
    if !recipient {
        return Err(forbidden());
    }
    // The share record alone is not proof: revalidate the payload
    // the way save_to_library does — gone is 404, unreachable is 403.
    match share["item_kind"].as_str() {
        Some("assembly") => {
            if !share_item_still_valid(&store, &user, &share).map_err(bad)? {
                return Err(not_found());
            }
        }
        _ => {
            let item = store
                .get_showroom(share["item_id"].as_str().unwrap_or(""))
                .map_err(bad)?
                .ok_or_else(|| not_found())?;
            if !store.showroom_item_access(&user, &item).map_err(bad)? {
                return Err(forbidden());
            }
        }
    }
    let lib_id = store
        .add_library_entry(
            &user.id,
            share["item_kind"].as_str().unwrap_or("showroom"),
            share["item_id"].as_str().unwrap_or(""),
            share["title"].as_str().unwrap_or(""),
            req.folder.as_deref().unwrap_or(""),
        )
        .map_err(bad)?;
    Ok(Json(json!({ "library_id": lib_id })))
}

#[derive(Deserialize)]
struct SaveLibraryReq {
    item_kind: String, // showroom only, for now
    item_id: String,
    folder: Option<String>,
}

/// Save a showroom item into my library — the same library entry
/// the share-accept path creates, for items that reached me
/// through the plaza or a group space rather than an inbox share
/// (the deck detail's 保存到我的库 button). The access gate is
/// the showroom access funnel, identical to share creation and
/// item export: anything I may open, I may save. Idempotent —
/// an existing entry is returned unchanged.
async fn save_to_library(
    State(st): State<St>,
    headers: HeaderMap,
    Json(req): Json<SaveLibraryReq>,
) -> Api<Value> {
    let user = auth(&st, &headers)?;
    if req.item_kind != "showroom" {
        return Err(bad("item_kind 只能是 showroom"));
    }
    let store = st.store.lock().unwrap();
    let item = store
        .get_showroom(&req.item_id)
        .map_err(bad)?
        .ok_or_else(|| not_found())?;
    if !store.showroom_item_access(&user, &item).map_err(bad)? {
        return Err(forbidden());
    }
    if let Some(e) = store
        .list_library(&user.id)
        .map_err(bad)?
        .iter()
        .find(|e| e["item_kind"] == "showroom" && e["item_id"] == req.item_id)
    {
        return Ok(Json(json!({ "library_id": e["id"] })));
    }
    let lib_id = store
        .add_library_entry(
            &user.id,
            "showroom",
            &item.id,
            &item.title,
            req.folder.as_deref().unwrap_or(""),
        )
        .map_err(bad)?;
    Ok(Json(json!({ "library_id": lib_id })))
}

async fn list_library(State(st): State<St>, headers: HeaderMap) -> Api<Value> {
    let user = auth(&st, &headers)?;
    let store = st.store.lock().unwrap();
    let entries = store.list_library(&user.id).map_err(bad)?;
    Ok(Json(json!({ "entries": entries })))
}

async fn remove_library_entry(
    State(st): State<St>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Api<Value> {
    let user = auth(&st, &headers)?;
    let store = st.store.lock().unwrap();
    store.remove_library_entry(&id, &user.id).map_err(bad)?;
    Ok(Json(json!({ "ok": true })))
}

/// Export a showroom item's deck at item level: live item + (source
/// group visible OR item shared to me / in my library / mine). A
/// guest session exports a live org_public item through the same
/// funnel, always as the company edition (org-model G2).
async fn export_showroom(
    State(st): State<St>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(req): Json<ExportReq>,
) -> Api<Value> {
    let auth_res = auth(&st, &headers);
    let guest = has_guest_session(&st, &headers);
    let (entries, source, exporter, kind) = {
        let store = st.store.lock().unwrap();
        let item = store
            .get_showroom(&id)
            .map_err(bad)?
            .ok_or_else(|| not_found())?;
        let exporter: Option<UserRec> = match auth_res {
            Ok(user) => {
                if !store.showroom_item_access(&user, &item).map_err(bad)? {
                    return Err(not_found());
                }
                Some(user)
            }
            Err(e) => {
                if !guest || !store.guest_item_visible(&item) {
                    return Err(e);
                }
                None
            }
        };
        let idx_id = item
            .source_index_id
            .clone()
            .ok_or_else(|| bad("该条目没有来源索引"))?;
        let commit = match &item.commit_id {
            Some(cid) => store.get_commit(cid).map_err(bad)?,
            None => {
                let head = store
                    .branch_head(&idx_id, "main")
                    .map_err(bad)?
                    .ok_or_else(|| bad("来源索引尚无提交"))?;
                store.get_commit(&head).map_err(bad)?
            }
        };
        let entries = commit.entries;
        let source = SourceRef {
            index_id: idx_id,
            branch: "main".to_string(),
            commit_id: commit.id.clone(),
            tag: None,
        };
        // Guests always receive the company edition; named
        // exporters keep their choice of kind.
        let kind = if exporter.is_some() {
            req.kind.as_deref().unwrap_or("personal")
        } else {
            "company"
        };
        (entries, source, exporter, kind)
    };
    let (deck, manifest, page_count) = build_export(
        &st,
        exporter.as_ref(),
        &entries,
        Some(source),
        kind,
        req.include_notes.unwrap_or(false),
    )?;
    Ok(Json(json!({
        "page_count": page_count,
        "manifest": manifest,
        "data_base64": b64e(&deck),
    })))
}

/// Convert an assembled deck PPTX to PDF with the DeckCraft CLI
/// (file.export via `run --export`): the same subprocess pattern as
/// the thumbnail renderer. `form` picks the file.export layout —
/// "slides" (one slide per page), "handout" (6 slides per sheet),
/// "notes" (slide + speaker notes per page); `page` restricts the
/// export to one 0-based deck page. textLayer is pinned on: the PDF
/// carries a real (invisible) text layer over the rasterised slide
/// artwork, so text stays selectable/searchable — the artwork itself
/// is raster at file.export's default 200 dpi, not vector paths
/// (deckcraft-pdf's own model; tools/verify_pdf_export.py is the
/// CI-side evidence gate for exactly this distinction).
fn deck_to_pdf(
    root: &std::path::Path,
    tag: &str,
    deck: &[u8],
    form: &str,
    page: Option<usize>,
) -> Result<Vec<u8>, String> {
    let bin = deckcraft_cli_bin();
    let work = root
        .join("pdf")
        .join(format!(".work-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&work);
    std::fs::create_dir_all(&work).map_err(|e| e.to_string())?;
    let cleanup = |work: &std::path::Path| {
        let _ = std::fs::remove_dir_all(work);
    };
    let pptx = work.join("deck.pptx");
    std::fs::write(&pptx, deck).map_err(|e| e.to_string())?;
    let out_pdf = work.join("out.pdf");
    let mut params = json!({ "textLayer": true });
    match form {
        "handout" => {
            params["layout"] = json!("handouts");
            params["perPage"] = json!(6);
        }
        "notes" => {
            params["layout"] = json!("notes");
        }
        _ => {
            params["layout"] = json!("slides");
        }
    }
    if let Some(p) = page {
        params["slides"] = json!([p]);
    }
    let out = std::process::Command::new(&bin)
        .arg("run")
        .arg("--in")
        .arg(&pptx)
        .arg("--export")
        .arg(&out_pdf)
        .arg("--export-params")
        .arg(params.to_string())
        .output()
        .map_err(|e| format!("启动 deckcraft-cli 失败（{bin}）: {e}"))?;
    if !out.status.success() || !out_pdf.exists() {
        let stderr = String::from_utf8_lossy(&out.stderr).to_string();
        cleanup(&work);
        return Err(format!("deckcraft-cli 导出 PDF 失败: {stderr}"));
    }
    let bytes = std::fs::read(&out_pdf).map_err(|e| e.to_string())?;
    cleanup(&work);
    if bytes.is_empty() {
        return Err("DeckCraft 导出未产出 PDF".into());
    }
    Ok(bytes)
}

#[derive(Deserialize)]
struct PdfExportReq {
    kind: Option<String>,
    form: Option<String>,
    page: Option<usize>,
    include_notes: Option<bool>,
}

/// Export a showroom item's deck as PDF (Q21): the same access funnel
/// as export_showroom (named users by item access; a guest session
/// only for a live org_public item, always the company edition), then
/// the assembled deck goes through DeckCraft file.export. `form`:
/// "slides" (default) | "handout" | "notes"; `page`: one 0-based
/// deck-global page instead of the whole deck. Coexists with the
/// PPTX export — build_export is shared, neither path touches the
/// other's output.
async fn export_showroom_pdf(
    State(st): State<St>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(req): Json<PdfExportReq>,
) -> Api<Value> {
    let form = match req.form.as_deref().unwrap_or("slides") {
        f @ ("slides" | "handout" | "notes") => f.to_string(),
        other => return Err(bad(format!("未知 PDF 版式：{other}"))),
    };
    let auth_res = auth(&st, &headers);
    let guest = has_guest_session(&st, &headers);
    let (entries, source, exporter, kind) = {
        let store = st.store.lock().unwrap();
        let item = store
            .get_showroom(&id)
            .map_err(bad)?
            .ok_or_else(|| not_found())?;
        let exporter: Option<UserRec> = match auth_res {
            Ok(user) => {
                if !store.showroom_item_access(&user, &item).map_err(bad)? {
                    return Err(not_found());
                }
                Some(user)
            }
            Err(e) => {
                if !guest || !store.guest_item_visible(&item) {
                    return Err(e);
                }
                None
            }
        };
        let idx_id = item
            .source_index_id
            .clone()
            .ok_or_else(|| bad("该条目没有来源索引"))?;
        let commit = match &item.commit_id {
            Some(cid) => store.get_commit(cid).map_err(bad)?,
            None => {
                let head = store
                    .branch_head(&idx_id, "main")
                    .map_err(bad)?
                    .ok_or_else(|| bad("来源索引尚无提交"))?;
                store.get_commit(&head).map_err(bad)?
            }
        };
        let entries = commit.entries;
        let source = SourceRef {
            index_id: idx_id,
            branch: "main".to_string(),
            commit_id: commit.id.clone(),
            tag: None,
        };
        // Guests always receive the company edition; named
        // exporters keep their choice of kind.
        let kind = if exporter.is_some() {
            req.kind.as_deref().unwrap_or("personal")
        } else {
            "company"
        };
        (entries, source, exporter, kind)
    };
    // The notes form is pointless without the speaker scripts written
    // back into the deck's notes pages, so it implies include_notes.
    let include_notes = req.include_notes.unwrap_or(form == "notes");
    let (deck, _manifest, page_count) = build_export(
        &st,
        exporter.as_ref(),
        &entries,
        Some(source),
        kind,
        include_notes,
    )?;
    if let Some(p) = req.page {
        if (p as i64) >= page_count {
            return Err(bad("页码超出成品页数"));
        }
    }
    let pdf = deck_to_pdf(&st.data_dir, &id, &deck, &form, req.page).map_err(bad)?;
    Ok(Json(json!({
        "page_count": if req.page.is_some() { 1 } else { page_count },
        "form": form,
        "data_base64": b64e(&pdf),
    })))
}

#[derive(Deserialize)]
struct AssemblyReq {
    name: String,
    entries: Vec<IndexEntry>,
}

async fn list_assemblies(State(st): State<St>, headers: HeaderMap) -> Api<Value> {
    let user = auth(&st, &headers)?;
    let store = st.store.lock().unwrap();
    let list = store.list_assemblies(&user.id).map_err(bad)?;
    Ok(Json(json!({ "assemblies": list })))
}

async fn create_assembly(
    State(st): State<St>,
    headers: HeaderMap,
    Json(req): Json<AssemblyReq>,
) -> Api<Value> {
    let user = auth(&st, &headers)?;
    let store = st.store.lock().unwrap();
    let id = store
        .save_assembly(&user.id, &req.name, &req.entries)
        .map_err(bad)?;
    Ok(Json(json!({ "assembly_id": id })))
}

#[derive(Deserialize)]
struct AssemblyExportReq {
    kind: Option<String>,
    include_notes: Option<bool>,
}

async fn export_assembly(
    State(st): State<St>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(req): Json<AssemblyExportReq>,
) -> Api<Value> {
    let user = auth(&st, &headers)?;
    let entries = {
        let store = st.store.lock().unwrap();
        store.assembly_entries(&id).map_err(|_| not_found())?
    };
    let (deck, manifest, page_count) = build_export(
        &st,
        Some(&user),
        &entries,
        None,
        req.kind.as_deref().unwrap_or("personal"),
        req.include_notes.unwrap_or(false),
    )?;
    Ok(Json(json!({
        "page_count": page_count,
        "manifest": manifest,
        "data_base64": b64e(&deck),
    })))
}

/// Stamp every request with its TCP peer IP (header
/// `x-qimu-peer-ip`) for session records and the standalone
/// loopback mapping. The value is written here, from the socket —
/// a client-sent header of the same name is replaced, never read.
async fn peer_ip_layer(
    axum::extract::ConnectInfo(addr): axum::extract::ConnectInfo<std::net::SocketAddr>,
    mut req: axum::extract::Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    if let Ok(v) = addr.ip().to_string().parse::<axum::http::HeaderValue>() {
        req.headers_mut().insert("x-qimu-peer-ip", v);
    }
    next.run(req).await
}

// ---------------- LAN discovery (§4: clients auto-find servers) -------

/// UDP port answering discovery broadcasts; the HTTP port stays 8730.
const DISCOVERY_PORT: u16 = 8731;
const DISCOVERY_MAGIC: &[u8] = b"QIMU_DISCOVER_V1";

fn machine_name() -> String {
    std::env::var("QIMU_SERVER_NAME")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .map(|s| s.trim().to_string())
        .or_else(|| {
            std::env::var("HOSTNAME")
                .or_else(|_| std::env::var("COMPUTERNAME"))
                .ok()
                .filter(|s| !s.trim().is_empty())
        })
        .unwrap_or_else(|| "启幕服务器".to_string())
}

/// The discovery protocol core: a `QIMU_DISCOVER_V1` datagram earns a
/// JSON hello naming this server and its HTTP port; anything else earns
/// silence. Pure, so the protocol is unit-testable without a network.
fn discovery_reply(payload: &[u8], http_port: u16) -> Option<String> {
    if payload != DISCOVERY_MAGIC {
        return None;
    }
    Some(
        json!({
            "app": "qimu",
            "name": machine_name(),
            "port": http_port,
            "version": env!("CARGO_PKG_VERSION"),
        })
        .to_string(),
    )
}

/// Answer discovery broadcasts with a unicast JSON hello so clients on
/// the LAN can list this server without knowing its address.
/// Best-effort: any failure is logged and never affects the HTTP service.
async fn discovery_task(http_port: u16) {
    let sock = match tokio::net::UdpSocket::bind(("0.0.0.0", DISCOVERY_PORT)).await {
        Ok(s) => s,
        Err(e) => {
            eprintln!("discovery: cannot bind UDP {DISCOVERY_PORT}: {e}");
            return;
        }
    };
    println!("qimu-server discovery on udp://0.0.0.0:{DISCOVERY_PORT}");
    let mut buf = [0u8; 512];
    loop {
        match sock.recv_from(&mut buf).await {
            Ok((n, src)) => {
                if let Some(hello) = discovery_reply(&buf[..n], http_port) {
                    let _ = sock.send_to(hello.as_bytes(), src).await;
                }
            }
            Err(e) => {
                eprintln!("discovery: recv error: {e}");
                tokio::time::sleep(std::time::Duration::from_millis(200)).await;
            }
        }
    }
}

#[cfg(test)]
mod discovery_tests {
    use super::*;

    #[test]
    fn magic_gets_a_hello() {
        let hello = discovery_reply(DISCOVERY_MAGIC, 8730).expect("reply");
        let v: Value = serde_json::from_str(&hello).unwrap();
        assert_eq!(v["app"], "qimu");
        assert_eq!(v["port"], 8730);
        assert!(v["name"].as_str().is_some_and(|n| !n.is_empty()));
        assert_eq!(v["version"], env!("CARGO_PKG_VERSION"));
    }

    #[test]
    fn anything_else_gets_silence() {
        assert!(discovery_reply(b"QIMU_DISCOVER_V2", 8730).is_none());
        assert!(discovery_reply(b"", 8730).is_none());
        assert!(discovery_reply(b" QIMU_DISCOVER_V1", 8730).is_none());
    }
}

#[cfg(test)]
mod collaboration_tests {
    use super::*;

    fn user(id: &str, name: &str, role: &str, disabled: bool) -> UserRec {
        UserRec {
            id: id.into(),
            username: name.into(),
            display_name: name.into(),
            dept_id: Some("dept".into()),
            role: role.into(),
            disabled,
        }
    }

    #[test]
    fn members_cannot_enumerate_org_contacts() {
        let departments = vec![DeptRec {
            id: "dept".into(),
            name: "技术部".into(),
            parent_id: None,
        }];
        let users = vec![user("u1", "alice", "member", false)];
        assert!(contact_rows("member", &departments, users, "").is_empty());
    }

    #[test]
    fn admins_get_filtered_enabled_contacts() {
        let departments = vec![DeptRec {
            id: "dept".into(),
            name: "技术部".into(),
            parent_id: None,
        }];
        let users = vec![
            user("u1", "alice", "member", false),
            user("u2", "bob", "member", true),
        ];
        let rows = contact_rows("dept_admin", &departments, users, "ali");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0]["id"], "u1");
        assert_eq!(rows[0]["dept_name"], "技术部");
    }

    struct TestDir(std::path::PathBuf);

    impl TestDir {
        fn new() -> Self {
            let nonce = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            Self(
                std::env::temp_dir()
                    .join(format!("qimu-server-test-{}-{nonce}", std::process::id())),
            )
        }
    }

    impl Drop for TestDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn group_share_requires_membership_and_reaches_only_members() {
        let dir = TestDir::new();
        let store = Store::open(&dir.0).unwrap();
        store.ensure_seed("test-password").unwrap();
        let alice = store
            .create_user("alice", "Alice", "test-password", None, "member")
            .unwrap();
        let bob = store
            .create_user("bob", "Bob", "test-password", None, "member")
            .unwrap();
        let outsider = store
            .create_user("outsider", "Outsider", "test-password", None, "member")
            .unwrap();
        let group_id = store.create_custom_group(&alice.id, "项目组").unwrap();
        store.add_custom_group_member(&group_id, &bob.id).unwrap();

        assert!(validate_share_target(&store, &alice, "group", &group_id).is_ok());
        assert!(validate_share_target(&store, &bob, "group", &group_id).is_ok());
        let denied = validate_share_target(&store, &outsider, "group", &group_id).unwrap_err();
        assert_eq!(denied.0, StatusCode::FORBIDDEN);

        let item = store
            .insert_showroom("演示稿", "", None, None, "Alice", "live")
            .unwrap();
        store
            .create_share(
                "showroom",
                &item.id,
                &item.title,
                &alice.id,
                "group",
                &group_id,
                "请查收",
            )
            .unwrap();

        let bob_spaces = group_space_items(&store, &bob).unwrap();
        assert_eq!(bob_spaces.len(), 1);
        assert_eq!(bob_spaces[0]["group"]["id"], group_id);
        assert_eq!(bob_spaces[0]["items"][0]["item_id"], item.id);
        assert_eq!(bob_spaces[0]["items"][0]["note"], "请查收");
        assert!(group_space_items(&store, &outsider)
            .unwrap()
            .is_empty());
    }

    #[test]
    fn showroom_rejects_tags_from_non_main_branches() {
        let dir = TestDir::new();
        let store = Store::open(&dir.0).unwrap();
        let group = store.create_group("技术").unwrap();
        let index = store
            .create_index("索引", None, &group.id, "admin", "private", Vec::new())
            .unwrap();
        let main_tag = store.create_tag(&index.id, "main", "1.0.0").unwrap();
        store.create_branch(&index.id, "draft", "main").unwrap();
        let draft_tag = store.create_tag(&index.id, "draft", "1.0.0").unwrap();

        assert_eq!(
            showroom_source_commit(&store, &index.id, Some(&main_tag.id)).unwrap(),
            main_tag.commit_id
        );
        assert_eq!(
            showroom_source_commit(&store, &index.id, None).unwrap(),
            main_tag.commit_id
        );
        assert_eq!(
            showroom_source_commit(&store, &index.id, Some(&draft_tag.id)),
            Err("只能推送主分支的发布版本".into())
        );
    }
}

// ---------------- main ----------------

fn dir_writable(p: &std::path::Path) -> bool {
    if std::fs::create_dir_all(p).is_err() {
        return false;
    }
    let probe = p.join(".qimu-write-probe");
    match std::fs::write(&probe, b"ok") {
        Ok(()) => {
            let _ = std::fs::remove_file(&probe);
            true
        }
        Err(_) => false,
    }
}

/// Data directory resolution: $QIMU_DATA_DIR wins; otherwise a `qimu-data`
/// folder beside the executable when writable (installed layouts, e.g.
/// D:\Program Files\启幕); otherwise a per-user/system fallback so a
/// read-only install dir (C:\Program Files) still works.
fn resolve_data_dir() -> std::path::PathBuf {
    if let Ok(d) = std::env::var("QIMU_DATA_DIR") {
        if !d.trim().is_empty() {
            return std::path::PathBuf::from(d);
        }
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let cand = dir.join("qimu-data");
            if dir_writable(&cand) {
                return cand;
            }
        }
    }
    #[cfg(windows)]
    {
        if let Ok(pd) = std::env::var("ProgramData") {
            let cand = std::path::PathBuf::from(pd).join("启幕").join("data");
            if dir_writable(&cand) {
                return cand;
            }
        }
    }
    let home = std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .unwrap_or_else(|_| ".".to_string());
    std::path::PathBuf::from(home)
        .join(".qimu")
        .join("server-data")
}

/// Startup failures of a double-clicked console app vanish together
/// with the console window ("闪一下就没"), leaving the user nothing to
/// report. Print the reason and wait for Enter so it can be read, then
/// exit non-zero.
fn fatal_startup(msg: &str) -> ! {
    eprintln!("启幕服务器启动失败：{msg}");
    eprintln!("按回车键关闭此窗口。");
    let mut line = String::new();
    let _ = std::io::stdin().read_line(&mut line);
    std::process::exit(1)
}

#[tokio::main]
async fn main() {
    let data_dir = resolve_data_dir();
    let store = match Store::open(&data_dir) {
        Ok(s) => s,
        Err(e) => fatal_startup(&format!(
            "无法打开数据目录 {}：{e}（数据库可能被占用，或目录不可写）",
            data_dir.display()
        )),
    };
    if store.user_count().map(|c| c == 0).unwrap_or(false) {
        let pw = std::env::var("QIMU_ADMIN_PASSWORD").unwrap_or_else(|_| "admin12345".to_string());
        if let Err(e) = store.ensure_seed(&pw) {
            fatal_startup(&format!("初始化管理员账号失败：{e}"));
        }
        println!("首次启动：已创建超级管理员 admin / {pw}（请登录后修改；可用环境变量 QIMU_ADMIN_PASSWORD 指定）");
    }
    let standalone = match std::env::var("QIMU_STANDALONE").as_deref() {
        Ok("1") | Ok("true") => true,
        Ok("0") | Ok("false") => false,
        _ => data_dir.join("standalone.on").exists(),
    };
    if standalone {
        println!("单机模式：回环来源的免登录请求按本机所有者处理（设 QIMU_STANDALONE=0 可禁用）");
    }
    let state: St = Arc::new(AppState {
        store: Mutex::new(store),
        translate_jobs: Mutex::new(HashMap::new()),
        data_dir: data_dir.clone(),
        standalone,
    });
    let app = Router::new()
        .route(
            "/health",
            get(|| async {
                Json(json!({
                    "status": "ok",
                    "service": "qimu-server",
                    "name": machine_name(),
                    "version": env!("CARGO_PKG_VERSION"),
                }))
            }),
        )
        .route("/update/releases.json", get(update_releases))
        .route("/update/:tag/:name", get(update_asset))
        .route("/v1/auth/login", post(login))
        .route("/v1/auth/guest", post(guest_auth))
        .route("/v1/auth/logout", post(logout))
        .route("/v1/me", get(me))
        .route("/v1/me/profile", axum::routing::put(put_profile))
        .route("/v1/org", get(org))
        .route("/v1/org/contacts", get(org_contacts))
        .route("/v1/org/departments", post(create_dept))
        .route("/v1/org/users", post(create_user))
        .route("/v1/org/users/:id/disable", post(disable_user))
        .route("/v1/groups", get(groups).post(create_group))
        .route("/v1/grants", get(grants).post(add_grant))
        .route(
            "/v1/company-profile",
            get(company_profile).put(put_company_profile),
        )
        .route("/v1/settings/languages", get(languages).put(put_languages))
        .route("/v1/settings/glossary", get(get_glossary).put(put_glossary))
        .route(
            "/v1/settings/cache-policy",
            get(get_cache_policy).put(put_cache_policy),
        )
        .route("/v1/settings/ai", get(get_ai_settings).put(put_ai_settings))
        .route("/v1/units/:id/translate", post(translate_unit))
        .route("/v1/units/:id/translate-jobs", post(start_translate_job))
        .route("/v1/translate-jobs/:job_id", get(get_translate_job))
        .route("/v1/ai/status", get(get_ai_status))
        .route("/v1/folders", get(folders).post(create_folder))
        .route("/v1/folders/:id/rename", post(rename_folder))
        .route("/v1/folders/:id", delete(delete_folder))
        .route("/v1/folders/:id/main-index", post(set_folder_main_index))
        .route("/v1/units", get(list_units).post(create_unit))
        .route("/v1/units/:id", get(get_unit).patch(patch_unit))
        .route(
            "/v1/units/:id/bump-revision",
            post(
                |State(st): State<St>, headers: HeaderMap, Path(id): Path<String>| async move {
                    let user = auth(&st, &headers)?;
                    let store = st.store.lock().unwrap();
                    let unit = store.get_unit(&id).map_err(|_| not_found())?;
                    drop(store);
                    need_cap(&st, &user, &unit.group_id, "publish")?;
                    let store = st.store.lock().unwrap();
                    let r = store.bump_unit_revision(&id).map_err(bad)?;
                    Ok::<_, (StatusCode, Json<Value>)>(Json(json!({ "current_revision": r })))
                },
            ),
        )
        .route("/v1/import", post(import_deck))
        .route("/v1/import/propose", post(import_propose))
        .route("/v1/units/:id/publish", post(publish))
        .route("/v1/versions/:id/recall", post(recall_version))
        .route("/v1/versions/:id/delete", post(delete_version))
        .route("/v1/versions/:id/fragment", get(version_fragment))
        .route("/v1/versions/:id/thumb/:page", get(version_thumb))
        .route(
            "/v1/versions/:id/tags",
            get(list_version_tags).post(add_version_tag),
        )
        .route("/v1/versions/:id", get(get_version))
        .route(
            "/v1/versions/:id/scripts",
            axum::routing::put(put_version_scripts),
        )
        .route(
            "/v1/units/:id/page-bindings",
            get(list_page_bindings).post(add_page_binding),
        )
        .route(
            "/v1/units/:id/page-bindings/:bid/delete",
            post(delete_page_binding),
        )
        .route("/v1/search", get(search))
        .route("/v1/indexes", get(list_indexes).post(create_index))
        .route("/v1/index-graphs", get(index_graphs))
        .route("/v1/indexes/:id/folder", post(move_index_to_folder))
        .route("/v1/indexes/:id/english", post(create_english_index))
        .route("/v1/indexes/:id", get(get_index))
        .route("/v1/indexes/:id/push-preview", get(index_push_preview))
        .route(
            "/v1/indexes/:id/commits",
            get(list_commits).post(post_commit),
        )
        .route("/v1/indexes/:id/branches", post(create_branch))
        .route("/v1/indexes/:id/merge/preview", post(merge_preview))
        .route("/v1/indexes/:id/merge", post(merge_execute))
        .route("/v1/indexes/:id/tags", post(create_tag))
        .route("/v1/indexes/:id/lifecycle", post(set_lifecycle))
        .route("/v1/indexes/:id/export", post(export_index))
        .route("/v1/decks/:deck_id/manifest", get(deck_manifest_by_id))
        .route("/v1/exports", get(list_exports))
        .route("/v1/check-updates", post(check_updates))
        .route("/v1/deck/update-package", post(update_package))
        .route(
            "/v1/attachments",
            get(list_attachments).post(add_attachment),
        )
        .route("/v1/attachments/:id/download", get(download_attachment))
        .route("/v1/showroom", get(list_showroom).post(submit_showroom))
        .route("/v1/showroom/:id/review", post(review_showroom))
        .route("/v1/submissions", get(list_submissions).post(create_submission))
        .route("/v1/submissions/:id/review", post(review_submission))
        .route("/v1/submissions/:id", delete(delete_submission))
        .route("/v1/trash", get(list_trash).post(add_trash))
        .route("/v1/trash/purge-all", post(purge_all_trash))
        .route("/v1/trash/:id/restore", post(restore_trash))
        .route("/v1/trash/:id/purge", post(purge_trash))
        .route("/v1/versions/:id/page-text", get(version_page_text))
        .route("/v1/versions/:id/edit-pages", post(edit_version_pages))
        .route("/v1/showroom/:id/export", post(export_showroom))
        .route("/v1/showroom/:id/export-pdf", post(export_showroom_pdf))
        .route(
            "/v1/custom-groups",
            get(list_custom_groups).post(create_custom_group),
        )
        .route(
            "/v1/custom-groups/:id/members",
            post(add_custom_group_member),
        )
        .route(
            "/v1/custom-groups/:id/members/remove",
            post(remove_custom_group_member),
        )
        .route("/v1/shares", post(create_share))
        .route("/v1/shares/inbox", get(list_inbox))
        .route("/v1/shares/:id/accept", post(accept_share))
        .route("/v1/group-space", get(group_space))
        .route("/v1/library", get(list_library).post(save_to_library))
        .route("/v1/library/:id/delete", post(remove_library_entry))
        .route("/v1/assemblies", get(list_assemblies).post(create_assembly))
        .route("/v1/assemblies/:id/export", post(export_assembly))
        .layer(DefaultBodyLimit::max(512 * 1024 * 1024))
        .layer(axum::middleware::from_fn(peer_ip_layer))
        .with_state(state);
    let port: u16 = std::env::var("QIMU_PORT")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(8730);
    let addr = format!("0.0.0.0:{port}");
    let listener = match tokio::net::TcpListener::bind(&addr).await {
        Ok(l) => l,
        Err(e) => {
            // Most common cause: another qimu-server is already
            // running (the installer registers a logon autostart
            // entry), so a double-clicked second instance has nothing
            // to do. Say so instead of dying on an expect() panic.
            let probe = std::net::SocketAddr::from(([127, 0, 0, 1], port));
            if std::net::TcpStream::connect(probe).is_ok() {
                fatal_startup(&format!(
                    "端口 {port} 上已有一个启幕服务器在运行，无需重复启动。\
                     直接打开客户端连接它即可；若要重启，先关掉已运行的实例。"
                ));
            }
            fatal_startup(&format!("无法监听 {addr}：{e}"))
        }
    };
    println!(
        "qimu-server listening on http://{addr} (data dir: {})",
        data_dir.display()
    );
    tokio::spawn(discovery_task(port));
    if let Err(e) = axum::serve(
        listener,
        app.into_make_service_with_connect_info::<std::net::SocketAddr>(),
    )
    .await
    {
        fatal_startup(&format!("服务器运行中断：{e}"));
    }
}

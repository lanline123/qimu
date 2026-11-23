//! qimu-server — the LAN host service (spec §4). One portable data
//! directory (SQLite + content-hash file store). All authorization is
//! enforced here, server-side: content in groups a user cannot see does
//! not exist in any response (spec §7).

use axum::{
    extract::{DefaultBodyLimit, Path, Query, State},
    http::{HeaderMap, StatusCode},
    routing::{delete, get, patch, post},
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
    data_dir: std::path::PathBuf,
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

fn auth(st: &St, headers: &HeaderMap) -> Result<UserRec, (StatusCode, Json<Value>)> {
    let token = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .ok_or_else(|| err(StatusCode::UNAUTHORIZED, "未登录"))?;
    let store = st.store.lock().unwrap();
    store
        .user_by_token(token)
        .map_err(|e| err(StatusCode::UNAUTHORIZED, e))
}

/// Optional auth for the anonymous guest read surface (user ruling
/// 2026-11-23: the client opens without login). Returns the user
/// when a valid session token is present, None when the header is
/// absent — and rejects a *present but invalid* token, so a stale
/// session still surfaces as 401 instead of silently downgrading.
fn auth_opt(st: &St, headers: &HeaderMap) -> Result<Option<UserRec>, (StatusCode, Json<Value>)> {
    let token = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .map(|v| v.trim().to_string());
    match token {
        None => Ok(None),
        Some(t) => {
            let store = st.store.lock().unwrap();
            store
                .user_by_token(&t)
                .map(Some)
                .map_err(|e| err(StatusCode::UNAUTHORIZED, e))
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

fn visible_set(st: &St, user: &UserRec) -> Result<Option<std::collections::HashSet<String>>, (StatusCode, Json<Value>)> {
    let store = st.store.lock().unwrap();
    store.visible_groups(user).map_err(bad)
}

// ---------------- auth & profile ----------------

#[derive(Deserialize)]
struct LoginReq {
    username: String,
    password: String,
}

async fn login(State(st): State<St>, Json(req): Json<LoginReq>) -> Api<Value> {
    let store = st.store.lock().unwrap();
    let (token, user) = store.login(&req.username, &req.password).map_err(bad)?;
    Ok(Json(json!({ "token": token, "user": user })))
}

async fn logout(State(st): State<St>, headers: HeaderMap) -> Api<Value> {
    let _ = auth(&st, &headers)?;
    let token = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .unwrap_or("");
    let store = st.store.lock().unwrap();
    store.logout(token).map_err(bad)?;
    Ok(Json(json!({ "ok": true })))
}

async fn me(State(st): State<St>, headers: HeaderMap) -> Api<Value> {
    let user = auth(&st, &headers)?;
    let store = st.store.lock().unwrap();
    let profile: HashMap<String, String> = store.profile(&user.id).map_err(bad)?.into_iter().collect();
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
        .add_grant(&req.group_id, &req.subject_type, &req.subject_id, &req.capability)
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
    Ok(Json(json!({ "languages": langs })))
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
        .set_setting("glossary", &serde_json::to_string(&req.pairs).unwrap_or_default())
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
    store.set_setting("ai_provider", &req.provider).map_err(bad)?;
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
        let model_id = ModelId::new(self.model.clone()).map_err(|e| format!("模型名不合法：{e}"))?;
        let mut info = ModelInfo::new(model_id.clone(), self.model.clone());
        info.capabilities = vec![spec];
        let config = craft_ai_openai::OpenAiConfig::new(format!(
            "{}/v1",
            self.endpoint.trim_end_matches('/')
        ))
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
        let content = content.trim().trim_start_matches("```json").trim_start_matches("```").trim_end_matches("```").trim();
        let out: Vec<String> = serde_json::from_str(content)
            .map_err(|e| format!("翻译服务未返回 JSON 数组：{e}"))?;
        Ok(out)
    }
}

#[derive(Deserialize)]
struct TranslateReq {
    version_id: String,
    target_lang: String,
}

/// Translate one version into a target language: creates (or reuses) the
/// target rendition and publishes a DRAFT 1.0.0 whose provenance is
/// recorded in the note and in `ai` page bindings (§3.13/§3.15).
async fn translate_unit(
    State(st): State<St>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(req): Json<TranslateReq>,
) -> Api<Value> {
    let user = auth(&st, &headers)?;
    let unit = {
        let store = st.store.lock().unwrap();
        store.get_unit(&id).map_err(|_| not_found())?
    };
    need_cap(&st, &user, &unit.group_id, "publish")?;
    let (src_version, src_rendition, frag_bytes, glossary, provider_cfg) = {
        let store = st.store.lock().unwrap();
        let v = store.get_version(&req.version_id).map_err(|_| not_found())?;
        let r = store.get_rendition(&v.rendition_id).map_err(bad)?;
        if r.unit_id != id {
            return Err(bad("版本不属于该内容块"));
        }
        let bytes = store.file_get(&v.fragment_hash).map_err(bad)?;
        let get = |k: &str| store.get_setting(k).ok().flatten().unwrap_or_default();
        (
            v,
            r,
            bytes,
            load_glossary(&store),
            (get("ai_provider"), get("ai_endpoint"), get("ai_model"), get("ai_api_key")),
        )
    };
    let (provider_name, endpoint, model, api_key) = provider_cfg;
    let translator: Box<dyn Translator> = match provider_name.as_str() {
        "openai-compat" => {
            if endpoint.is_empty() || api_key.is_empty() || model.is_empty() {
                return Err(bad("AI 翻译未配置完整（endpoint / model / api_key），请先在设置中配置，或使用 stub 提供方"));
            }
            Box::new(OpenAiCompat { endpoint, api_key, model })
        }
        _ => Box::new(qimu_core::translate::StubTranslator),
    };
    let pkg = Package::load(&frag_bytes).map_err(bad)?;
    let (mut new_pkg, run_count) = qimu_core::translate::translate_package(
        &pkg,
        translator.as_ref(),
        &req.target_lang,
        &glossary,
    )
    .map_err(bad)?;
    // translate the speaker scripts too, and write them into the notes
    let src_scripts: Vec<String> = src_version.scripts.clone();
    let non_empty: Vec<String> = src_scripts.iter().filter(|s| !s.trim().is_empty()).cloned().collect();
    let mut translated_scripts = src_scripts.clone();
    if !non_empty.is_empty() {
        let out = translator
            .translate(&non_empty, &req.target_lang, &glossary)
            .map_err(bad)?;
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
        .get_or_create_rendition(&id, &src_rendition.form, &req.target_lang)
        .map_err(bad)?;
    let version = store
        .insert_version(
            &rend.id,
            SemVer::new(1, 0, 0),
            src_version.basis_revision,
            &user.display_name,
            &format!(
                "AI 翻译草稿（待校对）· 源 {} v{} · {} 个文本run",
                src_rendition.lang, src_version.semver, run_count
            ),
            &new_bytes,
            "draft",
        )
        .map_err(bad)?;
    for i in 0..src_version.page_count {
        let _ = store.add_page_binding(
            &id,
            &src_version.id,
            i,
            &version.id,
            i,
            "ai",
            &user.display_name,
        );
    }
    Ok(Json(json!({
        "version": version,
        "rendition_id": rend.id,
        "provider": if provider_name.is_empty() { "stub".to_string() } else { provider_name },
        "translated_runs": run_count,
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
    Ok(Json(json!({ "folders": store.list_folders().map_err(bad)? })))
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
    Ok(Json(json!(
        store.create_folder(&req.name, req.parent_id.as_deref()).map_err(bad)?
    )))
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

async fn get_unit(
    State(st): State<St>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Api<Value> {
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
    let pkg = Package::load(&bytes).map_err(bad)?;
    let report = pptx::split_reassemble_verify(&pkg).map_err(bad)?;
    if !report.ok {
        return Err(bad(format!(
            "拆分校验未通过，整批导入已回滚: {}",
            report.structural_errors.join("; ")
        )));
    }
    let singles = pptx::split(&pkg).map_err(bad)?;
    let stem = req
        .filename
        .rsplit('/')
        .next()
        .unwrap_or("未命名")
        .trim_end_matches(".pptx")
        .trim_end_matches(".PPTX")
        .to_string();
    let blocks = req.blocks.unwrap_or_else(|| {
        vec![ImportBlock {
            name: stem.clone(),
            pages: (0..singles.len()).collect(),
            form: None,
            lang: None,
        }]
    });
    let store = st.store.lock().unwrap();
    let mut created = Vec::new();
    for block in &blocks {
        if block.pages.is_empty() {
            continue;
        }
        let frag_pkgs: Vec<Package> = block
            .pages
            .iter()
            .filter_map(|i| singles.get(*i).cloned())
            .collect();
        if frag_pkgs.len() != block.pages.len() {
            return Err(bad("块包含越界页码，整批导入已回滚"));
        }
        let is_full_deck = block.pages.len() == singles.len()
            && block.pages.iter().copied().collect::<Vec<_>>()
                == (0..singles.len()).collect::<Vec<_>>();
        let frag = if is_full_deck {
            pptx::reassemble(&frag_pkgs).map_err(bad)?
        } else {
            qimu_core::assemble::assemble(&frag_pkgs).map_err(bad)?
        };
        let frag_bytes = frag.to_bytes().map_err(bad)?;
        let unit = store
            .create_unit(
                &block.name,
                req.folder_id.as_deref(),
                &req.group_id,
                &user.display_name,
                block.lang.as_deref().unwrap_or("zh"),
                "",
            )
            .map_err(bad)?;
        let rend = store
            .get_or_create_rendition(
                &unit.id,
                block.form.as_deref().unwrap_or("标准"),
                block.lang.as_deref().unwrap_or("zh"),
            )
            .map_err(bad)?;
        let ver = store
            .insert_version(
                &rend.id,
                SemVer::new(1, 0, 0),
                unit.current_revision,
                &user.display_name,
                "导入",
                &frag_bytes,
                "normal",
            )
            .map_err(bad)?;
        created.push(json!({
            "unit_id": unit.id,
            "rendition_id": rend.id,
            "version_id": ver.id,
            "semver": ver.semver,
            "pages": ver.page_count,
        }));
    }
    Ok(Json(json!({
        "ok": true,
        "slide_count": report.slide_count,
        "fingerprints_match": report.fingerprints_match,
        "units": created,
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
    let user = auth(&st, &headers)?;
    let store = st.store.lock().unwrap();
    let ver = store.get_version(&id).map_err(|_| not_found())?;
    let rend = store.get_rendition(&ver.rendition_id).map_err(bad)?;
    let unit = store.get_unit(&rend.unit_id).map_err(bad)?;
    if !store.can_see_group(&user, &unit.group_id).map_err(bad)? {
        return Err(not_found());
    }
    let bytes = store.file_get(&ver.fragment_hash).map_err(bad)?;
    Ok(Json(json!({ "version": ver, "data_base64": b64e(&bytes) })))
}

/// Thumbnail renderer selection (step 3 of the storytold integration
/// ledger): DeckCraft by default — the pilot (tools/pilot_deckcraft_
/// render.py) rendered Redwood 17/17, Case2 7/7 and Case2b 1/1 pages with
/// mean abs pixel diff vs LibreOffice <= 4.8 per deck (worst page 8.2, a
/// chart page) at ~4x the speed. QIMU_THUMB_RENDERER=libreoffice pins
/// the old pipeline; a DeckCraft failure also falls back to it per render.
/// The on-disk cache is namespaced by this name (see version_thumb).
fn thumb_renderer() -> &'static str {
    match std::env::var("QIMU_THUMB_RENDERER").as_deref() {
        Ok("libreoffice") => "libreoffice",
        _ => "deckcraft",
    }
}

fn render_thumbs(root: &std::path::Path, version_id: &str, frag: &[u8]) -> Result<Vec<Vec<u8>>, String> {
    if thumb_renderer() == "deckcraft" {
        match render_thumbs_deckcraft(root, version_id, frag) {
            Ok(pages) => return Ok(pages),
            Err(e) => {
                eprintln!("[thumb] DeckCraft 渲染失败，回退 LibreOffice（{version_id}）: {e}");
            }
        }
    }
    render_thumbs_lo(root, version_id, frag)
}

/// Render a fragment PPTX to per-slide PNGs with the DeckCraft CLI
/// (storytold/deckcraft, MIT OR Apache-2.0). Binary resolution:
/// $QIMU_DECKCRAFT_CLI, then a deckcraft-cli sitting next to this server
/// executable, then PATH. Scale 0.75 lands near the old pipeline's
/// 55-DPI page size (733px wide for a 16:9 deck).
fn render_thumbs_deckcraft(root: &std::path::Path, version_id: &str, frag: &[u8]) -> Result<Vec<Vec<u8>>, String> {
    let bin = std::env::var("QIMU_DECKCRAFT_CLI")
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
        .unwrap_or_else(|| "deckcraft-cli".to_string());
    let work = root.join("thumbs").join(format!(".work-dc-{version_id}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&work);
    std::fs::create_dir_all(&work).map_err(|e| e.to_string())?;
    let cleanup = |work: &std::path::Path| {
        let _ = std::fs::remove_dir_all(work);
    };
    let pptx = work.join("frag.pptx");
    std::fs::write(&pptx, frag).map_err(|e| e.to_string())?;
    let outdir = work.join("dc");
    let out = std::process::Command::new(&bin)
        .arg("render")
        .arg(&pptx)
        .arg("--all")
        .arg(&outdir)
        .args(["--scale", "0.75"])
        .output()
        .map_err(|e| format!("启动 deckcraft-cli 失败（{bin}）: {e}"))?;
    if !out.status.success() {
        cleanup(&work);
        return Err(format!(
            "deckcraft-cli 渲染失败: {}",
            String::from_utf8_lossy(&out.stderr)
        ));
    }
    let mut pages: Vec<(u32, Vec<u8>)> = Vec::new();
    if let Ok(rd) = std::fs::read_dir(&outdir) {
        for ent in rd.flatten() {
            let name = ent.file_name().to_string_lossy().to_string();
            if let Some(num) = name
                .strip_prefix("slide-")
                .and_then(|n| n.strip_suffix(".png"))
                .and_then(|n| n.parse::<u32>().ok())
            {
                if let Ok(bytes) = std::fs::read(ent.path()) {
                    pages.push((num, bytes));
                }
            }
        }
    }
    pages.sort_by_key(|(n, _)| *n);
    cleanup(&work);
    if pages.is_empty() {
        return Err("DeckCraft 渲染未产出任何页面".into());
    }
    Ok(pages.into_iter().map(|(_, b)| b).collect())
}

/// Render a fragment PPTX to per-slide PNGs with LibreOffice headless
/// (PPTX -> PDF -> pdftoppm). Runs in a private temp dir with its own LO
/// profile so concurrent renders don't fight over the default profile.
fn render_thumbs_lo(root: &std::path::Path, version_id: &str, frag: &[u8]) -> Result<Vec<Vec<u8>>, String> {
    let work = root.join("thumbs").join(format!(".work-{version_id}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&work);
    std::fs::create_dir_all(&work).map_err(|e| e.to_string())?;
    let cleanup = |work: &std::path::Path| {
        let _ = std::fs::remove_dir_all(work);
    };
    let pptx = work.join("frag.pptx");
    std::fs::write(&pptx, frag).map_err(|e| e.to_string())?;
    // The profile must be a real file:// URI: with a relative data root
    // (the packaged default "qimu-data") the naive concatenation yields
    // file://qimu-data/... — "qimu-data" parses as a host name and
    // LibreOffice hangs forever. Absolutize, and normalize for Windows
    // (strip the \\?\ prefix, use forward slashes).
    let work_abs = std::fs::canonicalize(&work).unwrap_or_else(|_| work.clone());
    let profile_str = work_abs.join("lo-profile").display().to_string();
    let profile_str = profile_str
        .strip_prefix(r"\\?\")
        .unwrap_or(&profile_str)
        .replace('\\', "/");
    let profile = format!("file://{profile_str}");
    let lo = std::process::Command::new("soffice")
        .arg(format!("-env:UserInstallation={profile}"))
        .args(["--headless", "--convert-to", "pdf", "--outdir"])
        .arg(&work)
        .arg(&pptx)
        .output()
        .map_err(|e| format!("启动 LibreOffice 失败: {e}"))?;
    let pdf = work.join("frag.pdf");
    if !lo.status.success() || !pdf.exists() {
        cleanup(&work);
        return Err(format!(
            "LibreOffice 渲染失败: {}",
            String::from_utf8_lossy(&lo.stderr)
        ));
    }
    let ppm = std::process::Command::new("pdftoppm")
        .args(["-png", "-r", "55"])
        .arg(&pdf)
        .arg(work.join("page"))
        .output()
        .map_err(|e| format!("启动 pdftoppm 失败: {e}"))?;
    if !ppm.status.success() {
        cleanup(&work);
        return Err(format!(
            "pdftoppm 失败: {}",
            String::from_utf8_lossy(&ppm.stderr)
        ));
    }
    let mut pages: Vec<(u32, Vec<u8>)> = Vec::new();
    if let Ok(rd) = std::fs::read_dir(&work) {
        for ent in rd.flatten() {
            let name = ent.file_name().to_string_lossy().to_string();
            if let Some(num) = name
                .strip_prefix("page-")
                .and_then(|n| n.strip_suffix(".png"))
                .and_then(|n| n.parse::<u32>().ok())
            {
                if let Ok(bytes) = std::fs::read(ent.path()) {
                    pages.push((num, bytes));
                }
            }
        }
    }
    pages.sort_by_key(|(n, _)| *n);
    cleanup(&work);
    if pages.is_empty() {
        return Err("渲染未产出任何页面".into());
    }
    Ok(pages.into_iter().map(|(_, b)| b).collect())
}

/// GET /v1/versions/:id/thumb/:page — real slide thumbnail (PNG, base64
/// JSON like the fragment endpoint). Rendered once per version, then
/// cached on disk under <data>/thumbs/<version_id>/<page>.png.
async fn version_thumb(
    State(st): State<St>,
    headers: HeaderMap,
    Path((id, page)): Path<(String, usize)>,
) -> Api<Value> {
    let user = auth(&st, &headers)?;
    let (root, frag) = {
        let store = st.store.lock().unwrap();
        let ver = store.get_version(&id).map_err(|_| not_found())?;
        let rend = store.get_rendition(&ver.rendition_id).map_err(bad)?;
        let unit = store.get_unit(&rend.unit_id).map_err(bad)?;
        if !store.can_see_group(&user, &unit.group_id).map_err(bad)? {
            return Err(not_found());
        }
        let frag = store.file_get(&ver.fragment_hash).map_err(bad)?;
        (store.root().to_path_buf(), frag)
    };
    let cache_dir = root.join("thumbs").join(thumb_renderer()).join(&id);
    let cache_file = cache_dir.join(format!("{page}.png"));
    if !cache_file.exists() {
        let root2 = root.clone();
        let id2 = id.clone();
        let rendered = tokio::task::spawn_blocking(move || render_thumbs(&root2, &id2, &frag))
            .await
            .map_err(|e| bad(e.to_string()))?
            .map_err(bad)?;
        std::fs::create_dir_all(&cache_dir).map_err(|e| bad(e.to_string()))?;
        for (n, bytes) in rendered.iter().enumerate() {
            let f = cache_dir.join(format!("{n}.png"));
            if !f.exists() {
                std::fs::write(&f, bytes).map_err(|e| bad(e.to_string()))?;
            }
        }
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
    let user = auth(&st, &headers)?;
    let store = st.store.lock().unwrap();
    let v = store.get_version(&id).map_err(|_| not_found())?;
    let rend = store.get_rendition(&v.rendition_id).map_err(bad)?;
    let unit = store.get_unit(&rend.unit_id).map_err(bad)?;
    if !store.can_see_group(&user, &unit.group_id).map_err(bad)? {
        return Err(not_found());
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
    Ok(Json(json!({ "tags": store.list_version_tags(&id).map_err(bad)? })))
}

#[derive(Deserialize)]
struct SearchQuery {
    q: String,
}

async fn search(State(st): State<St>, headers: HeaderMap, Query(qp): Query<SearchQuery>) -> Api<Value> {
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
    let store = st.store.lock().unwrap();
    commit
        .entries
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

async fn get_index(
    State(st): State<St>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Query(q): Query<BranchQuery>,
) -> Api<Value> {
    let user = auth(&st, &headers)?;
    let branch = q.branch.unwrap_or_else(|| "main".to_string());
    let (index, branches, tags, head) = {
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
    store.set_index_lifecycle(&id, &req.lifecycle).map_err(bad)?;
    Ok(Json(json!({ "ok": true })))
}

// ---------------- export engine ----------------

/// Resolve entries to pages and assemble a deck with an embedded
/// manifest. Returns (deck bytes, manifest, page count).
fn build_export(
    st: &St,
    user: &UserRec,
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
        let scrub: &[&str] = if kind == "company" {
            personalize::PERSONAL_FIELDS
        } else {
            values.insert("name".to_string(), user.display_name.clone());
            for (k, v) in store.profile(&user.id).map_err(bad)? {
                if !v.is_empty() {
                    values.insert(k, v);
                }
            }
            &[]
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
        &user,
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
                            Some(s) => s
                                .iter()
                                .filter_map(|x| x.parse::<usize>().ok())
                                .collect(),
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
    let head_rends: std::collections::HashSet<&str> =
        head.entries.iter().map(|e| e.rendition_id.0.as_str()).collect();
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

async fn update_releases(
    State(st): State<St>,
) -> axum::response::Response {
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
}

async fn submit_showroom(
    State(st): State<St>,
    headers: HeaderMap,
    Json(req): Json<ShowroomReq>,
) -> Api<Value> {
    let user = auth(&st, &headers)?;
    let store = st.store.lock().unwrap();
    let commit = match &req.source_index_id {
        Some(iid) => store.branch_head(iid, "main").map_err(bad)?,
        None => None,
    };
    let rec = store
        .insert_showroom(
            &req.title,
            req.intro.as_deref().unwrap_or(""),
            req.source_index_id.as_deref(),
            commit.as_deref(),
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
        None => items.retain(|i| i.state == "live"),
        Some(u) if u.role != "super" => {
            items.retain(|i| i.state == "live" || i.submitted_by == u.display_name)
        }
        _ => {}
    }
    Ok(Json(json!({ "items": items })))
}

#[derive(Deserialize)]
struct ReviewReq {
    state: String, // live | rejected
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
    let store = st.store.lock().unwrap();
    store.set_showroom_state(&id, &req.state).map_err(bad)?;
    Ok(Json(json!({ "ok": true })))
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
    let id = store.create_custom_group(&user.id, &req.name).map_err(bad)?;
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
    let (_, owner) = store.custom_group(&id).map_err(bad)?.ok_or_else(|| not_found())?;
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
    let (_, owner) = store.custom_group(&id).map_err(bad)?.ok_or_else(|| not_found())?;
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
    // validate target
    match req.target_kind.as_str() {
        "user" => {
            if !store.list_users().map_err(bad)?.iter().any(|u| u.id == req.target_id) {
                return Err(bad("目标用户不存在"));
            }
        }
        "dept" => {
            if !store.list_departments().map_err(bad)?.iter().any(|d| d.id == req.target_id) {
                return Err(bad("目标部门不存在"));
            }
        }
        "group" => {
            let members = store.custom_group_members(&req.target_id).map_err(bad)?;
            if !members.iter().any(|m| m == &user.id) {
                return Err(forbidden());
            }
        }
        _ => return Err(bad("target_kind 只能是 user、dept 或 group")),
    }
    let id = store
        .create_share(&req.item_kind, &req.item_id, &title, &user.id, &req.target_kind, &req.target_id)
        .map_err(bad)?;
    Ok(Json(json!({ "share_id": id })))
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
        if visible {
            let mut s = s;
            s["in_library"] = json!(library.iter().any(|e| e["item_id"] == s["item_id"]));
            out.push(s);
        }
    }
    Ok(Json(json!({ "shares": out })))
}

/// Group space: shares targeted at custom groups I belong to, grouped.
async fn group_space(State(st): State<St>, headers: HeaderMap) -> Api<Value> {
    let user = auth(&st, &headers)?;
    let store = st.store.lock().unwrap();
    let groups = store.list_custom_groups_for(&user.id).map_err(bad)?;
    let shares = store.list_shares().map_err(bad)?;
    let mut out = Vec::new();
    for g in &groups {
        let items: Vec<Value> = shares
            .iter()
            .filter(|s| {
                s["target_kind"] == "group" && s["target_id"] == g["id"]
            })
            .cloned()
            .collect();
        out.push(json!({ "group": g, "items": items }));
    }
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
/// group visible OR item shared to me / in my library / mine).
async fn export_showroom(
    State(st): State<St>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(req): Json<ExportReq>,
) -> Api<Value> {
    let user = auth(&st, &headers)?;
    let (item, entries, source) = {
        let store = st.store.lock().unwrap();
        let item = store
            .get_showroom(&id)
            .map_err(bad)?
            .ok_or_else(|| not_found())?;
        if !store.showroom_item_access(&user, &item).map_err(bad)? {
            return Err(not_found());
        }
        let idx_id = item.source_index_id.clone().ok_or_else(|| bad("该条目没有来源索引"))?;
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
        let source = SourceRef {
            index_id: idx_id,
            branch: "main".to_string(),
            commit_id: commit.id.clone(),
            tag: None,
        };
        (item, commit.entries.clone(), source)
    };
    let _ = item;
    let kind = req.kind.as_deref().unwrap_or("personal");
    let (deck, manifest, page_count) = build_export(
        &st,
        &user,
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
        &user,
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

// ---------------- LAN discovery (§4: clients auto-find servers) -------

/// UDP port answering discovery broadcasts; the HTTP port stays 8730.
const DISCOVERY_PORT: u16 = 8731;
const DISCOVERY_MAGIC: &[u8] = b"QIMU_DISCOVER_V1";

fn machine_name() -> String {
    std::env::var("HOSTNAME")
        .or_else(|_| std::env::var("COMPUTERNAME"))
        .ok()
        .filter(|s| !s.trim().is_empty())
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
    std::path::PathBuf::from(home).join(".qimu").join("server-data")
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
    let state: St = Arc::new(AppState {
        store: Mutex::new(store),
        data_dir: data_dir.clone(),
    });
    let app = Router::new()
        .route("/health", get(|| async { Json(json!({ "status": "ok", "service": "qimu-server" })) }))
        .route("/update/releases.json", get(update_releases))
        .route("/update/:tag/:name", get(update_asset))
        .route("/v1/auth/login", post(login))
        .route("/v1/auth/logout", post(logout))
        .route("/v1/me", get(me))
        .route("/v1/me/profile", axum::routing::put(put_profile))
        .route("/v1/org", get(org))
        .route("/v1/org/departments", post(create_dept))
        .route("/v1/org/users", post(create_user))
        .route("/v1/org/users/:id/disable", post(disable_user))
        .route("/v1/groups", get(groups).post(create_group))
        .route("/v1/grants", get(grants).post(add_grant))
        .route("/v1/company-profile", get(company_profile).put(put_company_profile))
        .route("/v1/settings/languages", get(languages).put(put_languages))
        .route("/v1/settings/glossary", get(get_glossary).put(put_glossary))
        .route("/v1/settings/cache-policy", get(get_cache_policy).put(put_cache_policy))
        .route("/v1/settings/ai", get(get_ai_settings).put(put_ai_settings))
        .route("/v1/units/:id/translate", post(translate_unit))
        .route("/v1/folders", get(folders).post(create_folder))
        .route("/v1/folders/:id/rename", post(rename_folder))
        .route("/v1/folders/:id", delete(delete_folder))
        .route("/v1/folders/:id/main-index", post(set_folder_main_index))
        .route("/v1/units", get(list_units).post(create_unit))
        .route("/v1/units/:id", get(get_unit).patch(patch_unit))
        .route("/v1/units/:id/bump-revision", post(|State(st): State<St>, headers: HeaderMap, Path(id): Path<String>| async move {
            let user = auth(&st, &headers)?;
            let store = st.store.lock().unwrap();
            let unit = store.get_unit(&id).map_err(|_| not_found())?;
            drop(store);
            need_cap(&st, &user, &unit.group_id, "publish")?;
            let store = st.store.lock().unwrap();
            let r = store.bump_unit_revision(&id).map_err(bad)?;
            Ok::<_, (StatusCode, Json<Value>)>(Json(json!({ "current_revision": r })))
        }))
        .route("/v1/import", post(import_deck))
        .route("/v1/import/propose", post(import_propose))
        .route("/v1/units/:id/publish", post(publish))
        .route("/v1/versions/:id/recall", post(recall_version))
        .route("/v1/versions/:id/delete", post(delete_version))
        .route("/v1/versions/:id/fragment", get(version_fragment))
        .route("/v1/versions/:id/thumb/:page", get(version_thumb))
        .route("/v1/versions/:id/tags", get(list_version_tags).post(add_version_tag))
        .route("/v1/versions/:id", get(get_version))
        .route("/v1/versions/:id/scripts", axum::routing::put(put_version_scripts))
        .route(
            "/v1/units/:id/page-bindings",
            get(list_page_bindings).post(add_page_binding),
        )
        .route("/v1/units/:id/page-bindings/:bid/delete", post(delete_page_binding))
        .route("/v1/search", get(search))
        .route("/v1/indexes", get(list_indexes).post(create_index))
        .route("/v1/indexes/:id", get(get_index))
        .route("/v1/indexes/:id/commits", get(list_commits).post(post_commit))
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
        .route("/v1/attachments", get(list_attachments).post(add_attachment))
        .route("/v1/attachments/:id/download", get(download_attachment))
        .route("/v1/showroom", get(list_showroom).post(submit_showroom))
        .route("/v1/showroom/:id/review", post(review_showroom))
        .route("/v1/showroom/:id/export", post(export_showroom))
        .route("/v1/custom-groups", get(list_custom_groups).post(create_custom_group))
        .route("/v1/custom-groups/:id/members", post(add_custom_group_member))
        .route("/v1/custom-groups/:id/members/remove", post(remove_custom_group_member))
        .route("/v1/shares", post(create_share))
        .route("/v1/shares/inbox", get(list_inbox))
        .route("/v1/shares/:id/accept", post(accept_share))
        .route("/v1/group-space", get(group_space))
        .route("/v1/library", get(list_library))
        .route("/v1/library/:id/delete", post(remove_library_entry))
        .route("/v1/assemblies", get(list_assemblies).post(create_assembly))
        .route("/v1/assemblies/:id/export", post(export_assembly))
        .layer(DefaultBodyLimit::max(512 * 1024 * 1024))
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
    if let Err(e) = axum::serve(listener, app).await {
        fatal_startup(&format!("服务器运行中断：{e}"));
    }
}

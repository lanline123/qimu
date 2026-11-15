//! qimu-client — the Rust desktop client (spec §4): four top-level pages
//! 演示厅 / 内容页 / 索引页 / 设置, talking to qimu-server over HTTP.
//! All logic lives in qimu-core / the server; this crate is a frontend,
//! exactly like the CLI (spec §4.4 parity).
#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

use base64::Engine;
use eframe::egui;
use serde_json::{json, Value};
use std::collections::BTreeMap;

fn install_cjk_font(ctx: &egui::Context) {
    // Default UI font: Alibaba PuHuiTi 3.0, embedded at build time when
    // the font file is present in assets/ (see build.rs). egui's bundled
    // fonts have no CJK glyphs, so a system CJK font is the fallback.
    let mut fonts = egui::FontDefinitions::default();
    #[cfg(puhuiti)]
    {
        fonts.font_data.insert(
            "puhuiti".to_owned(),
            egui::FontData::from_static(include_bytes!(
                "../assets/AlibabaPuHuiTi-3-55-Regular.ttf"
            )),
        );
        for fam in [egui::FontFamily::Proportional, egui::FontFamily::Monospace] {
            fonts
                .families
                .entry(fam)
                .or_default()
                .insert(0, "puhuiti".to_owned());
        }
    }
    let candidates = [
        "/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc",
        "/usr/share/fonts/truetype/noto/NotoSansCJK-Regular.ttc",
        "C:\\Windows\\Fonts\\msyh.ttc",
        "C:\\Windows\\Fonts\\simhei.ttf",
        "/System/Library/Fonts/PingFang.ttc",
        "/usr/share/fonts/truetype/wqy/wqy-microhei.ttc",
    ];
    for path in candidates {
        if let Ok(bytes) = std::fs::read(path) {
            fonts.font_data.insert(
                "cjk".to_owned(),
                egui::FontData::from_owned(bytes),
            );
            for fam in [egui::FontFamily::Proportional, egui::FontFamily::Monospace] {
                fonts
                    .families
                    .entry(fam)
                    .or_default()
                    .push("cjk".to_owned());
            }
            break;
        }
    }
    ctx.set_fonts(fonts);
}

/// Design language of the Craft family apps in their dark theme (the
/// EffectCraft look the product owner picked): near-black canvas #121212,
/// chrome #1C1C1C, cards #232323 with #383838 hairlines, ghost controls,
/// one blue accent #2D8CEB for tab underlines, primaries and key marks;
/// selection is gray, never an accent flood. Spec: design/设计规范-EffectCraft风格.md.
pub const ACCENT: egui::Color32 = egui::Color32::from_rgb(0x2D, 0x8C, 0xEB);
pub const ACCENT_DEEP: egui::Color32 = egui::Color32::from_rgb(0x6C, 0xB2, 0xF5);
pub const CHROME: egui::Color32 = egui::Color32::from_rgb(0x1C, 0x1C, 0x1C);
pub const APP_BG: egui::Color32 = egui::Color32::from_rgb(0x12, 0x12, 0x12);
pub const CARD_BG: egui::Color32 = egui::Color32::from_rgb(0x23, 0x23, 0x23);
pub const CARD_BORDER: egui::Color32 = egui::Color32::from_rgb(0x38, 0x38, 0x38);
pub const SEPARATOR: egui::Color32 = egui::Color32::from_rgb(0x2A, 0x2A, 0x2A);
pub const TEXT_MAIN: egui::Color32 = egui::Color32::from_rgb(0xF2, 0xF2, 0xF2);
pub const TEXT_WEAK: egui::Color32 = egui::Color32::from_rgb(0x9E, 0x9E, 0x9E);
pub const TEXT_FAINT: egui::Color32 = egui::Color32::from_rgb(0x6E, 0x6E, 0x6E);
pub const HOVER_BG: egui::Color32 = egui::Color32::from_rgb(0x2C, 0x2C, 0x2C);

fn apply_modern_style(ctx: &egui::Context) {
    let mut style = (*ctx.style()).clone();
    let mut v = egui::Visuals::dark();
    v.panel_fill = CHROME;
    v.window_fill = CARD_BG;
    // egui 0.29 has no separate text-field fill; extreme_bg serves it —
    // Craft dark field gray, readable on cards and canvas alike.
    v.extreme_bg_color = egui::Color32::from_rgb(0x2A, 0x2A, 0x2A);
    v.faint_bg_color = CHROME;
    v.selection.bg_fill = egui::Color32::from_rgb(0x3A, 0x3A, 0x3A);
    v.selection.stroke = egui::Stroke::new(1.0, TEXT_MAIN);
    v.hyperlink_color = ACCENT;
    v.override_text_color = Some(TEXT_MAIN);
    v.window_rounding = egui::Rounding::same(10.0);
    v.menu_rounding = egui::Rounding::same(8.0);
    v.window_shadow = egui::epaint::Shadow {
        offset: egui::vec2(0.0, 4.0),
        blur: 18.0,
        spread: 0.0,
        color: egui::Color32::from_black_alpha(90),
    };
    v.popup_shadow = egui::epaint::Shadow {
        offset: egui::vec2(0.0, 3.0),
        blur: 12.0,
        spread: 0.0,
        color: egui::Color32::from_black_alpha(90),
    };
    let r = egui::Rounding::same(5.0);
    for w in [
        &mut v.widgets.noninteractive,
        &mut v.widgets.inactive,
        &mut v.widgets.hovered,
        &mut v.widgets.active,
        &mut v.widgets.open,
    ] {
        w.rounding = r;
    }
    v.widgets.noninteractive.bg_stroke = egui::Stroke::new(1.0, SEPARATOR);
    // ghost controls: no outlines; fills only where a control must stand out
    v.widgets.inactive.weak_bg_fill = egui::Color32::TRANSPARENT;
    v.widgets.inactive.bg_fill = egui::Color32::from_rgb(0x30, 0x30, 0x30);
    v.widgets.inactive.bg_stroke = egui::Stroke::NONE;
    v.widgets.hovered.weak_bg_fill = HOVER_BG;
    v.widgets.hovered.bg_fill = egui::Color32::from_rgb(0x3A, 0x3A, 0x3A);
    v.widgets.hovered.bg_stroke = egui::Stroke::NONE;
    v.widgets.active.weak_bg_fill = egui::Color32::from_rgb(0x38, 0x38, 0x38);
    v.widgets.active.bg_fill = egui::Color32::from_rgb(0x45, 0x45, 0x45);
    v.widgets.active.bg_stroke = egui::Stroke::NONE;
    v.widgets.open.weak_bg_fill = HOVER_BG;
    v.widgets.open.bg_stroke = egui::Stroke::NONE;
    style.visuals = v;
    use egui::{FontFamily, FontId, TextStyle};
    style.text_styles.insert(
        TextStyle::Heading,
        FontId::new(18.0, FontFamily::Proportional),
    );
    style
        .text_styles
        .insert(TextStyle::Body, FontId::new(13.0, FontFamily::Proportional));
    style.text_styles.insert(
        TextStyle::Button,
        FontId::new(13.0, FontFamily::Proportional),
    );
    style.text_styles.insert(
        TextStyle::Small,
        FontId::new(11.0, FontFamily::Proportional),
    );
    style
        .text_styles
        .insert(TextStyle::Monospace, FontId::new(12.0, FontFamily::Monospace));
    style.spacing.item_spacing = egui::vec2(6.0, 4.0);
    style.spacing.button_padding = egui::vec2(8.0, 3.0);
    style.spacing.interact_size.y = 22.0;
    style.spacing.indent = 14.0;
    style.spacing.window_margin = egui::Margin::same(10.0);
    ctx.set_style(style);
}

/// The one standard card (§4.2), Craft-style: white surface, hairline
/// #E3E3E3 border, 10px radius, soft shadow — cover-first layout.
fn card_frame() -> egui::Frame {
    egui::Frame::none()
        .fill(CARD_BG)
        .stroke(egui::Stroke::new(1.0, CARD_BORDER))
        .rounding(egui::Rounding::same(10.0))
        .inner_margin(egui::Margin::same(14.0))
        .shadow(egui::epaint::Shadow {
            offset: egui::vec2(0.0, 1.0),
            blur: 6.0,
            spread: 0.0,
            color: egui::Color32::from_black_alpha(8),
        })
}

/// Square-corner variant used by the index entry (PPT) cards.
fn card_frame_square() -> egui::Frame {
    card_frame().rounding(egui::Rounding::ZERO)
}

/// Flush variant (no inner margin) for cover-first cards: the cover
/// image runs edge to edge, the body adds its own padding.
fn card_frame_flush() -> egui::Frame {
    card_frame().inner_margin(egui::Margin::same(0.0))
}

/// Small tinted pill for statuses (version policy, excerpt, version no).
/// Painted manually with an exact-size allocation: a Frame here inherits
/// whatever layout surrounds it and stretches/wraps unpredictably.
fn chip(ui: &mut egui::Ui, text: &str, fg: egui::Color32, bg: egui::Color32) {
    let font = egui::FontId::new(11.0, egui::FontFamily::Proportional);
    let galley = ui.fonts(|f| f.layout_no_wrap(text.to_string(), font, fg));
    let size = egui::vec2(galley.size().x + 16.0, 18.0);
    let (rect, _) = ui.allocate_exact_size(size, egui::Sense::hover());
    let painter = ui.painter();
    painter.rect_filled(rect, 9.0, bg);
    painter.galley(
        egui::pos2(rect.min.x + 8.0, rect.center().y - galley.size().y / 2.0),
        galley,
        fg,
    );
}

pub const CHIP_GREEN_FG: egui::Color32 = egui::Color32::from_rgb(0x35, 0xB2, 0x6B);
pub const CHIP_GREEN_BG: egui::Color32 = egui::Color32::from_rgb(0x1E, 0x3A, 0x2A);
pub const CHIP_ORANGE_FG: egui::Color32 = egui::Color32::from_rgb(0xE8, 0xA1, 0x3A);
pub const CHIP_ORANGE_BG: egui::Color32 = egui::Color32::from_rgb(0x3D, 0x2E, 0x1A);
pub const CHIP_BLUE_BG: egui::Color32 = egui::Color32::from_rgb(0x1B, 0x2A, 0x3F);
pub const CHIP_GRAY_FG: egui::Color32 = egui::Color32::from_rgb(0x9E, 0x9E, 0x9E);
pub const CHIP_GRAY_BG: egui::Color32 = egui::Color32::from_rgb(0x30, 0x30, 0x30);

/// Craft-style tab (top bar + zone switchers): ghost text tab, the
/// selected one carries an accent underline.
fn pill(ui: &mut egui::Ui, selected: bool, label: &str) -> egui::Response {
    let text = egui::RichText::new(label).color(if selected {
        TEXT_MAIN
    } else {
        TEXT_WEAK
    });
    let btn = egui::Button::new(text)
        .fill(egui::Color32::TRANSPARENT)
        .stroke(egui::Stroke::NONE)
        .rounding(egui::Rounding::same(5.0))
        .min_size(egui::vec2(0.0, 26.0));
    let resp = ui.add(btn);
    if selected {
        let r = resp.rect;
        ui.painter().rect_filled(
            egui::Rect::from_min_max(
                egui::pos2(r.min.x + 7.0, r.max.y - 2.0),
                egui::pos2(r.max.x - 7.0, r.max.y),
            ),
            1.0,
            ACCENT,
        );
    }
    resp
}

/// Offline cache (§3.17): adopted snapshots live under ~/.qimu/cache with
/// a small JSON index, per account machine — usable with no server at all.
fn cache_dir() -> std::path::PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
    std::path::Path::new(&home).join(".qimu").join("cache")
}

fn load_cache_index() -> Vec<Value> {
    std::fs::read_to_string(cache_dir().join("index.json"))
        .ok()
        .and_then(|t| serde_json::from_str::<Value>(&t).ok())
        .and_then(|v| v.as_array().cloned())
        .unwrap_or_default()
}

/// At-rest encryption for the offline cache (§3.17: 静态加密必需）:
/// AES-256-GCM with a per-machine key kept in ~/.qimu/cache.key.
fn cache_key() -> Option<[u8; 32]> {
    use aes_gcm::aead::OsRng;
    use aes_gcm::aead::rand_core::RngCore;
    let dir = cache_dir();
    let path = dir.join("..").join("cache.key");
    if let Ok(hex) = std::fs::read_to_string(&path) {
        let hex = hex.trim();
        if hex.len() == 64 {
            let mut key = [0u8; 32];
            let mut ok = true;
            for i in 0..32 {
                match u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16) {
                    Ok(b) => key[i] = b,
                    Err(_) => {
                        ok = false;
                        break;
                    }
                }
            }
            if ok {
                return Some(key);
            }
        }
    }
    let mut key = [0u8; 32];
    OsRng.fill_bytes(&mut key);
    let hex: String = key.iter().map(|b| format!("{b:02x}")).collect();
    let _ = std::fs::create_dir_all(&dir);
    if std::fs::write(&path, hex).is_ok() {
        Some(key)
    } else {
        None
    }
}

fn encrypt_bytes(key: &[u8; 32], plain: &[u8]) -> Option<Vec<u8>> {
    use aes_gcm::aead::{Aead, OsRng};
    use aes_gcm::aead::rand_core::RngCore;
    use aes_gcm::{Aes256Gcm, Key, KeyInit, Nonce};
    let cipher = Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(key));
    let mut nonce = [0u8; 12];
    OsRng.fill_bytes(&mut nonce);
    let ct = cipher.encrypt(Nonce::from_slice(&nonce), plain).ok()?;
    let mut out = nonce.to_vec();
    out.extend_from_slice(&ct);
    Some(out)
}

fn decrypt_bytes(key: &[u8; 32], data: &[u8]) -> Option<Vec<u8>> {
    use aes_gcm::aead::Aead;
    use aes_gcm::{Aes256Gcm, Key, KeyInit, Nonce};
    if data.len() < 12 {
        return None;
    }
    let cipher = Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(key));
    cipher
        .decrypt(Nonce::from_slice(&data[..12]), &data[12..])
        .ok()
}

fn spawn_open(path: &std::path::Path) -> std::io::Result<std::process::Child> {
    if cfg!(target_os = "windows") {
        std::process::Command::new("cmd")
            .args(["/C", "start", ""])
            .arg(path)
            .spawn()
    } else if cfg!(target_os = "macos") {
        std::process::Command::new("open").arg(path).spawn()
    } else {
        std::process::Command::new("xdg-open").arg(path).spawn()
    }
}

fn save_cache_index(entries: &[Value]) {
    let dir = cache_dir();
    let _ = std::fs::create_dir_all(&dir);
    let _ = std::fs::write(
        dir.join("index.json"),
        serde_json::to_string_pretty(&json!(entries)).unwrap_or_default(),
    );
}

/// Short human label for one index entry's version policy (merge view).
fn semver_key(v: &str) -> (u64, u64, u64) {
    let mut it = v.split('.').map(|x| x.parse::<u64>().unwrap_or(0));
    (
        it.next().unwrap_or(0),
        it.next().unwrap_or(0),
        it.next().unwrap_or(0),
    )
}

fn entry_label(e: &Value) -> String {
    if e.is_null() {
        return "（无此条目）".into();
    }
    let pol = e.get("policy").cloned().unwrap_or(Value::Null);
    let mut base = if pol.get("locked").is_some() {
        format!(
            "锁定 v{}",
            e.get("locked_semver").and_then(|x| x.as_str()).unwrap_or("?")
        )
    } else {
        "跟随最新".to_string()
    };
    if let Some(slots) = e.get("excerpt_slots").and_then(|x| x.as_array()) {
        base += &format!(" · 摘录 {} 页", slots.len());
    }
    base
}

fn fmt_time(ms: i64) -> String {
    // unix ms -> "YYYY-MM-DD HH:MM" (UTC+8 display for the LAN product)
    let secs = ms / 1000 + 8 * 3600;
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    let (hh, mm) = (rem / 3600, (rem % 3600) / 60);
    let (y, mo, d) = civil_from_days(days);
    format!("{y:04}-{mo:02}-{d:02} {hh:02}:{mm:02}")
}

fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

fn b64e(bytes: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

fn b64d(s: &str) -> Result<Vec<u8>, String> {
    base64::engine::general_purpose::STANDARD
        .decode(s)
        .map_err(|e| e.to_string())
}

#[derive(Clone, Copy, PartialEq)]
enum Page {
    Showroom,
    Content,
    Index,
    Settings,
}

#[derive(Default)]
struct ContentState {
    folder_filter: Option<String>,
    selected_unit: Option<String>,
    detail: Option<Value>,
    search_text: String,
    search_hits: Option<Value>,
    import_group: String,
    new_unit_name: String,
    publish_bump: usize, // 0 major 1 minor 2 patch
    publish_form: String,
    publish_lang: String,
    publish_note: String,
    publish_file: Option<std::path::PathBuf>,
    bindings: Vec<Value>,
    bind_from: String,
    bind_to: String,
    bind_from_slot: Option<i64>,
    scripts_for: Option<String>, // version id whose 讲稿 is being edited
    scripts_edit: Vec<String>,
    translate_to: String,
}

#[derive(Default)]
struct IndexState {
    selected: Option<String>,
    detail: Option<Value>,
    branch: String,
    tag_name: String,
    new_branch: String,
    creating: bool,
    new_name: String,
    new_group: String,
    pick: Vec<(String, String, bool)>, // unit_id, rendition_id, checked
    commits: Option<Value>,
    exports: Option<Value>,
    merge_from: String,
    merge_preview: Option<Value>,
    merge_resolutions: std::collections::HashMap<String, String>,
    include_notes: bool,
    excerpt_for: Option<usize>,
    excerpt_sel: Vec<bool>,
    unit_versions: std::collections::HashMap<String, Vec<Value>>, // rendition_id -> versions, semver desc
    more_for: Option<usize>,
    insert_at: Option<usize>,
    insert_units: Vec<Value>,
}

#[derive(Default)]
struct SettingsState {
    section: usize,
    ai_provider: String,
    ai_endpoint: String,
    ai_model: String,
    ai_key: String,
    ai_has_key: bool,
    ai_loaded: bool,
    glossary_text: String,
    profile: BTreeMap<String, String>,
    company: BTreeMap<String, String>,
    languages_text: String,
    dept_name: String,
    user_name: String,
    user_display: String,
    user_password: String,
    group_name: String,
    grant_group: String,
    grant_subject_type: String,
    grant_subject_id: String,
    grant_capability: String,
}

struct App {
    server: String,
    username: String,
    password: String,
    token: Option<String>,
    user: Option<Value>,
    page: Page,
    msg: String,
    folders: Vec<Value>,
    groups: Vec<Value>,
    units: Vec<Value>,
    indexes: Vec<Value>,
    org: Value,
    grants: Vec<Value>,
    showroom_live: Vec<Value>,
    showroom_pending: Vec<Value>,
    sr_title: String,
    sr_index: String,
    sr_zone: String, // plaza | library | inbox | groups
    my_assemblies: Vec<Value>,
    inbox: Vec<Value>,
    group_spaces: Vec<Value>,
    my_teams: Vec<Value>,
    library_refs: Vec<Value>,
    my_id: String,
    share_for: Option<String>, // showroom item id being shared
    share_kind: String,        // user | dept | group
    share_target: String,
    new_team_name: String,
    team_member_pick: String,
    cache_entries: Vec<Value>,
    cache_policy: Value,
    index_covers: std::collections::HashMap<String, String>,
    content: ContentState,
    index: IndexState,
    settings: SettingsState,
    frames: u64,
    screenshot_path: Option<String>,
    screenshot_done: bool,
    thumbs: std::collections::HashMap<String, ThumbState>,
    thumb_tx: std::sync::mpsc::Sender<(String, Option<Vec<u8>>)>,
    thumb_rx: std::sync::mpsc::Receiver<(String, Option<Vec<u8>>)>,
}

enum ThumbState {
    Loading,
    Ready(egui::TextureHandle),
    Failed,
}

impl App {
    fn new(cc: &eframe::CreationContext<'_>) -> Self {
        install_cjk_font(&cc.egui_ctx);
        apply_modern_style(&cc.egui_ctx);
        let (thumb_tx, thumb_rx) = std::sync::mpsc::channel();
        let mut app = App {
            server: std::env::var("QIMU_SERVER").unwrap_or_else(|_| "http://127.0.0.1:8730".into()),
            username: std::env::var("QIMU_USERNAME").unwrap_or_else(|_| "admin".into()),
            password: std::env::var("QIMU_PASSWORD").unwrap_or_default(),
            token: None,
            user: None,
            page: match std::env::var("QIMU_PAGE").as_deref() {
                Ok("content") => Page::Content,
                Ok("index") => Page::Index,
                Ok("settings") => Page::Settings,
                _ => Page::Showroom,
            },
            msg: String::new(),
            folders: vec![],
            groups: vec![],
            units: vec![],
            indexes: vec![],
            org: json!({}),
            grants: vec![],
            showroom_live: vec![],
            showroom_pending: vec![],
            sr_title: String::new(),
            sr_index: String::new(),
            sr_zone: std::env::var("QIMU_ZONE").unwrap_or_else(|_| "plaza".into()),
            my_assemblies: vec![],
            inbox: vec![],
            group_spaces: vec![],
            my_teams: vec![],
            library_refs: vec![],
            my_id: String::new(),
            share_for: None,
            share_kind: "user".into(),
            share_target: String::new(),
            new_team_name: String::new(),
            team_member_pick: String::new(),
            cache_entries: load_cache_index(),
            cache_policy: json!({ "allowed": true, "max_mb": 500 }),
            index_covers: std::collections::HashMap::new(),
            content: ContentState {
                publish_bump: 1, // default preselect: minor （小版本）
                ..Default::default()
            },
            index: IndexState::default(),
            settings: SettingsState {
                grant_subject_type: "dept".into(),
                grant_capability: "view".into(),
                ..Default::default()
            },
            frames: 0,
            screenshot_path: std::env::var("QIMU_SCREENSHOT").ok(),
            screenshot_done: false,
            thumbs: std::collections::HashMap::new(),
            thumb_tx,
            thumb_rx,
        };
        if std::env::var("QIMU_AUTOLOGIN").is_ok() && !app.password.is_empty() {
            app.do_login();
            // Screenshot helper: QIMU_OPEN=index|unit opens the first item's detail.
            match std::env::var("QIMU_OPEN").as_deref() {
                Ok("index") => {
                    if let Some(first) = app.indexes.first() {
                        let id = s(first, "id");
                        app.open_index_detail(&id);
                    }
                }
                Ok("unit") => {
                    if let Some(first) = app.units.first() {
                        if let Some(u) = first.get("unit") {
                            let id = s(u, "id");
                            app.open_unit_detail(&id);
                        }
                    }
                }
                _ => {}
            }
        }
        app
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
            Ok(r) => r.into_json::<Value>().map_err(|e| e.to_string()),
            Err(ureq::Error::Status(_, r)) => {
                let text = r.into_string().unwrap_or_default();
                Err(serde_json::from_str::<Value>(&text)
                    .ok()
                    .and_then(|v| v.get("error").and_then(|e| e.as_str()).map(|s| s.to_string()))
                    .unwrap_or(text))
            }
            Err(e) => Err(format!("请求失败: {e}")),
        }
    }

    fn do_login(&mut self) {
        let r = self.call(
            "POST",
            "/v1/auth/login",
            Some(json!({ "username": self.username, "password": self.password })),
        );
        match r {
            Ok(v) => {
                self.token = v.get("token").and_then(|t| t.as_str()).map(|s| s.to_string());
                self.user = v.get("user").cloned();
                self.msg = "登录成功".into();
                self.refresh_all();
            }
            Err(e) => self.msg = format!("登录失败：{e}"),
        }
    }

    fn refresh_all(&mut self) {
        if let Ok(v) = self.call("GET", "/v1/folders", None) {
            self.folders = v.get("folders").and_then(|f| f.as_array()).cloned().unwrap_or_default();
        }
        if let Ok(v) = self.call("GET", "/v1/groups", None) {
            self.groups = v.get("groups").and_then(|f| f.as_array()).cloned().unwrap_or_default();
        }
        if let Ok(v) = self.call("GET", "/v1/units", None) {
            self.units = v.get("units").and_then(|f| f.as_array()).cloned().unwrap_or_default();
        }
        if let Ok(v) = self.call("GET", "/v1/indexes", None) {
            self.indexes = v.get("indexes").and_then(|f| f.as_array()).cloned().unwrap_or_default();
        }
        if let Ok(v) = self.call("GET", "/v1/org", None) {
            self.org = v;
        }
        if let Ok(v) = self.call("GET", "/v1/showroom?state=live", None) {
            self.showroom_live = v.get("items").and_then(|f| f.as_array()).cloned().unwrap_or_default();
            // plaza card covers: first resolved version of the source index
            for item in self.showroom_live.clone() {
                let src = s(&item, "source_index_id");
                if src.is_empty() || self.index_covers.contains_key(&src) {
                    continue;
                }
                self.index_covers.insert(src.clone(), String::new());
                if let Ok(d) = self.call("GET", &format!("/v1/indexes/{src}"), None) {
                    if let Some(vid) = d
                        .get("resolved_entries")
                        .and_then(|r| r.as_array())
                        .and_then(|a| a.first())
                        .and_then(|e| e.get("resolved"))
                        .and_then(|r| r.get("id"))
                        .and_then(|x| x.as_str())
                    {
                        self.index_covers.insert(src, vid.to_string());
                    }
                }
            }
        }
        if let Ok(v) = self.call("GET", "/v1/showroom?state=pending", None) {
            self.showroom_pending = v.get("items").and_then(|f| f.as_array()).cloned().unwrap_or_default();
        }
        if let Ok(v) = self.call("GET", "/v1/assemblies", None) {
            self.my_assemblies = v
                .get("assemblies")
                .and_then(|f| f.as_array())
                .cloned()
                .unwrap_or_default();
        }
        if let Ok(v) = self.call("GET", "/v1/settings/cache-policy", None) {
            self.cache_policy = v;
        }
        if let Ok(v) = self.call("GET", "/v1/shares/inbox", None) {
            self.inbox = v.get("shares").and_then(|f| f.as_array()).cloned().unwrap_or_default();
        }
        if let Ok(v) = self.call("GET", "/v1/group-space", None) {
            self.group_spaces = v.get("spaces").and_then(|f| f.as_array()).cloned().unwrap_or_default();
        }
        if let Ok(v) = self.call("GET", "/v1/custom-groups", None) {
            self.my_teams = v.get("groups").and_then(|f| f.as_array()).cloned().unwrap_or_default();
        }
        if let Ok(v) = self.call("GET", "/v1/library", None) {
            self.library_refs = v.get("entries").and_then(|f| f.as_array()).cloned().unwrap_or_default();
        }
        self.cache_entries = load_cache_index();
        if let Ok(v) = self.call("GET", "/v1/me", None) {
            if let Some(u) = v.get("user") {
                self.my_id = s(u, "id");
            }
            if let Some(p) = v.get("profile").and_then(|p| p.as_object()) {
                self.settings.profile = p
                    .iter()
                    .map(|(k, val)| (k.clone(), val.as_str().unwrap_or("").to_string()))
                    .collect();
            }
        }
        if let Ok(v) = self.call("GET", "/v1/company-profile", None) {
            if let Some(p) = v.get("fields").and_then(|p| p.as_object()) {
                self.settings.company = p
                    .iter()
                    .map(|(k, val)| (k.clone(), val.as_str().unwrap_or("").to_string()))
                    .collect();
            }
        }
        if let Ok(v) = self.call("GET", "/v1/settings/languages", None) {
            if let Some(l) = v.get("languages").and_then(|p| p.as_array()) {
                self.settings.languages_text = l
                    .iter()
                    .filter_map(|x| x.as_str())
                    .collect::<Vec<_>>()
                    .join(", ");
            }
        }
        if self.content.import_group.is_empty() {
            if let Some(g) = self.groups.first() {
                self.content.import_group =
                    g.get("id").and_then(|x| x.as_str()).unwrap_or("").to_string();
            }
        }
    }

    fn group_name(&self, id: &str) -> String {
        self.groups
            .iter()
            .find(|g| g.get("id").and_then(|x| x.as_str()) == Some(id))
            .and_then(|g| g.get("name").and_then(|x| x.as_str()))
            .unwrap_or(id)
            .to_string()
    }

    fn folder_name(&self, id: &str) -> String {
        self.folders
            .iter()
            .find(|f| f.get("id").and_then(|x| x.as_str()) == Some(id))
            .and_then(|f| f.get("name").and_then(|x| x.as_str()))
            .unwrap_or("")
            .to_string()
    }

    fn role(&self) -> String {
        self.user
            .as_ref()
            .and_then(|u| u.get("role"))
            .and_then(|r| r.as_str())
            .unwrap_or("member")
            .to_string()
    }

    fn open_unit_detail(&mut self, unit_id: &str) {
        self.content.selected_unit = Some(unit_id.to_string());
        match self.call("GET", &format!("/v1/units/{unit_id}"), None) {
            Ok(v) => {
                // default the publish target to the unit's first rendition
                if let Some(first) = v
                    .get("renditions")
                    .and_then(|r| r.as_array())
                    .and_then(|a| a.first())
                    .and_then(|r| r.get("rendition"))
                {
                    self.content.publish_form = s(first, "form");
                    self.content.publish_lang = s(first, "lang");
                }
                self.content.detail = Some(v);
                self.content.bind_from.clear();
                self.content.bind_to.clear();
                self.content.bind_from_slot = None;
                self.content.bindings = self
                    .call("GET", &format!("/v1/units/{unit_id}/page-bindings"), None)
                    .ok()
                    .and_then(|b| {
                        b.get("bindings").and_then(|x| x.as_array()).cloned()
                    })
                    .unwrap_or_default();
            }
            Err(e) => self.msg = format!("打开失败：{e}"),
        }
    }

    fn open_index_detail(&mut self, index_id: &str) {
        self.index.selected = Some(index_id.to_string());
        self.load_index_detail();
    }

    fn load_index_detail(&mut self) {
        let Some(id) = self.index.selected.clone() else {
            return;
        };
        let branch = if self.index.branch.is_empty() {
            "main".to_string()
        } else {
            self.index.branch.clone()
        };
        match self.call("GET", &format!("/v1/indexes/{id}?branch={branch}"), None) {
            Ok(v) => self.index.detail = Some(v),
            Err(e) => self.msg = format!("打开索引失败：{e}"),
        }
        // prefetch version history per rendition for the quick-switch strips
        self.index.unit_versions.clear();
        let mut seen_units: Vec<String> = Vec::new();
        if let Some(d) = self.index.detail.clone() {
            if let Some(arr) = d.get("resolved_entries").and_then(|r| r.as_array()) {
                for re in arr {
                    if let Some(e) = re.get("entry") {
                        let uid = s(e, "unit_id");
                        if !uid.is_empty() && !seen_units.contains(&uid) {
                            seen_units.push(uid);
                        }
                    }
                }
            }
        }
        for uid in seen_units {
            if let Ok(u) = self.call("GET", &format!("/v1/units/{uid}"), None) {
                if let Some(rends) = u.get("renditions").and_then(|r| r.as_array()) {
                    for r in rends {
                        let rid = r
                            .get("rendition")
                            .map(|x| s(x, "id"))
                            .unwrap_or_default();
                        let mut vs: Vec<Value> = r
                            .get("versions")
                            .and_then(|v| v.as_array())
                            .cloned()
                            .unwrap_or_default()
                            .into_iter()
                            .filter(|v| {
                                let st = s(v, "status");
                                st.is_empty() || st == "normal"
                            })
                            .collect();
                        vs.sort_by(|a, b| semver_key(&s(b, "semver")).cmp(&semver_key(&s(a, "semver"))));
                        self.index.unit_versions.insert(rid, vs);
                    }
                }
            }
        }
        if let Ok(v) = self.call("GET", &format!("/v1/indexes/{id}/commits?branch={branch}"), None) {
            self.index.commits = Some(v);
        }
        if let Ok(v) = self.call("GET", &format!("/v1/exports?index_id={id}"), None) {
            self.index.exports = Some(v);
        }
    }

    fn commit_entries(&mut self, entries: Vec<Value>, message: &str) {
        let Some(id) = self.index.selected.clone() else {
            return;
        };
        let branch = if self.index.branch.is_empty() {
            "main".to_string()
        } else {
            self.index.branch.clone()
        };
        let r = self.call(
            "POST",
            &format!("/v1/indexes/{id}/commits"),
            Some(json!({ "branch": branch, "message": message, "entries": entries })),
        );
        match r {
            Ok(_) => {
                self.msg = "已提交".into();
                self.load_index_detail();
            }
            Err(e) => self.msg = format!("提交失败：{e}"),
        }
    }

    fn current_entries(&self) -> Vec<Value> {
        self.index
            .detail
            .as_ref()
            .and_then(|d| d.get("resolved_entries"))
            .and_then(|r| r.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|re| re.get("entry").cloned())
                    .collect()
            })
            .unwrap_or_default()
    }
}

// ---------------- UI ----------------

fn s(v: &Value, key: &str) -> String {
    v.get(key).and_then(|x| x.as_str()).unwrap_or("").to_string()
}

fn i(v: &Value, key: &str) -> i64 {
    v.get(key).and_then(|x| x.as_i64()).unwrap_or(0)
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.frames += 1;
        // Collect finished thumbnail fetches and upload them as textures.
        while let Ok((key, bytes)) = self.thumb_rx.try_recv() {
            let state = bytes
                .and_then(|b| image::load_from_memory(&b).ok())
                .map(|img| {
                    let rgba = img.to_rgba8();
                    let (w, h) = rgba.dimensions();
                    let ci = egui::ColorImage::from_rgba_unmultiplied(
                        [w as usize, h as usize],
                        rgba.as_raw(),
                    );
                    ThumbState::Ready(ctx.load_texture(&key, ci, egui::TextureOptions::LINEAR))
                })
                .unwrap_or(ThumbState::Failed);
            self.thumbs.insert(key, state);
            ctx.request_repaint();
        }
        // Self-screenshot mode for headless verification (QIMU_SCREENSHOT).
        if let Some(path) = self.screenshot_path.clone() {
            if !self.screenshot_done && self.frames == 240 {
                ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot);
            }
            if !self.screenshot_done {
                for ev in ctx.input(|inp| inp.events.clone()) {
                    if let egui::Event::Screenshot { image, .. } = ev {
                        if let Some(buf) = image::RgbaImage::from_raw(
                            image.width() as u32,
                            image.height() as u32,
                            image.as_raw().to_vec(),
                        ) {
                            let _ = buf.save(&path);
                        }
                        self.screenshot_done = true;
                        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                    }
                }
            }
            ctx.request_repaint();
        }

        if self.token.is_none() {
            egui::CentralPanel::default()
                .frame(egui::Frame::none().fill(APP_BG))
                .show(ctx, |ui| {
                    ui.vertical_centered(|ui| {
                        ui.add_space(110.0);
                        card_frame().show(ui, |ui| {
                            ui.set_width(320.0);
                            ui.vertical_centered(|ui| {
                                ui.label(
                                    egui::RichText::new("启幕")
                                        .strong()
                                        .size(26.0)
                                        .color(ACCENT),
                                );
                                ui.label(
                                    egui::RichText::new("PPT 内容管理与更新系统")
                                        .color(TEXT_WEAK),
                                );
                            });
                            ui.add_space(14.0);
                            egui::Grid::new("login_grid")
                                .num_columns(2)
                                .spacing([10.0, 10.0])
                                .show(ui, |ui| {
                                    ui.label("服务器");
                                    ui.add(
                                        egui::TextEdit::singleline(&mut self.server)
                                            .desired_width(220.0),
                                    );
                                    ui.end_row();
                                    ui.label("账号");
                                    ui.add(
                                        egui::TextEdit::singleline(&mut self.username)
                                            .desired_width(220.0),
                                    );
                                    ui.end_row();
                                    ui.label("密码");
                                    ui.add(
                                        egui::TextEdit::singleline(&mut self.password)
                                            .password(true)
                                            .desired_width(220.0),
                                    );
                                    ui.end_row();
                                });
                            ui.add_space(12.0);
                            ui.vertical_centered(|ui| {
                                let login_btn = egui::Button::new(
                                    egui::RichText::new("登录").color(egui::Color32::WHITE),
                                )
                                .fill(ACCENT)
                                .min_size(egui::vec2(180.0, 32.0));
                                if ui.add(login_btn).clicked() {
                                    self.do_login();
                                }
                            });
                        });
                        if !self.msg.is_empty() {
                            ui.add_space(10.0);
                            ui.label(egui::RichText::new(&self.msg).color(TEXT_WEAK));
                        }
                    });
                });
            return;
        }

        egui::TopBottomPanel::top("top")
            .frame(
                egui::Frame::none()
                    .fill(CHROME)
                    .inner_margin(egui::Margin {
                        left: 14.0,
                        right: 14.0,
                        top: 8.0,
                        bottom: 8.0,
                    })
                    .stroke(egui::Stroke::new(1.0, SEPARATOR)),
            )
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.label(
                        egui::RichText::new("启幕")
                            .strong()
                            .size(18.0)
                            .color(TEXT_MAIN),
                    );
                    ui.add_space(10.0);
                    for (page, label) in [
                        (Page::Showroom, "演示厅"),
                        (Page::Content, "内容页"),
                        (Page::Index, "索引页"),
                        (Page::Settings, "设置"),
                    ] {
                        if pill(ui, self.page == page, label).clicked() {
                            self.page = page;
                        }
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.button("退出").clicked() {
                            let _ = self.call("POST", "/v1/auth/logout", None);
                            self.token = None;
                            self.user = None;
                        }
                        if ui.button("刷新").clicked() {
                            self.refresh_all();
                        }
                        let name = self
                            .user
                            .as_ref()
                            .map(|u| s(u, "display_name"))
                            .unwrap_or_default();
                        ui.label(
                            egui::RichText::new(format!("{name} · {}", self.role()))
                                .color(TEXT_WEAK),
                        );
                    });
                });
            });
        egui::TopBottomPanel::bottom("status")
            .frame(
                egui::Frame::none()
                    .fill(CHROME)
                    .inner_margin(egui::Margin {
                        left: 14.0,
                        right: 14.0,
                        top: 5.0,
                        bottom: 5.0,
                    })
                    .stroke(egui::Stroke::new(1.0, SEPARATOR)),
            )
            .show(ctx, |ui| {
                ui.label(egui::RichText::new(&self.msg).small().color(TEXT_WEAK));
            });

        egui::CentralPanel::default()
            .frame(
                egui::Frame::none()
                    .fill(APP_BG)
                    .inner_margin(egui::Margin::same(12.0)),
            )
            .show(ctx, |ui| match self.page {
                Page::Showroom => self.page_showroom(ui),
                Page::Content => self.page_content(ui),
                Page::Index => self.page_index(ui),
                Page::Settings => self.page_settings(ui),
            });
    }
}

impl App {
    /// Real slide thumbnail, fetched on a background thread (the server
    /// renders it once via LibreOffice and caches it on disk).
    fn thumb(&mut self, version_id: &str, page: usize) -> Option<egui::TextureHandle> {
        if version_id.is_empty() {
            return None;
        }
        let key = format!("{version_id}:{page}");
        match self.thumbs.get(&key) {
            Some(ThumbState::Ready(t)) => Some(t.clone()),
            Some(_) => None,
            None => {
                self.thumbs.insert(key.clone(), ThumbState::Loading);
                let (server, token, tx) = (
                    self.server.clone(),
                    self.token.clone(),
                    self.thumb_tx.clone(),
                );
                let url = format!("{server}/v1/versions/{version_id}/thumb/{page}");
                std::thread::spawn(move || {
                    let mut req = ureq::get(&url);
                    if let Some(t) = &token {
                        req = req.set("Authorization", &format!("Bearer {t}"));
                    }
                    let out = req
                        .call()
                        .ok()
                        .and_then(|r| r.into_json::<Value>().ok())
                        .and_then(|v| {
                            v.get("image_base64")
                                .and_then(|x| x.as_str())
                                .map(str::to_string)
                        })
                        .and_then(|s| b64d(&s).ok());
                    let _ = tx.send((key, out));
                });
                None
            }
        }
    }

    fn thumb_ui(&mut self, ui: &mut egui::Ui, version_id: &str, page: usize, width: f32) {
        let h = width * 9.0 / 16.0;
        if let Some(tex) = self.thumb(version_id, page) {
            ui.image((tex.id(), egui::vec2(width, h)));
        } else {
            ui.allocate_ui(egui::vec2(width, h), |ui| {
                egui::Frame::none()
                    .fill(HOVER_BG)
                    .show(ui, |ui| {
                        ui.set_min_size(egui::vec2(width, h));
                        ui.centered_and_justified(|ui| {
                            ui.label(
                                egui::RichText::new(format!("第 {} 页", page + 1))
                                    .color(TEXT_FAINT),
                            );
                        });
                    });
            });
        }
    }

    /// Clickable thumbnail (quick-switch strip / version popup). The
    /// currently resolved version gets an accent outline.
    fn thumb_button(
        &mut self,
        ui: &mut egui::Ui,
        version_id: &str,
        page: usize,
        width: f32,
        current: bool,
    ) -> bool {
        let h = width * 9.0 / 16.0;
        let (rect, resp) =
            ui.allocate_exact_size(egui::vec2(width, h), egui::Sense::click());
        if let Some(tex) = self.thumb(version_id, page) {
            ui.painter().image(
                tex.id(),
                rect,
                egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                egui::Color32::WHITE,
            );
        } else {
            ui.painter().rect_filled(rect, 0.0, HOVER_BG);
        }
        if current {
            ui.painter()
                .rect_stroke(rect, 0.0, egui::Stroke::new(1.5, ACCENT));
        } else if resp.hovered() {
            ui.painter()
                .rect_stroke(rect, 0.0, egui::Stroke::new(1.0, TEXT_WEAK));
        }
        resp.clicked()
    }

    /// Switch one index entry to a specific version: picking the latest
    /// restores follow-latest; a historical pick locks the entry.
    fn switch_entry_version(&mut self, pos: usize, version_id: &str, semver: &str, is_latest: bool) {
        let mut es = self.current_entries();
        if pos >= es.len() {
            return;
        }
        if is_latest {
            es[pos]["policy"] = json!("follow_latest");
            es[pos]["locked_semver"] = Value::Null;
        } else {
            es[pos]["policy"] = json!({ "locked": { "version_id": version_id } });
            es[pos]["locked_semver"] = json!(semver);
        }
        self.index.more_for = None;
        self.commit_entries(es, "切换版本");
    }

    /// Export the open index to a user-chosen file. kind: personal|company
    /// (§3.14 — the server resolves or blocks on personal placeholders).
    fn export_index_file(&mut self, index: &Value, kind: &str) {
        let Some(id) = self.index.selected.clone() else {
            return;
        };
        let Some(path) = rfd::FileDialog::new()
            .set_file_name(format!("{}.pptx", s(index, "name")))
            .save_file()
        else {
            return;
        };
        let r = self.call(
            "POST",
            &format!("/v1/indexes/{id}/export"),
            Some(json!({
                "branch": self.index.branch,
                "kind": kind,
                "include_notes": self.index.include_notes,
            })),
        );
        match r {
            Ok(v) => {
                if let Some(data) = v.get("data_base64").and_then(|x| x.as_str()) {
                    if let Ok(bytes) = b64d(data) {
                        let _ = std::fs::write(&path, bytes);
                        self.msg = format!(
                            "已导出 {} 页（{}）",
                            v.get("page_count").and_then(|x| x.as_i64()).unwrap_or(0),
                            if kind == "company" { "公用版" } else { "个人版" }
                        );
                        self.load_index_detail();
                    }
                }
            }
            Err(e) => self.msg = format!("导出失败：{e}"),
        }
    }

    /// Write an adopted snapshot into the local offline cache (§3.17).
    /// Administrator policy wins: caching can be disabled server-side and
    /// is capacity-capped; snapshots are stored AES-256-GCM encrypted.
    fn cache_deck(&mut self, key: &str, title: &str, bytes: &[u8], page_count: i64) {
        if self.cache_policy.get("allowed").and_then(|v| v.as_bool()) == Some(false) {
            self.msg = "管理员已关闭离线缓存".into();
            return;
        }
        let max_mb = self
            .cache_policy
            .get("max_mb")
            .and_then(|v| v.as_i64())
            .unwrap_or(500);
        let used: i64 = self
            .cache_entries
            .iter()
            .filter(|e| s(e, "key") != key)
            .map(|e| i(e, "bytes"))
            .sum();
        if used + bytes.len() as i64 > max_mb * 1024 * 1024 {
            self.msg = format!("超过管理员设定的缓存容量上限（{max_mb} MB）");
            return;
        }
        let Some(ck) = cache_key() else {
            self.msg = "无法创建本地缓存密钥".into();
            return;
        };
        let Some(enc) = encrypt_bytes(&ck, bytes) else {
            self.msg = "缓存加密失败".into();
            return;
        };
        let dir = cache_dir();
        let _ = std::fs::create_dir_all(&dir);
        let safe: String = key
            .chars()
            .map(|c| {
                if c.is_alphanumeric() || c == '-' || c == '_' {
                    c
                } else {
                    '_'
                }
            })
            .collect();
        let file = format!("{safe}.pptx.enc");
        match std::fs::write(dir.join(&file), &enc) {
            Ok(_) => {
                self.cache_entries.retain(|e| s(e, "key") != key);
                self.cache_entries.push(json!({
                    "key": key,
                    "title": title,
                    "page_count": page_count,
                    "file": file,
                    "bytes": bytes.len() as i64,
                    "enc": true,
                    "cached_ms": qimu_core::auth::now_ms(),
                }));
                save_cache_index(&self.cache_entries);
                self.msg = format!("已缓存到本地（加密）：{title}");
            }
            Err(e) => self.msg = format!("缓存失败：{e}"),
        }
    }

    fn remove_cache(&mut self, key: &str) {
        if let Some(e) = self
            .cache_entries
            .iter()
            .find(|e| s(e, "key") == key)
            .cloned()
        {
            let _ = std::fs::remove_file(cache_dir().join(s(&e, "file")));
        }
        self.cache_entries.retain(|e| s(e, "key") != key);
        save_cache_index(&self.cache_entries);
        self.msg = "已移除缓存".into();
    }

    fn open_cache(&mut self, key: &str) {
        let Some(e) = self
            .cache_entries
            .iter()
            .find(|e| s(e, "key") == key)
            .cloned()
        else {
            return;
        };
        let stored = cache_dir().join(s(&e, "file"));
        // encrypted entries are decrypted to a scratch copy for opening
        let path = if e.get("enc").and_then(|v| v.as_bool()) == Some(true) {
            let Some(ck) = cache_key() else {
                self.msg = "缺少本地缓存密钥，无法打开".into();
                return;
            };
            let data = match std::fs::read(&stored).ok().and_then(|d| decrypt_bytes(&ck, &d)) {
                Some(d) => d,
                None => {
                    self.msg = "缓存解密失败".into();
                    return;
                }
            };
            let open_dir = cache_dir().join("open");
            let _ = std::fs::create_dir_all(&open_dir);
            let p = open_dir.join(format!("{}.pptx", s(&e, "title")));
            if std::fs::write(&p, data).is_err() {
                self.msg = "写出临时副本失败".into();
                return;
            }
            p
        } else {
            stored
        };
        let r = if cfg!(target_os = "windows") {
            std::process::Command::new("cmd")
                .args(["/C", "start", ""])
                .arg(&path)
                .spawn()
        } else if cfg!(target_os = "macos") {
            std::process::Command::new("open").arg(&path).spawn()
        } else {
            std::process::Command::new("xdg-open").arg(&path).spawn()
        };
        self.msg = match r {
            Ok(_) => format!("已打开本地副本：{}", path.display()),
            Err(e) => format!("打开失败：{e}（文件在 {}）", path.display()),
        };
    }

    /// 开始演示 (§3.17/场景 38): materialize the deck and open it in the
    /// system presentation app. (DeckCraft fullscreen takes over here
    /// once it is deployed on the machine.)
    fn present_showroom(&mut self, item_id: &str, title: &str) {
        match self.call(
            "POST",
            &format!("/v1/showroom/{item_id}/export"),
            Some(json!({ "kind": "personal" })),
        ) {
            Ok(v) => {
                if let Some(data) = v.get("data_base64").and_then(|x| x.as_str()) {
                    if let Ok(bytes) = b64d(data) {
                        let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
                        let dir = std::path::Path::new(&home).join(".qimu").join("present");
                        let _ = std::fs::create_dir_all(&dir);
                        let path = dir.join(format!("{title}.pptx"));
                        match std::fs::write(&path, bytes) {
                            Ok(_) => match spawn_open(&path) {
                                Ok(_) => {
                                    self.msg = format!("已打开演示：{title}（系统默认演示应用）")
                                }
                                Err(e) => {
                                    self.msg = format!("演示文件已生成（{}），打开失败：{e}", path.display())
                                }
                            },
                            Err(e) => self.msg = format!("生成演示文件失败：{e}"),
                        }
                    }
                }
            }
            Err(e) => self.msg = format!("演示失败：{e}"),
        }
    }

    fn library_view(&mut self, ui: &mut egui::Ui) {
        ui.label(egui::RichText::new("我的收藏").strong());
        if self.library_refs.is_empty() {
            ui.label(egui::RichText::new("暂无收藏。从收到的分享里保存进来。").weak());
        }
        for e in self.library_refs.clone() {
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new(s(&e, "title")).strong());
                let folder = s(&e, "folder");
                if !folder.is_empty() {
                    ui.label(egui::RichText::new(folder).small().weak());
                }
                if s(&e, "item_kind") == "showroom" {
                    if ui.small_button("导出 PPT").clicked() {
                        let item_id = s(&e, "item_id");
                        let title = s(&e, "title");
                        if let Some(path) = rfd::FileDialog::new()
                            .set_file_name(format!("{title}.pptx"))
                            .save_file()
                        {
                            match self.call(
                                "POST",
                                &format!("/v1/showroom/{item_id}/export"),
                                Some(json!({ "kind": "personal" })),
                            ) {
                                Ok(v) => {
                                    if let Some(data) =
                                        v.get("data_base64").and_then(|x| x.as_str())
                                    {
                                        if let Ok(bytes) = b64d(data) {
                                            let _ = std::fs::write(&path, bytes);
                                            self.msg = "已导出".into();
                                        }
                                    }
                                }
                                Err(e2) => self.msg = format!("导出失败：{e2}"),
                            }
                        }
                    }
                }
                if ui.small_button("移除").clicked() {
                    let lid = s(&e, "id");
                    match self.call(
                        "POST",
                        &format!("/v1/library/{lid}/delete"),
                        Some(json!({})),
                    ) {
                        Ok(_) => {
                            self.msg = "已移除".into();
                            self.refresh_all();
                        }
                        Err(e2) => self.msg = format!("移除失败：{e2}"),
                    }
                }
            });
        }
        ui.separator();
        ui.label(egui::RichText::new("本地缓存（离线可用）").strong());
        if self.cache_entries.is_empty() {
            ui.label(egui::RichText::new("暂无缓存。在广场或我的组装里点「缓存到本地」。").weak());
        }
        for e in self.cache_entries.clone() {
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new(s(&e, "title")).strong());
                ui.label(format!(
                    "{} 页 · {}",
                    i(&e, "page_count"),
                    fmt_time(i(&e, "cached_ms"))
                ));
                let key = s(&e, "key");
                if ui.button("打开").clicked() {
                    self.open_cache(&key);
                }
                if ui.button("移除缓存").clicked() {
                    self.remove_cache(&key);
                }
            });
        }
        ui.separator();
        ui.label(egui::RichText::new("我的组装").strong());
        if self.my_assemblies.is_empty() {
            ui.label(egui::RichText::new("暂无组装").weak());
        }
        for a in self.my_assemblies.clone() {
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new(s(&a, "name")).strong());
                ui.label(format!(
                    "{} 个条目 · {}",
                    i(&a, "entry_count"),
                    fmt_time(i(&a, "created_ms"))
                ));
                if ui.button("缓存到本地").clicked() {
                    let aid = s(&a, "id");
                    let name = s(&a, "name");
                    match self.call(
                        "POST",
                        &format!("/v1/assemblies/{aid}/export"),
                        Some(json!({ "kind": "personal" })),
                    ) {
                        Ok(v) => {
                            if let Some(data) = v.get("data_base64").and_then(|x| x.as_str()) {
                                if let Ok(bytes) = b64d(data) {
                                    let pages = i(&v, "page_count");
                                    self.cache_deck(&aid, &name, &bytes, pages);
                                }
                            }
                        }
                        Err(e) => self.msg = format!("缓存失败：{e}"),
                    }
                }
            });
        }
    }

    fn share_panel(&mut self, ui: &mut egui::Ui) {
        let item_id = self.share_for.clone().unwrap_or_default();
        let title = self
            .showroom_live
            .iter()
            .find(|x| s(x, "id") == item_id)
            .map(|x| s(x, "title"))
            .unwrap_or_default();
        card_frame().show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new(format!("分享「{title}」")).strong());
                if ui.small_button("关闭").clicked() {
                    self.share_for = None;
                }
            });
            ui.horizontal(|ui| {
                for (k, label) in [("user", "给个人"), ("dept", "给部门"), ("group", "给群组")] {
                    if ui.selectable_label(self.share_kind == k, label).clicked() {
                        self.share_kind = k.into();
                        self.share_target.clear();
                    }
                }
            });
            let users = self
                .org
                .get("users")
                .and_then(|u| u.as_array())
                .cloned()
                .unwrap_or_default();
            let depts = self
                .org
                .get("departments")
                .and_then(|u| u.as_array())
                .cloned()
                .unwrap_or_default();
            let (options, current): (Vec<(String, String)>, String) = match self.share_kind.as_str() {
                "dept" => (
                    depts.iter().map(|d| (s(d, "id"), s(d, "name"))).collect(),
                    depts
                        .iter()
                        .find(|d| s(d, "id") == self.share_target)
                        .map(|d| s(d, "name"))
                        .unwrap_or_else(|| "选择部门".into()),
                ),
                "group" => (
                    self.my_teams.iter().map(|g| (s(g, "id"), s(g, "name"))).collect(),
                    self.my_teams
                        .iter()
                        .find(|g| s(g, "id") == self.share_target)
                        .map(|g| s(g, "name"))
                        .unwrap_or_else(|| "选择群组".into()),
                ),
                _ => (
                    users.iter().map(|u| (s(u, "id"), s(u, "display_name"))).collect(),
                    users
                        .iter()
                        .find(|u| s(u, "id") == self.share_target)
                        .map(|u| s(u, "display_name"))
                        .unwrap_or_else(|| "选择同事".into()),
                ),
            };
            ui.horizontal(|ui| {
                egui::ComboBox::from_id_salt("share_target")
                    .selected_text(current)
                    .show_ui(ui, |ui| {
                        for (id, name) in options {
                            ui.selectable_value(&mut self.share_target, id, name);
                        }
                    });
                if ui.button("发送分享").clicked() && !self.share_target.is_empty() {
                    match self.call(
                        "POST",
                        "/v1/shares",
                        Some(json!({
                            "item_kind": "showroom", "item_id": item_id,
                            "target_kind": self.share_kind, "target_id": self.share_target,
                        })),
                    ) {
                        Ok(_) => {
                            self.msg = "已分享".into();
                            self.share_for = None;
                            self.refresh_all();
                        }
                        Err(e) => self.msg = format!("分享失败：{e}"),
                    }
                }
            });
        });
        ui.add_space(6.0);
    }

    fn inbox_view(&mut self, ui: &mut egui::Ui) {
        if self.inbox.is_empty() {
            ui.label(egui::RichText::new("暂无收到的分享").weak());
            return;
        }
        for sh in self.inbox.clone() {
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new(s(&sh, "title")).strong());
                ui.label(
                    egui::RichText::new(format!(
                        "{} · {}",
                        s(&sh, "from_name"),
                        fmt_time(i(&sh, "created_ms"))
                    ))
                    .small()
                    .weak(),
                );
                if sh.get("in_library").and_then(|v| v.as_bool()) == Some(true) {
                    ui.label(egui::RichText::new("已在我的库").weak());
                } else if ui.small_button("保存到我的库").clicked() {
                    let sid = s(&sh, "id");
                    match self.call(
                        "POST",
                        &format!("/v1/shares/{sid}/accept"),
                        Some(json!({})),
                    ) {
                        Ok(_) => {
                            self.msg = "已保存到我的库".into();
                            self.refresh_all();
                        }
                        Err(e) => self.msg = format!("保存失败：{e}"),
                    }
                }
            });
        }
    }

    fn groups_view(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.label("新建群组");
            ui.text_edit_singleline(&mut self.new_team_name);
            if ui.button("建组").clicked() && !self.new_team_name.trim().is_empty() {
                let name = self.new_team_name.clone();
                match self.call("POST", "/v1/custom-groups", Some(json!({ "name": name }))) {
                    Ok(_) => {
                        self.new_team_name.clear();
                        self.msg = "群组已创建".into();
                        self.refresh_all();
                    }
                    Err(e) => self.msg = format!("建组失败：{e}"),
                }
            }
        });
        ui.add_space(6.0);
        let users = self
            .org
            .get("users")
            .and_then(|u| u.as_array())
            .cloned()
            .unwrap_or_default();
        egui::ScrollArea::vertical().show(ui, |ui| {
            for space in self.group_spaces.clone() {
                let g = space.get("group").cloned().unwrap_or(json!({}));
                let gid = s(&g, "id");
                ui.label(
                    egui::RichText::new(format!(
                        "{} · 组长 {}",
                        s(&g, "name"),
                        s(&g, "owner_name")
                    ))
                    .strong(),
                );
                let members = g
                    .get("members")
                    .and_then(|m| m.as_array())
                    .map(|arr| {
                        arr.iter()
                            .map(|m| s(m, "name"))
                            .collect::<Vec<_>>()
                            .join("、")
                    })
                    .unwrap_or_default();
                ui.label(egui::RichText::new(format!("成员：{members}")).small().weak());
                // owner can add members
                if s(&g, "owner_id") == self.my_id {
                    ui.horizontal(|ui| {
                        egui::ComboBox::from_id_salt(format!("tm_{gid}"))
                            .selected_text(
                                users
                                    .iter()
                                    .find(|u| s(u, "id") == self.team_member_pick)
                                    .map(|u| s(u, "display_name"))
                                    .unwrap_or_else(|| "选择同事".into()),
                            )
                            .show_ui(ui, |ui| {
                                for u in &users {
                                    ui.selectable_value(
                                        &mut self.team_member_pick,
                                        s(u, "id"),
                                        s(u, "display_name"),
                                    );
                                }
                            });
                        if ui.small_button("加成员").clicked()
                            && !self.team_member_pick.is_empty()
                        {
                            let pick = self.team_member_pick.clone();
                            match self.call(
                                "POST",
                                &format!("/v1/custom-groups/{gid}/members"),
                                Some(json!({ "user_id": pick })),
                            ) {
                                Ok(_) => {
                                    self.msg = "成员已加入".into();
                                    self.refresh_all();
                                }
                                Err(e) => self.msg = format!("加成员失败：{e}"),
                            }
                        }
                    });
                }
                if let Some(items) = space.get("items").and_then(|v| v.as_array()) {
                    if items.is_empty() {
                        ui.label(egui::RichText::new("暂无分享到本组的内容").weak());
                    }
                    for it in items {
                        ui.horizontal(|ui| {
                            ui.label(s(it, "title"));
                            ui.label(
                                egui::RichText::new(format!(
                                    "{} · {}",
                                    s(it, "from_name"),
                                    fmt_time(i(it, "created_ms"))
                                ))
                                .small()
                                .weak(),
                            );
                            if ui.small_button("保存到我的库").clicked() {
                                let sid = s(it, "id");
                                match self.call(
                                    "POST",
                                    &format!("/v1/shares/{sid}/accept"),
                                    Some(json!({})),
                                ) {
                                    Ok(_) => {
                                        self.msg = "已保存到我的库".into();
                                        self.refresh_all();
                                    }
                                    Err(e) => self.msg = format!("保存失败：{e}"),
                                }
                            }
                        });
                    }
                }
                ui.separator();
            }
            if self.group_spaces.is_empty() {
                ui.label(egui::RichText::new("还没加入任何群组").weak());
            }
        });
    }

    fn page_showroom(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.heading("演示厅");
            ui.add_space(6.0);
            for (zone, label) in [
                ("plaza", "广场"),
                ("library", "我的库"),
                ("inbox", "收到的分享"),
                ("groups", "群组空间"),
            ] {
                if pill(ui, self.sr_zone == zone, label).clicked() {
                    self.sr_zone = zone.into();
                }
            }
        });
        ui.add_space(4.0);
        if self.sr_zone == "library" {
            self.library_view(ui);
            return;
        }
        if self.sr_zone == "inbox" {
            self.inbox_view(ui);
            return;
        }
        if self.sr_zone == "groups" {
            self.groups_view(ui);
            return;
        }
        if self.share_for.is_some() {
            self.share_panel(ui);
        }
        ui.add_space(6.0);
        egui::ScrollArea::vertical().show(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                for item in self.showroom_live.clone() {
                    ui.allocate_ui(egui::vec2(300.0, 284.0), |ui| {
                        // allocate_ui inherits the wrapped horizontal layout;
                        // card content must flow top-down
                        ui.with_layout(egui::Layout::top_down(egui::Align::LEFT), |ui| {
                        card_frame_flush().show(ui, |ui| {
                            ui.set_min_size(egui::vec2(300.0, 284.0));
                            let src0 = s(&item, "source_index_id");
                            let cover = self
                                .index_covers
                                .get(&src0)
                                .cloned()
                                .unwrap_or_default();
                            if !cover.is_empty() {
                                self.thumb_ui(ui, &cover, 0, 300.0);
                            }
                            egui::Frame::none()
                                .inner_margin(egui::Margin {
                                    left: 14.0,
                                    right: 14.0,
                                    top: 10.0,
                                    bottom: 12.0,
                                })
                                .show(ui, |ui| {
                            ui.label(
                                egui::RichText::new(s(&item, "title")).strong().size(15.5),
                            );
                            let intro = s(&item, "intro");
                            if !intro.is_empty() {
                                ui.label(
                                    egui::RichText::new(intro).color(TEXT_WEAK),
                                );
                            }
                            ui.label(
                                egui::RichText::new(format!(
                                    "{} · {}",
                                    s(&item, "submitted_by"),
                                    fmt_time(i(&item, "created_ms"))
                                ))
                                .small()
                                .color(TEXT_WEAK),
                            );
                            ui.horizontal(|ui| {
                                if ui.small_button("开始演示").clicked() {
                                    self.present_showroom(&s(&item, "id"), &s(&item, "title"));
                                }
                                if ui.small_button("分享").clicked() {
                                    self.share_for = Some(s(&item, "id"));
                                    self.share_target.clear();
                                }
                            });
                            let src = s(&item, "source_index_id");
                            if !src.is_empty() {
                                if ui.small_button("缓存到本地").clicked() {
                                    let title = s(&item, "title");
                                    let key = format!("sr_{}", s(&item, "id"));
                                    match self.call(
                                        "POST",
                                        &format!("/v1/indexes/{src}/export"),
                                        Some(json!({ "branch": "main", "kind": "personal" })),
                                    ) {
                                        Ok(v) => {
                                            if let Some(data) =
                                                v.get("data_base64").and_then(|x| x.as_str())
                                            {
                                                if let Ok(bytes) = b64d(data) {
                                                    let pages = i(&v, "page_count");
                                                    self.cache_deck(&key, &title, &bytes, pages);
                                                }
                                            }
                                        }
                                        Err(e) => self.msg = format!("缓存失败：{e}"),
                                    }
                                }
                            }
                            });
                        });
                        });
                    });
                }
                if self.showroom_live.is_empty() {
                    ui.label("暂无内容。从索引页把成稿推送到演示厅，或在下方提交。");
                }
            });
        });
        ui.separator();
        ui.horizontal(|ui| {
            ui.label("提交到演示厅");
            ui.text_edit_singleline(&mut self.sr_title);
            egui::ComboBox::from_id_salt("sr_index")
                .selected_text(
                    self.indexes
                        .iter()
                        .find(|x| s(x, "id") == self.sr_index)
                        .map(|x| s(x, "name"))
                        .unwrap_or_else(|| "选择来源索引".into()),
                )
                .show_ui(ui, |ui| {
                    for x in self.indexes.clone() {
                        ui.selectable_value(&mut self.sr_index, s(&x, "id"), s(&x, "name"));
                    }
                });
            if ui.button("提交审核").clicked() {
                let r = self.call(
                    "POST",
                    "/v1/showroom",
                    Some(json!({
                        "title": self.sr_title,
                        "intro": "",
                        "source_index_id": if self.sr_index.is_empty() { None } else { Some(self.sr_index.clone()) },
                    })),
                );
                match r {
                    Ok(_) => {
                        self.msg = "已提交，等待审核".into();
                        self.refresh_all();
                    }
                    Err(e) => self.msg = format!("提交失败：{e}"),
                }
            }
        });
        if self.role() == "super" && !self.showroom_pending.is_empty() {
            ui.separator();
            ui.heading("待审队列");
            for item in self.showroom_pending.clone() {
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new(s(&item, "title")).strong());
                    ui.label(format!("提交人 {}", s(&item, "submitted_by")));
                    if ui.button("通过").clicked() {
                        let _ = self.call(
                            "POST",
                            &format!("/v1/showroom/{}/review", s(&item, "id")),
                            Some(json!({ "state": "live" })),
                        );
                        self.refresh_all();
                    }
                    if ui.button("驳回").clicked() {
                        let _ = self.call(
                            "POST",
                            &format!("/v1/showroom/{}/review", s(&item, "id")),
                            Some(json!({ "state": "rejected" })),
                        );
                        self.refresh_all();
                    }
                });
            }
        }
    }

    fn page_content(&mut self, ui: &mut egui::Ui) {
        if self.content.detail.is_some() {
            self.unit_detail(ui);
            return;
        }
        egui::SidePanel::left("folders")
            .resizable(false)
            .default_width(160.0)
            .show_inside(ui, |ui| {
                if ui
                    .selectable_label(self.content.folder_filter.is_none(), "全部")
                    .clicked()
                {
                    self.content.folder_filter = None;
                }
                for f in self.folders.clone() {
                    let fid = s(&f, "id");
                    let indent = if f.get("parent_id").is_some() { "　" } else { "" };
                    if ui
                        .selectable_label(
                            self.content.folder_filter.as_deref() == Some(fid.as_str()),
                            format!("{indent}{}", s(&f, "name")),
                        )
                        .clicked()
                    {
                        self.content.folder_filter = Some(fid);
                    }
                }
            });
        ui.horizontal(|ui| {
            ui.label("搜索");
            let resp = ui.text_edit_singleline(&mut self.content.search_text);
            if resp.lost_focus() && ui.input(|inp| inp.key_pressed(egui::Key::Enter)) {
                match self.call("GET", &format!("/v1/search?q={}", self.content.search_text), None) {
                    Ok(v) => self.content.search_hits = Some(v),
                    Err(e) => self.msg = e,
                }
            }
            if ui.button("智能分块导入").clicked() {
                if let Some(path) = rfd::FileDialog::new()
                    .add_filter("PowerPoint", &["pptx"])
                    .pick_file()
                {
                    if let Ok(bytes) = std::fs::read(&path) {
                        let name = path
                            .file_name()
                            .map(|n| n.to_string_lossy().to_string())
                            .unwrap_or_default();
                        match self.call(
                            "POST",
                            "/v1/import/propose",
                            Some(json!({ "data_base64": b64e(&bytes) })),
                        ) {
                            Ok(p) => {
                                let blocks = p.get("blocks").cloned().unwrap_or(json!([]));
                                let names: Vec<String> = blocks
                                    .as_array()
                                    .map(|arr| {
                                        arr.iter().map(|b| s(b, "name")).collect()
                                    })
                                    .unwrap_or_default();
                                let r = self.call(
                                    "POST",
                                    "/v1/import",
                                    Some(json!({
                                        "filename": name,
                                        "data_base64": b64e(&bytes),
                                        "folder_id": self.content.folder_filter,
                                        "group_id": self.content.import_group,
                                        "blocks": blocks,
                                    })),
                                );
                                match r {
                                    Ok(_) => {
                                        self.msg = format!(
                                            "智能分块导入完成：{} 块（{}）",
                                            names.len(),
                                            names.join("、")
                                        );
                                        self.refresh_all();
                                    }
                                    Err(e) => self.msg = format!("导入失败：{e}"),
                                }
                            }
                            Err(e) => self.msg = format!("提块失败：{e}"),
                        }
                    }
                }
            }
            if ui.button("导入 PPT").clicked() {
                if let Some(path) = rfd::FileDialog::new()
                    .add_filter("PowerPoint", &["pptx"])
                    .pick_file()
                {
                    if let Ok(bytes) = std::fs::read(&path) {
                        let name = path
                            .file_name()
                            .map(|n| n.to_string_lossy().to_string())
                            .unwrap_or_default();
                        let r = self.call(
                            "POST",
                            "/v1/import",
                            Some(json!({
                                "filename": name,
                                "data_base64": b64e(&bytes),
                                "folder_id": self.content.folder_filter,
                                "group_id": self.content.import_group,
                            })),
                        );
                        match r {
                            Ok(v) => {
                                self.msg = format!(
                                    "导入完成：{} 页，校验通过",
                                    v.get("slide_count").and_then(|x| x.as_i64()).unwrap_or(0)
                                );
                                self.refresh_all();
                            }
                            Err(e) => self.msg = format!("导入失败：{e}"),
                        }
                    }
                }
            }
            ui.label("导入到组");
            let cur_group = self.group_name(&self.content.import_group.clone());
            egui::ComboBox::from_id_salt("import_group")
                .selected_text(cur_group)
                .show_ui(ui, |ui| {
                    for g in self.groups.clone() {
                        ui.selectable_value(
                            &mut self.content.import_group,
                            s(&g, "id"),
                            s(&g, "name"),
                        );
                    }
                });
        });
        ui.horizontal(|ui| {
            ui.label("新建内容块");
            ui.text_edit_singleline(&mut self.content.new_unit_name);
            if ui.button("创建").clicked() && !self.content.new_unit_name.is_empty() {
                let r = self.call(
                    "POST",
                    "/v1/units",
                    Some(json!({
                        "name": self.content.new_unit_name,
                        "group_id": self.content.import_group,
                        "folder_id": self.content.folder_filter,
                    })),
                );
                match r {
                    Ok(_) => {
                        self.content.new_unit_name.clear();
                        self.refresh_all();
                    }
                    Err(e) => self.msg = e,
                }
            }
        });
        ui.separator();
        if let Some(hits) = self.content.search_hits.clone() {
            ui.label(egui::RichText::new("搜索结果").strong());
            if let Some(arr) = hits.get("hits").and_then(|h| h.as_array()) {
                for h in arr {
                    let u = h.get("unit").cloned().unwrap_or(json!({}));
                    let v = h.get("version").cloned().unwrap_or(json!({}));
                    if ui
                        .button(format!("{} · v{}", s(&u, "name"), s(&v, "semver")))
                        .clicked()
                    {
                        self.open_unit_detail(&s(&u, "id"));
                    }
                }
            }
            ui.separator();
        }
        egui::ScrollArea::vertical().show(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                for item in self.units.clone() {
                    let unit = item.get("unit").cloned().unwrap_or(json!({}));
                    if let Some(f) = &self.content.folder_filter {
                        if unit.get("folder_id").and_then(|x| x.as_str()) != Some(f.as_str()) {
                            continue;
                        }
                    }
                    let uid = s(&unit, "id");
                    let rends = item.get("renditions").and_then(|r| r.as_array()).cloned().unwrap_or_default();
                    let latest = rends
                        .first()
                        .and_then(|r| r.get("latest"))
                        .cloned()
                        .unwrap_or(Value::Null);
                    ui.allocate_ui(egui::vec2(300.0, 258.0), |ui| {
                        let resp = ui
                            .with_layout(egui::Layout::top_down(egui::Align::LEFT), |ui| {
                                card_frame_flush().show(ui, |ui| {
                            ui.set_min_size(egui::vec2(300.0, 258.0));
                            if latest.is_object() {
                                let vid = s(&latest, "id");
                                self.thumb_ui(ui, &vid, 0, 300.0);
                            } else {
                                ui.allocate_space(egui::vec2(300.0, 169.0));
                            }
                            egui::Frame::none()
                                .inner_margin(egui::Margin {
                                    left: 14.0,
                                    right: 14.0,
                                    top: 10.0,
                                    bottom: 12.0,
                                })
                                .show(ui, |ui| {
                                    ui.label(
                                        egui::RichText::new(s(&unit, "name"))
                                            .strong()
                                            .size(15.5),
                                    );
                                    ui.add_space(5.0);
                                    ui.horizontal(|ui| {
                                        if latest.is_object() {
                                            chip(
                                                ui,
                                                &format!("v{}", s(&latest, "semver")),
                                                ACCENT_DEEP,
                                                CHIP_BLUE_BG,
                                            );
                                            ui.label(
                                                egui::RichText::new(format!(
                                                    "{} 页",
                                                    i(&latest, "page_count")
                                                ))
                                                .small()
                                                .color(TEXT_WEAK),
                                            );
                                        } else {
                                            ui.label(
                                                egui::RichText::new("暂无版本")
                                                    .small()
                                                    .color(TEXT_WEAK),
                                            );
                                        }
                                        ui.with_layout(
                                            egui::Layout::right_to_left(egui::Align::Center),
                                            |ui| {
                                                ui.label(
                                                    egui::RichText::new(format!(
                                                        "{} · {}",
                                                        self.folder_name(
                                                            unit.get("folder_id")
                                                                .and_then(|x| x.as_str())
                                                                .unwrap_or("")
                                                        ),
                                                        s(&unit, "maintainer")
                                                    ))
                                                    .small()
                                                    .color(TEXT_WEAK),
                                                );
                                            },
                                        );
                                    });
                                });
                                })
                            })
                            .inner;
                        if resp.response.double_clicked() {
                            self.open_unit_detail(&uid);
                        }
                    });
                }
            });
        });
    }

    fn unit_detail(&mut self, ui: &mut egui::Ui) {
        let detail = self.content.detail.clone().unwrap();
        let unit = detail.get("unit").cloned().unwrap_or(json!({}));
        ui.horizontal(|ui| {
            if ui.button("← 返回").clicked() {
                self.content.detail = None;
                self.content.selected_unit = None;
                self.refresh_all();
                return;
            }
            ui.heading(s(&unit, "name"));
            ui.label(format!(
                "内容修订 r{} · 维护人 {}",
                i(&unit, "current_revision"),
                s(&unit, "maintainer")
            ));
        });
        ui.separator();
        egui::ScrollArea::vertical().show(ui, |ui| {
            if let Some(rends) = detail.get("renditions").and_then(|r| r.as_array()) {
                for r in rends.clone() {
                    let rend = r.get("rendition").cloned().unwrap_or(json!({}));
                    ui.label(
                        egui::RichText::new(format!(
                            "{} · {}",
                            s(&rend, "form"),
                            s(&rend, "lang")
                        ))
                        .strong(),
                    );
                    if let Some(versions) = r.get("versions").and_then(|v| v.as_array()) {
                        for v in versions {
                            ui.horizontal(|ui| {
                                let vid = s(v, "id");
                                self.thumb_ui(ui, &vid, 0, 96.0);
                                ui.label(format!(
                                    "v{} · {} 页 · {} · {}",
                                    s(v, "semver"),
                                    i(v, "page_count"),
                                    s(v, "author"),
                                    fmt_time(i(v, "created_ms"))
                                ));
                                if s(v, "status") == "recalled" {
                                    ui.label(
                                        egui::RichText::new("已撤回")
                                            .color(egui::Color32::RED),
                                    );
                                }
                                if !s(v, "note").is_empty() {
                                    ui.label(s(v, "note"));
                                }
                                if ui.button("下载片段").clicked() {
                                    if let Ok(resp) = self.call(
                                        "GET",
                                        &format!("/v1/versions/{}/fragment", s(v, "id")),
                                        None,
                                    ) {
                                        if let Some(data) = resp.get("data_base64").and_then(|x| x.as_str()) {
                                            if let (Ok(bytes), Some(path)) = (
                                                b64d(data),
                                                rfd::FileDialog::new()
                                                    .set_file_name("fragment.pptx")
                                                    .save_file(),
                                            ) {
                                                let _ = std::fs::write(path, bytes);
                                                self.msg = "已保存".into();
                                            }
                                        }
                                    }
                                }
                                if ui.button("讲稿").clicked() {
                                    let mut scripts: Vec<String> = v
                                        .get("scripts")
                                        .and_then(|x| x.as_array())
                                        .map(|arr| {
                                            arr.iter()
                                                .map(|t| t.as_str().unwrap_or("").to_string())
                                                .collect()
                                        })
                                        .unwrap_or_default();
                                    let pages = i(v, "page_count") as usize;
                                    while scripts.len() < pages {
                                        scripts.push(String::new());
                                    }
                                    self.content.scripts_edit = scripts;
                                    self.content.scripts_for = Some(s(v, "id"));
                                }
                                if s(v, "status") != "recalled" && ui.button("撤回").clicked() {
                                    let _ = self.call(
                                        "POST",
                                        &format!("/v1/versions/{}/recall", s(v, "id")),
                                        None,
                                    );
                                    if let Some(uid) = self.content.selected_unit.clone() {
                                        self.open_unit_detail(&uid);
                                    }
                                }
                            });
                        }
                    }
                }
            }
            ui.separator();
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new("AI 翻译（生成草稿）").strong());
                let cur_to = if self.content.translate_to.is_empty() {
                    "zh".to_string()
                } else {
                    self.content.translate_to.clone()
                };
                egui::ComboBox::from_id_salt("translate_to")
                    .selected_text(match cur_to.as_str() {
                        "en" => "英语",
                        "ja" => "日语",
                        _ => "中文",
                    })
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut self.content.translate_to, "zh".into(), "中文");
                        ui.selectable_value(&mut self.content.translate_to, "en".into(), "英语");
                        ui.selectable_value(&mut self.content.translate_to, "ja".into(), "日语");
                    });
                if ui.button("翻译最新版本").clicked() {
                    // latest version of the first rendition in the detail
                    let first_version = self.content.detail.as_ref().and_then(|d| {
                        d.get("renditions")
                            .and_then(|r| r.as_array())
                            .and_then(|arr| arr.first())
                            .and_then(|r| r.get("versions"))
                            .and_then(|v| v.as_array())
                            .and_then(|arr| arr.first())
                            .map(|v| s(v, "id"))
                    });
                    match (self.content.selected_unit.clone(), first_version) {
                        (Some(uid), Some(vid)) => {
                            let to = if self.content.translate_to.is_empty() {
                                "zh".to_string()
                            } else {
                                self.content.translate_to.clone()
                            };
                            match self.call(
                                "POST",
                                &format!("/v1/units/{uid}/translate"),
                                Some(json!({ "version_id": vid, "target_lang": to })),
                            ) {
                                Ok(v) => {
                                    self.msg = format!(
                                        "译文草稿已生成（{} 个文本，待校对）",
                                        i(&v, "translated_runs")
                                    );
                                    self.open_unit_detail(&uid);
                                }
                                Err(e) => self.msg = format!("翻译失败：{e}"),
                            }
                        }
                        _ => self.msg = "暂无可翻译的版本".into(),
                    }
                }
            });
            if let Some(vid) = self.content.scripts_for.clone() {
                ui.separator();
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new("逐页讲稿（保存不升版本）").strong());
                    if ui.button("保存讲稿").clicked() {
                        let scripts = self.content.scripts_edit.clone();
                        match self.call(
                            "PUT",
                            &format!("/v1/versions/{vid}/scripts"),
                            Some(json!({ "scripts": scripts })),
                        ) {
                            Ok(_) => {
                                self.msg = "讲稿已保存".into();
                                if let Some(uid) = self.content.selected_unit.clone() {
                                    self.open_unit_detail(&uid);
                                }
                            }
                            Err(e) => self.msg = format!("保存讲稿失败：{e}"),
                        }
                    }
                    if ui.button("关闭").clicked() {
                        self.content.scripts_for = None;
                    }
                });
                let n = self.content.scripts_edit.len();
                for idx in 0..n {
                    ui.horizontal(|ui| {
                        ui.label(format!("第 {} 页", idx + 1));
                        ui.add(
                            egui::TextEdit::multiline(&mut self.content.scripts_edit[idx])
                                .desired_width(560.0)
                                .desired_rows(2),
                        );
                    });
                }
            }
            ui.separator();
            ui.label(egui::RichText::new("发布新版本").strong());
            ui.horizontal(|ui| {
                if ui.button("选择片段文件").clicked() {
                    self.content.publish_file = rfd::FileDialog::new()
                        .add_filter("PowerPoint", &["pptx"])
                        .pick_file();
                }
                if let Some(p) = &self.content.publish_file {
                    ui.label(p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default());
                }
            });
            ui.horizontal(|ui| {
                ui.label("形态");
                ui.add(egui::TextEdit::singleline(&mut self.content.publish_form).desired_width(80.0));
                ui.label("语言");
                ui.add(egui::TextEdit::singleline(&mut self.content.publish_lang).desired_width(60.0));
                ui.label("升版方式");
                for (idx, label) in [(0usize, "大版本"), (1, "小版本"), (2, "文字调整")] {
                    ui.radio_value(&mut self.content.publish_bump, idx, label);
                }
                ui.label("备注");
                ui.text_edit_singleline(&mut self.content.publish_note);
            });
            if ui.button("发布").clicked() {
                if let (Some(path), Some(uid)) =
                    (self.content.publish_file.clone(), self.content.selected_unit.clone())
                {
                    if let Ok(bytes) = std::fs::read(&path) {
                        let bump = ["major", "minor", "patch"][self.content.publish_bump];
                        let r = self.call(
                            "POST",
                            &format!("/v1/units/{uid}/publish"),
                            Some(json!({
                                "data_base64": b64e(&bytes),
                                "form": self.content.publish_form,
                                "lang": self.content.publish_lang,
                                "bump": bump,
                                "note": self.content.publish_note,
                            })),
                        );
                        match r {
                            Ok(v) => {
                                self.msg = format!("已发布 v{}", s(&v, "semver"));
                                self.open_unit_detail(&uid);
                            }
                            Err(e) => self.msg = format!("发布失败：{e}"),
                        }
                    }
                } else {
                    self.msg = "先选择片段文件".into();
                }
            }
            ui.separator();
            ui.label(egui::RichText::new("语言绑定（逐页）").strong());
            let mut all_versions: Vec<(String, String, i64)> = Vec::new();
            if let Some(rends) = detail.get("renditions").and_then(|r| r.as_array()) {
                for r in rends {
                    let rend = r.get("rendition").cloned().unwrap_or(json!({}));
                    if let Some(vs) = r.get("versions").and_then(|v| v.as_array()) {
                        for v in vs {
                            all_versions.push((
                                s(v, "id"),
                                format!(
                                    "{} · {} · v{}",
                                    s(&rend, "form"),
                                    s(&rend, "lang"),
                                    s(v, "semver")
                                ),
                                i(v, "page_count"),
                            ));
                        }
                    }
                }
            }
            if all_versions.len() >= 2 {
                if self.content.bind_from.is_empty() {
                    self.content.bind_from = all_versions[0].0.clone();
                }
                if self.content.bind_to.is_empty() {
                    self.content.bind_to = all_versions[1].0.clone();
                }
                ui.horizontal(|ui| {
                    ui.label("基准");
                    let cur = all_versions
                        .iter()
                        .find(|v| v.0 == self.content.bind_from)
                        .map(|v| v.1.clone())
                        .unwrap_or_default();
                    egui::ComboBox::from_id_salt("bind_from")
                        .selected_text(cur)
                        .show_ui(ui, |ui| {
                            for v in &all_versions {
                                ui.selectable_value(
                                    &mut self.content.bind_from,
                                    v.0.clone(),
                                    &v.1,
                                );
                            }
                        });
                    ui.label("对应");
                    let cur = all_versions
                        .iter()
                        .find(|v| v.0 == self.content.bind_to)
                        .map(|v| v.1.clone())
                        .unwrap_or_default();
                    egui::ComboBox::from_id_salt("bind_to")
                        .selected_text(cur)
                        .show_ui(ui, |ui| {
                            for v in &all_versions {
                                ui.selectable_value(&mut self.content.bind_to, v.0.clone(), &v.1);
                            }
                        });
                });
                let from_pages = all_versions
                    .iter()
                    .find(|v| v.0 == self.content.bind_from)
                    .map(|v| v.2)
                    .unwrap_or(0);
                let to_pages = all_versions
                    .iter()
                    .find(|v| v.0 == self.content.bind_to)
                    .map(|v| v.2)
                    .unwrap_or(0);
                ui.label("基准页（点选）：");
                ui.horizontal(|ui| {
                    for p in 0..from_pages {
                        ui.vertical(|ui| {
                            let vid = self.content.bind_from.clone();
                            self.thumb_ui(ui, &vid, p as usize, 84.0);
                            let selected = self.content.bind_from_slot == Some(p);
                            if ui
                                .selectable_label(selected, format!("第{}页", p + 1))
                                .clicked()
                            {
                                self.content.bind_from_slot = Some(p);
                            }
                        });
                    }
                });
                ui.label("对应页（点「绑定/已绑定」切换）：");
                ui.horizontal(|ui| {
                    for p in 0..to_pages {
                        ui.vertical(|ui| {
                            let vid = self.content.bind_to.clone();
                            self.thumb_ui(ui, &vid, p as usize, 84.0);
                            let existing = self.content.bindings.iter().find(|b| {
                                s(b, "from_version_id") == self.content.bind_from
                                    && self.content.bind_from_slot == Some(i(b, "from_slot"))
                                    && s(b, "to_version_id") == self.content.bind_to
                                    && i(b, "to_slot") == p
                            }).map(|b| s(b, "id"));
                            match (self.content.bind_from_slot, existing) {
                                (Some(_), Some(bid)) => {
                                    if ui.button("已绑定 ✓").clicked() {
                                        if let Some(uid) = self.content.selected_unit.clone() {
                                            let _ = self.call(
                                                "POST",
                                                &format!("/v1/units/{uid}/page-bindings/{bid}/delete"),
                                                None,
                                            );
                                            self.open_unit_detail(&uid);
                                        }
                                    }
                                }
                                (Some(fs), None) => {
                                    if ui.button("绑定").clicked() {
                                        if let Some(uid) = self.content.selected_unit.clone() {
                                            let r = self.call(
                                                "POST",
                                                &format!("/v1/units/{uid}/page-bindings"),
                                                Some(json!({
                                                    "from_version_id": self.content.bind_from,
                                                    "from_slot": fs,
                                                    "to_version_id": self.content.bind_to,
                                                    "to_slot": p,
                                                })),
                                            );
                                            match r {
                                                Ok(_) => self.open_unit_detail(&uid),
                                                Err(e) => {
                                                    self.msg = format!("绑定失败：{e}")
                                                }
                                            }
                                        }
                                    }
                                }
                                _ => {
                                    ui.label("先选基准页");
                                }
                            }
                        });
                    }
                });
                if !self.content.bindings.is_empty() {
                    ui.label(
                        egui::RichText::new(format!("已有绑定 {} 条", self.content.bindings.len()))
                            .small()
                            .weak(),
                    );
                }
            } else {
                ui.label(egui::RichText::new("至少两个版本后可做逐页绑定").weak());
            }
        });
    }
}

impl App {
    fn page_index(&mut self, ui: &mut egui::Ui) {
        if self.index.creating {
            self.index_create(ui);
            return;
        }
        egui::SidePanel::left("index_list")
            .resizable(false)
            .default_width(220.0)
            .show_inside(ui, |ui| {
                if ui.button("＋ 新建索引").clicked() {
                    self.index.creating = true;
                    self.index.pick.clear();
                    for item in &self.units {
                        let unit = item.get("unit").cloned().unwrap_or(json!({}));
                        if let Some(rends) = item.get("renditions").and_then(|r| r.as_array()) {
                            for r in rends {
                                let rend = r.get("rendition").cloned().unwrap_or(json!({}));
                                self.index.pick.push((s(&unit, "id"), s(&rend, "id"), false));
                            }
                        }
                    }
                    if self.index.new_group.is_empty() {
                        if let Some(g) = self.groups.first() {
                            self.index.new_group = s(g, "id");
                        }
                    }
                }
                ui.separator();
                for x in self.indexes.clone() {
                    let lifecycle = s(&x, "lifecycle");
                    let badge = match lifecycle.as_str() {
                        "history" => " · 历史",
                        "void" => " · 已作废",
                        _ => "",
                    };
                    if ui
                        .selectable_label(
                            self.index.selected.as_deref() == Some(s(&x, "id").as_str()),
                            format!("{}{}", s(&x, "name"), badge),
                        )
                        .clicked()
                    {
                        self.index.branch = "main".into();
                        self.open_index_detail(&s(&x, "id"));
                    }
                }
            });
        if self.index.detail.is_none() {
            ui.label("从左侧选择一个索引，或新建索引。");
            return;
        }
        self.index_detail(ui);
    }

    fn index_create(&mut self, ui: &mut egui::Ui) {
        ui.heading("新建索引 · 选内容");
        ui.horizontal(|ui| {
            ui.label("名称");
            ui.text_edit_singleline(&mut self.index.new_name);
            ui.label("内容组");
            egui::ComboBox::from_id_salt("new_index_group")
                .selected_text(self.group_name(&self.index.new_group.clone()))
                .show_ui(ui, |ui| {
                    for g in self.groups.clone() {
                        ui.selectable_value(&mut self.index.new_group, s(&g, "id"), s(&g, "name"));
                    }
                });
            if ui.button("创建索引").clicked() {
                let entries: Vec<Value> = self
                    .index
                    .pick
                    .iter()
                    .filter(|(_, _, on)| *on)
                    .map(|(u, r, _)| json!({ "unit_id": u, "rendition_id": r }))
                    .collect();
                if entries.is_empty() {
                    self.msg = "至少选一个内容块".into();
                } else {
                    let r = self.call(
                        "POST",
                        "/v1/indexes",
                        Some(json!({
                            "name": self.index.new_name,
                            "group_id": self.index.new_group,
                            "scope": "company",
                            "entries": entries,
                        })),
                    );
                    match r {
                        Ok(v) => {
                            self.index.creating = false;
                            self.refresh_all();
                            self.open_index_detail(&s(&v, "id"));
                        }
                        Err(e) => self.msg = format!("创建失败：{e}"),
                    }
                }
            }
            if ui.button("取消").clicked() {
                self.index.creating = false;
            }
        });
        ui.separator();
        let mut pick = self.index.pick.clone();
        egui::ScrollArea::vertical().show(ui, |ui| {
            for item in self.units.clone() {
                let unit = item.get("unit").cloned().unwrap_or(json!({}));
                if let Some(rends) = item.get("renditions").and_then(|r| r.as_array()) {
                    for r in rends {
                        let rend = r.get("rendition").cloned().unwrap_or(json!({}));
                        if let Some(slot) = pick
                            .iter_mut()
                            .find(|(u, rr, _)| u == &s(&unit, "id") && rr == &s(&rend, "id"))
                        {
                            ui.checkbox(
                                &mut slot.2,
                                format!("{} · {} · {}", s(&unit, "name"), s(&rend, "form"), s(&rend, "lang")),
                            );
                        }
                    }
                }
            }
        });
        self.index.pick = pick;
    }

    fn index_detail(&mut self, ui: &mut egui::Ui) {
        let detail = self.index.detail.clone().unwrap();
        let index = detail.get("index").cloned().unwrap_or(json!({}));
        let branches = detail.get("branches").and_then(|b| b.as_array()).cloned().unwrap_or_default();
        let tags = detail.get("tags").and_then(|t| t.as_array()).cloned().unwrap_or_default();
        ui.horizontal(|ui| {
            ui.heading(s(&index, "name"));
            ui.label(format!(
                "{} · {}",
                self.folder_name(
                    index.get("folder_id").and_then(|x| x.as_str()).unwrap_or("")
                ),
                s(&index, "maintainer")
            ));
        });
        ui.horizontal(|ui| {
            ui.label("分支");
            let mut branch = if self.index.branch.is_empty() {
                "main".to_string()
            } else {
                self.index.branch.clone()
            };
            egui::ComboBox::from_id_salt("branch")
                .selected_text(branch.clone())
                .show_ui(ui, |ui| {
                    for b in &branches {
                        if let Some(bs) = b.as_str() {
                            ui.selectable_value(&mut branch, bs.to_string(), bs);
                        }
                    }
                });
            if branch != self.index.branch {
                self.index.branch = branch;
                self.load_index_detail();
                return;
            }
            ui.label("版本");
            for t in &tags {
                ui.label(format!("V{}", s(t, "name")));
            }
            ui.text_edit_singleline(&mut self.index.tag_name);
            if ui.button("发布版本").clicked() && !self.index.tag_name.is_empty() {
                if let Some(id) = self.index.selected.clone() {
                    let r = self.call(
                        "POST",
                        &format!("/v1/indexes/{id}/tags"),
                        Some(json!({ "branch": self.index.branch, "name": self.index.tag_name })),
                    );
                    match r {
                        Ok(_) => {
                            self.msg = "已发布".into();
                            self.load_index_detail();
                        }
                        Err(e) => self.msg = e,
                    }
                }
            }
        });
        ui.horizontal(|ui| {
            ui.checkbox(&mut self.index.include_notes, "添加备注");
            if ui.button("导出（个人版）").clicked() {
                self.export_index_file(&index, "personal");
            }
            if ui.button("导出（公用版）").clicked() {
                self.export_index_file(&index, "company");
            }
            ui.text_edit_singleline(&mut self.index.new_branch);
            if ui.button("拉分支").clicked() && !self.index.new_branch.is_empty() {
                if let Some(id) = self.index.selected.clone() {
                    let r = self.call(
                        "POST",
                        &format!("/v1/indexes/{id}/branches"),
                        Some(json!({ "name": self.index.new_branch, "from": self.index.branch })),
                    );
                    match r {
                        Ok(_) => {
                            self.msg = "已拉分支".into();
                            self.load_index_detail();
                        }
                        Err(e) => self.msg = e,
                    }
                }
            }
            if ui.button("推送到演示厅").clicked() {
                if let Some(id) = self.index.selected.clone() {
                    let r = self.call(
                        "POST",
                        "/v1/showroom",
                        Some(json!({
                            "title": s(&index, "name"),
                            "intro": "",
                            "source_index_id": id,
                        })),
                    );
                    match r {
                        Ok(_) => self.msg = "已推送，等待审核".into(),
                        Err(e) => self.msg = e,
                    }
                }
            }
        });
        ui.separator();
        // entry tile wall: horizontal cards, one per row
        let entries = self.current_entries();
        let resolved = detail
            .get("resolved_entries")
            .and_then(|r| r.as_array())
            .cloned()
            .unwrap_or_default();
        egui::ScrollArea::vertical().show(ui, |ui| {
            for (pos, re) in resolved.iter().enumerate() {
                let entry = re.get("entry").cloned().unwrap_or(json!({}));
                let ver = re.get("resolved").cloned().unwrap_or(Value::Null);
                let locked = entry
                    .get("policy")
                    .map(|p| p.get("locked").is_some())
                    .unwrap_or(false);
                if pos > 0 {
                    ui.vertical_centered(|ui| {
                        if ui.small_button("+ 插入条目").clicked() {
                            self.index.insert_at = Some(pos);
                            if let Ok(v) = self.call("GET", "/v1/units", None) {
                                self.index.insert_units = v
                                    .get("units")
                                    .and_then(|u| u.as_array())
                                    .cloned()
                                    .unwrap_or_default();
                            }
                        }
                    });
                    ui.add_space(2.0);
                }
                card_frame_square()
                    .inner_margin(egui::Margin::same(20.0))
                    .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.vertical(|ui| {
                            ui.label(format!("#{}", pos + 1));
                            if ver.is_object() {
                                let vid = s(&ver, "id");
                                let total = i(&ver, "page_count");
                                // first page large by default; extra pages
                                // as small thumbs beside it (2 rows x 3)
                                ui.horizontal(|ui| {
                                    self.thumb_ui(ui, &vid, 0, 232.0);
                                    if total > 1 {
                                        ui.vertical(|ui| {
                                            for row in 0..2 {
                                                ui.horizontal(|ui| {
                                                    for col in 0..3 {
                                                        let p = 1 + row * 3 + col;
                                                        if (p as i64) < total {
                                                            self.thumb_ui(
                                                                ui,
                                                                &vid,
                                                                p,
                                                                86.0,
                                                            );
                                                        }
                                                    }
                                                });
                                            }
                                        });
                                    }
                                });
                                ui.label(
                                    egui::RichText::new(format!("共 {} 页", total))
                                        .small()
                                        .color(TEXT_FAINT),
                                );
                            } else {
                                ui.label("无版本");
                            }
                        });
                        ui.separator();
                        ui.vertical(|ui| {
                            // top row: name left; policy chip, lock toggle and
                            // excerpt at the top right; delete is a red ✕ in
                            // the card's top-right corner
                            ui.horizontal(|ui| {
                                ui.label(
                                    egui::RichText::new(re.get("unit_name").and_then(|x| x.as_str()).unwrap_or(""))
                                        .strong()
                                        .size(15.0),
                                );
                                ui.label(
                                    egui::RichText::new(format!(
                                        "{} · {} · {}",
                                        re.get("form").and_then(|x| x.as_str()).unwrap_or(""),
                                        re.get("lang").and_then(|x| x.as_str()).unwrap_or(""),
                                        if ver.is_object() {
                                            format!("v{}", s(&ver, "semver"))
                                        } else {
                                            "无版本".into()
                                        }
                                    ))
                                    .size(12.5)
                                    .color(TEXT_WEAK),
                                );
                                ui.with_layout(
                                    egui::Layout::right_to_left(egui::Align::Center),
                                    |ui| {
                                        if ui
                                            .button(
                                                egui::RichText::new("×")
                                                    .color(egui::Color32::from_rgb(0xE5, 0x53, 0x4B))
                                                    .size(17.0)
                                                    .strong(),
                                            )
                                            .clicked()
                                        {
                                            let mut es = entries.clone();
                                            es.remove(pos);
                                            self.commit_entries(es, "删除条目");
                                            return;
                                        }
                                        if ui.button("↓").clicked() && pos + 1 < entries.len() {
                                            let mut es = entries.clone();
                                            es.swap(pos, pos + 1);
                                            self.commit_entries(es, "调整顺序");
                                            return;
                                        }
                                        if ui.button("↑").clicked() && pos > 0 {
                                            let mut es = entries.clone();
                                            es.swap(pos, pos - 1);
                                            self.commit_entries(es, "调整顺序");
                                            return;
                                        }
                                        if ui.button("选页").clicked() {
                                            let total = i(&ver, "page_count") as usize;
                                            let mut sel = vec![true; total];
                                            if let Some(slots) = entry
                                                .get("excerpt_slots")
                                                .and_then(|x| x.as_array())
                                            {
                                                sel = vec![false; total];
                                                for sl in slots {
                                                    if let Some(n) = sl
                                                        .as_str()
                                                        .and_then(|x| x.parse::<usize>().ok())
                                                    {
                                                        if n < total {
                                                            sel[n] = true;
                                                        }
                                                    }
                                                }
                                            }
                                            self.index.excerpt_sel = sel;
                                            self.index.excerpt_for = Some(pos);
                                        }
                                        if locked {
                                            if ui
                                                .button(
                                                    egui::RichText::new("解除锁定")
                                                        .color(CHIP_GREEN_FG),
                                                )
                                                .clicked()
                                            {
                                                let mut es = entries.clone();
                                                es[pos]["policy"] = json!("follow_latest");
                                                es[pos]["locked_semver"] = Value::Null;
                                                self.commit_entries(es, "解除锁定");
                                                return;
                                            }
                                            chip(ui, "版本锁定", CHIP_ORANGE_FG, CHIP_ORANGE_BG);
                                        } else {
                                            if ui.button("锁定当前版本").clicked() {
                                                if ver.is_object() {
                                                    let mut es = entries.clone();
                                                    es[pos]["policy"] = json!({ "locked": { "version_id": s(&ver, "id") } });
                                                    es[pos]["locked_semver"] =
                                                        json!(s(&ver, "semver"));
                                                    self.commit_entries(es, "锁定版本");
                                                    return;
                                                }
                                            }
                                            chip(ui, "跟随最新", CHIP_GREEN_FG, CHIP_GREEN_BG);
                                        }
                                    },
                                );
                            });
                            ui.horizontal(|ui| {
                                if let Some(slots) =
                                    entry.get("excerpt_slots").and_then(|x| x.as_array())
                                {
                                    chip(
                                        ui,
                                        &format!("摘录 {} 页", slots.len()),
                                        ACCENT_DEEP,
                                        CHIP_BLUE_BG,
                                    );
                                }
                                if re.get("substituted").and_then(|x| x.as_bool()) == Some(true) {
                                    chip(ui, "原版已删 · 已替代", CHIP_ORANGE_FG, CHIP_ORANGE_BG);
                                }
                            });
                            // quick-switch strip: history versions, newest
                            // first; last item opens the full-history popup
                            if let Some(versions) = self
                                .index
                                .unit_versions
                                .get(&s(&entry, "rendition_id"))
                                .cloned()
                            {
                                if versions.len() > 1 {
                                    let cur_id = if ver.is_object() {
                                        s(&ver, "id")
                                    } else {
                                        String::new()
                                    };
                                    let latest_id = s(&versions[0], "id");
                                    ui.horizontal_wrapped(|ui| {
                                        for v in versions.iter().take(7) {
                                            let vid = s(v, "id");
                                            let is_cur = vid == cur_id;
                                            ui.vertical(|ui| {
                                                if self.thumb_button(ui, &vid, 0, 54.0, is_cur) {
                                                    self.switch_entry_version(
                                                        pos,
                                                        &vid,
                                                        &s(v, "semver"),
                                                        vid == latest_id,
                                                    );
                                                }
                                                ui.label(
                                                    egui::RichText::new(format!(
                                                        "v{}",
                                                        s(v, "semver")
                                                    ))
                                                    .small()
                                                    .color(if is_cur { ACCENT } else { TEXT_WEAK }),
                                                );
                                            });
                                        }
                                        if ui.button("更多").clicked() {
                                            self.index.more_for = Some(pos);
                                        }
                                    });
                                }
                            }
                        });
                    });
                });
                if self.index.excerpt_for == Some(pos) {
                    let ver2 = re.get("resolved").cloned().unwrap_or(Value::Null);
                    card_frame_square().show(ui, |ui| {
                        ui.label(egui::RichText::new("摘录选页（只导出勾选的页）").strong());
                        if ver2.is_object() {
                            let vid = s(&ver2, "id");
                            let total = i(&ver2, "page_count") as usize;
                            ui.horizontal_wrapped(|ui| {
                                for p in 0..total {
                                    ui.vertical(|ui| {
                                        self.thumb_ui(ui, &vid, p, 110.0);
                                        if p < self.index.excerpt_sel.len() {
                                            ui.checkbox(
                                                &mut self.index.excerpt_sel[p],
                                                format!("第 {} 页", p + 1),
                                            );
                                        }
                                    });
                                }
                            });
                        }
                        ui.horizontal(|ui| {
                            if ui.small_button("全选").clicked() {
                                for b in self.index.excerpt_sel.iter_mut() {
                                    *b = true;
                                }
                            }
                            if ui.small_button("清空").clicked() {
                                for b in self.index.excerpt_sel.iter_mut() {
                                    *b = false;
                                }
                            }
                            if ui.button("保存选页").clicked() {
                                let chosen: Vec<usize> = self
                                    .index
                                    .excerpt_sel
                                    .iter()
                                    .enumerate()
                                    .filter(|(_, b)| **b)
                                    .map(|(n, _)| n)
                                    .collect();
                                if chosen.is_empty() {
                                    self.msg = "至少选一页".into();
                                } else {
                                    let mut es = entries.clone();
                                    if chosen.len() == self.index.excerpt_sel.len() {
                                        es[pos]["excerpt_slots"] = Value::Null;
                                    } else {
                                        es[pos]["excerpt_slots"] = json!(chosen
                                            .iter()
                                            .map(|n| n.to_string())
                                            .collect::<Vec<_>>());
                                    }
                                    self.index.excerpt_for = None;
                                    self.commit_entries(es, "摘录选页");
                                    return;
                                }
                            }
                            if ui.button("取消").clicked() {
                                self.index.excerpt_for = None;
                            }
                        });
                    });
                }
                ui.add_space(12.0);
            }
            ui.separator();
            ui.label(egui::RichText::new("分支合并").strong());
            ui.horizontal(|ui| {
                ui.label("来源分支");
                ui.text_edit_singleline(&mut self.index.merge_from);
                if ui.button("预览合并").clicked() && !self.index.merge_from.is_empty() {
                    if let Some(id) = self.index.selected.clone() {
                        let into = self.index.branch.clone();
                        let from = self.index.merge_from.clone();
                        match self.call(
                            "POST",
                            &format!("/v1/indexes/{id}/merge/preview"),
                            Some(json!({ "from": from, "into": into })),
                        ) {
                            Ok(v) => {
                                self.index.merge_preview = Some(v);
                                self.index.merge_resolutions.clear();
                            }
                            Err(e) => self.msg = format!("合并预览失败：{e}"),
                        }
                    }
                }
            });
            if let Some(preview) = self.index.merge_preview.clone() {
                let conflicts = preview
                    .get("conflicts")
                    .and_then(|c| c.as_array())
                    .cloned()
                    .unwrap_or_default();
                let mut names: std::collections::HashMap<String, String> =
                    std::collections::HashMap::new();
                for re in &resolved {
                    if let Some(en) = re.get("entry") {
                        names.insert(
                            s(en, "unit_id"),
                            re.get("unit_name")
                                .and_then(|x| x.as_str())
                                .unwrap_or("")
                                .to_string(),
                        );
                    }
                }
                if conflicts.is_empty() {
                    ui.label(
                        egui::RichText::new("无冲突，可自动合并")
                            .color(CHIP_GREEN_FG),
                    );
                } else {
                    ui.label(format!("{} 个条目冲突，逐条裁决：", conflicts.len()));
                    for c in &conflicts {
                        let key = s(c, "key");
                        let uname = names
                            .get(&s(c, "unit_id"))
                            .cloned()
                            .unwrap_or_else(|| "条目".to_string());
                        let ours = c.get("ours").cloned().unwrap_or(Value::Null);
                        let theirs = c.get("theirs").cloned().unwrap_or(Value::Null);
                        ui.horizontal(|ui| {
                            ui.label(format!(
                                "{}：本分支「{}」 / 对方「{}」",
                                uname,
                                entry_label(&ours),
                                entry_label(&theirs)
                            ));
                            let cur = self.index.merge_resolutions.get(&key).cloned();
                            if ui
                                .selectable_label(cur.as_deref() == Some("ours"), "用本分支")
                                .clicked()
                            {
                                self.index
                                    .merge_resolutions
                                    .insert(key.clone(), "ours".to_string());
                            }
                            if ui
                                .selectable_label(cur.as_deref() == Some("theirs"), "用对方")
                                .clicked()
                            {
                                self.index
                                    .merge_resolutions
                                    .insert(key.clone(), "theirs".to_string());
                            }
                        });
                    }
                }
                let unresolved = conflicts
                    .iter()
                    .filter(|c| !self.index.merge_resolutions.contains_key(&s(c, "key")))
                    .count();
                ui.horizontal(|ui| {
                    if ui
                        .add_enabled(unresolved == 0, egui::Button::new("执行合并"))
                        .clicked()
                    {
                        if let Some(id) = self.index.selected.clone() {
                            let resolutions: serde_json::Map<String, Value> = self
                                .index
                                .merge_resolutions
                                .iter()
                                .map(|(k, v)| (k.clone(), json!(v)))
                                .collect();
                            let r = self.call(
                                "POST",
                                &format!("/v1/indexes/{id}/merge"),
                                Some(json!({
                                    "from": self.index.merge_from,
                                    "into": self.index.branch,
                                    "resolutions": resolutions,
                                })),
                            );
                            match r {
                                Ok(_) => {
                                    self.msg = "合并完成".into();
                                    self.index.merge_preview = None;
                                    self.load_index_detail();
                                }
                                Err(e) => self.msg = format!("合并失败：{e}"),
                            }
                        }
                    }
                    if unresolved > 0 {
                        ui.label(format!("还剩 {unresolved} 条未裁决"));
                    }
                });
            }
            // full-history popup (opened from a strip's 「更多」)
            if let Some(pos) = self.index.more_for {
                let entries_now = self.current_entries();
                let rid = entries_now
                    .get(pos)
                    .map(|e| s(e, "rendition_id"))
                    .unwrap_or_default();
                let versions = self
                    .index
                    .unit_versions
                    .get(&rid)
                    .cloned()
                    .unwrap_or_default();
                let cur_id = self
                    .index
                    .detail
                    .as_ref()
                    .and_then(|d| d.get("resolved_entries"))
                    .and_then(|r| r.as_array())
                    .and_then(|a| a.get(pos))
                    .and_then(|re| re.get("resolved"))
                    .map(|v| s(v, "id"))
                    .unwrap_or_default();
                let latest_id = versions.first().map(|v| s(v, "id")).unwrap_or_default();
                let mut open = true;
                let mut switch: Option<(String, String, bool)> = None;
                egui::Window::new("全部历史版本")
                    .open(&mut open)
                    .collapsible(false)
                    .resizable(false)
                    .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
                    .show(ui.ctx(), |ui| {
                        egui::ScrollArea::vertical()
                            .max_height(430.0)
                            .show(ui, |ui| {
                                ui.horizontal_wrapped(|ui| {
                                    for v in &versions {
                                        let vid = s(v, "id");
                                        let is_cur = vid == cur_id;
                                        ui.vertical(|ui| {
                                            if self.thumb_button(ui, &vid, 0, 110.0, is_cur) {
                                                switch = Some((
                                                    vid.clone(),
                                                    s(v, "semver"),
                                                    vid == latest_id,
                                                ));
                                            }
                                            ui.label(
                                                egui::RichText::new(format!(
                                                    "v{} · {} 页",
                                                    s(v, "semver"),
                                                    i(v, "page_count")
                                                ))
                                                .small()
                                                .color(if is_cur { ACCENT } else { TEXT_WEAK }),
                                            );
                                        });
                                    }
                                });
                            });
                    });
                if !open {
                    self.index.more_for = None;
                }
                if let Some((vid, sv, is_latest)) = switch {
                    self.switch_entry_version(pos, &vid, &sv, is_latest);
                }
            }
            // insert-entry popup (opened from a between-cards insert row)
            if let Some(pos) = self.index.insert_at {
                let units = self.index.insert_units.clone();
                let mut open = true;
                let mut chosen: Option<(String, String)> = None;
                egui::Window::new("插入条目")
                    .open(&mut open)
                    .collapsible(false)
                    .resizable(false)
                    .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
                    .show(ui.ctx(), |ui| {
                        egui::ScrollArea::vertical()
                            .max_height(430.0)
                            .show(ui, |ui| {
                                for u in &units {
                                    let unit = u.get("unit").cloned().unwrap_or(json!({}));
                                    if let Some(rends) =
                                        u.get("renditions").and_then(|r| r.as_array())
                                    {
                                        for r in rends {
                                            let rend =
                                                r.get("rendition").cloned().unwrap_or(json!({}));
                                            if r.get("latest")
                                                .map(|l| l.is_object())
                                                .unwrap_or(false)
                                            {
                                                let label = format!(
                                                    "{} · {} · {}",
                                                    s(&unit, "name"),
                                                    s(&rend, "form"),
                                                    s(&rend, "lang")
                                                );
                                                if ui.button(label).clicked() {
                                                    chosen =
                                                        Some((s(&unit, "id"), s(&rend, "id")));
                                                }
                                            }
                                        }
                                    }
                                }
                            });
                    });
                if !open {
                    self.index.insert_at = None;
                }
                if let Some((uid, rid)) = chosen {
                    let mut es = self.current_entries();
                    let at = pos.min(es.len());
                    es.insert(at, json!({ "unit_id": uid, "rendition_id": rid }));
                    self.index.insert_at = None;
                    self.commit_entries(es, "插入条目");
                }
            }
            ui.separator();
            ui.label(egui::RichText::new("提交历史").strong());
            if let Some(commits) = self
                .index
                .commits
                .clone()
                .and_then(|c| c.get("commits").and_then(|x| x.as_array()).cloned())
            {
                for c in commits {
                    ui.label(format!(
                        "{} · {} · {}",
                        s(&c, "message"),
                        s(&c, "author"),
                        fmt_time(i(&c, "created_ms"))
                    ));
                }
            }
            ui.label(egui::RichText::new("导出记录").strong());
            if let Some(exports) = self
                .index
                .exports
                .clone()
                .and_then(|c| c.get("exports").and_then(|x| x.as_array()).cloned())
            {
                for e in exports {
                    ui.label(format!(
                        "{} 页 · {} · {}",
                        i(&e, "page_count"),
                        fmt_time(i(&e, "created_ms")),
                        s(&e, "id")
                    ));
                }
            }
        });
    }

    fn page_settings(&mut self, ui: &mut egui::Ui) {
        // Board layout: left section nav, one section at a time on the
        // right; sections a role may not see are not rendered at all.
        let role = self.role();
        let mut sections: Vec<&str> = vec!["个人资料"];
        if role != "member" {
            sections.push("组织管理");
        }
        if role == "super" {
            sections.extend([
                "内容组与授权",
                "公司公共资料",
                "内容语言",
                "AI 翻译",
                "离线缓存策略",
            ]);
        }
        if self.settings.section >= sections.len() {
            self.settings.section = 0;
        }
        let current = sections[self.settings.section];
        egui::SidePanel::left("settings_nav")
            .resizable(false)
            .default_width(190.0)
            .frame(
                egui::Frame::none()
                    .fill(CHROME)
                    .inner_margin(egui::Margin::same(10.0)),
            )
            .show_inside(ui, |ui| {
                for (i, name) in sections.iter().enumerate() {
                    if ui
                        .selectable_label(self.settings.section == i, *name)
                        .clicked()
                    {
                        self.settings.section = i;
                    }
                    ui.add_space(4.0);
                }
            });
        egui::CentralPanel::default()
            .frame(
                egui::Frame::none()
                    .fill(APP_BG)
                    .inner_margin(egui::Margin::same(20.0)),
            )
            .show_inside(ui, |ui| {
                egui::ScrollArea::vertical().show(ui, |ui| {
                    ui.heading(current);
                    ui.add_space(10.0);
                    match current {
                        "个人资料" => {
                            egui::Grid::new("profile_grid")
                                .num_columns(2)
                                .spacing([14.0, 10.0])
                                .show(ui, |ui| {
                                    for (key, label) in [
                                        ("title", "职务"),
                                        ("english_name", "英文名"),
                                        ("phone", "手机号"),
                                        ("email", "邮箱"),
                                        ("wechat", "微信"),
                                    ] {
                                        ui.label(label);
                                        let v = self
                                            .settings
                                            .profile
                                            .entry(key.to_string())
                                            .or_default();
                                        ui.add_sized(
                                            [300.0, 26.0],
                                            egui::TextEdit::singleline(v),
                                        );
                                        ui.end_row();
                                    }
                                });
                            ui.add_space(12.0);
                            if ui
                                .add(
                                    egui::Button::new(
                                        egui::RichText::new("保存资料")
                                            .color(egui::Color32::WHITE),
                                    )
                                    .fill(ACCENT)
                                    .rounding(15.0),
                                )
                                .clicked()
                            {
                                let fields: Value = json!(self.settings.profile);
                                match self.call(
                                    "PUT",
                                    "/v1/me/profile",
                                    Some(json!({ "fields": fields })),
                                ) {
                                    Ok(_) => self.msg = "已保存".into(),
                                    Err(e) => self.msg = e,
                                }
                            }
                        }
                        "组织管理" => {
                            let depts = self
                                .org
                                .get("departments")
                                .and_then(|d| d.as_array())
                                .cloned()
                                .unwrap_or_default();
                            for d in &depts {
                                ui.label(format!("· {}", s(d, "name")));
                            }
                            ui.horizontal(|ui| {
                                ui.label("新部门");
                                ui.add_sized(
                                    [240.0, 26.0],
                                    egui::TextEdit::singleline(&mut self.settings.dept_name),
                                );
                                if ui.button("添加").clicked()
                                    && !self.settings.dept_name.is_empty()
                                {
                                    match self.call(
                                        "POST",
                                        "/v1/org/departments",
                                        Some(json!({ "name": self.settings.dept_name })),
                                    ) {
                                        Ok(_) => {
                                            self.settings.dept_name.clear();
                                            self.refresh_all();
                                        }
                                        Err(e) => self.msg = e,
                                    }
                                }
                            });
                            ui.add_space(10.0);
                            let users = self
                                .org
                                .get("users")
                                .and_then(|d| d.as_array())
                                .cloned()
                                .unwrap_or_default();
                            for u in &users {
                                ui.label(format!(
                                    "{} · {} · {}",
                                    s(u, "username"),
                                    s(u, "display_name"),
                                    s(u, "role")
                                ));
                            }
                            ui.horizontal(|ui| {
                                ui.label("新账号");
                                ui.add_sized(
                                    [150.0, 26.0],
                                    egui::TextEdit::singleline(&mut self.settings.user_name),
                                );
                                ui.label("姓名");
                                ui.add_sized(
                                    [150.0, 26.0],
                                    egui::TextEdit::singleline(
                                        &mut self.settings.user_display,
                                    ),
                                );
                                ui.label("密码");
                                ui.add_sized(
                                    [150.0, 26.0],
                                    egui::TextEdit::singleline(
                                        &mut self.settings.user_password,
                                    )
                                    .password(true),
                                );
                                if ui.button("创建账号").clicked() {
                                    match self.call(
                                        "POST",
                                        "/v1/org/users",
                                        Some(json!({
                                            "username": self.settings.user_name,
                                            "display_name": self.settings.user_display,
                                            "password": self.settings.user_password,
                                            "role": "member",
                                        })),
                                    ) {
                                        Ok(_) => {
                                            self.settings.user_name.clear();
                                            self.refresh_all();
                                        }
                                        Err(e) => self.msg = e,
                                    }
                                }
                            });
                        }
                        "内容组与授权" => {
                            for g in self.groups.clone() {
                                ui.label(format!("· {}", s(&g, "name")));
                            }
                            ui.horizontal(|ui| {
                                ui.label("新内容组");
                                ui.add_sized(
                                    [240.0, 26.0],
                                    egui::TextEdit::singleline(&mut self.settings.group_name),
                                );
                                if ui.button("添加").clicked()
                                    && !self.settings.group_name.is_empty()
                                {
                                    match self.call(
                                        "POST",
                                        "/v1/groups",
                                        Some(json!({ "name": self.settings.group_name })),
                                    ) {
                                        Ok(_) => {
                                            self.settings.group_name.clear();
                                            self.refresh_all();
                                        }
                                        Err(e) => self.msg = e,
                                    }
                                }
                            });
                            ui.add_space(10.0);
                            if ui.button("载入授权列表").clicked() {
                                match self.call("GET", "/v1/grants", None) {
                                    Ok(v) => {
                                        self.grants = v
                                            .get("grants")
                                            .and_then(|g| g.as_array())
                                            .cloned()
                                            .unwrap_or_default()
                                    }
                                    Err(e) => self.msg = e,
                                }
                            }
                            for g in self.grants.clone() {
                                ui.label(format!(
                                    "{} → {}:{} {}",
                                    self.group_name(&s(&g, "group_id")),
                                    s(&g, "subject_type"),
                                    s(&g, "subject_id"),
                                    s(&g, "capability")
                                ));
                            }
                            ui.horizontal(|ui| {
                                egui::ComboBox::from_id_salt("grant_group")
                                    .selected_text(
                                        self.group_name(&self.settings.grant_group.clone()),
                                    )
                                    .show_ui(ui, |ui| {
                                        for g in self.groups.clone() {
                                            ui.selectable_value(
                                                &mut self.settings.grant_group,
                                                s(&g, "id"),
                                                s(&g, "name"),
                                            );
                                        }
                                    });
                                egui::ComboBox::from_id_salt("grant_st")
                                    .selected_text(self.settings.grant_subject_type.clone())
                                    .show_ui(ui, |ui| {
                                        ui.selectable_value(
                                            &mut self.settings.grant_subject_type,
                                            "dept".into(),
                                            "部门",
                                        );
                                        ui.selectable_value(
                                            &mut self.settings.grant_subject_type,
                                            "user".into(),
                                            "个人",
                                        );
                                    });
                                ui.text_edit_singleline(&mut self.settings.grant_subject_id);
                                egui::ComboBox::from_id_salt("grant_cap")
                                    .selected_text(self.settings.grant_capability.clone())
                                    .show_ui(ui, |ui| {
                                        for (v, l) in [
                                            ("view", "查看"),
                                            ("use", "使用"),
                                            ("publish", "发布"),
                                            ("manage", "管理"),
                                        ] {
                                            ui.selectable_value(
                                                &mut self.settings.grant_capability,
                                                v.into(),
                                                l,
                                            );
                                        }
                                    });
                                if ui.button("添加授权").clicked() {
                                    match self.call(
                                        "POST",
                                        "/v1/grants",
                                        Some(json!({
                                            "group_id": self.settings.grant_group,
                                            "subject_type": self.settings.grant_subject_type,
                                            "subject_id": self.settings.grant_subject_id,
                                            "capability": self.settings.grant_capability,
                                        })),
                                    ) {
                                        Ok(_) => self.msg = "已添加授权".into(),
                                        Err(e) => self.msg = e,
                                    }
                                }
                            });
                        }
                        "公司公共资料" => {
                            egui::Grid::new("company_grid")
                                .num_columns(2)
                                .spacing([14.0, 10.0])
                                .show(ui, |ui| {
                                    for (key, label) in [
                                        ("phone", "公司电话"),
                                        ("website", "官网"),
                                        ("address", "地址"),
                                        ("gongzhonghao", "公众号"),
                                    ] {
                                        ui.label(label);
                                        let v = self
                                            .settings
                                            .company
                                            .entry(key.to_string())
                                            .or_default();
                                        ui.add_sized(
                                            [300.0, 26.0],
                                            egui::TextEdit::singleline(v),
                                        );
                                        ui.end_row();
                                    }
                                });
                            ui.add_space(12.0);
                            if ui
                                .add(
                                    egui::Button::new(
                                        egui::RichText::new("保存公司资料")
                                            .color(egui::Color32::WHITE),
                                    )
                                    .fill(ACCENT)
                                    .rounding(15.0),
                                )
                                .clicked()
                            {
                                let fields: Value = json!(self.settings.company);
                                match self.call(
                                    "PUT",
                                    "/v1/company-profile",
                                    Some(json!({ "fields": fields })),
                                ) {
                                    Ok(_) => self.msg = "已保存".into(),
                                    Err(e) => self.msg = e,
                                }
                            }
                        }
                        "内容语言" => {
                            ui.horizontal(|ui| {
                                ui.label("内容语言清单");
                                ui.add_sized(
                                    [300.0, 26.0],
                                    egui::TextEdit::singleline(
                                        &mut self.settings.languages_text,
                                    ),
                                );
                                if ui.button("保存").clicked() {
                                    let langs: Vec<String> = self
                                        .settings
                                        .languages_text
                                        .split(',')
                                        .map(|x| x.trim().to_string())
                                        .filter(|x| !x.is_empty())
                                        .collect();
                                    match self.call(
                                        "PUT",
                                        "/v1/settings/languages",
                                        Some(json!({ "languages": langs })),
                                    ) {
                                        Ok(_) => self.msg = "已保存".into(),
                                        Err(e) => self.msg = e,
                                    }
                                }
                            });
                        }
                        "AI 翻译" => {
                            if !self.settings.ai_loaded {
                                self.settings.ai_loaded = true;
                                if let Ok(v) = self.call("GET", "/v1/settings/ai", None) {
                                    self.settings.ai_provider = s(&v, "provider");
                                    self.settings.ai_endpoint = s(&v, "endpoint");
                                    self.settings.ai_model = s(&v, "model");
                                    self.settings.ai_has_key = v
                                        .get("has_key")
                                        .and_then(|x| x.as_bool())
                                        .unwrap_or(false);
                                }
                                if let Ok(v) = self.call("GET", "/v1/settings/glossary", None) {
                                    if let Some(pairs) =
                                        v.get("pairs").and_then(|x| x.as_array())
                                    {
                                        self.settings.glossary_text = pairs
                                            .iter()
                                            .map(|pr| {
                                                format!(
                                                    "{}={}",
                                                    s(pr, "source"),
                                                    s(pr, "target")
                                                )
                                            })
                                            .collect::<Vec<_>>()
                                            .join("\n");
                                    }
                                }
                            }
                            egui::Grid::new("ai_grid")
                                .num_columns(2)
                                .spacing([14.0, 10.0])
                                .show(ui, |ui| {
                                    ui.label("提供方");
                                    egui::ComboBox::from_id_salt("ai_provider")
                                        .selected_text(self.settings.ai_provider.clone())
                                        .show_ui(ui, |ui| {
                                            ui.selectable_value(
                                                &mut self.settings.ai_provider,
                                                "stub".into(),
                                                "stub（本地占位）",
                                            );
                                            ui.selectable_value(
                                                &mut self.settings.ai_provider,
                                                "openai-compat".into(),
                                                "OpenAI 兼容接口",
                                            );
                                        });
                                    ui.end_row();
                                    ui.label("接口地址");
                                    ui.add_sized(
                                        [300.0, 26.0],
                                        egui::TextEdit::singleline(
                                            &mut self.settings.ai_endpoint,
                                        ),
                                    );
                                    ui.end_row();
                                    ui.label("模型");
                                    ui.add_sized(
                                        [300.0, 26.0],
                                        egui::TextEdit::singleline(
                                            &mut self.settings.ai_model,
                                        ),
                                    );
                                    ui.end_row();
                                    ui.label("API 密钥");
                                    ui.add_sized(
                                        [300.0, 26.0],
                                        egui::TextEdit::singleline(&mut self.settings.ai_key)
                                            .password(true),
                                    );
                                    ui.end_row();
                                });
                            ui.label(
                                egui::RichText::new(if self.settings.ai_has_key {
                                    "密钥：已设置（留空保存则不更换）"
                                } else {
                                    "密钥：未设置"
                                })
                                .small()
                                .color(TEXT_WEAK),
                            );
                            ui.add_space(8.0);
                            if ui
                                .add(
                                    egui::Button::new(
                                        egui::RichText::new("保存 AI 设置")
                                            .color(egui::Color32::WHITE),
                                    )
                                    .fill(ACCENT)
                                    .rounding(15.0),
                                )
                                .clicked()
                            {
                                let mut body = json!({
                                    "provider": self.settings.ai_provider,
                                    "endpoint": self.settings.ai_endpoint,
                                    "model": self.settings.ai_model,
                                });
                                if !self.settings.ai_key.is_empty() {
                                    body["api_key"] = json!(self.settings.ai_key);
                                }
                                match self.call("PUT", "/v1/settings/ai", Some(body)) {
                                    Ok(_) => {
                                        if !self.settings.ai_key.is_empty() {
                                            self.settings.ai_has_key = true;
                                            self.settings.ai_key.clear();
                                        }
                                        self.msg = "AI 设置已保存".into();
                                    }
                                    Err(e) => self.msg = e,
                                }
                            }
                            ui.add_space(16.0);
                            ui.label(egui::RichText::new("术语表（每行一条：源词=译词）").strong());
                            ui.add_sized(
                                [440.0, 120.0],
                                egui::TextEdit::multiline(&mut self.settings.glossary_text),
                            );
                            ui.add_space(8.0);
                            if ui.button("保存术语表").clicked() {
                                let pairs: Vec<Value> = self
                                    .settings
                                    .glossary_text
                                    .lines()
                                    .filter_map(|l| {
                                        let (a, b) = l.split_once('=')?;
                                        Some(json!({
                                            "source": a.trim(),
                                            "target": b.trim(),
                                        }))
                                    })
                                    .collect();
                                match self.call(
                                    "PUT",
                                    "/v1/settings/glossary",
                                    Some(json!({ "pairs": pairs })),
                                ) {
                                    Ok(_) => self.msg = "术语表已保存".into(),
                                    Err(e) => self.msg = e,
                                }
                            }
                        }
                        "离线缓存策略" => {
                            let mut allowed = self
                                .cache_policy
                                .get("allowed")
                                .and_then(|v| v.as_bool())
                                .unwrap_or(true);
                            let mut max_mb = self
                                .cache_policy
                                .get("max_mb")
                                .and_then(|v| v.as_i64())
                                .unwrap_or(500);
                            ui.horizontal(|ui| {
                                ui.checkbox(&mut allowed, "允许离线缓存");
                                ui.label("容量上限（MB）");
                                ui.add(egui::DragValue::new(&mut max_mb).range(10..=100000));
                                if ui.button("保存策略").clicked() {
                                    match self.call(
                                        "PUT",
                                        "/v1/settings/cache-policy",
                                        Some(json!({
                                            "allowed": allowed,
                                            "max_mb": max_mb
                                        })),
                                    ) {
                                        Ok(_) => {
                                            self.cache_policy =
                                                json!({ "allowed": allowed, "max_mb": max_mb });
                                            self.msg = "缓存策略已保存".into();
                                        }
                                        Err(e) => self.msg = e,
                                    }
                                }
                            });
                        }
                        _ => {}
                    }
                });
            });
    }
}

fn main() -> eframe::Result<()> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1440.0, 900.0])
            .with_title("启幕 · PPT 管理器"),
        ..Default::default()
    };
    eframe::run_native(
        "启幕",
        options,
        Box::new(|cc| Ok(Box::new(App::new(cc)))),
    )
}

#[cfg(test)]
mod cache_crypto_tests {
    use super::*;

    #[test]
    fn encrypt_decrypt_round_trip() {
        let key = [7u8; 32];
        let plain = b"PK\x03\x04 fake pptx bytes for round trip";
        let enc = encrypt_bytes(&key, plain).expect("encrypt");
        assert_ne!(&enc[12..], &plain[..]);
        let back = decrypt_bytes(&key, &enc).expect("decrypt");
        assert_eq!(back, plain.to_vec());
        let wrong = [8u8; 32];
        assert!(decrypt_bytes(&wrong, &enc).is_none());
    }
}

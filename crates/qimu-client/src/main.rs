//! qimu-client — the Rust desktop client (spec §4): four top-level pages
//! 演示厅 / 内容页 / 索引页 / 设置, talking to qimu-server over HTTP.
//! All logic lives in qimu-core / the server; this crate is a frontend,
//! exactly like the CLI (spec §4.4 parity).

use base64::Engine;
use eframe::egui;
use serde_json::{json, Value};
use std::collections::BTreeMap;

fn install_cjk_font(ctx: &egui::Context) {
    // egui's bundled fonts have no CJK glyphs; load a system CJK font.
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
            let mut fonts = egui::FontDefinitions::default();
            fonts.font_data.insert(
                "cjk".to_owned(),
                egui::FontData::from_owned(bytes),
            );
            for fam in [egui::FontFamily::Proportional, egui::FontFamily::Monospace] {
                fonts
                    .families
                    .entry(fam)
                    .or_default()
                    .insert(0, "cjk".to_owned());
            }
            ctx.set_fonts(fonts);
            return;
        }
    }
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
}

#[derive(Default)]
struct SettingsState {
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
    content: ContentState,
    index: IndexState,
    settings: SettingsState,
    frames: u64,
    screenshot_path: Option<String>,
    screenshot_done: bool,
}

impl App {
    fn new(cc: &eframe::CreationContext<'_>) -> Self {
        install_cjk_font(&cc.egui_ctx);
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
        }
        if let Ok(v) = self.call("GET", "/v1/showroom?state=pending", None) {
            self.showroom_pending = v.get("items").and_then(|f| f.as_array()).cloned().unwrap_or_default();
        }
        if let Ok(v) = self.call("GET", "/v1/me", None) {
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
        // Self-screenshot mode for headless verification (QIMU_SCREENSHOT).
        if let Some(path) = self.screenshot_path.clone() {
            if !self.screenshot_done && self.frames == 45 {
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
            egui::CentralPanel::default().show(ctx, |ui| {
                ui.vertical_centered(|ui| {
                    ui.add_space(120.0);
                    ui.heading("启幕");
                    ui.label("PPT 内容管理与更新系统");
                    ui.add_space(16.0);
                    ui.horizontal(|ui| {
                        ui.label("服务器");
                        ui.text_edit_singleline(&mut self.server);
                    });
                    ui.horizontal(|ui| {
                        ui.label("账号　");
                        ui.text_edit_singleline(&mut self.username);
                    });
                    ui.horizontal(|ui| {
                        ui.label("密码　");
                        ui.add(egui::TextEdit::singleline(&mut self.password).password(true));
                    });
                    ui.add_space(8.0);
                    if ui.button("登录").clicked() {
                        self.do_login();
                    }
                    if !self.msg.is_empty() {
                        ui.label(&self.msg);
                    }
                });
            });
            return;
        }

        egui::TopBottomPanel::top("top").show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.heading("启幕");
                ui.separator();
                for (page, label) in [
                    (Page::Showroom, "演示厅"),
                    (Page::Content, "内容页"),
                    (Page::Index, "索引页"),
                    (Page::Settings, "设置"),
                ] {
                    if ui
                        .selectable_label(self.page == page, label)
                        .clicked()
                    {
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
                    ui.label(format!("{name} · {}", self.role()));
                });
            });
        });
        egui::TopBottomPanel::bottom("status").show(ctx, |ui| {
            ui.label(&self.msg);
        });

        egui::CentralPanel::default().show(ctx, |ui| match self.page {
            Page::Showroom => self.page_showroom(ui),
            Page::Content => self.page_content(ui),
            Page::Index => self.page_index(ui),
            Page::Settings => self.page_settings(ui),
        });
    }
}

impl App {
    fn page_showroom(&mut self, ui: &mut egui::Ui) {
        ui.heading("演示厅 · 广场");
        ui.add_space(6.0);
        egui::ScrollArea::vertical().show(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                for item in self.showroom_live.clone() {
                    ui.allocate_ui(egui::vec2(300.0, 128.0), |ui| {
                        egui::Frame::group(ui.style()).show(ui, |ui| {
                            ui.set_min_size(egui::vec2(284.0, 112.0));
                            ui.label(egui::RichText::new(s(&item, "title")).strong());
                            ui.label(s(&item, "intro"));
                            ui.label(
                                egui::RichText::new(format!(
                                    "{} · {}",
                                    s(&item, "submitted_by"),
                                    fmt_time(i(&item, "created_ms"))
                                ))
                                .small()
                                .weak(),
                            );
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
                    ui.allocate_ui(egui::vec2(300.0, 128.0), |ui| {
                        let resp = egui::Frame::group(ui.style()).show(ui, |ui| {
                            ui.set_min_size(egui::vec2(284.0, 112.0));
                            ui.label(egui::RichText::new(s(&unit, "name")).strong().size(16.0));
                            if latest.is_object() {
                                ui.label(format!(
                                    "v{} · {} 页",
                                    s(&latest, "semver"),
                                    i(&latest, "page_count")
                                ));
                            } else {
                                ui.label("暂无版本");
                            }
                            ui.label(
                                egui::RichText::new(format!(
                                    "{} · {}",
                                    self.folder_name(
                                        unit.get("folder_id").and_then(|x| x.as_str()).unwrap_or("")
                                    ),
                                    s(&unit, "maintainer")
                                ))
                                .small()
                                .weak(),
                            );
                        });
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
            if ui.button("导出 PPT").clicked() {
                if let Some(id) = self.index.selected.clone() {
                    if let Some(path) = rfd::FileDialog::new()
                        .set_file_name(format!("{}.pptx", s(&index, "name")))
                        .save_file()
                    {
                        let r = self.call(
                            "POST",
                            &format!("/v1/indexes/{id}/export"),
                            Some(json!({ "branch": self.index.branch, "kind": "personal" })),
                        );
                        match r {
                            Ok(v) => {
                                if let Some(data) = v.get("data_base64").and_then(|x| x.as_str()) {
                                    if let Ok(bytes) = b64d(data) {
                                        let _ = std::fs::write(&path, bytes);
                                        self.msg = format!(
                                            "已导出 {} 页",
                                            v.get("page_count").and_then(|x| x.as_i64()).unwrap_or(0)
                                        );
                                        self.load_index_detail();
                                    }
                                }
                            }
                            Err(e) => self.msg = format!("导出失败：{e}"),
                        }
                    }
                }
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
                egui::Frame::group(ui.style()).show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.vertical(|ui| {
                            ui.label(
                                egui::RichText::new(format!(
                                    "{} 页",
                                    if ver.is_object() { i(&ver, "page_count") } else { 0 }
                                ))
                                .size(20.0)
                                .strong(),
                            );
                            ui.label(format!("#{}", pos + 1));
                        });
                        ui.separator();
                        ui.vertical(|ui| {
                            ui.label(
                                egui::RichText::new(re.get("unit_name").and_then(|x| x.as_str()).unwrap_or(""))
                                    .strong()
                                    .size(15.0),
                            );
                            ui.label(format!(
                                "{} · {} · {}",
                                re.get("form").and_then(|x| x.as_str()).unwrap_or(""),
                                re.get("lang").and_then(|x| x.as_str()).unwrap_or(""),
                                if ver.is_object() {
                                    format!("v{}", s(&ver, "semver"))
                                } else {
                                    "无版本".into()
                                }
                            ));
                            if re.get("substituted").and_then(|x| x.as_bool()) == Some(true) {
                                ui.label(
                                    egui::RichText::new("原版已删 · 已替代")
                                        .color(egui::Color32::from_rgb(200, 120, 0)),
                                );
                            }
                        });
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui.button("删除").clicked() {
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
                            if locked {
                                if ui
                                    .button(egui::RichText::new("解除锁定").color(egui::Color32::from_rgb(30, 150, 60)))
                                    .clicked()
                                {
                                    let mut es = entries.clone();
                                    es[pos]["policy"] = json!("follow_latest");
                                    es[pos]["locked_semver"] = Value::Null;
                                    self.commit_entries(es, "解除锁定");
                                    return;
                                }
                                ui.label(
                                    egui::RichText::new("版本锁定")
                                        .color(egui::Color32::from_rgb(220, 120, 0))
                                        .strong(),
                                );
                            } else {
                                if ui.button("锁定当前版本").clicked() {
                                    if ver.is_object() {
                                        let mut es = entries.clone();
                                        es[pos]["policy"] = json!({ "locked": { "version_id": s(&ver, "id") } });
                                        es[pos]["locked_semver"] = json!(s(&ver, "semver"));
                                        self.commit_entries(es, "锁定版本");
                                        return;
                                    }
                                }
                                ui.label(
                                    egui::RichText::new("跟随最新")
                                        .color(egui::Color32::from_rgb(30, 150, 60))
                                        .strong(),
                                );
                            }
                        });
                    });
                });
                ui.add_space(4.0);
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
        egui::ScrollArea::vertical().show(ui, |ui| {
            ui.heading("个人资料");
            for (key, label) in [
                ("title", "职务"),
                ("english_name", "英文名"),
                ("phone", "手机号"),
                ("email", "邮箱"),
                ("wechat", "微信"),
            ] {
                ui.horizontal(|ui| {
                    ui.label(label);
                    let v = self.settings.profile.entry(key.to_string()).or_default();
                    ui.text_edit_singleline(v);
                });
            }
            if ui.button("保存个人资料").clicked() {
                let fields: Value = json!(self.settings.profile);
                match self.call("PUT", "/v1/me/profile", Some(json!({ "fields": fields }))) {
                    Ok(_) => self.msg = "已保存".into(),
                    Err(e) => self.msg = e,
                }
            }
            if self.role() == "member" {
                return;
            }
            ui.separator();
            ui.heading("组织管理");
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
                ui.text_edit_singleline(&mut self.settings.dept_name);
                if ui.button("添加").clicked() && !self.settings.dept_name.is_empty() {
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
                ui.text_edit_singleline(&mut self.settings.user_name);
                ui.label("姓名");
                ui.text_edit_singleline(&mut self.settings.user_display);
                ui.label("密码");
                ui.add(egui::TextEdit::singleline(&mut self.settings.user_password).password(true));
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
            if self.role() != "super" {
                return;
            }
            ui.separator();
            ui.heading("内容组与授权");
            for g in self.groups.clone() {
                ui.label(format!("· {}", s(&g, "name")));
            }
            ui.horizontal(|ui| {
                ui.label("新内容组");
                ui.text_edit_singleline(&mut self.settings.group_name);
                if ui.button("添加").clicked() && !self.settings.group_name.is_empty() {
                    // group creation endpoint is POST /v1/groups (super only)
                    match self.call("POST", "/v1/groups", Some(json!({ "name": self.settings.group_name }))) {
                        Ok(_) => {
                            self.settings.group_name.clear();
                            self.refresh_all();
                        }
                        Err(e) => self.msg = e,
                    }
                }
            });
            if ui.button("载入授权列表").clicked() {
                match self.call("GET", "/v1/grants", None) {
                    Ok(v) => self.grants = v.get("grants").and_then(|g| g.as_array()).cloned().unwrap_or_default(),
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
                    .selected_text(self.group_name(&self.settings.grant_group.clone()))
                    .show_ui(ui, |ui| {
                        for g in self.groups.clone() {
                            ui.selectable_value(&mut self.settings.grant_group, s(&g, "id"), s(&g, "name"));
                        }
                    });
                egui::ComboBox::from_id_salt("grant_st")
                    .selected_text(self.settings.grant_subject_type.clone())
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut self.settings.grant_subject_type, "dept".into(), "部门");
                        ui.selectable_value(&mut self.settings.grant_subject_type, "user".into(), "个人");
                    });
                ui.text_edit_singleline(&mut self.settings.grant_subject_id);
                egui::ComboBox::from_id_salt("grant_cap")
                    .selected_text(self.settings.grant_capability.clone())
                    .show_ui(ui, |ui| {
                        for (v, l) in [("view", "查看"), ("use", "使用"), ("publish", "发布"), ("manage", "管理")] {
                            ui.selectable_value(&mut self.settings.grant_capability, v.into(), l);
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
            ui.separator();
            ui.heading("公司公共资料");
            for (key, label) in [
                ("phone", "公司电话"),
                ("website", "官网"),
                ("address", "地址"),
                ("gongzhonghao", "公众号"),
            ] {
                ui.horizontal(|ui| {
                    ui.label(label);
                    let v = self.settings.company.entry(key.to_string()).or_default();
                    ui.text_edit_singleline(v);
                });
            }
            if ui.button("保存公司资料").clicked() {
                let fields: Value = json!(self.settings.company);
                match self.call("PUT", "/v1/company-profile", Some(json!({ "fields": fields }))) {
                    Ok(_) => self.msg = "已保存".into(),
                    Err(e) => self.msg = e,
                }
            }
            ui.horizontal(|ui| {
                ui.label("内容语言清单");
                ui.text_edit_singleline(&mut self.settings.languages_text);
                if ui.button("保存").clicked() {
                    let langs: Vec<String> = self
                        .settings
                        .languages_text
                        .split(',')
                        .map(|x| x.trim().to_string())
                        .filter(|x| !x.is_empty())
                        .collect();
                    match self.call("PUT", "/v1/settings/languages", Some(json!({ "languages": langs }))) {
                        Ok(_) => self.msg = "已保存".into(),
                        Err(e) => self.msg = e,
                    }
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

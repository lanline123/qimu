use super::*;
use qimu_core::model::IndexEntry;
use std::collections::{BTreeMap, HashSet};

#[derive(Clone, Copy, PartialEq, Eq)]
enum ShareMode {
    Person,
    Department,
    Group,
    Link,
}

pub(super) struct ShareDialogState {
    item_id: String,
    title: String,
    meta: String,
    mode: ShareMode,
    target: String,
    note: String,
    search: String,
    contacts: Vec<Value>,
    reload_contacts: bool,
    link: String,
    link_error: String,
}

pub(super) struct PullDialogState {
    share_id: Option<String>,
    item_kind: String,
    item_id: String,
    title: String,
    meta: String,
    folder: String,
    new_folder: String,
    folders: Vec<String>,
}

pub(super) struct PushDialogState {
    index_id: String,
    title: String,
    published_version: Option<String>,
    published_tag_id: Option<String>,
    pinned_versions: Vec<(IndexEntry, String)>,
    rows: Vec<(String, Option<String>, String)>,
    initial: bool,
    note: String,
}

/// File-name stem for a generated share artifact: deck
/// titles are user text, so path separators and control
/// characters fold to underscores; a title with nothing
/// usable left falls back to a neutral stem.
fn sanitize_file_stem(title: &str) -> String {
    let cleaned: String = title
        .chars()
        .map(|c| {
            if c.is_control()
                || matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|')
            {
                '_'
            } else {
                c
            }
        })
        .collect();
    let trimmed = cleaned.trim().trim_matches('.');
    if trimmed.is_empty() {
        "分享稿件".to_string()
    } else {
        trimmed.to_string()
    }
}

fn department_depth(departments: &[Value], id: &str) -> usize {
    let mut depth = 0;
    let mut current = id.to_string();
    for _ in 0..departments.len() {
        let Some(dept) = departments.iter().find(|dept| s(dept, "id") == current) else {
            break;
        };
        let Some(parent) = dept["parent_id"].as_str() else {
            break;
        };
        depth += 1;
        current = parent.to_string();
    }
    depth
}

fn resolved_version(rows: &[Value]) -> Vec<(String, String, String)> {
    let mut occurrences = BTreeMap::<String, usize>::new();
    let mut out = Vec::with_capacity(rows.len());
    for row in rows {
        let entry = &row["entry"];
        let base = format!("{}:{}", s(entry, "unit_id"), s(entry, "rendition_id"));
        let occurrence = occurrences.entry(base.clone()).or_default();
        let key = format!("{base}#{occurrence}");
        *occurrence += 1;
        out.push((
            key,
            row["unit_name"]
                .as_str()
                .unwrap_or("未命名条目")
                .to_string(),
            row["resolved"]["semver"].as_str().unwrap_or("").to_string(),
        ));
    }
    out
}

pub(super) fn preview_pins(rows: &[Value]) -> Result<Vec<(IndexEntry, String)>, String> {
    rows.iter()
        .map(|row| {
            let entry = serde_json::from_value(row["entry"].clone())
                .map_err(|_| "推送摘要缺少内容块条目".to_string())?;
            let version = s(&row["resolved"], "id");
            if version.is_empty() {
                return Err("内容块尚无已发布版本，请先发布后再推送".into());
            }
            Ok((entry, version))
        })
        .collect()
}

impl App {
    pub(super) fn prototype_dialogs_ui(&mut self, ctx: &egui::Context) {
        self.share_dialog_ui(ctx);
        self.pull_dialog_ui(ctx);
        self.push_dialog_ui(ctx);
    }

    pub(super) fn open_share_dialog(&mut self, item: &Value) {
        // The dialog hosts two coexisting share forms (user
        // ruling 2026-11-24): the server forms (个人 / 部门 /
        // 群组, POST /v1/shares) and the export-style 链接
        // form, which is login-free and works offline. The
        // dialog therefore opens unconditionally and starts on
        // the 链接 tab whenever the server forms are not usable
        // right now; the server forms keep their gates at the
        // send point in share_dialog_ui (the org-online
        // re-check from audit issue 4, plus the identity check
        // that used to sit here).
        let server_ready = self.org_online() && !self.anon_active();
        let pages = i(item, "page_count").max(0);
        let meta = if pages > 0 {
            format!("{} 页 · 维护人 {}", pages, s(item, "submitted_by"))
        } else {
            format!("维护人 {}", s(item, "submitted_by"))
        };
        let mut state = ShareDialogState {
            item_id: s(item, "id"),
            title: s(item, "title"),
            meta,
            mode: if server_ready {
                ShareMode::Person
            } else {
                ShareMode::Link
            },
            target: String::new(),
            note: String::new(),
            search: String::new(),
            contacts: Vec::new(),
            reload_contacts: server_ready,
            link: String::new(),
            link_error: String::new(),
        };
        if server_ready {
            if let Ok(response) = self.call("GET", "/v1/org/contacts", None) {
                state.contacts = response["users"].as_array().cloned().unwrap_or_default();
                state.target = state
                    .contacts
                    .iter()
                    .find(|user| s(user, "id") != self.my_id)
                    .map(|user| s(user, "id"))
                    .unwrap_or_default();
                state.reload_contacts = false;
            }
        }
        self.share_dialog = Some(state);
    }

    fn share_dialog_ui(&mut self, ctx: &egui::Context) {
        let Some(mut state) = self.share_dialog.take() else {
            return;
        };
        let mut submit = false;
        let mut copy_link = false;
        let mut want_link_gen = false;
        let dialog = egui::Modal::new(egui::Id::unique("share-deck")).show(ctx, |ui| {
            ui.set_width(620.0);
            ui.heading(tr("分享这套演示稿"));
            ui.label(
                egui::RichText::new(format!("「{}」· {}", state.title, state.meta))
                    .size(12.0)
                    .color(palette_color(TEXT_WEAK)),
            );
            ui.add_space(8.0);
            let previous_mode = state.mode;
            ui.horizontal(|ui| {
                for (mode, label) in [
                    (ShareMode::Person, "个人"),
                    (ShareMode::Department, "部门"),
                    (ShareMode::Group, "群组"),
                    (ShareMode::Link, "链接"),
                ] {
                    ui.selectable_value(&mut state.mode, mode, tr(label));
                }
            });
            if state.mode != previous_mode {
                state.target.clear();
                state.search.clear();
                state.reload_contacts = state.mode == ShareMode::Person;
            }
            // The 链接 form generates its artifact after the
            // modal, in the same deferred slot as the contacts
            // reload: generation may assemble the deck and call
            // the server fallback, neither of which belongs in
            // the modal closure.
            want_link_gen = state.mode == ShareMode::Link
                && state.link.is_empty()
                && state.link_error.is_empty();
            ui.add_space(6.0);
            match state.mode {
                ShareMode::Person => {
                    ui.horizontal(|ui| {
                        let response = ui.add_sized(
                            [ui.available_width() - 78.0, 28.0],
                            egui::TextEdit::singleline(&mut state.search)
                                .hint_text(tr("搜索公司通讯录姓名或账号")),
                        );
                        if (response.lost_focus()
                            && ui.input(|input| input.key_pressed(egui::Key::Enter)))
                            || ui.button(tr("搜索")).clicked()
                        {
                            state.reload_contacts = true;
                        }
                    });
                    ui.label(
                        egui::RichText::new(tr("公司通讯录"))
                            .size(11.5)
                            .color(palette_color(TEXT_WEAK)),
                    );
                    if !self.my_contacts.is_empty() {
                        ui.horizontal_wrapped(|ui| {
                            ui.label(
                                egui::RichText::new(tr("常用联系人："))
                                    .size(11.0)
                                    .color(palette_color(TEXT_WEAK)),
                            );
                            for name in &self.my_contacts {
                                let matched = state.contacts.iter().find(|c| {
                                    s(c, "display_name") == *name || s(c, "username") == *name
                                });
                                let selected = matched
                                    .map(|c| s(c, "id") == state.target)
                                    .unwrap_or(false);
                                if ui.selectable_label(selected, name).clicked() {
                                    if let Some(c) = matched {
                                        state.target = s(c, "id");
                                    } else {
                                        state.search = name.clone();
                                        state.reload_contacts = true;
                                    }
                                }
                            }
                        });
                        ui.add_space(4.0);
                    }
                    egui::ScrollArea::vertical()
                        .id_salt("share-contacts")
                        .max_height(190.0)
                        .show(ui, |ui| {
                            if state.contacts.is_empty() {
                                if self.role() == "member" {
                                    ui.label(tr("普通成员不可浏览公司通讯录，可切换到群组分享。"));
                                } else {
                                    ui.label(tr("没有匹配的同事"));
                                }
                            }
                            for contact in &state.contacts {
                                let id = s(contact, "id");
                                let selected = state.target == id;
                                let text = format!(
                                    "{}  ·  {}",
                                    s(contact, "display_name"),
                                    if s(contact, "dept_name").is_empty() {
                                        s(contact, "username")
                                    } else {
                                        s(contact, "dept_name")
                                    }
                                );
                                if ui
                                    .add(
                                        egui::Button::new(egui::RichText::new(text).color(
                                            palette_color(if selected {
                                                ACCENT_DEEP
                                            } else {
                                                TEXT_MAIN
                                            }),
                                        ))
                                        .fill(
                                            if selected {
                                                palette_color(SEL_BG)
                                            } else {
                                                egui::Color32::TRANSPARENT
                                            },
                                        ),
                                    )
                                    .clicked()
                                {
                                    state.target = id;
                                }
                            }
                        });
                }
                ShareMode::Department => {
                    let departments = self.org["departments"]
                        .as_array()
                        .cloned()
                        .unwrap_or_default();
                    egui::ScrollArea::vertical()
                        .id_salt("share-departments")
                        .max_height(220.0)
                        .show(ui, |ui| {
                            for dept in departments {
                                let id = s(&dept, "id");
                                let depth = department_depth(
                                    self.org["departments"]
                                        .as_array()
                                        .map(Vec::as_slice)
                                        .unwrap_or_default(),
                                    &id,
                                );
                                let selected = state.target == id;
                                if ui
                                    .add(
                                        egui::Button::new(format!(
                                            "{}{}",
                                            "　".repeat(depth),
                                            s(&dept, "name")
                                        ))
                                        .fill(
                                            if selected {
                                                palette_color(SEL_BG)
                                            } else {
                                                egui::Color32::TRANSPARENT
                                            },
                                        ),
                                    )
                                    .clicked()
                                {
                                    state.target = id;
                                }
                            }
                        });
                }
                ShareMode::Group => {
                    egui::ScrollArea::vertical()
                        .id_salt("share-groups")
                        .max_height(220.0)
                        .show(ui, |ui| {
                            if self.my_teams.is_empty() {
                                ui.label(tr("还没有加入任何群组"));
                            }
                            for group in &self.my_teams {
                                let id = s(group, "id");
                                let selected = state.target == id;
                                let member_count = group["members"]
                                    .as_array()
                                    .map(Vec::len)
                                    .unwrap_or_default();
                                if ui
                                    .add(
                                        egui::Button::new(format!(
                                            "{} · {} 位成员",
                                            s(group, "name"),
                                            member_count
                                        ))
                                        .fill(
                                            if selected {
                                                palette_color(SEL_BG)
                                            } else {
                                                egui::Color32::TRANSPARENT
                                            },
                                        ),
                                    )
                                    .clicked()
                                {
                                    state.target = id;
                                }
                            }
                        });
                }
                ShareMode::Link => {
                    if !state.link.is_empty() {
                        ui.horizontal(|ui| {
                            let mut shown = state.link.clone();
                            ui.add_sized(
                                [ui.available_width() - 92.0, 28.0],
                                egui::TextEdit::singleline(&mut shown),
                            );
                            if ui.button(tr("复制链接")).clicked() {
                                copy_link = true;
                            }
                        });
                        ui.label(
                            egui::RichText::new(tr(
                                "链接指向本机生成的分享文件；把文件发给同事，对方导入即可打开。",
                            ))
                            .size(11.5)
                            .color(palette_color(TEXT_WEAK)),
                        );
                    } else if !state.link_error.is_empty() {
                        ui.label(
                            egui::RichText::new(format!(
                                "分享文件生成失败：{}",
                                state.link_error
                            ))
                            .size(12.0)
                            .color(palette_color(TEXT_WEAK)),
                        );
                        if ui.button(tr("重新生成")).clicked() {
                            state.link_error.clear();
                        }
                    } else {
                        ui.label(tr("正在生成分享文件…"));
                    }
                }
            }
            ui.add_space(8.0);
            ui.label(tr("附言（可选）"));
            ui.add_sized(
                [ui.available_width(), 68.0],
                egui::TextEdit::multiline(&mut state.note).hint_text(tr("给收件人补充说明")),
            );
            ui.add_space(10.0);
            ui.horizontal(|ui| {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let can_send = if state.mode == ShareMode::Link {
                        !state.link.is_empty()
                    } else {
                        !state.target.is_empty()
                    };
                    if ui
                        .add_enabled(can_send, egui::Button::new(tr("发送分享")))
                        .clicked()
                    {
                        submit = true;
                    }
                    if btn_default(ui, tr("取消")).clicked() {
                        ui.close();
                    }
                });
            });
        });
        if copy_link {
            ctx.copy_text(state.link.clone());
            self.msg = "链接已复制".into();
        }
        if state.reload_contacts {
            let query = state.search.trim().to_string();
            if let Ok(response) = self.call(
                "GET",
                &format!("/v1/org/contacts?q={}", query_component(&query)),
                None,
            ) {
                state.contacts = response["users"].as_array().cloned().unwrap_or_default();
                if !state
                    .contacts
                    .iter()
                    .any(|user| s(user, "id") == state.target)
                {
                    state.target = state
                        .contacts
                        .iter()
                        .find(|user| s(user, "id") != self.my_id)
                        .map(|user| s(user, "id"))
                        .unwrap_or_default();
                }
            }
            state.reload_contacts = false;
        }
        if dialog.should_close() {
            return;
        }
        if want_link_gen {
            self.generate_share_link(&mut state);
        }
        if submit {
            // The 链接 form's send button copies the link and
            // closes (prototype shareDlg: shareOk in link mode
            // is doShareLink). No server round-trip and no gate:
            // the artifact was generated locally.
            if state.mode == ShareMode::Link {
                ctx.copy_text(state.link.clone());
                self.msg = "链接已复制".into();
                return;
            }
            // Execution-point re-check (audit issue 4): the
            // dialog may have been opened while online and the
            // network dropped since — an open-time gate is not
            // enough. On a failed check the dialog stays open
            // with its draft intact. The identity check sits at
            // this same point: since the 链接 form landed
            // (2026-11-24) the dialog opens without gates so
            // the link form stays login-free, and the server
            // forms enforce both gates here instead.
            if !self.require_org_online("分享给…") {
                self.share_dialog = Some(state);
                return;
            }
            if !self.need_identity("分享给…") {
                self.share_dialog = Some(state);
                return;
            }
            let target_kind = match state.mode {
                ShareMode::Person => "user",
                ShareMode::Department => "dept",
                ShareMode::Group => "group",
                ShareMode::Link => unreachable!(),
            };
            match self.call(
                "POST",
                "/v1/shares",
                Some(json!({
                    "item_kind": "showroom",
                    "item_id": state.item_id,
                    "target_kind": target_kind,
                    "target_id": state.target,
                    "note": state.note.trim(),
                })),
            ) {
                Ok(_) => self.msg = "分享已发送".into(),
                Err(error) => self.msg = format!("分享失败：{error}"),
            }
            return;
        }
        self.share_dialog = Some(state);
    }

    /// The 链接 share form (the prototype shareDlg's fourth
    /// tab, restored 2026-11-24): generate the share artifact
    /// locally — the deck's PPTX, acquired exactly the way
    /// export_showroom_pptx acquires it, minus the save
    /// dialog — write it into the share folder under the
    /// cache dir, and present its file:// link for copying.
    /// The prototype's link pointed at a LAN server that
    /// would serve the share; this phase has no anonymous
    /// share-serving endpoint, so the link points at the
    /// generated file itself: the user sends the file on,
    /// the recipient imports it. No login and no online
    /// gate — with the deck in the local library the whole
    /// form works offline.
    fn generate_share_link(&mut self, state: &mut ShareDialogState) {
        match self.share_link_bytes(&state.item_id) {
            Ok(bytes) => {
                let dir = cache_dir().join("share");
                let path = dir.join(format!("{}.pptx", sanitize_file_stem(&state.title)));
                match std::fs::create_dir_all(&dir).and_then(|_| std::fs::write(&path, &bytes))
                {
                    Ok(()) => {
                        let shown = path.display().to_string().replace('\\', "/");
                        state.link = if shown.starts_with('/') {
                            format!("file://{shown}")
                        } else {
                            format!("file:///{shown}")
                        };
                        state.link_error.clear();
                    }
                    Err(e) => state.link_error = format!("写入失败：{e}"),
                }
            }
            Err(e) => state.link_error = e,
        }
    }

    /// The PPTX bytes behind the 链接 share form: local
    /// assembly first (a deck in the local library), the
    /// server's export as the fallback — the acquisition
    /// half of export_showroom_pptx without its save-dialog
    /// landing.
    fn share_link_bytes(&mut self, item_id: &str) -> Result<Vec<u8>, String> {
        if let Some(local) = &self.local {
            match local.assemble_showroom_deck(item_id, false) {
                Some(Ok((bytes, _page_count))) => return Ok(bytes),
                Some(Err(e)) => {
                    eprintln!("[qimu] 本地组装分享稿件失败，回退服务端导出：{e}");
                }
                None => {}
            }
        }
        let v = self.call(
            "POST",
            &format!("/v1/showroom/{item_id}/export"),
            Some(json!({ "kind": "personal" })),
        )?;
        let data = v
            .get("data_base64")
            .and_then(|x| x.as_str())
            .ok_or_else(|| "服务端导出未返回文件内容".to_string())?;
        b64d(data)
    }

    pub(super) fn open_pull_dialog(&mut self, item: &Value, share_id: Option<String>) {
        if !self.require_org_online("保存到我的库") {
            return;
        }
        if !self.need_identity("保存到我的库") {
            return;
        }
        let item_kind = share_id.as_ref().map_or_else(
            || "showroom".to_string(),
            |_| item["item_kind"].as_str().unwrap_or("showroom").to_string(),
        );
        let item_id = if share_id.is_some() {
            s(item, "item_id")
        } else {
            s(item, "id")
        };
        let mut folders = self
            .library_refs
            .iter()
            .map(|entry| s(entry, "folder"))
            .filter(|folder| !folder.is_empty())
            .collect::<HashSet<_>>()
            .into_iter()
            .collect::<Vec<_>>();
        folders.sort();
        let folder = folders
            .first()
            .cloned()
            .unwrap_or_else(|| "默认目录".into());
        let meta = if share_id.is_some() {
            format!("来自 {} 的分享 · 保存后正式进库", s(item, "from_name"))
        } else {
            format!("维护人 {}", s(item, "submitted_by"))
        };
        self.pull_dialog = Some(PullDialogState {
            share_id,
            item_kind,
            item_id,
            title: s(item, "title"),
            meta,
            folder,
            new_folder: String::new(),
            folders,
        });
    }

    fn pull_dialog_ui(&mut self, ctx: &egui::Context) {
        let Some(mut state) = self.pull_dialog.take() else {
            return;
        };
        let mut submit = false;
        let dialog = egui::Modal::new(egui::Id::unique("pull-into-library")).show(ctx, |ui| {
            ui.set_width(460.0);
            ui.heading(format!("{} · {}", tr("保存到我的库"), state.title));
            ui.label(
                egui::RichText::new(format!("{} · 引用，跟随来源更新", state.meta))
                    .size(12.0)
                    .color(palette_color(TEXT_WEAK)),
            );
            ui.add_space(8.0);
            ui.label(tr("选择“我的哪个目录”"));
            let mut folders = state.folders.clone();
            if !folders.iter().any(|folder| folder == &state.folder) {
                folders.push(state.folder.clone());
            }
            for folder in folders {
                ui.selectable_value(&mut state.folder, folder.clone(), folder);
            }
            ui.add_space(6.0);
            ui.label(tr("或输入新目录"));
            ui.add_sized(
                [ui.available_width(), 28.0],
                egui::TextEdit::singleline(&mut state.new_folder).hint_text(tr("新目录名称")),
            );
            ui.add_space(12.0);
            ui.horizontal(|ui| {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if btn_primary(ui, tr("保存到这个目录")).clicked() {
                        submit = true;
                    }
                    if btn_default(ui, tr("取消")).clicked() {
                        ui.close();
                    }
                });
            });
        });
        if dialog.should_close() {
            return;
        }
        if submit {
            // Execution-point re-check (audit issue 4), same as
            // the share dialog: confirm-time, not open-time. On a
            // failed check the dialog stays open with its draft
            // intact.
            if !self.require_org_online("保存到我的库") {
                self.pull_dialog = Some(state);
                return;
            }
            let folder = if state.new_folder.trim().is_empty() {
                state.folder.clone()
            } else {
                state.new_folder.trim().to_string()
            };
            let result = match &state.share_id {
                Some(share_id) => self.call(
                    "POST",
                    &format!("/v1/shares/{share_id}/accept"),
                    Some(json!({ "folder": folder })),
                ),
                None => self.call(
                    "POST",
                    "/v1/library",
                    Some(json!({
                        "item_kind": state.item_kind,
                        "item_id": state.item_id,
                        "folder": folder,
                    })),
                ),
            };
            match result {
                Ok(_) => {
                    self.msg = format!("已保存到「{folder}」（引用 · 跟随来源）");
                    self.refresh_all();
                }
                Err(error) => self.msg = format!("保存失败：{error}"),
            }
            return;
        }
        self.pull_dialog = Some(state);
    }

    pub(super) fn open_push_dialog(&mut self, id: &str, title: &str) {
        if !self.require_org_online("推送到演示厅") {
            return;
        }
        if !self.need_identity("推送到演示厅") {
            return;
        }
        let branch = match self.index.stack.top() {
            IndexFrame::Detail(state) => state.branch.clone(),
            _ => "main".into(),
        };
        let mut response = match self.call_work(
            "GET",
            &format!(
                "/v1/indexes/{id}/push-preview?branch={}",
                query_component(&branch)
            ),
            None,
        ) {
            Ok(response) => response,
            Err(error) => {
                self.msg = format!("读取推送摘要失败：{error}");
                return;
            }
        };
        if response["previous_entries"]
            .as_array()
            .is_none_or(Vec::is_empty)
        {
            if let Some(previous) = self
                .showroom_live
                .iter()
                .find(|item| s(item, "source_index_id") == id)
            {
                response["previous_entries"] = previous["resolved_entries"].clone();
            }
        }
        if response["previous_entries"]
            .as_array()
            .is_none_or(Vec::is_empty)
        {
            // F8: a local index that landed through the P3 chain
            // is named by its SUBMISSION's landed ids — the
            // entry's source_index_id is the server-minted index,
            // never the local id the lookup above tries — so the
            // summary showed 「初次上架」 forever. Resolve through
            // my landed submission for this source instead, then
            // take that entry's pinned rows.
            let landed_item_id = self
                .my_submissions
                .iter()
                .find(|sub| {
                    s(sub, "source_index_id") == id
                        && s(sub, "state") == "landed"
                        && !s(sub, "landed_showroom_id").is_empty()
                })
                .map(|sub| s(sub, "landed_showroom_id"));
            if let Some(item_id) = landed_item_id {
                if let Some(previous) = self
                    .showroom_live
                    .iter()
                    .find(|item| s(item, "id") == item_id)
                {
                    response["previous_entries"] =
                        previous["resolved_entries"].clone();
                }
            }
        }
        let current_rows = response["current_entries"]
            .as_array()
            .map(Vec::as_slice)
            .unwrap_or_default();
        let pinned_versions = match preview_pins(current_rows) {
            Ok(pins) => pins,
            Err(error) => {
                self.msg = error;
                return;
            }
        };
        let current = resolved_version(current_rows);
        let previous = resolved_version(
            response["previous_entries"]
                .as_array()
                .map(Vec::as_slice)
                .unwrap_or_default(),
        );
        let initial = previous.is_empty();
        let previous_versions = previous
            .iter()
            .map(|(key, _, version)| (key.as_str(), version.as_str()))
            .collect::<BTreeMap<_, _>>();
        let rows = current
            .iter()
            .filter(|(key, _, version)| {
                initial
                    || previous_versions
                        .get(key.as_str())
                        .is_none_or(|old| *old != version)
            })
            .map(|(key, title, version)| {
                (
                    title.clone(),
                    previous_versions
                        .get(key.as_str())
                        .map(|old| (*old).to_string()),
                    version.clone(),
                )
            })
            .collect();
        self.push_dialog = Some(PushDialogState {
            index_id: id.to_string(),
            title: title.to_string(),
            published_version: response["published_version"].as_str().map(str::to_owned),
            published_tag_id: response["published_tag_id"].as_str().map(str::to_owned),
            pinned_versions,
            rows,
            initial,
            note: String::new(),
        });
    }

    fn push_dialog_ui(&mut self, ctx: &egui::Context) {
        let Some(mut state) = self.push_dialog.take() else {
            return;
        };
        let mut submit = false;
        let dialog = egui::Modal::new(egui::Id::unique("push-index-check")).show(ctx, |ui| {
            ui.set_width(580.0);
            ui.heading(format!("{} · {}", tr("推送到演示厅"), state.title));
            match &state.published_version {
                Some(version) => {
                    ui.label(format!(
                        "{} {version} · {}",
                        tr("当前发布版"),
                        tr("上架取当前发布版，演示厅审核通过后生效")
                    ));
                }
                None => {
                    ui.label(
                        egui::RichText::new(tr("尚未发布索引版本，请先发布后再推送。"))
                            .color(palette_color(ROSE)),
                    );
                }
            }
            ui.add_space(6.0);
            if state.rows.is_empty() {
                ui.label(tr("内容块版本没有变化 · 将按当前发布版重新推送。"));
            } else {
                ui.label(if state.initial {
                    tr("初次上架 · 全部条目")
                } else {
                    tr("自上次发布以来的内容块变化")
                });
                egui::ScrollArea::vertical()
                    .id_salt("push-change-rows")
                    .max_height(230.0)
                    .show(ui, |ui| {
                        for (title, from, to) in &state.rows {
                            ui.horizontal(|ui| {
                                ui.label(egui::RichText::new(title).strong());
                                ui.with_layout(
                                    egui::Layout::right_to_left(egui::Align::Center),
                                    |ui| {
                                        if let Some(from) = from {
                                            ui.label(format!("{from} → {to}"));
                                        } else {
                                            ui.label(format!("{} {to}", tr("初次纳入")));
                                        }
                                    },
                                );
                            });
                            ui.separator();
                        }
                    });
            }
            ui.add_space(6.0);
            ui.label(tr("发布说明"));
            ui.add_sized(
                [ui.available_width(), 62.0],
                egui::TextEdit::multiline(&mut state.note).hint_text(tr("本次推送说明")),
            );
            ui.add_space(10.0);
            ui.horizontal(|ui| {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui
                        .add_enabled(
                            state.published_tag_id.is_some(),
                            egui::Button::new(tr("确认推送")),
                        )
                        .clicked()
                    {
                        submit = true;
                    }
                    if btn_default(ui, tr("取消")).clicked() {
                        ui.close();
                    }
                });
            });
        });
        if dialog.should_close() {
            return;
        }
        if submit {
            self.push_index_to_showroom(
                &state.index_id,
                &state.title,
                state.published_tag_id.as_deref().unwrap_or_default(),
                &state.pinned_versions,
                state.note.trim(),
            );
            return;
        }
        self.push_dialog = Some(state);
    }
}

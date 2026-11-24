use super::*;
use std::collections::{BTreeMap, HashSet};

#[derive(Clone, Copy, PartialEq, Eq)]
enum ShareMode {
    Person,
    Department,
    Group,
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
    rows: Vec<(String, Option<String>, String)>,
    initial: bool,
    note: String,
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

impl App {
    pub(super) fn prototype_dialogs_ui(&mut self, ctx: &egui::Context) {
        self.share_dialog_ui(ctx);
        self.pull_dialog_ui(ctx);
        self.push_dialog_ui(ctx);
    }

    pub(super) fn open_share_dialog(&mut self, item: &Value) {
        if !self.need_identity("分享给…") {
            return;
        }
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
            mode: ShareMode::Person,
            target: String::new(),
            note: String::new(),
            search: String::new(),
            contacts: Vec::new(),
            reload_contacts: true,
        };
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
        self.share_dialog = Some(state);
    }

    fn share_dialog_ui(&mut self, ctx: &egui::Context) {
        let Some(mut state) = self.share_dialog.take() else {
            return;
        };
        let mut submit = false;
        let dialog = egui::Modal::new(egui::Id::unique("share-deck")).show(ctx, |ui| {
            ui.set_width(620.0);
            ui.heading(tr("分享这套演示稿"));
            ui.label(
                egui::RichText::new(format!("「{}」· {}", state.title, state.meta))
                    .size(12.0)
                    .color(palette_color(TEXT_WEAK)),
            );
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                for (mode, label) in [
                    (ShareMode::Person, "个人"),
                    (ShareMode::Department, "部门"),
                    (ShareMode::Group, "群组"),
                ] {
                    ui.selectable_value(&mut state.mode, mode, tr(label));
                }
            });
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
                    egui::ScrollArea::vertical()
                        .id_salt("share-contacts")
                        .max_height(190.0)
                        .show(ui, |ui| {
                            if state.contacts.is_empty() {
                                ui.label(tr("没有匹配的同事"));
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
                            for group in self.groups.clone() {
                                let id = s(&group, "id");
                                let selected = state.target == id;
                                if ui
                                    .add(
                                        egui::Button::new(format!(
                                            "{} · {} 位成员",
                                            s(&group, "name"),
                                            group["member_count"].as_u64().unwrap_or(0)
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
                    if ui
                        .add_enabled(!state.target.is_empty(), egui::Button::new(tr("发送分享")))
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
        if submit {
            let target_kind = match state.mode {
                ShareMode::Person => "user",
                ShareMode::Department => "dept",
                ShareMode::Group => "group",
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

    pub(super) fn open_pull_dialog(&mut self, item: &Value, share_id: Option<String>) {
        if !self.need_identity("收进我的") {
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
            ui.heading(format!("{} · {}", tr("收进我的"), state.title));
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
        if !self.need_identity("推送到演示厅") {
            return;
        }
        let branch = match self.index.stack.top() {
            IndexFrame::Detail(state) => state.branch.clone(),
            _ => "main".into(),
        };
        let response = match self.call(
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
        let current = resolved_version(
            response["current_entries"]
                .as_array()
                .map(Vec::as_slice)
                .unwrap_or_default(),
        );
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
                state.note.trim(),
            );
            return;
        }
        self.push_dialog = Some(state);
    }
}

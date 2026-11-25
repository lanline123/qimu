use super::*;
use qimu_core::model::IndexEntry;

#[derive(Clone, Copy, Default, PartialEq, Eq)]
enum IndexStatus {
    #[default]
    Active,
    History,
    All,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum IndexScope {
    Company,
    ParentOrg,
    DepartmentTree,
    Department,
    Private,
}

impl IndexScope {
    const ALL: [Self; 5] = [
        Self::Company,
        Self::ParentOrg,
        Self::DepartmentTree,
        Self::Department,
        Self::Private,
    ];

    pub(super) fn of(index: &Value) -> Self {
        match index["scope"].as_str() {
            Some("company") => Self::Company,
            Some("parent_org" | "ancestor" | "org") => Self::ParentOrg,
            Some("dept_sub" | "deptsub") => Self::DepartmentTree,
            Some("dept") => Self::Department,
            _ => Self::Private,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Company => "全公司",
            Self::ParentOrg => "上级组织",
            Self::DepartmentTree => "本部门及子部门",
            Self::Department => "仅本部门",
            Self::Private => "未公开",
        }
    }

    pub(super) fn color(self) -> egui::Color32 {
        let (r, g, b) = match self {
            Self::Company => (0x3B, 0x82, 0xC4),
            Self::ParentOrg => (0x8B, 0x7C, 0xC9),
            Self::DepartmentTree => (0x4D, 0xA6, 0x74),
            Self::Department => (0xD0, 0x8A, 0x3E),
            Self::Private => (0x8A, 0x93, 0x8C),
        };
        egui::Color32::from_rgb(r, g, b)
    }
}

#[derive(Default)]
pub(super) struct IndexOverviewControls {
    status: IndexStatus,
    scope: Option<IndexScope>,
    pub(super) relationships: bool,
}

impl IndexOverviewControls {
    pub(super) fn matches(&self, index: &Value) -> bool {
        let status = index["lifecycle"].as_str();
        let status_ok = match self.status {
            IndexStatus::Active => matches!(status, None | Some("" | "active")),
            IndexStatus::History => status == Some("history"),
            IndexStatus::All => true,
        };
        status_ok
            && self
                .scope
                .is_none_or(|scope| scope == IndexScope::of(index))
    }

    fn ui(&mut self, ui: &mut egui::Ui, icons: ChromeIcons, enabled: bool) {
        for (relationship, icon, label) in [
            (true, ChromeIcon::Lineage, "派生关系"),
            (false, ChromeIcon::Directory, "按目录"),
        ] {
            if icons
                .tool(
                    ui,
                    icon,
                    label,
                    enabled,
                    self.relationships == relationship,
                    false,
                )
                .clicked()
            {
                self.relationships = relationship;
            }
        }
        tool_vsep(ui);
        ui.add_enabled_ui(enabled, |ui| {
            for scope in IndexScope::ALL.into_iter().rev() {
                let selected = enabled && self.scope == Some(scope);
                let (rect, response) =
                    ui.allocate_exact_size(egui::vec2(18.0, 18.0), egui::Sense::click());
                response.widget_info(|| {
                    egui::WidgetInfo::selected(
                        egui::Role::Button,
                        enabled,
                        selected,
                        tr(scope.label()),
                    )
                });
                if selected || response.hovered() || response.has_focus() {
                    ui.painter().rect_stroke(
                        rect,
                        5.0,
                        egui::Stroke::new(1.0, ACCENT),
                        egui::StrokeKind::Inside,
                    );
                }
                let color = if enabled {
                    scope.color()
                } else {
                    palette_color(TEXT_WEAK)
                };
                ui.painter().circle_filled(rect.center(), 5.0, color);
                if response.on_hover_text(tr(scope.label())).clicked() {
                    self.scope = (!selected).then_some(scope);
                }
            }
        });
        tool_vsep(ui);
        for (status, icon, label) in [
            (IndexStatus::All, ChromeIcon::All, "全部"),
            (IndexStatus::History, ChromeIcon::History, "历史"),
            (IndexStatus::Active, ChromeIcon::Active, "使用中"),
        ] {
            if icons
                .tool(ui, icon, label, enabled, self.status == status, false)
                .clicked()
            {
                self.status = status;
            }
        }
    }
}

pub(super) enum IndexMoreAction {
    English(String),
    Move(Option<String>),
    History,
    Delete,
}

impl App {
    pub(super) fn index_toolbar(&mut self, ui: &mut egui::Ui) {
        let editor =
            self.page == Page::Index && matches!(self.index.stack.top(), IndexFrame::Detail(_));
        let overview =
            self.page == Page::Index && matches!(self.index.stack.top(), IndexFrame::Overview(_));
        let can_merge = editor
            && matches!(self.index.stack.top(), IndexFrame::Detail(st)
            if st.detail["branches"].as_array().is_some_and(|branches| branches.len() > 1));
        let icons = ChromeIcons;
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let more = icons.tool(ui, ChromeIcon::More, "更多操作", editor, false, true);
            let mut selected = None;
            if editor {
                egui::Popup::menu(&more).show(|ui| {
                    ui.set_min_width(156.0);
                    for (index, label) in ["制作英文版", "移动到其他文件夹", "转为历史", "删除索引"]
                        .iter()
                        .enumerate()
                    {
                        let enabled = index != 2
                            || matches!(self.index.stack.top(), IndexFrame::Detail(st)
                            if st.detail["index"]["lifecycle"] == "active");
                        if ui
                            .add_enabled(enabled, egui::Button::new(tr(label)))
                            .clicked()
                        {
                            selected = Some(index);
                            ui.close();
                        }
                    }
                });
            }
            let merge = icons
                .tool(ui, ChromeIcon::Merge, "发起合并", can_merge, false, true)
                .clicked();
            let publish = icons
                .tool(ui, ChromeIcon::Publish, "发布版本", editor, false, true)
                .clicked();
            let push = icons
                .tool(ui, ChromeIcon::Push, "提交到演示厅", editor, false, true)
                .clicked();
            if let IndexFrame::Detail(st) = self.index.stack.top_mut() {
                if let Some(selected) = selected {
                    let index = &st.detail["index"];
                    st.more_action = Some(match selected {
                        0 => IndexMoreAction::English(format!("{} · EN", s(index, "name"))),
                        1 => IndexMoreAction::Move(index["folder_id"].as_str().map(str::to_owned)),
                        2 => IndexMoreAction::History,
                        _ => IndexMoreAction::Delete,
                    });
                }
                if merge {
                    st.view = IndexDetailView::Commits;
                    st.focus_merge = true;
                }
                if publish {
                    st.focus_tag = true;
                }
                if push {
                    let id = st.index_id.clone();
                    let title = s(&st.detail["index"], "name");
                    self.open_push_dialog(&id, &title);
                }
            }
            tool_vsep(ui);
            self.index.overview_controls.ui(ui, icons, overview);
        });
    }

    /// Submit an index for showroom review (local-first P3,
    /// design §6.3). An index that lives in the local library is
    /// submitted as a real-byte package (POST /v1/submissions):
    /// the content itself travels, waits in the server's staging
    /// zone, and becomes organization content only when an
    /// approval lands it. A resubmission after a rejection
    /// automatically supersedes the rejected row (the server
    /// requires the link). Content that is not in the local
    /// library (organization-side indexes) keeps the legacy
    /// reference submission for this cycle (design §8.3), as does
    /// the whole action while the local library is closed.
    pub(super) fn push_index_to_showroom(
        &mut self,
        id: &str,
        title: &str,
        published_tag_id: &str,
        pinned_versions: &[(IndexEntry, String)],
        note: &str,
    ) {
        if !self.require_org_online("推送到演示厅") {
            return;
        }
        if !self.need_identity("推送到演示厅") {
            return;
        }
        if let Some(local) = &self.local {
            // F9 pre-check (the server enforces the same rule):
            // one pending package per source index — submitting
            // a second one while the first is still in review
            // forks the review state, so point at the standing
            // package instead of building another.
            if let Some(prev) = self.my_submissions.iter().find(|x| {
                s(x, "source_index_id") == id && s(x, "state") == "pending"
            }) {
                self.msg = format!(
                    "该索引已有一份待审提交（{}），请先在「我的提交」中撤回或等待审批完成",
                    s(prev, "title")
                );
                return;
            }
            match local.build_submission_package(id, published_tag_id, pinned_versions) {
                Some(Ok(mut body)) => {
                    body["title"] = json!(title);
                    body["intro"] = json!(note);
                    body["note"] = json!(note);
                    body["source_index_id"] = json!(id);
                    if let Some(prev) = self.my_submissions.iter().find(|x| {
                        s(x, "source_index_id") == id
                            && matches!(s(x, "state").as_str(), "rejected" | "expired")
                    }) {
                        body["supersedes"] = json!(s(prev, "id"));
                    }
                    self.msg = match self.call("POST", "/v1/submissions", Some(body)) {
                        Ok(_) => "已提交，等待审核".into(),
                        Err(error) => error,
                    };
                    return;
                }
                Some(Err(e)) => {
                    self.msg = format!("提交包构建失败：{e}");
                    return;
                }
                None => {}
            }
        }
        // The reference submission carries the push summary's
        // confirmed resolution with it (publish pins): the item
        // the approval lands publishes exactly the versions the
        // summary displayed. Without pins (an empty summary —
        // not expected from the dialog) the field is omitted and
        // the server captures the submit-time resolution.
        let mut body = json!({
            "title": title,
            "intro": note,
            "source_index_id": id,
            "published_tag_id": published_tag_id,
        });
        if !pinned_versions.is_empty() {
            body["pinned_versions"] = json!(pinned_versions
                .iter()
                .map(|(_, v)| v.clone())
                .collect::<Vec<_>>());
        }
        self.msg = match self.call("POST", "/v1/showroom", Some(body)) {
            Ok(_) => "已提交，等待审核".into(),
            Err(error) => error,
        };
    }

    pub(super) fn index_more_dialog(
        &mut self,
        ctx: &egui::Context,
        st: &mut IndexDetailState,
        action: &mut Option<IndexDetailAction>,
    ) {
        let Some(mut pending) = st.more_action.take() else {
            return;
        };
        let title = match pending {
            IndexMoreAction::English(_) => "制作英文版",
            IndexMoreAction::Move(_) => "移动到其他文件夹",
            IndexMoreAction::History => "转为历史",
            IndexMoreAction::Delete => "删除索引",
        };
        let mut submit = false;
        let mut cancel = false;
        let dialog = egui::Modal::new(egui::Id::unique("index-more-action")).show(ctx, |ui| {
            ui.heading(tr(title));
            ui.set_min_width(330.0);
            let can_submit = match &mut pending {
                IndexMoreAction::English(name) => {
                    ui.label(tr("使用各内容块已发布的英文版本创建独立索引。"));
                    ui.text_edit_singleline(name);
                    !name.trim().is_empty()
                }
                IndexMoreAction::Move(folder) => {
                    let mut selected = folder
                        .as_deref()
                        .map(|id| self.folder_name(id))
                        .unwrap_or_else(|| tr("未分类").into());
                    if selected.is_empty() {
                        selected = tr("未分类").into();
                    }
                    egui::ComboBox::from_id_salt("move-index-folder")
                        .width(240.0)
                        .selected_text(selected)
                        .show_ui(ui, |ui| {
                            ui.selectable_value(folder, None, tr("未分类"));
                            // Index-scope folders only (sidebar
                            // isolation, user ruling 2026-11-24):
                            // an index can only home in the index
                            // page's own tree.
                            for item in self
                                .folders
                                .iter()
                                .filter(|f| s(*f, "scope") == "index")
                            {
                                let id = s(item, "id");
                                let mut names = vec![s(item, "name")];
                                let mut parent = item["parent_id"].as_str();
                                for _ in 0..self.folders.len() {
                                    let Some(next) = parent.and_then(|id| {
                                        self.folders.iter().find(|item| item["id"] == id)
                                    }) else {
                                        break;
                                    };
                                    names.push(s(next, "name"));
                                    parent = next["parent_id"].as_str();
                                }
                                names.reverse();
                                ui.selectable_value(folder, Some(id), names.join(" / "));
                            }
                        });
                    folder.as_deref() != st.detail["index"]["folder_id"].as_str()
                }
                IndexMoreAction::History => {
                    ui.label(tr("索引将移到「历史」筛选结果。"));
                    true
                }
                IndexMoreAction::Delete => {
                    ui.label(tr("索引将移到回收站，可从回收站恢复。"));
                    true
                }
            };
            ui.horizontal(|ui| {
                if ui.button(tr("取消")).clicked() {
                    cancel = true;
                }
                if ui
                    .add_enabled(can_submit, egui::Button::new(tr("确认")))
                    .clicked()
                {
                    submit = true;
                }
            });
        });
        if !submit {
            if !cancel && !dialog.should_close() {
                st.more_action = Some(pending);
            }
            return;
        }
        let id = &st.index_id;
        let response = match &pending {
            IndexMoreAction::English(name) => self.call_work(
                "POST",
                &format!("/v1/indexes/{id}/english"),
                Some(json!({ "branch": if st.branch.is_empty() { "main" } else { &st.branch }, "name": name.trim() })),
            ),
            IndexMoreAction::Move(folder) => self.call_work(
                "POST",
                &format!("/v1/indexes/{id}/folder"),
                Some(json!({ "folder_id": folder })),
            ),
            IndexMoreAction::History => self.call_work(
                "POST",
                &format!("/v1/indexes/{id}/lifecycle"),
                Some(json!({ "lifecycle": "history" })),
            ),
            IndexMoreAction::Delete => {
                if self.trash_add("index", id) {
                    st.closed = true;
                } else {
                    st.more_action = Some(pending);
                }
                return;
            }
        };
        match response {
            Ok(result) => {
                self.refresh_all();
                self.msg = format!("{} · {}", tr(title), tr("已完成"));
                if matches!(pending, IndexMoreAction::English(_)) {
                    *action = Some(IndexDetailAction::OpenGraphTarget {
                        index: s(&result, "id"),
                        branch: "main".into(),
                        commit: None,
                    });
                } else {
                    *action = Some(IndexDetailAction::Reload);
                }
            }
            Err(error) => {
                self.msg = error;
                st.more_action = Some(pending);
            }
        }
    }
}

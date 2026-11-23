use super::*;

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

    pub(super) fn ui(&mut self, ui: &mut egui::Ui) {
        let rtl = ui.layout().main_dir() == egui::Direction::RightToLeft;
        if rtl {
            self.view_buttons(ui, true);
            tool_vsep(ui);
            self.scope_buttons(ui, true);
            tool_vsep(ui);
            self.status_buttons(ui, true);
        } else {
            self.status_buttons(ui, false);
            tool_vsep(ui);
            self.scope_buttons(ui, false);
            tool_vsep(ui);
            self.view_buttons(ui, false);
        }
    }

    fn status_buttons(&mut self, ui: &mut egui::Ui, rtl: bool) {
        let mut states = [IndexStatus::Active, IndexStatus::History, IndexStatus::All];
        let mut labels = ["使用中", "历史", "全部"];
        if rtl {
            states.reverse();
            labels.reverse();
        }
        let selected = states
            .iter()
            .position(|state| *state == self.status)
            .unwrap_or(0);
        if let Some(next) = segmented(ui, &labels, selected, SegStyle::Soft) {
            self.status = states[next];
        }
    }

    fn scope_buttons(&mut self, ui: &mut egui::Ui, rtl: bool) {
        let mut scopes = IndexScope::ALL;
        if rtl {
            scopes.reverse();
        }
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 3.0;
            ui.spacing_mut().button_padding.x = 5.0;
            for scope in scopes {
                let selected = self.scope == Some(scope);
                let button = egui::Button::new(egui::RichText::new("●").color(scope.color()))
                    .selected(selected)
                    .min_size(egui::vec2(22.0, 24.0));
                let response = ui.add(button).on_hover_text(tr(scope.label()));
                if response.clicked() {
                    self.scope = (!selected).then_some(scope);
                }
            }
        });
    }

    fn view_buttons(&mut self, ui: &mut egui::Ui, rtl: bool) {
        let mut labels = ["按目录", "派生关系"];
        if rtl {
            labels.reverse();
        }
        if let Some(next) = segmented(
            ui,
            &labels,
            usize::from(self.relationships ^ rtl),
            SegStyle::Soft,
        ) {
            self.relationships = (next == 1) ^ rtl;
        }
    }
}

pub(super) struct BranchLineage {
    name: String,
    head: String,
    inherited: Option<(String, String)>,
}

impl App {
    pub(super) fn index_toolbar(&mut self, ui: &mut egui::Ui) {
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.horizontal(|ui| match self.index.stack.top() {
                IndexFrame::Overview(_) => self.index.overview_controls.ui(ui),
                IndexFrame::Detail(st) => {
                    let can_merge = st.detail["branches"]
                        .as_array()
                        .is_some_and(|branches| branches.len() > 1);
                    let merge = ui
                        .add_enabled(can_merge, egui::Button::new(tr("发起合并")))
                        .on_disabled_hover_text(tr("需要至少两个分支"))
                        .clicked();
                    let publish = ui.button(tr("发布版本")).clicked();
                    let push = ui.button(tr("推送到演示厅")).clicked();
                    if let IndexFrame::Detail(st) = self.index.stack.top_mut() {
                        if merge {
                            st.view = IndexDetailView::Commits;
                            st.focus_merge = true;
                        }
                        if publish {
                            st.focus_tag = true;
                        }
                    }
                    if push {
                        if let IndexFrame::Detail(st) = self.index.stack.top() {
                            let id = st.index_id.clone();
                            let title = s(&st.detail["index"], "name");
                            self.push_index_to_showroom(&id, &title);
                        }
                    }
                }
                IndexFrame::Create(_) => {}
            });
        });
    }

    fn push_index_to_showroom(&mut self, id: &str, title: &str) {
        if !self.need_identity("推送到演示厅") {
            return;
        }
        self.msg = match self.call(
            "POST",
            "/v1/showroom",
            Some(json!({ "title": title, "intro": "", "source_index_id": id })),
        ) {
            Ok(_) => "已推送，等待审核".into(),
            Err(error) => error,
        };
    }

    pub(super) fn load_index_lineage(
        &self,
        id: &str,
        detail: &Value,
    ) -> Result<Vec<BranchLineage>, String> {
        let mut lineage = Vec::new();
        for branch in detail["branches"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
        {
            let response = self.call(
                "GET",
                &format!(
                    "/v1/indexes/{id}/commits?branch={}",
                    query_component(branch)
                ),
                None,
            )?;
            let commits = response["commits"]
                .as_array()
                .map(Vec::as_slice)
                .unwrap_or(&[]);
            // A branch with no commits of its own still points to the
            // inherited commit. Use recorded commit ownership for the edge.
            let inherited = commits.iter().find(|commit| commit["branch"] != branch);
            lineage.push(BranchLineage {
                name: branch.to_owned(),
                head: commits
                    .first()
                    .map(|commit| s(commit, "id"))
                    .unwrap_or_default(),
                inherited: inherited.map(|commit| (s(commit, "branch"), s(commit, "id"))),
            });
        }
        Ok(lineage)
    }

    pub(super) fn index_overview_relationships(
        &mut self,
        ui: &mut egui::Ui,
        ov: &mut OverviewState,
        items: &[Value],
    ) -> Option<String> {
        let mut open = None;
        egui::ScrollArea::vertical()
            .id_salt("index-overview-relationships")
            .show(ui, |ui| {
                for item in items {
                    let id = s(item, "id");
                    let name = s(item, "name");
                    egui::Frame::NONE
                        .fill(palette_color(CARD_BG))
                        .stroke(egui::Stroke::new(1.0, IndexScope::of(item).color()))
                        .corner_radius(6)
                        .inner_margin(12)
                        .show(ui, |ui| {
                            ui.set_width(ui.available_width());
                            egui::CollapsingHeader::new(&name)
                                .id_salt(&id)
                                .show(ui, |ui| {
                                    if ui.button(tr("打开索引")).clicked() {
                                        open = Some(id.clone());
                                    }
                                    if ui.button(tr("刷新关系")).clicked() {
                                        ov.lineages.remove(&id);
                                    }
                                    let result =
                                        ov.lineages.entry(id.clone()).or_insert_with(|| {
                                            let detail = self.call(
                                                "GET",
                                                &format!("/v1/indexes/{id}"),
                                                None,
                                            )?;
                                            self.load_index_lineage(&id, &detail)
                                        });
                                    show_lineage(ui, result, None);
                                });
                        });
                    ui.add_space(8.0);
                }
            });
        open
    }

    pub(super) fn index_detail_lineage(
        &self,
        ui: &mut egui::Ui,
        st: &mut IndexDetailState,
        action: &mut Option<IndexDetailAction>,
    ) {
        ui.horizontal(|ui| {
            ui.heading(tr("派生关系"));
            if ui.button(tr("刷新关系")).clicked() {
                st.lineage = None;
            }
        });
        let result = st
            .lineage
            .get_or_insert_with(|| self.load_index_lineage(&st.index_id, &st.detail));
        if let Some(branch) = show_lineage(ui, result, Some(&st.branch)) {
            *action = Some(IndexDetailAction::ChangeBranch(branch));
        }
    }
}

fn show_lineage(
    ui: &mut egui::Ui,
    result: &Result<Vec<BranchLineage>, String>,
    current: Option<&str>,
) -> Option<String> {
    let branches = match result {
        Ok(branches) if !branches.is_empty() => branches,
        Ok(_) => {
            ui.label(tr("没有可显示的分支"));
            return None;
        }
        Err(error) => {
            ui.label(egui::RichText::new(error).color(palette_color(TEXT_WEAK)));
            return None;
        }
    };
    let mut next = None;
    for branch in branches {
        ui.horizontal_wrapped(|ui| {
            if let Some((source, commit)) = &branch.inherited {
                ui.label(egui::RichText::new(source).strong());
                ui.label(egui::RichText::new(commit).monospace().weak());
                ui.label("→");
            }
            if current.is_some() {
                if ui
                    .selectable_label(current == Some(branch.name.as_str()), &branch.name)
                    .clicked()
                    && current != Some(branch.name.as_str())
                {
                    next = Some(branch.name.clone());
                }
            } else {
                ui.label(egui::RichText::new(&branch.name).strong());
            }
            if branch.head.is_empty() {
                ui.label(egui::RichText::new(tr("暂无提交")).weak());
            } else {
                ui.label(egui::RichText::new(&branch.head).monospace().weak());
            }
        });
    }
    next
}

use super::*;

impl App {
    pub(super) fn index_detail_tabs(&self, ui: &mut egui::Ui, st: &mut IndexDetailState) {
        let views = [
            IndexDetailView::Wall,
            IndexDetailView::Commits,
            IndexDetailView::Versions,
            IndexDetailView::Exports,
            IndexDetailView::Lineage,
        ];
        let selected = views.iter().position(|view| *view == st.view).unwrap_or(0);
        if let Some(next) = segmented(
            ui,
            &["条目图墙", "提交图", "历史版本", "导出记录", "派生关系"],
            selected,
            SegStyle::Soft,
        ) {
            st.view = views[next];
        }
    }

    pub(super) fn index_history_view(
        &mut self,
        ui: &mut egui::Ui,
        st: &mut IndexDetailState,
        action: &mut Option<IndexDetailAction>,
    ) {
        egui::ScrollArea::vertical()
            .id_salt((&st.index_id, st.view as usize))
            .show(ui, |ui| match st.view {
                IndexDetailView::Commits => {
                    Self::index_commit_graph(ui, st);
                    ui.separator();
                    self.index_merge_ui(ui, st, action);
                }
                IndexDetailView::Versions => Self::index_published_versions(ui, st, action),
                IndexDetailView::Exports => Self::index_export_records(ui, st, action),
                IndexDetailView::Lineage => self.index_detail_lineage(ui, st, action),
                IndexDetailView::Wall => {}
            });
    }

    fn index_commit_graph(ui: &mut egui::Ui, st: &mut IndexDetailState) {
        ui.heading("提交图");
        let Some(commits) = st
            .commits
            .as_ref()
            .and_then(|list| list["commits"].as_array())
        else {
            ui.label(egui::RichText::new("提交记录暂无法读取。").color(palette_color(TEXT_WEAK)));
            return;
        };
        if commits.is_empty() {
            ui.label(egui::RichText::new("这个分支还没有提交。").color(palette_color(TEXT_WEAK)));
            return;
        }
        let selected = match st.selected_commit.as_deref() {
            Some(id) => commits.iter().position(|commit| commit["id"] == id),
            None => Some(0),
        };

        for (position, commit) in commits.iter().enumerate() {
            let is_selected = selected == Some(position);
            let parent_id = commit["parent_id"].as_str();
            ui.horizontal(|ui| {
                let size = egui::vec2(18.0, 32.0);
                let (rect, _) = ui.allocate_exact_size(size, egui::Sense::hover());
                let center = rect.center();
                let painter = ui.painter();
                let stroke = egui::Stroke::new(1.0, palette_color(FIELD_BORDER));
                if position > 0 && commits[position - 1]["parent_id"] == commit["id"] {
                    painter.line_segment(
                        [
                            egui::pos2(center.x, rect.top() - ui.spacing().item_spacing.y),
                            center,
                        ],
                        stroke,
                    );
                }
                if commits
                    .get(position + 1)
                    .is_some_and(|next| parent_id == next["id"].as_str())
                {
                    painter.line_segment([center, egui::pos2(center.x, rect.bottom())], stroke);
                }
                painter.circle_filled(
                    center,
                    3.5,
                    if is_selected {
                        ACCENT
                    } else {
                        palette_color(TEXT_WEAK)
                    },
                );
                if ui
                    .selectable_label(is_selected, s(commit, "message"))
                    .clicked()
                {
                    st.selected_commit = Some(s(commit, "id"));
                }
                ui.label(
                    egui::RichText::new(s(commit, "branch"))
                        .small()
                        .color(palette_color(TEXT_WEAK)),
                );
                ui.label(
                    egui::RichText::new(fmt_time(i(commit, "created_ms")))
                        .small()
                        .color(palette_color(TEXT_WEAK)),
                );
            });
        }
        ui.add_space(8.0);
        let Some(commit) = selected.and_then(|index| commits.get(index)) else {
            ui.label(
                egui::RichText::new("所选提交不在当前返回的记录中。")
                    .color(palette_color(TEXT_WEAK)),
            );
            return;
        };
        card_frame().show(ui, |ui| {
            ui.label(egui::RichText::new("提交详情").strong());
            ui.label(format!(
                "{} · {}",
                s(commit, "author"),
                fmt_time(i(commit, "created_ms"))
            ));
            ui.label(format!("提交 ID：{}", s(commit, "id")));
            if let Some(parent) = commit["parent_id"].as_str() {
                ui.label(format!("父提交：{parent}"));
            }
            let count = commit["entries"].as_array().map_or(0, Vec::len);
            ui.label(format!("{count} 个条目"));
        });
    }

    fn index_published_versions(
        ui: &mut egui::Ui,
        st: &IndexDetailState,
        action: &mut Option<IndexDetailAction>,
    ) {
        ui.heading("历史版本");
        let tags = st.detail["tags"]
            .as_array()
            .map(Vec::as_slice)
            .unwrap_or_default();
        if tags.is_empty() {
            ui.label(egui::RichText::new("暂无发布版本。").color(palette_color(TEXT_WEAK)));
            return;
        }
        for tag in tags {
            card_frame().show(ui, |ui| {
                ui.horizontal_wrapped(|ui| {
                    ui.label(
                        egui::RichText::new(format!("V{}", s(tag, "name")))
                            .strong()
                            .color(palette_color(ACCENT_DEEP)),
                    );
                    ui.label(s(tag, "branch"));
                    if ui.small_button("查看提交").clicked() {
                        *action = Some(IndexDetailAction::FocusCommit {
                            branch: s(tag, "branch"),
                            id: s(tag, "commit_id"),
                        });
                    }
                });
                ui.label(
                    egui::RichText::new(s(tag, "commit_id"))
                        .small()
                        .color(palette_color(TEXT_WEAK)),
                );
            });
        }
    }

    fn index_export_records(
        ui: &mut egui::Ui,
        st: &IndexDetailState,
        action: &mut Option<IndexDetailAction>,
    ) {
        ui.heading("导出记录");
        let Some(exports) = st
            .exports
            .as_ref()
            .and_then(|list| list["exports"].as_array())
        else {
            ui.label(egui::RichText::new("导出记录暂无法读取。").color(palette_color(TEXT_WEAK)));
            return;
        };
        if exports.is_empty() {
            ui.label(egui::RichText::new("暂无导出记录。").color(palette_color(TEXT_WEAK)));
            return;
        }
        for record in exports {
            let edition = match record["kind"].as_str() {
                Some("personal") => "个人版",
                Some("company" | "public") => "公用版",
                _ => "导出",
            };
            let coordinate = match record["tag"].as_str() {
                Some(tag) => format!("{} · V{tag}", s(record, "branch")),
                None => s(record, "branch"),
            };
            card_frame().show(ui, |ui| {
                ui.horizontal_wrapped(|ui| {
                    ui.label(egui::RichText::new(edition).strong());
                    ui.label(coordinate);
                    ui.label(format!("{} 页", i(record, "page_count")));
                    ui.label(
                        egui::RichText::new(fmt_time(i(record, "created_ms")))
                            .color(palette_color(TEXT_WEAK)),
                    );
                    if ui.small_button("查看提交").clicked() {
                        *action = Some(IndexDetailAction::FocusCommit {
                            branch: s(record, "branch"),
                            id: s(record, "commit_id"),
                        });
                    }
                });
                ui.label(
                    egui::RichText::new(format!("提交 {}", s(record, "commit_id")))
                        .small()
                        .color(palette_color(TEXT_WEAK)),
                );
            });
        }
    }
}

//! Repository timelines shared by the overview and the index editor.
use super::*;

pub(super) type IndexGraphs = BTreeMap<String, IndexGraph>;

struct GraphTarget {
    branch: String,
    commit: Option<String>,
}

impl App {
    fn load_index_graphs(&self, index_id: Option<&str>) -> Result<IndexGraphs, String> {
        let path = index_id.map_or_else(
            || "/v1/index-graphs".to_string(),
            |id| format!("/v1/index-graphs?index_id={}", query_component(id)),
        );
        let mut response = self.call("GET", &path, None)?;
        serde_json::from_value(response["graphs"].take()).map_err(|error| error.to_string())
    }

    pub(super) fn index_overview_relationships(
        &self,
        ui: &mut egui::Ui,
        overview: &mut OverviewState,
        items: &[Value],
        action: &mut Option<IndexDetailAction>,
    ) {
        if ui.button(tr("刷新关系")).clicked() {
            overview.graphs = None;
        }
        let graphs = match overview
            .graphs
            .get_or_insert_with(|| self.load_index_graphs(None))
        {
            Ok(graphs) => graphs,
            Err(error) => {
                ui.label(error.as_str());
                return;
            }
        };
        egui::ScrollArea::vertical()
            .id_salt("index-overview-relationships")
            .show(ui, |ui| {
                for item in items {
                    let id = s(item, "id");
                    let Some(graph) = graphs.get(&id) else {
                        continue;
                    };
                    egui::Frame::NONE
                        .fill(palette_color(CARD_BG))
                        .stroke(egui::Stroke::new(1.0, palette_color(SEPARATOR)))
                        .corner_radius(6)
                        .inner_margin(12)
                        .show(ui, |ui| {
                            ui.set_width(ui.available_width());
                            ui.horizontal(|ui| {
                                ui.heading(format!("{} · {}", tr("仓库"), s(item, "name")));
                                if item["lifecycle"] == "history" {
                                    badge(ui, tr("历史"), BadgeTone::Warn);
                                }
                                if ui.button(tr("打开索引")).clicked() {
                                    *action = Some(IndexDetailAction::OpenGraphTarget {
                                        index: id.clone(),
                                        branch: "main".into(),
                                        commit: None,
                                    });
                                }
                            });
                            self.graph_origin(ui, &id, graph, action);
                            if let Some(target) = timeline(ui, &id, graph, None) {
                                *action = Some(IndexDetailAction::OpenGraphTarget {
                                    index: id.clone(),
                                    branch: target.branch,
                                    commit: target.commit,
                                });
                            }
                        });
                    ui.add_space(12.0);
                }
            });
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
        let result = st.lineage.get_or_insert_with(|| {
            self.load_index_graphs(Some(&st.index_id))?
                .remove(&st.index_id)
                .ok_or_else(|| tr("没有可显示的分支").to_string())
        });
        let graph = match result {
            Ok(graph) => graph,
            Err(error) => {
                ui.label(error.as_str());
                return;
            }
        };
        self.graph_origin(ui, &st.index_id, graph, action);
        if let Some(target) = timeline(ui, &st.index_id, graph, Some(&st.branch)) {
            if let Some(id) = target.commit {
                *action = Some(IndexDetailAction::FocusCommit {
                    branch: target.branch,
                    id,
                });
            } else if target.branch != st.branch {
                *action = Some(IndexDetailAction::ChangeBranch(target.branch));
            }
        }
    }

    fn graph_origin(
        &self,
        ui: &mut egui::Ui,
        index_id: &str,
        graph: &IndexGraph,
        action: &mut Option<IndexDetailAction>,
    ) {
        let Some(main) = graph.branches.iter().find(|branch| branch.name == "main") else {
            return;
        };
        let Some(source) = main
            .source_index_id
            .as_deref()
            .filter(|source| *source != index_id)
        else {
            return;
        };
        let branch = main.source_branch.as_deref().unwrap_or_default();
        let commit = main.fork_commit_id.as_deref().unwrap_or_default();
        let repo = self.indexes.iter().find(|item| item["id"] == source);
        ui.horizontal_wrapped(|ui| {
            ui.label(tr("来源索引"));
            if let Some(repo) = repo {
                if ui.link(format!("{} · {branch}", s(repo, "name"))).clicked() {
                    *action = Some(IndexDetailAction::OpenGraphTarget {
                        index: source.into(),
                        branch: branch.into(),
                        commit: main.fork_commit_id.clone(),
                    });
                }
            } else {
                ui.label(format!("{source} · {branch}"));
            }
            ui.label(egui::RichText::new(commit).monospace().weak());
        });
    }
}

fn timeline(
    ui: &mut egui::Ui,
    index_id: &str,
    graph: &IndexGraph,
    current: Option<&str>,
) -> Option<GraphTarget> {
    if graph.branches.is_empty() {
        ui.label(tr("没有可显示的分支"));
        return None;
    }
    const LABEL_WIDTH: f32 = 118.0;
    const LANE_HEIGHT: f32 = 34.0;
    const COLORS: [u32; 7] = [
        0x2f6bf5, 0x3fae74, 0x8b5cf6, 0xe0913a, 0xf2555a, 0x0ea5e9, 0xd946ef,
    ];
    let lane_color = |lane: usize| {
        let rgb = COLORS[lane % COLORS.len()];
        egui::Color32::from_rgb((rgb >> 16) as u8, (rgb >> 8) as u8, rgb as u8)
    };
    let lanes: BTreeMap<&str, usize> = graph
        .branches
        .iter()
        .enumerate()
        .map(|(lane, branch)| (branch.name.as_str(), lane))
        .collect();
    let mut tags = BTreeMap::<(&str, &str), Vec<&str>>::new();
    for tag in &graph.tags {
        tags.entry((&tag.branch, &tag.commit_id))
            .or_default()
            .push(&tag.name);
    }
    let mut tag_widths = BTreeMap::<&str, usize>::new();
    for ((_, id), names) in &tags {
        let width =
            names.iter().map(|name| name.chars().count()).sum::<usize>() + 3 * (names.len() - 1);
        tag_widths
            .entry(*id)
            .and_modify(|existing| *existing = (*existing).max(width))
            .or_insert(width);
    }
    let mut positions = BTreeMap::new();
    let mut next_x = LABEL_WIDTH;
    for commit in &graph.commits {
        next_x += 18.0;
        positions.insert(commit.id.as_str(), next_x);
        if let Some(widest_tag) = tag_widths.get(commit.id.as_str()) {
            next_x += 18.0 + 8.0 * *widest_tag as f32;
        }
    }
    let mut selection = None;
    ui.label(egui::RichText::new(tr("时间 →")).size(11.0).weak());
    egui::ScrollArea::horizontal()
        .id_salt(("index-graph-timeline", index_id))
        .show(ui, |ui| {
            let width = (next_x + 42.0).max(ui.available_width());
            let (rect, _) = ui.allocate_exact_size(
                egui::vec2(width, LANE_HEIGHT * graph.branches.len() as f32),
                egui::Sense::hover(),
            );
            let point =
                |x: f32, lane: usize| rect.min + egui::vec2(x, (lane as f32 + 0.5) * LANE_HEIGHT);
            for (lane, branch) in graph.branches.iter().enumerate() {
                let color = lane_color(lane);
                let source = branch
                    .source_branch
                    .as_deref()
                    .filter(|_| branch.source_index_id.as_deref() == Some(index_id))
                    .and_then(|name| lanes.get(name).copied());
                let fork_x = branch
                    .fork_commit_id
                    .as_deref()
                    .and_then(|id| positions.get(id).copied());
                let start = if source.is_some() {
                    fork_x.unwrap_or(LABEL_WIDTH)
                } else {
                    LABEL_WIDTH
                };
                let head = branch
                    .head_commit
                    .as_deref()
                    .and_then(|id| positions.get(id).copied())
                    .unwrap_or(start);
                let lane_start = start + if source.is_some() { 12.0 } else { 0.0 };
                let from = point(lane_start, lane);
                let to = point(head.max(lane_start) + 9.0, lane);
                ui.painter()
                    .line_segment([from, to], egui::Stroke::new(2.0, color));
                if let Some(parent) = source {
                    let origin = point(start, parent);
                    let control = egui::vec2(12.0, 0.0);
                    ui.painter()
                        .add(egui::epaint::CubicBezierShape::from_points_stroke(
                            [origin, origin + control, from - control, from],
                            false,
                            egui::Color32::TRANSPARENT,
                            egui::Stroke::new(2.0, color),
                        ));
                    ui.painter().circle_filled(origin, 3.5, color);
                }
                let label = egui::Rect::from_min_size(
                    point(2.0, lane) - egui::vec2(0.0, 11.0),
                    egui::vec2(110.0, 22.0),
                );
                let description = if branch.name != "main" && branch.source_branch.is_none() {
                    tr("旧分支 · 来源未记录").to_owned()
                } else if let Some(source) = &branch.source_branch {
                    format!(
                        "{} {source} · {}",
                        tr("分叉自"),
                        branch
                            .fork_commit_id
                            .as_deref()
                            .unwrap_or_else(|| tr("暂无提交"))
                    )
                } else {
                    tr("主线").to_owned()
                };
                if graph_pill(
                    ui,
                    label,
                    (index_id, "branch", lane),
                    &branch.name,
                    color,
                    current == Some(branch.name.as_str()),
                    &description,
                ) {
                    selection = Some(GraphTarget {
                        branch: branch.name.clone(),
                        commit: None,
                    });
                }
            }
            for commit in &graph.commits {
                let Some(lane) = lanes.get(commit.branch.as_str()).copied() else {
                    continue;
                };
                let center = point(positions[commit.id.as_str()], lane);
                let dot = egui::Rect::from_center_size(center, egui::Vec2::splat(14.0));
                if !ui.is_rect_visible(dot) {
                    continue;
                }
                let color = lane_color(lane);
                ui.painter().circle_filled(center, 4.0, color);
                let hint = format!("{} · {}", commit.id, commit.message);
                let clicked = ui
                    .interact(
                        dot,
                        ui.make_persistent_id((index_id, "commit", &commit.id)),
                        egui::Sense::click(),
                    )
                    .on_hover_text(&hint)
                    .clicked();
                if clicked {
                    selection = Some(GraphTarget {
                        branch: commit.branch.clone(),
                        commit: Some(commit.id.clone()),
                    });
                }
            }
            for ((branch, commit), names) in &tags {
                let (Some(lane), Some(x)) =
                    (lanes.get(branch).copied(), positions.get(commit).copied())
                else {
                    continue;
                };
                let text = names.join(" · ");
                let tag = egui::Rect::from_min_size(
                    point(x + 9.0, lane) - egui::vec2(0.0, 9.0),
                    egui::vec2(12.0 + 8.0 * text.chars().count() as f32, 18.0),
                );
                if graph_pill(
                    ui,
                    tag,
                    (index_id, "tag", branch, commit),
                    &text,
                    lane_color(lane),
                    true,
                    commit,
                ) {
                    selection = Some(GraphTarget {
                        branch: (*branch).into(),
                        commit: Some((*commit).into()),
                    });
                }
            }
        });
    if graph
        .branches
        .iter()
        .any(|branch| branch.name != "main" && branch.source_branch.is_none())
    {
        ui.label(
            egui::RichText::new(tr("旧分支 · 来源未记录"))
                .size(11.0)
                .weak(),
        );
    }
    selection
}

fn graph_pill(
    ui: &mut egui::Ui,
    rect: egui::Rect,
    id: impl std::hash::Hash + std::fmt::Debug,
    title: &str,
    color: egui::Color32,
    filled: bool,
    hint: &str,
) -> bool {
    if !ui.is_rect_visible(rect) {
        return false;
    }
    let response = ui.interact(rect, ui.make_persistent_id(id), egui::Sense::click());
    response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, title));
    let ink = if filled {
        if color == ACCENT {
            egui::Color32::WHITE
        } else {
            egui::Color32::from_gray(15)
        }
    } else {
        palette_color(TEXT_MAIN)
    };
    let fill = if filled {
        color
    } else {
        palette_color(FIELD_BG)
    };
    let painter = ui.painter().with_clip_rect(rect.intersect(ui.clip_rect()));
    painter.rect(
        rect,
        8.0,
        fill,
        egui::Stroke::new(1.0, color),
        egui::StrokeKind::Inside,
    );
    painter.text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        title,
        egui::FontId::proportional(11.0),
        ink,
    );
    response
        .on_hover_text(format!("{title} · {hint}"))
        .clicked()
}

use super::*;

// ----------------------------------------------------------------
// #101 内容块详情页 — full rebuild (user order 2026-11-25: 不要基于
// 现有的页面做，要完全重构，1:1 对齐原型). The rendering below is
// written from the extracted spec (~/workspace/qimu-101-rebuild/
// SPEC.md; prototype 启幕原型-内容索引页.html, renderDetail @2589)
// rather than evolved from the previous page: header → thumbnails
// wall (with the per-page language bind bar) → bind panel →
// embedded blocks → history versions. The old "语言绑定（逐页）"
// workbench section does not exist in the prototype and is gone;
// binding lives in exactly the prototype's two places — the thumb
// bar's chips (single-click direct toggle) and the panel below
// the wall. Data assembly, the drill stack and the bottom-bar
// contract stay with the caller (main.rs unit_detail keeps only
// the crumb, the pick-pages mode and the client-only sections).
// ----------------------------------------------------------------

#[derive(Clone)]
pub(super) struct UnitVersion {
    pub rendition_id: String,
    pub form: String,
    pub lang: String,
    pub version: Value,
}

impl UnitVersion {
    fn label(&self) -> String {
        format!(
            "{} · {} · v{}",
            self.form,
            self.lang,
            s(&self.version, "semver")
        )
    }
}

#[derive(Clone)]
pub(super) struct PageBindingPick {
    pub from_version_id: String,
    pub from_slot: i64,
    pub to_version_id: String,
    pub target_lang: String,
}

#[derive(Clone)]
pub(super) struct PageBindingRequest {
    pub unit_id: String,
    pub from_version_id: String,
    pub from_slot: i64,
    pub to_version_id: String,
    pub to_slot: i64,
}

/// The bind panel's open state (prototype `bindPanel{key, pi,
/// lang}`): which page of the shown version it serves, and which
/// target language its candidate list currently shows. Lives on
/// the frame's state so a detail reload (every bind action
/// reloads) keeps the panel where the user left it.
#[derive(Clone)]
pub(super) struct BindPanelState {
    pub page: usize,
    pub lang: String,
}

#[derive(Clone)]
pub(super) struct LightState {
    pub unit_id: String,
    pub version: Value,
    pub slot: usize,
    pub bindings: Vec<Value>,
    pub targets: Vec<UnitVersion>,
    pub target_index: usize,
}

pub(super) fn unit_versions(detail: &Value) -> Vec<UnitVersion> {
    detail
        .get("renditions")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .flat_map(|row| {
            let rendition = row.get("rendition").cloned().unwrap_or(Value::Null);
            row.get("versions")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .map(|version| UnitVersion {
                    rendition_id: s(&rendition, "id"),
                    form: s(&rendition, "form"),
                    lang: s(&rendition, "lang"),
                    version: version.clone(),
                })
                .collect::<Vec<_>>()
        })
        .collect()
}

fn latest_version(row: &Value) -> Option<Value> {
    row.get("versions")
        .and_then(Value::as_array)?
        .iter()
        .find(|version| s(version, "status") != "recalled")
        .or_else(|| row.get("versions").and_then(Value::as_array)?.first())
        .cloned()
}

pub(super) fn binding_targets(detail: &Value, from_version_id: &str) -> Vec<UnitVersion> {
    let source_rendition = unit_versions(detail)
        .into_iter()
        .find(|item| s(&item.version, "id") == from_version_id)
        .map(|item| item.rendition_id)
        .unwrap_or_default();
    let mut targets = detail
        .get("renditions")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|row| s(&row["rendition"], "id") != source_rendition)
        .filter_map(|row| {
            let rendition = row.get("rendition")?;
            Some(UnitVersion {
                rendition_id: s(rendition, "id"),
                form: s(rendition, "form"),
                lang: s(rendition, "lang"),
                version: latest_version(row)?,
            })
        })
        .collect::<Vec<_>>();
    targets.sort_by_key(|target| (target.lang != "en", target.lang.clone()));
    targets
}

fn exact_binding(bindings: &[Value], request: &PageBindingRequest) -> Option<Value> {
    bindings
        .iter()
        .find(|binding| {
            s(binding, "from_version_id") == request.from_version_id
                && i(binding, "from_slot") == request.from_slot
                && s(binding, "to_version_id") == request.to_version_id
                && i(binding, "to_slot") == request.to_slot
                && s(binding, "kind") != "missing"
        })
        .cloned()
}

fn occupied_binding(bindings: &[Value], request: &PageBindingRequest) -> Option<Value> {
    bindings
        .iter()
        .find(|binding| {
            s(binding, "from_version_id") != request.from_version_id
                && s(binding, "to_version_id") == request.to_version_id
                && i(binding, "to_slot") == request.to_slot
                && s(binding, "kind") != "missing"
        })
        .cloned()
}

fn page_title(version: &Value, slot: usize) -> String {
    version
        .get("pages")
        .and_then(Value::as_array)
        .and_then(|pages| pages.get(slot))
        .map(|page| s(page, "title"))
        .filter(|title| !title.is_empty())
        .unwrap_or_else(|| format!("第 {} 页", slot + 1))
}

/// The page's one-line content summary (the prototype's `d`,
/// carried on the version's page rows since the R3 data line).
/// Empty for pages imported before the field existed — callers
/// present that as the prototype's empty state, never a
/// stand-in text.
fn page_desc(version: &Value, slot: usize) -> String {
    version
        .get("pages")
        .and_then(Value::as_array)
        .and_then(|pages| pages.get(slot))
        .map(|page| s(page, "desc"))
        .unwrap_or_default()
}

/// Cap a desc for a single-line painted slot (bind candidate,
/// #406 badge): the data keeps the full line; only the painted
/// copy is cut, with an ellipsis marking the cut.
fn desc_line(desc: &str, max_chars: usize) -> String {
    if desc.chars().count() <= max_chars {
        return desc.to_string();
    }
    let cut: String = desc.chars().take(max_chars).collect();
    format!("{}…", cut.trim_end())
}

/// The light view's bind head names the target language
/// (prototype .lb-bind-head: 英语), not its code.
fn lang_display_name(code: &str) -> String {
    match code {
        "en" => "英语".to_string(),
        "zh" => "中文".to_string(),
        "ja" => "日语".to_string(),
        "bi" => "双语".to_string(),
        other => other.to_uppercase(),
    }
}

fn page_note(version: &Value, slot: usize) -> String {
    version
        .get("scripts")
        .and_then(Value::as_array)
        .and_then(|scripts| scripts.get(slot))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// One page's bind state for one target language, read off the
/// frame's binding rows (prototype bindsOf[pi][lang]): a real
/// page binding, a missing-translation mark (kind = "missing",
/// to_slot = -1 — see Store::add_page_binding), or nothing.
enum PageBindState {
    Bound(Value),
    Missing(Value),
    Unbound,
}

fn page_bind_state(
    bindings: &[Value],
    from_version_id: &str,
    slot: usize,
    target_version_id: &str,
) -> PageBindState {
    let mut missing = None;
    for binding in bindings {
        if s(binding, "from_version_id") != from_version_id
            || i(binding, "from_slot") != slot as i64
            || s(binding, "to_version_id") != target_version_id
        {
            continue;
        }
        if s(binding, "kind") == "missing" {
            missing = Some(binding.clone());
        } else if i(binding, "to_slot") >= 0 {
            return PageBindState::Bound(binding.clone());
        }
    }
    match missing {
        Some(binding) => PageBindState::Missing(binding),
        None => PageBindState::Unbound,
    }
}

/// Deferred outcomes of one frame's wall/panel/history drawing.
/// The draw closures borrow the thumbnail cache through `self`,
/// so every store-touching follow-up is collected and executed
/// after the section has been drawn (the pattern the previous
/// page used for its binding requests).
enum DetailAct {
    Bind(PageBindingRequest),
    Unbind {
        binding_id: String,
        ok_msg: &'static str,
    },
    MarkMissing {
        from_version_id: String,
        from_slot: i64,
        to_version_id: String,
    },
    PublishDraft(String),
    DropVersion(String),
}

impl App {
    pub(super) fn open_light(&mut self, st: &mut UnitState, version: &Value, slot: usize) {
        let page_count = i(version, "page_count").max(0) as usize;
        if page_count == 0 || slot >= page_count {
            return;
        }
        let unit_id = st.unit_id.clone();
        let version_id = s(version, "id");
        let bindings = st.bindings.clone();
        let targets = binding_targets(&st.detail, &version_id);
        st.selected_page = Some(slot);
        st.bind_pick = None;
        self.overlays
            .retain(|overlay| !matches!(overlay, Overlay::Light(_)));
        self.overlays.push(Overlay::Light(LightState {
            unit_id,
            version: version.clone(),
            slot,
            bindings,
            targets,
            target_index: 0,
        }));
    }

    /// The prototype's bind language set for the thumb bar and
    /// the panel (bindLangs @2730): the content languages minus
    /// the shown rendition's own language, plus 双语 — always the
    /// full set, whether a version in that language exists or not
    /// (audit R5). The client's language list comes from the
    /// server settings (lang_options); the zh/en/ja fallback
    /// mirrors the translate picker when it has not loaded.
    fn bind_lang_codes(&self, shown_lang: &str, targets: &[UnitVersion]) -> Vec<String> {
        let mut codes: Vec<String> = Vec::new();
        let mut push = |codes: &mut Vec<String>, code: &str| {
            if code != shown_lang && !codes.iter().any(|c| c == code) {
                codes.push(code.to_string());
            }
        };
        if self.lang_options.is_empty() {
            for code in ["zh", "en", "ja"] {
                push(&mut codes, code);
            }
        } else {
            for (code, _) in &self.lang_options {
                push(&mut codes, code);
            }
        }
        for target in targets {
            push(&mut codes, &target.lang);
        }
        push(&mut codes, "bi");
        codes
    }

    fn run_detail_act(&mut self, unit_id: &str, bindings: &[Value], act: DetailAct) {
        match act {
            DetailAct::Bind(request) => {
                self.request_page_binding(request, bindings);
            }
            DetailAct::Unbind { binding_id, ok_msg } => {
                self.delete_page_binding_msg(unit_id, &binding_id, ok_msg);
            }
            DetailAct::MarkMissing {
                from_version_id,
                from_slot,
                to_version_id,
            } => {
                self.apply_missing_mark(unit_id, &from_version_id, from_slot, &to_version_id);
            }
            DetailAct::PublishDraft(version_id) => {
                match self.call_work(
                    "POST",
                    &format!("/v1/versions/{version_id}/publish-draft"),
                    None,
                ) {
                    Ok(_) => {
                        self.msg = "草稿已发布为最新版本".into();
                        self.reload_unit_detail(unit_id);
                        // The wall card's latest version / page
                        // count changed with the publish.
                        self.changed(Change::Units);
                    }
                    Err(e) => self.msg = format!("发布草稿失败：{e}"),
                }
            }
            DetailAct::DropVersion(version_id) => {
                match self.call_work(
                    "POST",
                    &format!("/v1/versions/{version_id}/delete"),
                    None,
                ) {
                    Ok(_) => {
                        self.msg = "已放弃该草稿".into();
                        self.reload_unit_detail(unit_id);
                        self.changed(Change::Units);
                    }
                    Err(e) => self.msg = format!("放弃草稿失败：{e}"),
                }
            }
        }
    }

    // ------------------------------------------------------------
    // Header (SPEC 一): one baseline row — h2 title (double-click
    // inline edit), the intro + 「当前 v…」 hint, the kind tag in
    // the badges slot. The prototype's third hint segment is the
    // version POLICY (锁定版本 / 跟随最新): entry-level data,
    // plumbed into the frame by the index entry path that opened
    // it (audit R1, EntryPolicyView on UnitState) — it is NOT
    // replaced by the maintainer here. Opened through no entry
    // (content wall, search, home…), the segment renders the
    // prototype's default form: a block's policy defaults to
    // follow, so it reads 跟随最新. The header delete button the
    // old page had does not exist in the prototype and is gone
    // (D1, report); deletion stays on the shell bottom bar.
    // ------------------------------------------------------------
    pub(super) fn unit_header_ui(
        &mut self,
        ui: &mut egui::Ui,
        st: &mut UnitState,
        unit: &Value,
        kind_tag: Option<&'static str>,
    ) {
        let head_latest = st
            .detail
            .get("renditions")
            .and_then(|r| r.as_array())
            .and_then(|arr| arr.first())
            .and_then(|r| r.get("latest"))
            .cloned()
            .unwrap_or(Value::Null);
        ui.horizontal(|ui| {
            if matches!(&st.meta_edit, Some((MetaField::Name, _))) {
                self.meta_edit_ui(ui, st, MetaField::Name);
            } else {
                let title = ui.add(
                    egui::Label::new(
                        egui::RichText::new(s(unit, "name")).size(20.0).strong(),
                    )
                    .sense(egui::Sense::click()),
                );
                if title.double_clicked() {
                    st.meta_edit = Some((MetaField::Name, s(unit, "name")));
                    st.meta_edit_focus = true;
                }
            }
            if let Some(tag) = kind_tag {
                chip(ui, tag, palette_color(CHIP_GRAY_FG), CHIP_GRAY_BG);
            }
            // .hint: 13px, --muted, on the title's baseline row.
            let mut hint_has_intro = false;
            if matches!(&st.meta_edit, Some((MetaField::Description, _))) {
                hint_has_intro = true;
                self.meta_edit_ui(ui, st, MetaField::Description);
            } else if !s(unit, "description").is_empty() {
                hint_has_intro = true;
                let intro = ui.add(
                    egui::Label::new(
                        egui::RichText::new(s(unit, "description"))
                            .size(13.0)
                            .color(palette_color(TEXT_FAINT)),
                    )
                    .sense(egui::Sense::click()),
                );
                if intro.double_clicked() {
                    st.meta_edit = Some((MetaField::Description, s(unit, "description")));
                    st.meta_edit_focus = true;
                }
            }
            // The prototype's hint version segment is always
            // the plain version string straight from the data
            // (prototype @2600: intro, then the version, then
            // the policy); it has no fallback form, so when the
            // shown rendition carries no semver the segment is
            // simply absent. The earlier revision-counter text
            // here was a client invention and is gone.
            let head_semver = s(&head_latest, "semver");
            if !head_semver.is_empty() {
                ui.label(
                    egui::RichText::new(format!(
                        "{}当前 v{}",
                        if hint_has_intro { " · " } else { "" },
                        head_semver
                    ))
                    .size(13.0)
                    .color(palette_color(TEXT_FAINT)),
                );
            }
            // Third hint segment: the version policy of the
            // entry this detail was opened through (prototype:
            // 锁定版本 / 跟随最新; a locked entry names its
            // version). No entry context → the prototype's
            // default form, 跟随最新 (see the header note).
            let policy_text = match &st.entry_policy {
                Some(EntryPolicyView::Locked { semver }) if !semver.is_empty() => {
                    format!("锁定版本 v{semver}")
                }
                Some(EntryPolicyView::Locked { .. }) => "锁定版本".to_string(),
                _ => "跟随最新".to_string(),
            };
            ui.label(
                egui::RichText::new(format!(
                    "{}{policy_text}",
                    if hint_has_intro || !head_semver.is_empty() {
                        " · "
                    } else {
                        ""
                    }
                ))
                .size(13.0)
                .color(palette_color(TEXT_FAINT)),
            );
        });
    }

    // ------------------------------------------------------------
    // The #101 body, in the prototype's block order: thumbnails
    // (+ bind panel directly after), embedded blocks, history.
    // ------------------------------------------------------------
    pub(super) fn unit_101_body(&mut self, ui: &mut egui::Ui, st: &mut UnitState) {
        let versions = unit_versions(&st.detail);
        let Some(first) = versions.first() else {
            return;
        };
        if st
            .selected_rendition
            .as_ref()
            .is_none_or(|id| versions.iter().all(|item| &item.rendition_id != id))
        {
            st.selected_rendition = Some(first.rendition_id.clone());
        }
        let selected_id = st.selected_rendition.clone().unwrap_or_default();
        // The shown version: a history version the user clicked
        // wins over the rendition's latest (views unification,
        // user ruling 2026-11-25) — the wall, and with it the
        // bottom-bar views, address that frozen page sequence.
        let Some(shown) = versions
            .iter()
            .find(|item| {
                item.rendition_id == selected_id
                    && st.selected_version.as_deref() == Some(s(&item.version, "id").as_str())
            })
            .or_else(|| versions.iter().find(|item| item.rendition_id == selected_id))
            .cloned()
        else {
            return;
        };
        let mut acts: Vec<DetailAct> = Vec::new();
        self.unit_thumbs_ui(ui, st, &versions, &shown, &mut acts);
        self.unit_kids_ui(ui, st);
        self.unit_history_ui(ui, st, &mut acts);
        let unit_id = st.unit_id.clone();
        // The bind confirm flow reads the frame's own binding
        // snapshot (the same one the wall rendered from).
        let bindings = st.bindings.clone();
        for act in acts {
            self.run_detail_act(&unit_id, &bindings, act);
        }
    }

    /// Renditions of the detail, deduplicated in payload order —
    /// the wall switches renditions, not versions.
    fn rendition_rows(versions: &[UnitVersion]) -> Vec<(String, String, String)> {
        let mut rows: Vec<(String, String, String)> = Vec::new();
        for item in versions {
            if !rows.iter().any(|(id, _, _)| *id == item.rendition_id) {
                rows.push((
                    item.rendition_id.clone(),
                    item.form.clone(),
                    item.lang.clone(),
                ));
            }
        }
        rows
    }

    // ------------------------------------------------------------
    // Thumbnails section (SPEC 三): h3 + the rendition segctl,
    // then the framed cells — real page thumbnails over the
    // prototype's frame geometry (2px --line, radius 6, --panel),
    // the toolbar strip (8/10/10) holding the bind bar and the
    // caption. The CSS grid's minmax(230px, 1fr) columns are
    // reproduced by measuring the available width and dividing
    // it into equal columns (declared approximation: egui has
    // no grid; the count follows the width, cells stay equal).
    // ------------------------------------------------------------
    fn unit_thumbs_ui(
        &mut self,
        ui: &mut egui::Ui,
        st: &mut UnitState,
        versions: &[UnitVersion],
        shown: &UnitVersion,
        acts: &mut Vec<DetailAct>,
    ) {
        let version_id = s(&shown.version, "id");
        let page_count = i(&shown.version, "page_count").max(0) as usize;
        let targets = binding_targets(&st.detail, &version_id);
        let lang_codes = self.bind_lang_codes(&shown.lang, &targets);
        let picked = st.selected_version.is_some();
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new("页面缩略图").strong().size(16.0));
            if picked {
                // The wall is showing a frozen history version,
                // not the rendition's latest — the prototype's
                // title suffix slot (its EN-mode version tag).
                ui.label(
                    egui::RichText::new(format!(" · v{}", s(&shown.version, "semver")))
                        .size(16.0)
                        .color(palette_color(TEXT_FAINT)),
                );
            }
            if let Some(pick) = &st.bind_pick {
                ui.label(
                    egui::RichText::new(format!(
                        "为第 {} 页挑{}页——单击缩略图完成绑定",
                        pick.from_slot + 1,
                        pick.target_lang.to_uppercase()
                    ))
                    .color(palette_color(ACCENT_DEEP)),
                );
                if ui.small_button("取消挑选").clicked() {
                    st.bind_pick = None;
                }
            }
            // Rendition switch: the prototype's segctl form,
            // right-aligned on the section title row. Only shown
            // when the block actually has several renditions.
            let rows = Self::rendition_rows(versions);
            if rows.len() > 1 {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let labels: Vec<String> = rows
                        .iter()
                        .map(|(_, form, lang)| format!("{form} · {lang}"))
                        .collect();
                    let active = rows
                        .iter()
                        .position(|(id, _, _)| *id == shown.rendition_id)
                        .unwrap_or(0);
                    let label_refs: Vec<&str> = labels.iter().map(|l| l.as_str()).collect();
                    if let Some(next) = segmented(ui, &label_refs, active, SegStyle::Soft) {
                        st.selected_rendition = Some(rows[next].0.clone());
                        st.selected_version = None;
                        st.selected_page = None;
                        st.bind_pick = None;
                        st.bind_panel = None;
                    }
                });
            }
        });
        ui.add_space(6.0);
        let picking = st.bind_pick.is_some();
        let saved_spacing = ui.spacing().item_spacing;
        ui.spacing_mut().item_spacing = egui::vec2(12.0, 12.0);
        let avail = ui.available_width();
        let cols = (((avail + 12.0) / (230.0 + 12.0)).floor() as usize).max(1);
        let cell_w = ((avail - 12.0 * (cols as f32 - 1.0)) / cols as f32).max(170.0);
        let inner_w = (cell_w - 4.0).max(120.0);
        ui.horizontal_wrapped(|ui| {
            for slot in 0..page_count {
                let frame_resp = egui::Frame::NONE
                    .fill(palette_color(PANEL_BG))
                    .stroke(egui::Stroke::new(2.0, palette_color(LINE_34)))
                    .corner_radius(6)
                    .inner_margin(egui::Margin::same(0))
                    .show(ui, |ui| {
                        ui.vertical(|ui| {
                            ui.set_width(inner_w);
                            let response = self.thumb_response(
                                ui,
                                &version_id,
                                slot,
                                inner_w,
                                st.selected_page == Some(slot),
                            );
                            if picking {
                                if response.clicked() {
                                    if let Some(pick) = st.bind_pick.clone() {
                                        acts.push(DetailAct::Bind(PageBindingRequest {
                                            unit_id: st.unit_id.clone(),
                                            from_version_id: pick.from_version_id,
                                            from_slot: pick.from_slot,
                                            to_version_id: pick.to_version_id,
                                            to_slot: slot as i64,
                                        }));
                                        st.bind_pick = None;
                                    }
                                }
                            } else {
                                if response.clicked() {
                                    st.selected_page = Some(slot);
                                }
                                if response.double_clicked() {
                                    self.open_light(st, &shown.version, slot);
                                }
                            }
                            // .tb: 8 top / 10 sides / 10 bottom.
                            ui.add_space(8.0);
                            ui.horizontal(|ui| {
                                ui.add_space(10.0);
                                if !picking {
                                    self.bind_bar_ui(
                                        ui, st, shown, slot, &targets, &lang_codes, acts,
                                    );
                                }
                            });
                            ui.horizontal(|ui| {
                                ui.add_space(10.0);
                                // .tb-desc: the page's 说明 (d),
                                // carried on the version's page
                                // rows since the R3 data line. A
                                // page without one (stock imported
                                // before the field) renders the
                                // prototype's empty state — an
                                // empty line, no stand-in text.
                                ui.label(
                                    egui::RichText::new(page_desc(&shown.version, slot))
                                        .size(12.0)
                                        .color(palette_color(TEXT_FAINT)),
                                );
                            });
                            ui.add_space(10.0);
                        });
                    });
                if picking {
                    // .thumb-grid.picking: a 2px dashed accent
                    // outline per cell — egui strokes have no
                    // dash, so the outline is solid (declared).
                    let rect = frame_resp.response.rect.expand(2.0);
                    ui.painter().rect_stroke(
                        rect,
                        8.0,
                        egui::Stroke::new(2.0, ACCENT),
                        egui::StrokeKind::Outside,
                    );
                }
            }
        });
        ui.spacing_mut().item_spacing = saved_spacing;
        // The bind panel renders directly after the wall
        // (prototype @2603 concatenation order).
        if !picking && st.bind_panel.is_some() {
            self.unit_bind_panel_ui(ui, st, shown, &targets, &lang_codes, acts);
        }
    }

    /// The per-page bind bar (SPEC 三, bindBarHtml @2744): one
    /// chip per language of the full content-language set, in
    /// the .lang-chip form enlarged for the thumb bar (min-width
    /// 32, height 24), then the panel-open pill (.bind-open
    /// @683 — its CSS and handler exist in the prototype build
    /// but no element emits it there; the pill is restored here
    /// to its CSS spec so the panel is reachable, see report).
    /// Chip click is the prototype's DIRECT TOGGLE (@5026):
    /// bound → unbind, missing → unmark, unbound → bind to the
    /// same-index page (an occupied target goes through the
    /// existing move-confirm overlay, the prototype's
    /// bindConfirmDlg). When the same-index bind is impossible
    /// (no version in that language, or the page has no
    /// counterpart), the click opens the panel instead.
    #[allow(clippy::too_many_arguments)]
    fn bind_bar_ui(
        &mut self,
        ui: &mut egui::Ui,
        st: &mut UnitState,
        shown: &UnitVersion,
        slot: usize,
        targets: &[UnitVersion],
        lang_codes: &[String],
        acts: &mut Vec<DetailAct>,
    ) {
        let version_id = s(&shown.version, "id");
        for lang in lang_codes {
            let target = targets.iter().find(|t| &t.lang == lang);
            let state = target.map(|t| {
                page_bind_state(&st.bindings, &version_id, slot, &s(&t.version, "id"))
            });
            let (fill, stroke, fg) = match &state {
                Some(PageBindState::Bound(_)) => {
                    (ACCENT, egui::Stroke::new(1.0, ACCENT), egui::Color32::WHITE)
                }
                Some(PageBindState::Missing(_)) => (
                    CHIP_ORANGE_BG,
                    egui::Stroke::new(1.0, palette_color(CHIP_ORANGE_FG)),
                    palette_color(CHIP_ORANGE_FG),
                ),
                _ => (
                    palette_color(HOVER_BG),
                    egui::Stroke::new(1.0, palette_color(FIELD_BORDER)),
                    palette_color(TEXT_WEAK),
                ),
            };
            let tip = match &state {
                Some(PageBindState::Bound(b)) => format!(
                    "{} · 已绑定 ↔ 第 {} 页（点击解绑）",
                    lang_display_name(lang),
                    i(b, "to_slot") + 1
                ),
                Some(PageBindState::Missing(_)) => {
                    format!("{} · 已标缺译（点击取消）", lang_display_name(lang))
                }
                _ => format!(
                    "{} · 未绑定（点击绑定到第 {} 页）",
                    lang_display_name(lang),
                    slot + 1
                ),
            };
            let chip = egui::Button::new(
                egui::RichText::new(lang.to_uppercase()).size(10.0).strong().color(fg),
            )
            .fill(fill)
            .stroke(stroke)
            .corner_radius(6)
            .min_size(egui::vec2(32.0, 24.0));
            if ui.add(chip).on_hover_text(tip).clicked() {
                match (target, state) {
                    (Some(_), Some(PageBindState::Bound(b))) => {
                        acts.push(DetailAct::Unbind {
                            binding_id: s(&b, "id"),
                            ok_msg: "已解除页面绑定",
                        });
                    }
                    (Some(_), Some(PageBindState::Missing(b))) => {
                        acts.push(DetailAct::Unbind {
                            binding_id: s(&b, "id"),
                            ok_msg: "已取消缺译标记",
                        });
                    }
                    (Some(t), _) if slot < i(&t.version, "page_count").max(0) as usize => {
                        acts.push(DetailAct::Bind(PageBindingRequest {
                            unit_id: st.unit_id.clone(),
                            from_version_id: version_id.clone(),
                            from_slot: slot as i64,
                            to_version_id: s(&t.version, "id"),
                            to_slot: slot as i64,
                        }));
                    }
                    _ => {
                        st.bind_panel = Some(BindPanelState {
                            page: slot,
                            lang: lang.clone(),
                        });
                    }
                }
            }
        }
        // .bind-open pill: opens (or closes) the panel for this
        // page; accent-filled while the panel serves this page.
        let panel_here = st
            .bind_panel
            .as_ref()
            .is_some_and(|p| p.page == slot);
        let pill = egui::Button::new(
            egui::RichText::new("绑定")
                .size(11.5)
                .strong()
                .color(if panel_here {
                    egui::Color32::WHITE
                } else {
                    palette_color(TEXT_MAIN)
                }),
        )
        .fill(if panel_here {
            ACCENT
        } else {
            palette_color(HOVER_BG)
        })
        .stroke(if panel_here {
            egui::Stroke::new(1.0, ACCENT)
        } else {
            egui::Stroke::new(1.0, palette_color(LINE_34))
        })
        .corner_radius(6)
        .min_size(egui::vec2(0.0, 28.0));
        if ui.add(pill).clicked() {
            if panel_here {
                st.bind_panel = None;
            } else {
                // Default language: the first with no entry at
                // all (prototype @2620), else the first code.
                let default_lang = lang_codes
                    .iter()
                    .find(|code| {
                        targets
                            .iter()
                            .find(|t| &t.lang == *code)
                            .is_none_or(|t| {
                                matches!(
                                    page_bind_state(
                                        &st.bindings,
                                        &version_id,
                                        slot,
                                        &s(&t.version, "id")
                                    ),
                                    PageBindState::Unbound
                                )
                            })
                    })
                    .cloned()
                    .or_else(|| lang_codes.first().cloned())
                    .unwrap_or_else(|| "en".to_string());
                st.bind_panel = Some(BindPanelState {
                    page: slot,
                    lang: default_lang,
                });
            }
        }
    }

    // ------------------------------------------------------------
    // Bind panel (SPEC 四, bindPanelHtml @2751): --panel fill,
    // 1px --line border, radius 6, 14px padding, directly below
    // the wall. Title, a segctl language switch whose labels
    // carry the bind status, the candidate grid (targetPagesFor
    // = the target rendition's latest version's pages), and the
    // action row. Candidate click follows tryPageBind: the
    // already-bound candidate only reports; an occupied one goes
    // through the move-confirm overlay; otherwise it binds and
    // the panel closes (applyPageBind @2761). When the page is
    // marked missing for the language, the candidate area is
    // replaced by the missing banner.
    // ------------------------------------------------------------
    #[allow(clippy::too_many_arguments)]
    fn unit_bind_panel_ui(
        &mut self,
        ui: &mut egui::Ui,
        st: &mut UnitState,
        shown: &UnitVersion,
        targets: &[UnitVersion],
        lang_codes: &[String],
        acts: &mut Vec<DetailAct>,
    ) {
        let Some(panel) = st.bind_panel.clone() else {
            return;
        };
        let version_id = s(&shown.version, "id");
        let page_count = i(&shown.version, "page_count").max(0) as usize;
        if panel.page >= page_count {
            st.bind_panel = None;
            return;
        }
        let pi = panel.page;
        let lang = panel.lang.clone();
        let target = targets.iter().find(|t| t.lang == lang).cloned();
        let cur_state = target.as_ref().map(|t| {
            page_bind_state(&st.bindings, &version_id, pi, &s(&t.version, "id"))
        });
        let is_missing = matches!(cur_state, Some(PageBindState::Missing(_)));
        ui.add_space(12.0);
        egui::Frame::NONE
            .fill(palette_color(PANEL_BG))
            .stroke(egui::Stroke::new(1.0, palette_color(LINE_34)))
            .corner_radius(6)
            .inner_margin(egui::Margin::same(14))
            .show(ui, |ui| {
                ui.label(
                    egui::RichText::new(format!(
                        "第 {} 页「{}」· 绑定其他语言的对应页",
                        pi + 1,
                        page_title(&shown.version, pi)
                    ))
                    .size(15.0)
                    .strong(),
                );
                ui.add_space(10.0);
                // .segctl: --panel2 container, 2px inset, buttons
                // 26px tall; the selected one steps to --panel3.
                egui::Frame::NONE
                    .fill(palette_color(HOVER_BG))
                    .stroke(egui::Stroke::new(1.0, palette_color(SEPARATOR)))
                    .corner_radius(6)
                    .inner_margin(egui::Margin::same(2))
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing.x = 0.0;
                            ui.spacing_mut().button_padding = egui::vec2(12.0, 0.0);
                            for code in lang_codes {
                                let suffix = match targets.iter().find(|t| &t.lang == code) {
                                    Some(t) => match page_bind_state(
                                        &st.bindings,
                                        &version_id,
                                        pi,
                                        &s(&t.version, "id"),
                                    ) {
                                        PageBindState::Bound(b) => {
                                            format!(" · 已绑第 {} 页", i(&b, "to_slot") + 1)
                                        }
                                        PageBindState::Missing(_) => " · 缺译".to_string(),
                                        PageBindState::Unbound => String::new(),
                                    },
                                    None => String::new(),
                                };
                                let active = *code == lang;
                                let btn = egui::Button::new(
                                    egui::RichText::new(format!(
                                        "{}{}",
                                        lang_display_name(code),
                                        suffix
                                    ))
                                    .size(12.5)
                                    .strong()
                                    .color(if active {
                                        palette_color(TEXT_MAIN)
                                    } else {
                                        palette_color(TEXT_WEAK)
                                    }),
                                )
                                .fill(if active {
                                    palette_color(PRESSED_BG)
                                } else {
                                    egui::Color32::TRANSPARENT
                                })
                                .stroke(egui::Stroke::NONE)
                                .corner_radius(6)
                                .min_size(egui::vec2(0.0, 26.0));
                                if ui.add(btn).clicked() && !active {
                                    if let Some(p) = &mut st.bind_panel {
                                        p.lang = code.clone();
                                    }
                                }
                            }
                        });
                    });
                if is_missing {
                    ui.add_space(10.0);
                    egui::Frame::NONE
                        .fill(CHIP_ORANGE_BG)
                        .stroke(egui::Stroke::new(
                            1.0,
                            palette_color(CHIP_ORANGE_FG),
                        ))
                        .corner_radius(6)
                        .inner_margin(egui::Margin {
                            left: 12,
                            right: 12,
                            top: 10,
                            bottom: 10,
                        })
                        .show(ui, |ui| {
                            ui.label(
                                egui::RichText::new("已标缺译 / 无对应页")
                                    .size(13.0)
                                    .strong(),
                            );
                        });
                } else if let Some(t) = &target {
                    let target_id = s(&t.version, "id");
                    let cand_pages = i(&t.version, "page_count").max(0) as usize;
                    ui.add_space(10.0);
                    let saved_spacing = ui.spacing().item_spacing;
                    ui.spacing_mut().item_spacing = egui::vec2(8.0, 8.0);
                    let avail_w = ui.available_width();
                    // .bind-cands (prototype @687): CSS grid,
                    // repeat(auto-fill, minmax(170px, 1fr)),
                    // gap 8 -- the candidate cards stretch
                    // with the panel width. egui has no CSS
                    // grid, so the column count is derived
                    // from the available width with the same
                    // minmax semantics and the rows are laid
                    // out explicitly; the card height stays
                    // fixed at 58, approximating the
                    // prototype's two-line content height
                    // (9px vertical padding around a 13px
                    // line and an 11.5px line).
                    let cols = (((avail_w + 8.0) / (170.0 + 8.0)).floor() as usize).max(1);
                    let card_w = (avail_w - (cols - 1) as f32 * 8.0) / cols as f32;
                    for row_start in (0..cand_pages).step_by(cols) {
                        ui.horizontal(|ui| {
                            for ci in row_start..(row_start + cols).min(cand_pages) {
                                let cand = PageBindingRequest {
                                    unit_id: st.unit_id.clone(),
                                    from_version_id: version_id.clone(),
                                    from_slot: pi as i64,
                                    to_version_id: target_id.clone(),
                                    to_slot: ci as i64,
                                };
                                let is_cur = exact_binding(&st.bindings, &cand).is_some();
                                let owner = occupied_binding(&st.bindings, &cand)
                                    .map(|b| i(&b, "from_slot"));
                                // .bind-cand: 2px --line border (the
                                // current one accent on accent-soft),
                                // radius 6, 9/10 padding, two lines.
                                let (rect, resp) = ui.allocate_exact_size(
                                    egui::vec2(card_w, 58.0),
                                    egui::Sense::click(),
                                );
                                let painter = ui.painter();
                                let hovered = resp.hovered();
                                let (fill, stroke) = if is_cur {
                                    (SEL_BG, egui::Stroke::new(2.0, ACCENT))
                                } else if hovered {
                                    (
                                        palette_color(PANEL_BG),
                                        egui::Stroke::new(2.0, ACCENT),
                                    )
                                } else {
                                    (
                                        palette_color(PANEL_BG),
                                        egui::Stroke::new(2.0, palette_color(LINE_34)),
                                    )
                                };
                                painter.rect_filled(rect, 6.0, fill);
                                painter.rect_stroke(
                                    rect,
                                    6.0,
                                    stroke,
                                    egui::StrokeKind::Inside,
                                );
                                let line1 = format!(
                                    "第 {} 页 · {}",
                                    ci + 1,
                                    page_title(&t.version, ci)
                                );
                                let g1 = painter.layout_no_wrap(
                                    line1,
                                    egui::FontId::new(13.0, egui::FontFamily::Proportional),
                                    palette_color(TEXT_MAIN),
                                );
                                painter.galley(
                                    egui::pos2(rect.min.x + 10.0, rect.min.y + 9.0),
                                    g1,
                                    palette_color(TEXT_MAIN),
                                );
                                // Line 2 is the candidate page's own
                                // 说明 (prototype: cp.d) plus the state
                                // suffix. A page without a desc (stock)
                                // falls back to the target rendition's
                                // label — real data about the target,
                                // not an invented page summary.
                                let cand_desc = page_desc(&t.version, ci);
                                let mut line2 = if cand_desc.is_empty() {
                                    t.label()
                                } else {
                                    desc_line(&cand_desc, 30)
                                };
                                if is_cur {
                                    line2.push_str(" · 当前绑定");
                                } else if let Some(owner_slot) = owner {
                                    line2.push_str(&format!(
                                        " · 已绑在中文第 {} 页，点选将移动绑定",
                                        owner_slot + 1
                                    ));
                                }
                                let g2 = painter.layout_no_wrap(
                                    line2,
                                    egui::FontId::new(11.5, egui::FontFamily::Proportional),
                                    palette_color(TEXT_FAINT),
                                );
                                painter.galley(
                                    egui::pos2(rect.min.x + 10.0, rect.min.y + 32.0),
                                    g2,
                                    palette_color(TEXT_FAINT),
                                );
                                if resp.clicked() {
                                    if is_cur {
                                        self.msg = format!(
                                            "第 {} 页已经绑在{}第 {} 页上了。",
                                            pi + 1,
                                            lang_display_name(&lang),
                                            ci + 1
                                        );
                                    } else {
                                        st.bind_panel = None;
                                        acts.push(DetailAct::Bind(cand));
                                    }
                                }
                            }
                        });
                    }
                    ui.spacing_mut().item_spacing = saved_spacing;
                } else {
                    ui.add_space(10.0);

                    ui.label(
                        egui::RichText::new(format!(
                            "尚无{}版本，暂时没有可绑定的对应页。",
                            lang_display_name(&lang)
                        ))
                        .size(13.0)
                        .color(palette_color(TEXT_FAINT)),
                    );
                }
                // .bind-acts: the row under the candidates.
                ui.add_space(12.0);
                ui.horizontal(|ui| {
                    if let Some(PageBindState::Bound(b)) = &cur_state {
                        if btn_default(ui, &format!("解绑{}", lang_display_name(&lang)))
                            .clicked()
                        {
                            acts.push(DetailAct::Unbind {
                                binding_id: s(b, "id"),
                                ok_msg: "已解除页面绑定",
                            });
                        }
                    }
                    if is_missing {
                        if btn_default(ui, "取消缺译标记").clicked() {
                            if let Some(PageBindState::Missing(b)) = &cur_state {
                                acts.push(DetailAct::Unbind {
                                    binding_id: s(b, "id"),
                                    ok_msg: "已取消缺译标记",
                                });
                            }
                        }
                    } else if let Some(t) = &target {
                        if btn_default(ui, "标缺译 / 无对应页").clicked() {
                            acts.push(DetailAct::MarkMissing {
                                from_version_id: version_id.clone(),
                                from_slot: pi as i64,
                                to_version_id: s(&t.version, "id"),
                            });
                        }
                    }
                    if btn_default(ui, "收起").clicked() {
                        st.bind_panel = None;
                    }
                });
            });
    }

    // ------------------------------------------------------------
    // Embedded blocks (SPEC 五, kidsHtml @2597): rendered only
    // when the detail payload carries a `children` array — the
    // current payload does not (data gap, see report: the
    // minimal fix is for the unit detail assembly to attach
    // children shaped like list rows: {unit, renditions[].latest,
    // children?}). When present, each child is a standard card:
    // cover, title, a sub-page strip, and the accent main action
    // that drills into the child.
    // ------------------------------------------------------------
    fn unit_kids_ui(&mut self, ui: &mut egui::Ui, st: &mut UnitState) {
        let Some(children) = st
            .detail
            .get("children")
            .and_then(Value::as_array)
            .filter(|arr| !arr.is_empty())
            .cloned()
        else {
            return;
        };
        ui.add_space(20.0);
        ui.label(egui::RichText::new("内嵌内容块").strong().size(16.0));
        ui.add_space(6.0);
        let saved_spacing = ui.spacing().item_spacing;
        ui.spacing_mut().item_spacing = egui::vec2(12.0, 12.0);
        let avail = ui.available_width();
        let cols = (((avail + 12.0) / (230.0 + 12.0)).floor() as usize).max(1);
        let cell_w = ((avail - 12.0 * (cols as f32 - 1.0)) / cols as f32).max(170.0);
        let inner_w = (cell_w - 2.0).max(120.0);
        let mut drill: Option<String> = None;
        ui.horizontal_wrapped(|ui| {
            for child in &children {
                let cunit = child.get("unit").cloned().unwrap_or(Value::Null);
                let latest = child
                    .get("renditions")
                    .and_then(Value::as_array)
                    .and_then(|arr| arr.first())
                    .and_then(|r| r.get("latest"))
                    .cloned()
                    .unwrap_or(Value::Null);
                let latest_id = s(&latest, "id");
                let nested = child
                    .get("children")
                    .and_then(Value::as_array)
                    .map(|a| a.len())
                    .unwrap_or(0);
                egui::Frame::NONE
                    .fill(palette_color(PANEL_BG))
                    .stroke(egui::Stroke::new(1.0, palette_color(LINE_34)))
                    .corner_radius(6)
                    .inner_margin(egui::Margin::same(0))
                    .show(ui, |ui| {
                        ui.vertical(|ui| {
                            ui.set_width(inner_w);
                            if !latest_id.is_empty() {
                                let resp =
                                    self.thumb_response(ui, &latest_id, 0, inner_w, false);
                                if resp.clicked() {
                                    drill = Some(s(&cunit, "id"));
                                }
                            }
                            egui::Frame::NONE
                                .inner_margin(egui::Margin {
                                    left: 12,
                                    right: 12,
                                    top: 10,
                                    bottom: 11,
                                })
                                .show(ui, |ui| {
                                    ui.add(
                                        egui::Label::new(
                                            egui::RichText::new(s(&cunit, "name"))
                                                .size(13.0)
                                                .strong(),
                                        )
                                        .truncate(),
                                    );
                                    if nested > 0 {
                                        ui.add_space(4.0);
                                        chip(
                                            ui,
                                            &format!("内含 {nested} 个子块"),
                                            palette_color(ACCENT_DEEP),
                                            SEL_BG,
                                        );
                                    }
                                    // .std-sub: the child's pages
                                    // as a strip of mini bars
                                    // under a dashed rule.
                                    let pages = latest
                                        .get("pages")
                                        .and_then(Value::as_array)
                                        .cloned()
                                        .unwrap_or_default();
                                    if !pages.is_empty() {
                                        ui.add_space(8.0);
                                        dashed_rule(ui);
                                        ui.add_space(8.0);
                                        ui.horizontal(|ui| {
                                            ui.spacing_mut().item_spacing.x = 5.0;
                                            let shown_pages = pages.len().min(8);
                                            let bar_w = ((inner_w - 24.0
                                                - 5.0 * (shown_pages as f32 - 1.0))
                                                / shown_pages as f32)
                                                .max(28.0);
                                            for (idx, page) in
                                                pages.iter().take(shown_pages).enumerate()
                                            {
                                                egui::Frame::NONE
                                                    .fill(palette_color(HOVER_BG))
                                                    .stroke(egui::Stroke::new(
                                                        1.0,
                                                        palette_color(LINE_34),
                                                    ))
                                                    .corner_radius(8)
                                                    .inner_margin(egui::Margin {
                                                        left: 7,
                                                        right: 7,
                                                        top: 5,
                                                        bottom: 5,
                                                    })
                                                    .show(ui, |ui| {
                                                        ui.set_width(bar_w - 14.0);
                                                        ui.add(
                                                            egui::Label::new(
                                                                egui::RichText::new(format!(
                                                                    "{}. {}",
                                                                    idx + 1,
                                                                    s(page, "title")
                                                                ))
                                                                .size(11.0)
                                                                .strong(),
                                                            )
                                                            .truncate(),
                                                        );
                                                    });
                                            }
                                            if pages.len() > shown_pages {
                                                ui.label(
                                                    egui::RichText::new("…")
                                                        .size(11.0)
                                                        .color(palette_color(TEXT_FAINT)),
                                                );
                                            }
                                        });
                                    }
                                    ui.add_space(8.0);
                                    // .show-actions .main: the
                                    // accent drill action.
                                    if ui
                                        .add_sized(
                                            egui::vec2(inner_w - 24.0, 30.0),
                                            egui::Button::new(
                                                egui::RichText::new("打开维护 · 点开下钻 →")
                                                    .size(12.0)
                                                    .color(egui::Color32::WHITE),
                                            )
                                            .fill(ACCENT)
                                            .corner_radius(6),
                                        )
                                        .clicked()
                                    {
                                        drill = Some(s(&cunit, "id"));
                                    }
                                });
                        });
                    });
            }
        });
        ui.spacing_mut().item_spacing = saved_spacing;
        if let Some(child_id) = drill {
            if !child_id.is_empty() {
                self.open_unit_detail(&child_id);
            }
        }
    }

    // ------------------------------------------------------------
    // History (SPEC 六, histHtmlForBlk @2555): one section; the
    // client's versions arrive grouped per rendition, so each
    // rendition's line rides under a small subhead (declared
    // grouping difference — the prototype's single line is the
    // block's own semver line, the client's semvers are per
    // rendition). Cards follow the final cascade: --panel fill,
    // 1px --line border, radius 6, 10px padding. The in-use
    // version (the clicked history version, else the rendition's
    // latest — views unification ruling) takes the accent
    // border and the accent bigver; a draft takes the amber
    // border (the prototype's dashed .draftv — egui strokes
    // have no dash, declared). The prototype's separate blue
    // .selected state has no client counterpart: selecting a
    // version IS putting it in use here. Its language chips
    // (which translation hangs on which version) have no data
    // behind them in the client model — not rendered (report).
    // Card selection hot zones: the bigver chip, the thumbs and
    // the caption (egui has no whole-card background click
    // layer that leaves the head buttons live — declared).
    // ------------------------------------------------------------
    fn unit_history_ui(
        &mut self,
        ui: &mut egui::Ui,
        st: &mut UnitState,
        acts: &mut Vec<DetailAct>,
    ) {
        ui.add_space(20.0);
        ui.label(egui::RichText::new("历史版本").strong().size(16.0));
        ui.add_space(6.0);
        let Some(rends) = st
            .detail
            .get("renditions")
            .and_then(|r| r.as_array())
            .cloned()
        else {
            return;
        };
        let unit = st.detail.get("unit").cloned().unwrap_or(json!({}));
        for row in &rends {
            let rend = row.get("rendition").cloned().unwrap_or(json!({}));
            let rend_id = s(&rend, "id");
            ui.label(
                egui::RichText::new(format!("{} · {}", s(&rend, "form"), s(&rend, "lang")))
                    .size(13.0)
                    .color(palette_color(TEXT_WEAK)),
            );
            ui.add_space(4.0);
            // The in-use version of this rendition: the user's
            // history pick while it belongs here, else the
            // rendition's latest (first non-recalled).
            let cur_id = st
                .selected_version
                .clone()
                .filter(|vid| {
                    row.get("versions")
                        .and_then(Value::as_array)
                        .is_some_and(|vs| vs.iter().any(|v| s(v, "id") == *vid))
                })
                .or_else(|| latest_version(row).map(|v| s(&v, "id")))
                .unwrap_or_default();
            let Some(versions) = row.get("versions").and_then(|v| v.as_array()) else {
                continue;
            };
            let saved_spacing = ui.spacing().item_spacing;
            ui.spacing_mut().item_spacing = egui::vec2(12.0, 12.0);
            ui.horizontal_wrapped(|ui| {
                for v in versions {
                    let vid = s(v, "id");
                    let is_draft = s(v, "status") == "draft";
                    let is_recalled = s(v, "status") == "recalled";
                    let is_cur = vid == cur_id;
                    let stroke = if is_draft {
                        egui::Stroke::new(1.0, palette_color(CHIP_ORANGE_FG))
                    } else if is_cur {
                        egui::Stroke::new(1.0, ACCENT)
                    } else {
                        egui::Stroke::new(1.0, palette_color(LINE_34))
                    };
                    egui::Frame::NONE
                        .fill(palette_color(PANEL_BG))
                        .stroke(stroke)
                        .corner_radius(6)
                        .inner_margin(egui::Margin::same(10))
                        .show(ui, |ui| {
                            ui.set_width(264.0);
                            // .vh-card-head: bigver chip, status
                            // badges, then the head actions.
                            ui.horizontal_wrapped(|ui| {
                                let ver_text = format!("v{}", s(v, "semver"));
                                let ver_font = egui::FontId::new(
                                    12.0,
                                    egui::FontFamily::Proportional,
                                );
                                let ver_fg = if is_cur {
                                    egui::Color32::WHITE
                                } else {
                                    palette_color(TEXT_MAIN)
                                };
                                let ver_bg = if is_cur {
                                    ACCENT
                                } else {
                                    palette_color(PRESSED_BG)
                                };
                                let galley = ui
                                    .painter()
                                    .layout_no_wrap(ver_text, ver_font, ver_fg);
                                let (rect, ver_resp) = ui.allocate_exact_size(
                                    egui::vec2(galley.size().x + 16.0, 24.0),
                                    egui::Sense::click(),
                                );
                                ui.painter().rect_filled(rect, 6.0, ver_bg);
                                ui.painter().galley(
                                    egui::pos2(
                                        rect.min.x + 8.0,
                                        rect.center().y - galley.size().y / 2.0,
                                    ),
                                    galley,
                                    ver_fg,
                                );
                                if ver_resp.clicked() {
                                    st.selected_version = Some(vid.clone());
                                    st.selected_rendition = Some(rend_id.clone());
                                    st.selected_page = None;
                                    st.bind_pick = None;
                                }
                                if ver_resp.double_clicked() {
                                    self.open_light(st, v, 0);
                                }
                                if is_draft {
                                    chip(
                                        ui,
                                        "草稿",
                                        palette_color(CHIP_ORANGE_FG),
                                        CHIP_ORANGE_BG,
                                    );
                                }
                                if is_recalled {
                                    chip(ui, "已撤回", palette_color(ROSE), ROSE_SOFT_BG);
                                }
                                ui.with_layout(
                                    egui::Layout::right_to_left(egui::Align::Center),
                                    |ui| {
                                        if vh_btn(ui, "下载片段").clicked() {
                                            if let Ok(resp) = self.call_work(
                                                "GET",
                                                &format!("/v1/versions/{vid}/fragment"),
                                                None,
                                            ) {
                                                if let Some(data) = resp
                                                    .get("data_base64")
                                                    .and_then(|x| x.as_str())
                                                {
                                                    if let (Ok(bytes), Some(path)) = (
                                                        b64d(data),
                                                        rfd::FileDialog::new()
                                                            .set_file_name("fragment.pptx")
                                                            .save_file(),
                                                    ) {
                                                        // Audit2 E-M10: the write result decides the message.
                                                        self.msg = match std::fs::write(
                                                            path, bytes,
                                                        ) {
                                                            Ok(()) => "已保存".into(),
                                                            Err(e) => {
                                                                format!("保存失败：{e}")
                                                            }
                                                        };
                                                    }
                                                }
                                            }
                                        }
                                        if vh_btn(ui, "讲稿").clicked() {
                                            let mut scripts: Vec<String> = v
                                                .get("scripts")
                                                .and_then(|x| x.as_array())
                                                .map(|arr| {
                                                    arr.iter()
                                                        .map(|t| {
                                                            t.as_str().unwrap_or("").to_string()
                                                        })
                                                        .collect()
                                                })
                                                .unwrap_or_default();
                                            let pages = i(v, "page_count") as usize;
                                            while scripts.len() < pages {
                                                scripts.push(String::new());
                                            }
                                            st.scripts_edit = scripts;
                                            st.scripts_for = Some(vid.clone());
                                        }
                                        if !is_recalled && vh_btn(ui, "撤回").clicked() {
                                            let _ = self.call_work(
                                                "POST",
                                                &format!("/v1/versions/{vid}/recall"),
                                                None,
                                            );
                                            self.reload_unit_detail(&st.unit_id.clone());
                                        }
                                        if !is_recalled && vh_btn(ui, "编辑页面").clicked() {
                                            self.open_page_editor(&unit, v, 0);
                                        }
                                        if is_draft {
                                            // The prototype puts
                                            // ALL card-head actions
                                            // after the flex spacer,
                                            // in the one right-hand
                                            // group (prototype
                                            // @2564); the draft
                                            // pair leads that group.
                                            // This right-to-left
                                            // layout stacks later
                                            // widgets further left,
                                            // so adding 放弃 then
                                            // 发布为最新版本 here
                                            // renders them at the
                                            // group's left edge in
                                            // the prototype's
                                            // order.
                                            if vh_btn(ui, "放弃").clicked() {
                                                acts.push(DetailAct::DropVersion(
                                                    vid.clone(),
                                                ));
                                            }
                                            if vh_btn_primary(ui, "发布为最新版本").clicked() {
                                                acts.push(DetailAct::PublishDraft(
                                                    vid.clone(),
                                                ));
                                            }
                                        }
                                    },
                                );
                            });
                            ui.add_space(9.0);
                            // .vh-thumbs: click selects the
                            // version (views unification ruling:
                            // the wall above and the bottom-bar
                            // views follow the pick); double-click
                            // opens the light view at that page.
                            ui.horizontal_wrapped(|ui| {
                                let page_count = i(v, "page_count").max(0) as usize;
                                for p in 0..page_count {
                                    let thumb =
                                        self.thumb_response(ui, &vid, p, 120.0, false);
                                    if thumb.clicked() {
                                        st.selected_version = Some(vid.clone());
                                        st.selected_rendition = Some(rend_id.clone());
                                        st.selected_page = None;
                                        st.bind_pick = None;
                                    }
                                    if thumb.double_clicked() {
                                        self.open_light(st, v, p);
                                    }
                                }
                            });
                            ui.add_space(8.0);
                            // .vh-caption: meta line, then the
                            // revision note. The .vh-cap-meta
                            // order is the prototype's: date
                            // first, then author (prototype
                            // @2564: (h.date||h.time), then
                            // h.author, joined by " · ").
                            let meta = ui.add(
                                egui::Label::new(
                                    egui::RichText::new(format!(
                                        "{} · {}",
                                        fmt_time(i(v, "created_ms")),
                                        s(v, "author")
                                    ))
                                    .size(11.5)
                                    .color(palette_color(TEXT_FAINT)),
                                )
                                .sense(egui::Sense::click()),
                            );
                            if meta.clicked() {
                                st.selected_version = Some(vid.clone());
                                st.selected_rendition = Some(rend_id.clone());
                                st.selected_page = None;
                                st.bind_pick = None;
                            }
                            if !s(v, "note").is_empty() {
                                ui.label(
                                    egui::RichText::new(s(v, "note"))
                                        .size(11.5)
                                        .color(palette_color(TEXT_FAINT)),
                                );
                            }
                        });
                }
            });
            ui.spacing_mut().item_spacing = saved_spacing;
            ui.add_space(8.0);
        }
    }
}

/// The history card head's button form (prototype: .btn at the
/// card-head size — min-height 24, 12px text, 0/10 padding).
fn vh_btn(ui: &mut egui::Ui, label: &str) -> egui::Response {
    ui.add(
        egui::Button::new(
            egui::RichText::new(label)
                .size(12.0)
                .color(palette_color(TEXT_MAIN)),
        )
        .fill(palette_color(HOVER_BG))
        .stroke(egui::Stroke::new(1.0, palette_color(FIELD_BORDER)))
        .corner_radius(6)
        .min_size(egui::vec2(0.0, 24.0)),
    )
}

fn vh_btn_primary(ui: &mut egui::Ui, label: &str) -> egui::Response {
    ui.add(
        egui::Button::new(
            egui::RichText::new(label)
                .size(12.0)
                .color(egui::Color32::WHITE),
        )
        .fill(ACCENT)
        .corner_radius(6)
        .min_size(egui::vec2(0.0, 24.0)),
    )
}

impl App {
    fn request_page_binding(&mut self, request: PageBindingRequest, bindings: &[Value]) {
        if exact_binding(bindings, &request).is_some() {
            self.msg = "页面已经绑定".into();
            return;
        }
        if let Some(owner) = occupied_binding(bindings, &request) {
            self.overlays.push(Overlay::Confirm(ConfirmState {
                title: "移动页面绑定？".into(),
                body: format!(
                    "目标页已绑定在第 {} 页。继续会解除原绑定，并改绑到第 {} 页。",
                    i(&owner, "from_slot") + 1,
                    request.from_slot + 1
                ),
                confirm_label: "移动绑定".into(),
                danger: false,
                action: ConfirmAction::MovePageBinding(request),
            }));
        } else {
            self.apply_page_binding(request);
        }
    }

    pub(super) fn apply_page_binding(&mut self, request: PageBindingRequest) {
        let Ok(bindings) = self.call_work(
            "GET",
            &format!("/v1/units/{}/page-bindings", request.unit_id),
            None,
        ) else {
            return;
        };
        let bindings = bindings["bindings"].as_array().cloned().unwrap_or_default();
        let mut replaced = Vec::new();
        for binding in bindings {
            let same_source_target = s(&binding, "from_version_id") == request.from_version_id
                && i(&binding, "from_slot") == request.from_slot
                && s(&binding, "to_version_id") == request.to_version_id;
            let occupied = s(&binding, "from_version_id") != request.from_version_id
                && s(&binding, "to_version_id") == request.to_version_id
                && i(&binding, "to_slot") == request.to_slot
                && s(&binding, "kind") != "missing";
            if same_source_target || occupied {
                replaced.push(binding);
            }
        }
        let mut removed = Vec::new();
        for binding in &replaced {
            let id = s(binding, "id");
            if let Err(error) = self.call_work(
                "POST",
                &format!("/v1/units/{}/page-bindings/{id}/delete", request.unit_id),
                None,
            ) {
                for binding in &removed {
                    self.restore_page_binding(&request.unit_id, binding);
                }
                self.msg = format!("移动绑定失败：{error}");
                return;
            }
            removed.push(binding.clone());
        }
        let result = self.call_work(
            "POST",
            &format!("/v1/units/{}/page-bindings", request.unit_id),
            Some(json!({
                "from_version_id": request.from_version_id,
                "from_slot": request.from_slot,
                "to_version_id": request.to_version_id,
                "to_slot": request.to_slot,
            })),
        );
        if let Err(error) = result {
            for binding in &removed {
                self.restore_page_binding(&request.unit_id, binding);
            }
            self.msg = format!("绑定失败：{error}");
            return;
        }
        self.msg = "页面绑定已更新".into();
        self.reload_unit_detail(&request.unit_id);
        // Bindings reshape the resolved page sequence, so the
        // wall card's page count can move with them.
        self.changed(Change::Units);
    }

    fn restore_page_binding(&mut self, unit_id: &str, binding: &Value) {
        let _ = self.call_work(
            "POST",
            &format!("/v1/units/{unit_id}/page-bindings"),
            Some(json!({
                "from_version_id": s(binding, "from_version_id"),
                "from_slot": i(binding, "from_slot"),
                "to_version_id": s(binding, "to_version_id"),
                "to_slot": i(binding, "to_slot"),
                "kind": s(binding, "kind"),
            })),
        );
    }

    fn delete_page_binding(&mut self, unit_id: &str, binding_id: &str) {
        self.delete_page_binding_msg(unit_id, binding_id, "已解除页面绑定");
    }

    fn delete_page_binding_msg(&mut self, unit_id: &str, binding_id: &str, ok_msg: &str) {
        match self.call_work(
            "POST",
            &format!("/v1/units/{unit_id}/page-bindings/{binding_id}/delete"),
            None,
        ) {
            Ok(_) => {
                self.msg = ok_msg.into();
                self.reload_unit_detail(unit_id);
                // Same reasoning as apply_page_binding: the
                // resolved sequence (and page count) can move.
                self.changed(Change::Units);
            }
            Err(error) => self.msg = format!("解绑失败：{error}"),
        }
    }

    /// 标缺译 / 无对应页： replace whatever this page had toward
    /// the target version (a real binding or an older mark) with
    /// one kind = "missing" row (to_slot = -1; core stores it as
    /// the mark, see Store::add_page_binding). Unmarking is just
    /// deleting that row.
    fn apply_missing_mark(
        &mut self,
        unit_id: &str,
        from_version_id: &str,
        from_slot: i64,
        to_version_id: &str,
    ) {
        let Ok(bindings) = self.call_work(
            "GET",
            &format!("/v1/units/{unit_id}/page-bindings"),
            None,
        ) else {
            return;
        };
        let rows = bindings["bindings"].as_array().cloned().unwrap_or_default();
        for binding in &rows {
            if s(binding, "from_version_id") == from_version_id
                && i(binding, "from_slot") == from_slot
                && s(binding, "to_version_id") == to_version_id
            {
                let id = s(binding, "id");
                let _ = self.call_work(
                    "POST",
                    &format!("/v1/units/{unit_id}/page-bindings/{id}/delete"),
                    None,
                );
            }
        }
        match self.call_work(
            "POST",
            &format!("/v1/units/{unit_id}/page-bindings"),
            Some(json!({
                "from_version_id": from_version_id,
                "from_slot": from_slot,
                "to_version_id": to_version_id,
                "to_slot": -1,
                "kind": "missing",
            })),
        ) {
            Ok(_) => {
                self.msg = "已标缺译".into();
                self.reload_unit_detail(unit_id);
                self.changed(Change::Units);
            }
            Err(e) => self.msg = format!("标缺译失败：{e}"),
        }
    }

    pub(super) fn light_ui(
        &mut self,
        ui: &mut egui::Ui,
        screen: egui::Rect,
        mut light: LightState,
    ) -> Option<Overlay> {
        let mut request = None;
        let mut unbind = None;
        let mut start_pick = false;
        if light.targets.is_empty() {
            light.target_index = 0;
        } else {
            light.target_index = light.target_index.min(light.targets.len() - 1);
        }
        let preview_screen = egui::Rect::from_min_size(
            screen.min,
            egui::vec2(screen.width(), (screen.height() - 46.0).max(1.0)),
        );
        egui::Area::new(egui::Id::new("content_light"))
            .fixed_pos(preview_screen.min)
            .order(egui::Order::Foreground)
            .show(ui.ctx(), |ui| {
                egui::Frame::NONE
                    .fill(palette_color(PANEL_BG))
                    .show(ui, |ui| {
                        ui.set_min_size(preview_screen.size());
                        // Prototype .lb-layout (lightDlg): the
                        // detail shell pads 22px at the sides and
                        // 16px at the top, the two columns sit
                        // 14px apart, and the sidebar is a fixed
                        // 300px (.lb-side). The stage takes the
                        // remaining width and its height follows
                        // the slide's own aspect — no height cap.
                        let side_width = 300.0;
                        let left_width =
                            (screen.width() - 22.0 - 14.0 - side_width - 22.0).max(260.0);
                        ui.horizontal_top(|ui| {
                            ui.add_space(22.0);
                            ui.vertical(|ui| {
                                ui.set_width(left_width);
                                ui.add_space(16.0);
                                let image_width = left_width;
                                ui.with_layout(
                                    egui::Layout::top_down(egui::Align::Center),
                                    |ui| {
                                        self.thumb_ui(
                                            ui,
                                            &s(&light.version, "id"),
                                            light.slot,
                                            image_width,
                                        );
                                    },
                                );
                                ui.add_space(10.0);
                                // Prototype .note-box: --panel2
                                // fill, 12px radius, 12/14 padding,
                                // 10px below the stage (inline
                                // margin in the lightDlg markup).
                                egui::Frame::NONE
                                    .fill(palette_color(HOVER_BG))
                                    .corner_radius(12)
                                    .inner_margin(egui::Margin {
                                        left: 14,
                                        right: 14,
                                        top: 12,
                                        bottom: 12,
                                    })
                                    .show(ui, |ui| {
                                        ui.set_width(left_width - 28.0);
                                        ui.label(
                                            egui::RichText::new("这一页的备注（讲稿）").strong(),
                                        );
                                        let note = page_note(&light.version, light.slot);
                                        // Prototype note-box: an
                                        // empty note reads as a
                                        // sentence, not a dash
                                        // (drill re-audit R4).
                                        ui.label(if note.is_empty() {
                                            "这一页暂无备注。"
                                        } else {
                                            &note
                                        });
                                    });
                            });
                            ui.add_space(14.0);
                            egui::ScrollArea::vertical()
                                .max_height(preview_screen.height() - 32.0)
                                .show(ui, |ui| {
                                    ui.set_width(side_width);
                                    ui.add_space(16.0);
                                    // Prototype .lb-side: a plain
                                    // column on the page ground —
                                    // no filled panel. Language
                                    // head, candidate standard
                                    // cards, then the full-width
                                    // bind button.
                                    if light.targets.is_empty() {
                                        ui.label(
                                            egui::RichText::new("暂无其他语言版本").weak(),
                                        );
                                        return;
                                    }
                                    let target = light.targets[light.target_index].clone();
                                    ui.label(
                                        egui::RichText::new(lang_display_name(&target.lang))
                                            .strong()
                                            .size(15.0),
                                    );
                                    if light.targets.len() > 1 {
                                        ui.horizontal_wrapped(|ui| {
                                            for (index, t) in
                                                light.targets.iter().enumerate()
                                            {
                                                if ui
                                                    .selectable_label(
                                                        light.target_index == index,
                                                        t.lang.to_uppercase(),
                                                    )
                                                    .clicked()
                                                {
                                                    light.target_index = index;
                                                }
                                            }
                                        });
                                    }
                                    ui.add_space(8.0);
                                    let target_id = s(&target.version, "id");
                                    let pages = i(&target.version, "page_count").max(0) as usize;
                                    for slot in 0..pages {
                                        let candidate = PageBindingRequest {
                                            unit_id: light.unit_id.clone(),
                                            from_version_id: s(&light.version, "id"),
                                            from_slot: light.slot as i64,
                                            to_version_id: target_id.clone(),
                                            to_slot: slot as i64,
                                        };
                                        let exact = exact_binding(&light.bindings, &candidate);
                                        let occupied =
                                            occupied_binding(&light.bindings, &candidate);
                                        // Prototype: the candidates
                                        // are standard cards (the
                                        // wall's own component),
                                        // stacked at the sidebar's
                                        // full width; the bound one
                                        // takes the selected accent
                                        // border and a × that
                                        // unbinds (the prototype's
                                        // cover-corner × sits at
                                        // the title row's right end
                                        // here — declared).
                                        let clicked = standard_card_v(
                                            ui,
                                            exact.is_some(),
                                            None,
                                            true,
                                            CardVariant::Normal,
                                            |ui| {
                                                ui.set_width(side_width - 2.0);
                                                ui.spacing_mut().item_spacing.y = 0.0;
                                                let response = self.thumb_response(
                                                    ui,
                                                    &target_id,
                                                    slot,
                                                    side_width - 2.0,
                                                    false,
                                                );
                                                egui::Frame::NONE
                                                    .inner_margin(egui::Margin {
                                                        left: 12,
                                                        right: 12,
                                                        top: 11,
                                                        bottom: 12,
                                                    })
                                                    .show(ui, |ui| {
                                                        ui.horizontal(|ui| {
                                                            ui.add(
                                                                egui::Label::new(
                                                                    egui::RichText::new(
                                                                        page_title(
                                                                            &target.version,
                                                                            slot,
                                                                        ),
                                                                    )
                                                                    .strong()
                                                                    .size(13.0),
                                                                )
                                                                .truncate(),
                                                            );
                                                            if exact.is_some() {
                                                                ui.with_layout(
                                                                    egui::Layout::right_to_left(
                                                                        egui::Align::Center,
                                                                    ),
                                                                    |ui| {
                                                                        if ui
                                                                            .small_button("×")
                                                                            .on_hover_text(
                                                                                "解除绑定",
                                                                            )
                                                                            .clicked()
                                                                        {
                                                                            unbind =
                                                                                exact.clone();
                                                                        }
                                                                    },
                                                                );
                                                            }
                                                        });
                                                        // Badges row (prototype
                                                        // stdCard badges): the
                                                        // candidate page's 说明
                                                        // as an info badge
                                                        // (audit R3 — the data
                                                        // rides the version's
                                                        // page rows since the
                                                        // R3 data line; a page
                                                        // without one shows no
                                                        // badge), then the
                                                        // occupied badge.
                                                        let cand_desc =
                                                            page_desc(&target.version, slot);
                                                        if !cand_desc.is_empty()
                                                            || occupied.is_some()
                                                        {
                                                            ui.horizontal_wrapped(|ui| {
                                                                if !cand_desc.is_empty() {
                                                                    chip(
                                                                        ui,
                                                                        &desc_line(
                                                                            &cand_desc, 26,
                                                                        ),
                                                                        palette_color(
                                                                            ACCENT_DEEP,
                                                                        ),
                                                                        CHIP_BLUE_BG,
                                                                    );
                                                                }
                                                                if occupied.is_some() {
                                                                    chip(
                                                                        ui,
                                                                        "已被其他页占用",
                                                                        palette_color(
                                                                            CHIP_ORANGE_FG,
                                                                        ),
                                                                        CHIP_ORANGE_BG,
                                                                    );
                                                                }
                                                            });
                                                        }
                                                    });
                                                response.clicked()
                                            },
                                        )
                                        .inner;
                                        if clicked && exact.is_none() {
                                            request = Some(candidate);
                                        }
                                        ui.add_space(8.0);
                                    }
                                    ui.add_space(2.0);
                                    // Prototype #lbGoBind: primary,
                                    // the sidebar's full width.
                                    if ui
                                        .add_sized(
                                            egui::vec2(side_width, 28.0),
                                            egui::Button::new(
                                                egui::RichText::new(tr("绑定"))
                                                    .size(12.5)
                                                    .color(egui::Color32::WHITE),
                                            )
                                            .fill(ACCENT)
                                            .corner_radius(6),
                                        )
                                        .clicked()
                                    {
                                        start_pick = true;
                                    }
                                });
                        });
                    });
            });
        if let Some(binding) = unbind {
            self.delete_page_binding(&light.unit_id, &s(&binding, "id"));
            if let ContentFrame::Unit(st) = self.content.stack.top() {
                light.bindings = st.bindings.clone();
            }
        }
        if let Some(request) = request {
            self.request_page_binding(request, &light.bindings);
            if let ContentFrame::Unit(st) = self.content.stack.top() {
                light.bindings = st.bindings.clone();
            }
        }
        if start_pick {
            let Some(target) = light.targets.get(light.target_index).cloned() else {
                return Some(Overlay::Light(light));
            };
            if let ContentFrame::Unit(st) = self.content.stack.top_mut() {
                st.selected_rendition = Some(target.rendition_id);
                st.bind_pick = Some(PageBindingPick {
                    from_version_id: s(&light.version, "id"),
                    from_slot: light.slot as i64,
                    to_version_id: s(&target.version, "id"),
                    target_lang: target.lang,
                });
            }
            self.msg = format!("为第 {} 页挑对应页——单击缩略图完成绑定", light.slot + 1);
            return None;
        }
        Some(Overlay::Light(light))
    }

    pub(super) fn open_light_editor(&mut self) {
        let Some(Overlay::Light(light_ref)) = self.overlays.last() else {
            return;
        };
        let light = light_ref.clone();
        let unit = match self.content.stack.top() {
            ContentFrame::Unit(st) => st.detail["unit"].clone(),
            _ => return,
        };
        self.overlays.pop();
        self.open_page_editor(&unit, &light.version, light.slot);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn binding_targets_use_latest_non_recalled_versions() {
        let detail = json!({"renditions": [
            {"rendition": {"id": "r-zh", "lang": "zh"}, "versions": [{"id": "v-zh", "status": "normal"}]},
            {"rendition": {"id": "r-en", "lang": "en"}, "versions": [
                {"id": "v-en-old", "status": "recalled"},
                {"id": "v-en-new", "status": "normal"}
            ]}
        ]});
        let targets = binding_targets(&detail, "v-zh");
        assert_eq!(targets.len(), 1);
        assert_eq!(s(&targets[0].version, "id"), "v-en-new");
    }

    #[test]
    fn occupied_binding_requires_a_different_source_page() {
        let request = PageBindingRequest {
            unit_id: "u".into(),
            from_version_id: "v-zh".into(),
            from_slot: 2,
            to_version_id: "v-en".into(),
            to_slot: 1,
        };
        let bindings = vec![json!({
            "id": "b",
            "from_version_id": "v-zh",
            "from_slot": 1,
            "to_version_id": "v-en",
            "to_slot": 1
        })];
        assert_eq!(
            s(&occupied_binding(&bindings, &request).unwrap(), "id"),
            "b"
        );
    }

    #[test]
    fn missing_marks_are_not_bindings() {
        // A kind = "missing" row (to_slot = -1) must never read
        // as an exact or occupying binding, and page_bind_state
        // must surface it as the mark it is.
        let bindings = vec![json!({
            "id": "m",
            "from_version_id": "v-zh",
            "from_slot": 0,
            "to_version_id": "v-en",
            "to_slot": -1,
            "kind": "missing"
        })];
        let request = PageBindingRequest {
            unit_id: "u".into(),
            from_version_id: "v-zh".into(),
            from_slot: 0,
            to_version_id: "v-en".into(),
            to_slot: 0,
        };
        assert!(exact_binding(&bindings, &request).is_none());
        assert!(occupied_binding(&bindings, &request).is_none());
        assert!(matches!(
            page_bind_state(&bindings, "v-zh", 0, "v-en"),
            PageBindState::Missing(_)
        ));
    }
}

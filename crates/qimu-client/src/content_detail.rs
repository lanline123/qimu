use super::*;

// ----------------------------------------------------------------
// #101 内容块详情页 — rebuild2 (user order 2026-11-25 21:09/21:22:
// 推倒重构 + 发布治理数据模型一起建). Written field-by-field
// from the extracted spec (~/workspace/qimu-101-spec2/SPEC.md;
// prototype 启幕原型-内容索引页.html, renderDetail @2589) with
// DIFFERENCES.md as the acceptance list; the previous page's
// code served only as the data-wiring reference.
//
// Page skeleton (SPEC §1): header (title + intro + hint +
// status badges) → thumbnails wall (rendition segctl, bind bar,
// bind panel directly after) → embedded blocks → history (the
// unnumbered working-draft card, then ONE line of version
// cards for the shown rendition). The wall always shows the
// rendition's working head (the unnumbered draft when one
// exists, else the latest version); history cards only SELECT
// — the bottom-bar views open the selection (user rulings
// 2026-11-25). The three client-only sections the old page
// carried below the body (AI 翻译 / 逐页讲稿编辑 / 发布新版本）
// are not part of this page; their whereabouts are recorded
// in the rebuild2 REPORT §四.
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

/// One rendition of the detail payload, governance fields
/// included: the numbered versions in payload (semver) order,
/// the unnumbered working draft riding its own field, the
/// user-arranged line order, and the version-use marker.
#[derive(Clone)]
struct RendRow {
    rendition_id: String,
    form: String,
    lang: String,
    versions: Vec<Value>,
    working: Option<Value>,
    latest: Option<Value>,
    line_order: Vec<String>,
    use_sel: Option<String>,
}

fn rendition_rows(detail: &Value) -> Vec<RendRow> {
    detail
        .get("renditions")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .map(|row| {
            let rendition = row.get("rendition").cloned().unwrap_or(Value::Null);
            RendRow {
                rendition_id: s(&rendition, "id"),
                form: s(&rendition, "form"),
                lang: s(&rendition, "lang"),
                versions: row
                    .get("versions")
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default(),
                working: row
                    .get("working")
                    .filter(|w| w.is_object())
                    .cloned(),
                latest: row
                    .get("latest")
                    .filter(|w| w.is_object())
                    .cloned(),
                line_order: row
                    .get("line_order")
                    .and_then(Value::as_array)
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|x| x.as_str().map(str::to_string))
                            .collect()
                    })
                    .unwrap_or_default(),
                use_sel: row
                    .get("use_sel")
                    .and_then(Value::as_str)
                    .filter(|x| !x.is_empty())
                    .map(str::to_string),
            }
        })
        .collect()
}

/// The history line's display order for one rendition. When
/// the user has arranged the line (any version carries a
/// line_pos), the payload's `line_order` rules as-is. Otherwise
/// the standing ruling applies: drafts first (stable), then
/// the payload's semver order.
fn display_versions(row: &RendRow) -> Vec<Value> {
    let arranged = row
        .versions
        .iter()
        .any(|v| v.get("line_pos").is_some_and(|x| !x.is_null()));
    if arranged {
        let mut out: Vec<Value> = row
            .line_order
            .iter()
            .filter_map(|id| row.versions.iter().find(|v| s(v, "id") == *id).cloned())
            .collect();
        for v in &row.versions {
            if !out.iter().any(|o| s(o, "id") == s(v, "id")) {
                out.push(v.clone());
            }
        }
        out
    } else {
        let mut out: Vec<Value> = row
            .versions
            .iter()
            .filter(|v| s(v, "status") == "draft")
            .cloned()
            .collect();
        out.extend(
            row.versions
                .iter()
                .filter(|v| s(v, "status") != "draft")
                .cloned(),
        );
        out
    }
}

pub(super) fn unit_versions(detail: &Value) -> Vec<UnitVersion> {
    rendition_rows(detail)
        .into_iter()
        .flat_map(|row| {
            row.versions
                .into_iter()
                .map(|version| UnitVersion {
                    rendition_id: row.rendition_id.clone(),
                    form: row.form.clone(),
                    lang: row.lang.clone(),
                    version,
                })
                .collect::<Vec<_>>()
        })
        .collect()
}

/// Resolve which rendition version is active for the bottom-bar play /
/// view actions when #101 is open:
/// - selected working card -> `row.working.or(row.latest)`
/// - selected numbered card -> that version in the active rendition
/// - default (wall head) -> `row.working.or(row.latest)`
/// Returns `(version_obj, show_version_tag_in_title)`.
pub(super) fn resolve_unit_bar_version(st: &UnitState) -> Option<(UnitVersion, bool)> {
    let rows = rendition_rows(&st.detail);
    let row = st
        .selected_rendition
        .as_ref()
        .and_then(|id| rows.iter().find(|r| &r.rendition_id == id))
        .or_else(|| rows.first())?;
    let latest_id = row.latest.as_ref().map(|v| s(v, "id")).unwrap_or_default();
    let (version, non_latest) = if st.selected_working_card {
        let v = row.working.clone().or_else(|| row.latest.clone())?;
        let differs = row.working.is_some() && s(&v, "id") != latest_id;
        (v, differs)
    } else if let Some(vid) = &st.selected_version {
        let v = row
            .working
            .iter()
            .chain(row.versions.iter())
            .find(|v| s(v, "id") == *vid)
            .cloned()
            .or_else(|| row.working.clone())
            .or_else(|| row.latest.clone())?;
        let differs = s(&v, "id") != latest_id;
        (v, differs)
    } else {
        let v = row.working.clone().or_else(|| row.latest.clone())?;
        let differs = row.working.is_some() && s(&v, "id") != latest_id;
        (v, differs)
    };
    Some((
        UnitVersion {
            rendition_id: row.rendition_id.clone(),
            form: row.form.clone(),
            lang: row.lang.clone(),
            version,
        },
        non_latest,
    ))
}

/// Resolve the selected history version (or live working draft) as a
/// delete target for the bottom-bar trash button.
pub(super) fn selected_version_delete_target(st: &UnitState) -> Option<(String, String)> {
    let rows = rendition_rows(&st.detail);
    let row = st
        .selected_rendition
        .as_ref()
        .and_then(|id| rows.iter().find(|r| &r.rendition_id == id))
        .or_else(|| rows.first())?;
    if st.selected_working_card {
        let w = row.working.as_ref()?;
        let id = s(w, "id");
        if id.is_empty() {
            return None;
        }
        return Some((id, "未定号草稿".to_string()));
    }
    let vid = st.selected_version.as_ref()?;
    let ver = row
        .working
        .iter()
        .chain(row.versions.iter())
        .find(|v| s(v, "id") == *vid)?;
    let label = if s(ver, "status") == "working" || s(ver, "semver").is_empty() {
        "未定号草稿".to_string()
    } else {
        format!("v{}", s(ver, "semver"))
    };
    Some((vid.clone(), label))
}

/// The rendition's working head as the payload assembly
/// resolved it (core latest_version): the first version that
/// is neither recalled, rejected nor the working draft.
fn latest_version(row: &Value) -> Option<Value> {
    row.get("versions")
        .and_then(Value::as_array)?
        .iter()
        .find(|version| {
            !matches!(
                s(version, "status").as_str(),
                "recalled" | "rejected" | "working"
            )
        })
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
/// (prototype .lb-bind-head: 英语）, not its code.
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

/// Pages of `version` whose entity ids do not occur in
/// `head` — the prototype's 新增 count for a draft publish.
fn added_pages(version: &Value, head: Option<&Value>) -> usize {
    let Some(head) = head else {
        return i(version, "page_count").max(0) as usize;
    };
    let head_ids: Vec<String> = head
        .get("pages")
        .and_then(Value::as_array)
        .map(|pages| pages.iter().map(|p| s(p, "id")).collect())
        .unwrap_or_default();
    version
        .get("pages")
        .and_then(Value::as_array)
        .map(|pages| {
            pages
                .iter()
                .filter(|p| !head_ids.contains(&s(p, "id")))
                .count()
        })
        .unwrap_or_else(|| i(version, "page_count").max(0) as usize)
}

/// Deferred outcomes of one frame's wall/panel/history drawing.
/// The draw closures borrow the thumbnail cache through `self`,
/// so every store-touching follow-up is collected and executed
/// after the section has been drawn.
pub(crate) enum DetailAct {
    Bind(PageBindingRequest),
    Unbind {
        binding_id: String,
        ok_msg: String,
    },
    MarkMissing {
        from_version_id: String,
        from_slot: i64,
        to_version_id: String,
    },
    /// A numbered draft's publish (its card's inline input).
    PublishDraft {
        version_id: String,
        note: String,
        added: usize,
        semver: String,
        bump: String,
    },
    /// The unnumbered working card's publish. `working_id` is
    /// the live working row when one exists; without one the
    /// run side resolves the mirror state (head draft → its
    /// publish path; published head → seed, then publish).
    PublishWorking {
        working_id: Option<String>,
        rendition_id: String,
        head_id: String,
        head_is_draft: bool,
        note: String,
        added: usize,
        bump: String,
    },
    OpenDraftPublish(DraftPublishState),
    /// 定号插入： the working draft dropped onto a version card.
    InsertWorkingAt {
        working_id: Option<String>,
        rendition_id: String,
        base_version_id: String,
        base_semver: String,
    },
    DropVersion(String),
    ReorderPages {
        version_id: String,
        page_ids: Vec<String>,
        /// The page sequence BEFORE the drag (undo, Ctrl+Z):
        /// the reorder's inverse is the same core call with
        /// this sequence, so the act carries it along.
        old_page_ids: Vec<String>,
    },
    /// Super-admin renumber of a history card's bigver.
    RenumberVersion {
        version_id: String,
        from: String,
        to: String,
    },
    /// Version-use marker set / clear (USE_SEL).
    UseSel {
        rendition_id: String,
        version_id: Option<String>,
        semver: String,
    },
    /// One history language chip's cycle (explicit mount).
    LangMount {
        version_id: String,
        lang: String,
        state: &'static str,
        toast: String,
    },
    /// History-card drag: reorder the line and renumber it.
    ReorderLine {
        rendition_id: String,
        version_ids: Vec<String>,
    },
}

impl App {

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

    /// The display name of the language a binding addresses,
    /// resolved through the open frame's own detail (the
    /// prototype's bind copy names languages, never codes).
    fn target_lang_name(
        &self,
        unit_id: &str,
        from_version_id: &str,
        to_version_id: &str,
    ) -> String {
        if let ContentFrame::Unit(st) = self.content.stack.top() {
            if st.unit_id == unit_id {
                if let Some(t) = binding_targets(&st.detail, from_version_id)
                    .iter()
                    .find(|t| s(&t.version, "id") == to_version_id)
                {
                    return lang_display_name(&t.lang);
                }
            }
        }
        "目标语言".to_string()
    }

    /// Open #406 (the light view) at one page of one version.
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

    pub(crate) fn run_detail_act(&mut self, unit_id: &str, bindings: &[Value], act: DetailAct) {
        match act {
            DetailAct::Bind(request) => {
                self.request_page_binding(request, bindings);
            }
            DetailAct::Unbind { binding_id, ok_msg } => {
                self.delete_page_binding_msg(unit_id, &binding_id, &ok_msg);
            }
            DetailAct::MarkMissing {
                from_version_id,
                from_slot,
                to_version_id,
            } => {
                self.apply_missing_mark(unit_id, &from_version_id, from_slot, &to_version_id);
            }
            DetailAct::PublishDraft {
                version_id,
                note,
                added,
                semver,
                bump,
            } => {
                match self.call_work(
                    "POST",
                    &format!("/v1/versions/{version_id}/publish-draft"),
                    Some(json!({ "note": note, "bump": bump })),
                ) {
                    Ok(resp) => {
                        let published = resp.get("version").cloned().unwrap_or(Value::Null);
                        let published_semver = s(&published, "semver");
                        let semver = if published_semver.is_empty() {
                            semver
                        } else {
                            published_semver
                        };
                        let pages = i(&published, "page_count");
                        self.msg = format!(
                            "草稿已发布为正式最新版本 {}（{} 页）。",
                            semver, pages
                        );
                        let _ = added;
                        self.reload_unit_detail(unit_id);
                        self.changed(Change::Units);
                    }
                    Err(e) => self.msg = format!("发布草稿失败：{e}"),
                }
            }
            DetailAct::OpenDraftPublish(state) => {
                self.overlays
                    .retain(|overlay| !matches!(overlay, Overlay::DraftPublish(_)));
                self.overlays.push(Overlay::DraftPublish(state));
            }
            DetailAct::PublishWorking {
                working_id,
                rendition_id,
                head_id,
                head_is_draft,
                note,
                added,
                bump,
            } => {
                // Resolve the mirror state first: no working
                // row + a draft head publishes through the
                // draft's own path; no working row + a published
                // head seeds the draft, then publishes it.
                let live_id = match working_id {
                    Some(id) => Some(id),
                    None if head_is_draft => {
                        match self.call_work(
                            "POST",
                            &format!("/v1/versions/{head_id}/publish-draft"),
                            Some(json!({ "note": note, "bump": bump })),
                        ) {
                            Ok(resp) => {
                                // Mirror state: the published thing
                                // IS the numbered draft, so its own
                                // handler toast applies (@3901).
                                let semver = resp
                                    .get("version")
                                    .map(|v| s(v, "semver"))
                                    .unwrap_or_default();
                                let pages = resp
                                    .get("version")
                                    .map(|v| i(v, "page_count"))
                                    .unwrap_or(0);
                                self.msg = format!(
                                    "草稿已发布为正式最新版本 {}（{} 页）。",
                                    semver, pages
                                );
                                let _ = added;
                                self.reload_unit_detail(unit_id);
                                self.changed(Change::Units);
                                return;
                            }
                            Err(e) => {
                                self.msg = format!("发布草稿失败：{e}");
                                return;
                            }
                        }
                    }
                    None => match self.call_work(
                        "POST",
                        &format!("/v1/renditions/{rendition_id}/seed-working"),
                        Some(json!({})),
                    ) {
                        Ok(resp) => resp
                            .get("version")
                            .map(|v| s(v, "id"))
                            .filter(|x| !x.is_empty()),
                        Err(e) => {
                            self.msg = format!("建立未定号草稿失败：{e}");
                            return;
                        }
                    },
                };
                let Some(wid) = live_id else {
                    self.msg = "建立未定号草稿失败".into();
                    return;
                };
                match self.call_work(
                    "POST",
                    &format!("/v1/versions/{wid}/publish-working"),
                    Some(json!({ "note": note, "bump": bump })),
                ) {
                    Ok(resp) => {
                        let semver = resp
                            .get("version")
                            .map(|v| s(v, "semver"))
                            .unwrap_or_default();
                        self.msg = format!("已发布 {}（新增 {} 页）。", semver, added);
                        self.reload_unit_detail(unit_id);
                        self.changed(Change::Units);
                    }
                    Err(e) => self.msg = format!("发布草稿失败：{e}"),
                }
            }
            DetailAct::InsertWorkingAt {
                working_id,
                rendition_id,
                base_version_id,
                base_semver,
            } => {
                let live_id = match working_id {
                    Some(id) => Some(id),
                    None => match self.call_work(
                        "POST",
                        &format!("/v1/renditions/{rendition_id}/seed-working"),
                        Some(json!({})),
                    ) {
                        Ok(resp) => resp
                            .get("version")
                            .map(|v| s(v, "id"))
                            .filter(|x| !x.is_empty()),
                        Err(e) => {
                            self.msg = format!("建立未定号草稿失败：{e}");
                            return;
                        }
                    },
                };
                let Some(wid) = live_id else {
                    self.msg = "建立未定号草稿失败".into();
                    return;
                };
                match self.call_work(
                    "POST",
                    &format!("/v1/versions/{wid}/insert-at"),
                    Some(json!({ "base_version_id": base_version_id })),
                ) {
                    Ok(resp) => {
                        let new_semver = resp
                            .get("version")
                            .map(|v| s(v, "semver"))
                            .unwrap_or_default();
                        let shifts: Vec<String> = resp
                            .get("shifts")
                            .and_then(Value::as_array)
                            .map(|arr| {
                                arr.iter()
                                    .map(|sh| {
                                        format!("{} → {}", s(sh, "from"), s(sh, "to"))
                                    })
                                    .collect()
                            })
                            .unwrap_or_default();
                        self.msg = if shifts.is_empty() {
                            format!(
                                "已插入归位：新版定号 {}（以 {} 为基 · 小版本定号）· 落在首位，成为最新版",
                                new_semver, base_semver
                            )
                        } else {
                            format!(
                                "已插入归位：新版定号 {}（以 {} 为基 · 小版本定号）· 级联顺移：{}",
                                new_semver,
                                base_semver,
                                shifts.join("、")
                            )
                        };
                        self.reload_unit_detail(unit_id);
                        self.changed(Change::Units);
                    }
                    Err(e) => self.msg = format!("定号插入失败：{e}"),
                }
            }
            DetailAct::DropVersion(version_id) => {
                match self.call_work(
                    "POST",
                    &format!("/v1/versions/{version_id}/delete"),
                    None,
                ) {
                    Ok(_) => {
                        self.msg = "草稿已放弃。".into();
                        self.reload_unit_detail(unit_id);
                        self.changed(Change::Units);
                    }
                    Err(e) => self.msg = format!("放弃草稿失败：{e}"),
                }
            }
            DetailAct::ReorderPages {
                version_id,
                page_ids,
                old_page_ids,
            } => {
                match self.call_work(
                    "POST",
                    &format!("/v1/versions/{version_id}/reorder-pages"),
                    Some(json!({ "page_ids": page_ids })),
                ) {
                    Ok(resp) => {
                        // The governed entry may have landed the
                        // order on a freshly seeded working
                        // draft; the undo entry and the toast
                        // address the version that holds it now.
                        let held_id = resp
                            .get("version")
                            .map(|v| s(v, "id"))
                            .filter(|x| !x.is_empty())
                            .unwrap_or_else(|| version_id.clone());
                        self.msg = if held_id != version_id {
                            "页序已调整 · 已存入未定号草稿，发布时定号".into()
                        } else {
                            "草稿页序已更新".into()
                        };
                        self.push_undo(
                            "调整草稿页序".to_string(),
                            UndoAction::ReorderDraft {
                                version_id: held_id,
                                unit_id: unit_id.to_string(),
                                old_page_ids,
                            },
                        );
                        self.reload_unit_detail(unit_id);
                        self.changed(Change::Units);
                    }
                    Err(e) => self.msg = format!("调整页序失败：{e}"),
                }
            }
            DetailAct::RenumberVersion { version_id, from, to } => {
                match self.call_work(
                    "POST",
                    &format!("/v1/versions/{version_id}/renumber"),
                    Some(json!({ "semver": to })),
                ) {
                    Ok(_) => {
                        self.msg = format!(
                            "版本号已从 {} 改为 {}（编号变更记录已留存，版本身份不变）。",
                            from, to
                        );
                        self.reload_unit_detail(unit_id);
                        self.changed(Change::Units);
                    }
                    Err(e) => self.msg = format!("改号失败：{e}"),
                }
            }
            DetailAct::UseSel {
                rendition_id,
                version_id,
                semver,
            } => {
                let body = match &version_id {
                    Some(vid) => json!({ "rendition_id": rendition_id, "version_id": vid }),
                    None => json!({ "rendition_id": rendition_id }),
                };
                match self.call_work(
                    "POST",
                    &format!("/v1/units/{unit_id}/use-sel"),
                    Some(body),
                ) {
                    Ok(_) => {
                        self.msg = if version_id.is_none() {
                            "已恢复默认使用版。".into()
                        } else {
                            format!("已选用 {}。", semver)
                        };
                        self.reload_unit_detail(unit_id);
                    }
                    Err(e) => self.msg = format!("选用失败：{e}"),
                }
            }
            DetailAct::LangMount {
                version_id,
                lang,
                state,
                toast,
            } => {
                match self.call_work(
                    "POST",
                    &format!("/v1/units/{unit_id}/lang-mount"),
                    Some(json!({
                        "version_id": version_id,
                        "lang": lang,
                        "state": state,
                    })),
                ) {
                    Ok(_) => {
                        self.msg = toast;
                        self.reload_unit_detail(unit_id);
                    }
                    Err(e) => self.msg = format!("更新语言挂载失败：{e}"),
                }
            }
            DetailAct::ReorderLine {
                rendition_id,
                version_ids,
            } => {
                match self.call_work(
                    "POST",
                    &format!("/v1/units/{unit_id}/line-order"),
                    Some(json!({
                        "rendition_id": rendition_id,
                        "version_ids": version_ids,
                    })),
                ) {
                    Ok(_) => {
                        self.msg = "版本排序完成，版本号已同步调整。".into();
                        self.reload_unit_detail(unit_id);
                        self.changed(Change::Units);
                    }
                    Err(e) => self.msg = format!("版本排序失败：{e}"),
                }
            }
        }
    }
}

impl App {
    // ------------------------------------------------------------
    // Header (SPEC §2): one row that wraps — h2 title (double-
    // click inline edit), the intro + version + policy hint,
    // then the status badges from the payload (governance data,
    // SPEC §15.1). The prototype's header carries NO kind tag
    // and NO delete button (D1); the kind tag lives on the wall
    // cards. The hint's third segment is the version POLICY of
    // the entry this detail was opened through (audit R1,
    // EntryPolicyView); opened through no entry it renders the
    // prototype's default form, 跟随最新.
    // ------------------------------------------------------------
    pub(super) fn unit_header_ui(&mut self, ui: &mut egui::Ui, st: &mut UnitState, unit: &Value) {
        let rows = rendition_rows(&st.detail);
        let active_row = st
            .selected_rendition
            .as_ref()
            .and_then(|id| rows.iter().find(|r| &r.rendition_id == id))
            .or_else(|| rows.first());
        let head_ver = active_row
            .and_then(|r| r.latest.clone().or_else(|| r.working.clone()))
            .unwrap_or(Value::Null);
        let missing_lang = active_row
            .is_some_and(|r| r.lang != "zh" && r.latest.is_none() && r.working.is_none());
        // HD1 (visual pass 2026-11-26): the prototype's head
        // group is a baseline-aligned flex row with 12px gaps
        // (renderDetail @2589: the inner group carries
        // align-items:baseline; the hint is one text node —
        // intro · 当前 vX · 跟随最新/锁定版本 — and the status
        // badges ride the same line). egui has no baseline
        // layout; bottom-of-line-box alignment is the closest
        // faithful form (for the 20px title against the 13px
        // hint it lands within ~2px of the shared baseline,
        // where the old centered wrapped row visibly floated
        // the hint high). The hint segments therefore sit in
        // an inner zero-spacing row keeping their textual
        // " · " joins, and the 12px group gap applies between
        // title, hint cluster and badges.
        // Use a bounded initial row height (28px) instead of `with_layout`
        // over the whole remaining viewport height: `Align::BOTTOM` inside
        // an unbounded `max_rect` places the header at the bottom edge of
        // the window and collapses the ScrollArea below it to 0px height.
        ui.allocate_ui_with_layout(
            egui::vec2(ui.available_width(), 28.0),
            egui::Layout::left_to_right(egui::Align::BOTTOM).with_main_wrap(true),
            |ui| {
                ui.spacing_mut().item_spacing.x = 12.0;
                if matches!(&st.meta_edit, Some((MetaField::Name, _))) {
                    self.meta_edit_ui(ui, st, MetaField::Name);
                } else {
                    let title = ui.add(
                        egui::Label::new(
                            egui::RichText::new(s(unit, "name")).size(20.0).strong(),
                        )
                        .wrap()
                        .sense(egui::Sense::click()),
                    );
                    if title.double_clicked() {
                        st.meta_edit = Some((MetaField::Name, s(unit, "name")));
                        st.meta_edit_focus = true;
                    }
                }
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 0.0;
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
                            st.meta_edit =
                                Some((MetaField::Description, s(unit, "description")));
                            st.meta_edit_focus = true;
                        }
                    }
                    let head_semver = s(&head_ver, "semver");
                    let head_ver_label = if !head_semver.is_empty() {
                        Some(format!("当前 v{head_semver}"))
                    } else if s(&head_ver, "status") == "working" {
                        Some("当前 草稿".to_string())
                    } else {
                        None
                    };
                    if let Some(ver_txt) = &head_ver_label {
                        ui.label(
                            egui::RichText::new(format!(
                                "{}{}",
                                if hint_has_intro { " · " } else { "" },
                                ver_txt
                            ))
                            .size(13.0)
                            .color(palette_color(TEXT_FAINT)),
                        );
                    }
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
                            if hint_has_intro || head_ver_label.is_some() {
                                " · "
                            } else {
                                ""
                            }
                        ))
                        .size(13.0)
                        .color(palette_color(TEXT_FAINT)),
                    );
                });
                // Status badges (prototype .h-badges, right after the
                // hint): derived server/core-side from the unit's
                // rendition state and shipped on the payload — ok
                // badges take the info form, warn badges the warn
                // form. The 编辑中 badge has no state source in the
                // client and is not fabricated (REPORT).
                if let Some(badges) = st
                    .detail
                    .get("status_badges")
                    .and_then(Value::as_array)
                {
                    for badge in badges {
                        let text = s(badge, "text");
                        if text.is_empty() {
                            continue;
                        }
                        if s(badge, "kind") == "warn" {
                            chip(ui, &text, palette_color(CHIP_ORANGE_FG), CHIP_ORANGE_BG);
                        } else {
                            chip(ui, &text, palette_color(ACCENT_DEEP), CHIP_BLUE_BG);
                        }
                    }
                }
            },
        );
        if missing_lang {
            ui.add_space(10.0);
            egui::Frame::NONE
                .fill(palette_color(CHIP_ORANGE_BG))
                .stroke(egui::Stroke::new(1.0, palette_color(CHIP_ORANGE_FG)))
                .corner_radius(6)
                .inner_margin(egui::Margin::symmetric(12, 10))
                .show(ui, |ui| {
                    ui.label(
                        egui::RichText::new("缺译 · 此条目尚无对应语言版")
                            .size(13.0)
                            .strong(),
                    );
                });
        }
    }

    // ------------------------------------------------------------
    // The #101 body, in the prototype's block order: thumbnails
    // (+ bind panel directly after), embedded blocks, history.
    // The wall shows the selected rendition's working head —
    // the unnumbered draft when one exists, else its latest
    // version. A history selection never moves the wall.
    // ------------------------------------------------------------
    pub(super) fn unit_101_body(&mut self, ui: &mut egui::Ui, st: &mut UnitState) {
        let rows = rendition_rows(&st.detail);
        let Some(first) = rows.first() else {
            return;
        };
        if st
            .selected_rendition
            .as_ref()
            .is_none_or(|id| rows.iter().all(|r| &r.rendition_id != id))
        {
            st.selected_rendition = Some(first.rendition_id.clone());
        }
        let selected_id = st.selected_rendition.clone().unwrap_or_default();
        let Some(row) = rows.iter().find(|r| r.rendition_id == selected_id) else {
            return;
        };
        let shown_version = row.working.clone().or_else(|| row.latest.clone());
        let Some(shown_version) = shown_version else {
            return;
        };
        let shown = UnitVersion {
            rendition_id: row.rendition_id.clone(),
            form: row.form.clone(),
            lang: row.lang.clone(),
            version: shown_version,
        };
        let mut acts: Vec<DetailAct> = Vec::new();
        // .dsec margin-top 20: the gap between the header and
        // the first section (DIFFERENCES 差 25).
        ui.add_space(20.0);
        self.unit_thumbs_ui(ui, st, &rows, &shown, &mut acts);
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

    // ------------------------------------------------------------
    // Thumbnails section (SPEC §4): h3 + the rendition segctl,
    // then the framed cells — real page thumbnails over the
    // prototype's frame geometry (2px --line, radius 6, --panel),
    // the toolbar strip (8/10/10) holding the bind bar and the
    // caption. Rows are chunked explicitly (the Frame-in-
    // horizontal_wrapped construction cannot wrap — the trio2
    // finding), with the CSS grid's minmax(230px, 1fr) column
    // math: equal columns, no width floor.
    // ------------------------------------------------------------
    fn unit_thumbs_ui(
        &mut self,
        ui: &mut egui::Ui,
        st: &mut UnitState,
        rows: &[RendRow],
        shown: &UnitVersion,
        acts: &mut Vec<DetailAct>,
    ) {
        let version_id = s(&shown.version, "id");
        let page_count = i(&shown.version, "page_count").max(0) as usize;
        let targets = binding_targets(&st.detail, &version_id);
        let lang_codes = self.bind_lang_codes(&shown.lang, &targets);
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new("页面缩略图").strong().size(16.0));
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
            if rows.len() > 1 {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let labels: Vec<String> = rows
                        .iter()
                        .map(|r| format!("{} · {}", r.form, r.lang))
                        .collect();
                    let active = rows
                        .iter()
                        .position(|r| r.rendition_id == shown.rendition_id)
                        .unwrap_or(0);
                    let label_refs: Vec<&str> = labels.iter().map(|l| l.as_str()).collect();
                    if let Some(next) = segmented(ui, &label_refs, active, SegStyle::Soft) {
                        st.selected_rendition = Some(rows[next].rendition_id.clone());
                        st.selected_version = None;
                        st.selected_working_card = false;
                        st.selected_page = None;
                        st.bind_pick = None;
                        st.bind_panel = None;
                        st.thumb_cell_h.clear();
                        st.hist_card_h.clear();
                    }
                });
            }
        });
        // No spacer under the section title (top-height
        // unification, user ruling 2026-11-28): the title row
        // is followed directly by the thumb grid — the caller
        // zeroes the item spacing for the first row's junction
        // below, so the title-to-grid gap carries neither the
        // old add_space(4) nor the two item spacings around it.
        let picking = st.bind_pick.is_some();
        // Wall page drag (user ruling 2026-11-25 21:22): EVERY
        // shown version's wall is draggable — drafts and the
        // working draft reorder in place, a published version's
        // drop lands in the unnumbered working draft through
        // the governed core entry. Only id-less pages (stock
        // that never entity-ized) cannot be addressed, so the
        // drag stays off there. The per-slot rects are collected
        // during the wall's draw and resolved afterwards, the
        // N4 showroom wall's pattern.
        let draggable = !picking
            && page_count >= 2
            && shown
                .version
                .get("pages")
                .and_then(Value::as_array)
                .is_some_and(|pages| {
                    pages.len() == page_count
                        && pages.iter().all(|p| !s(p, "id").is_empty())
                });
        let mut thumb_rects: Vec<(usize, egui::Rect)> = Vec::new();
        let mut cell_rects: Vec<(usize, egui::Rect)> = Vec::new();
        // T1 (visual pass 2026-11-26): the prototype has NO
        // hint line above the wall — the section title is
        // followed directly by the thumb grid (renderDetail
        // @2597 concatenation). The draft/working drag hint
        // that stood here is removed; the drag itself is
        // unchanged (seam + ghost still teach it in motion).
        let avail = ui.available_width();
        let cols = (((avail + 12.0) / (230.0 + 12.0)).floor() as usize).max(1);
        let cell_w = (avail - 12.0 * (cols as f32 - 1.0)) / cols as f32;
        let inner_w = (cell_w - 4.0).max(120.0);
        if (inner_w - st.thumb_cell_w).abs() > 0.5 {
            st.thumb_cell_h.clear();
            st.thumb_cell_w = inner_w;
        }
        // The first row is placed with item_spacing.y = 0 (see
        // the title-row comment above); the saved value is
        // restored as soon as the first row is placed, so the
        // junctions between rows keep their normal spacing.
        let saved_spacing_y = ui.spacing().item_spacing.y;
        ui.spacing_mut().item_spacing.y = 0.0;
        for row_start in (0..page_count).step_by(cols) {
            let row_end = (row_start + cols).min(page_count);
            let row_target = (row_start..row_end)
                .filter_map(|slot| st.thumb_cell_h.get(&slot))
                .fold(0.0f32, |a, &h| a.max(h));
            ui.horizontal_top(|ui| {
                ui.spacing_mut().item_spacing.x = 12.0;
                for slot in row_start..row_end {
                    let own_natural = st.thumb_cell_h.get(&slot).copied().unwrap_or(0.0);
                    let pad = (row_target - own_natural).max(0.0);
                    let frame_resp = egui::Frame::NONE
                        .fill(palette_color(PANEL_BG))
                        // Outer .thumb-frame turns accent when selected;
                        // thumb_response_art keeps `current = false` so no
                        // inner duplicate border is painted inside it.
                        .stroke(egui::Stroke::new(
                            1.5,
                            if st.selected_page == Some(slot) {
                                ACCENT
                            } else {
                                palette_color(LINE_34)
                            },
                        ))
                        .corner_radius(6)
                        .inner_margin(egui::Margin::same(0))
                        .show(ui, |ui| {
                            ui.vertical(|ui| {
                                ui.set_width(inner_w);
                                let response = self.thumb_response_art(
                                    ui,
                                    &version_id,
                                    slot,
                                    inner_w,
                                    false,
                                    &format!("{} / {}", slot + 1, page_count),
                                    &page_title(&shown.version, slot),
                                );
                                thumb_rects.push((slot, response.rect));
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
                                    // While a page drag is armed the
                                    // release belongs to the drop, not
                                    // to the click (N4's rule).
                                    if response.clicked() && st.page_drag.is_none() {
                                        st.selected_page = Some(slot);
                                    }
                                    if response.double_clicked() && st.page_drag.is_none() {
                                        self.open_light(st, &shown.version, slot);
                                    }
                                }
                                // .tb: 8 top / 10 sides / 10 bottom.
                                // Wrap chips cleanly within inner_w - 20.
                                let tb_w = (inner_w - 20.0).max(20.0);
                                ui.add_space(8.0);
                                egui::Frame::NONE
                                    .inner_margin(egui::Margin::symmetric(10, 0))
                                    .show(ui, |ui| {
                                        ui.set_width(tb_w);
                                        ui.horizontal_wrapped(|ui| {
                                            ui.spacing_mut().item_spacing = egui::vec2(5.0, 5.0);
                                            self.bind_bar_ui(
                                                ui,
                                                st,
                                                shown,
                                                slot,
                                                &targets,
                                                &lang_codes,
                                                acts,
                                                picking,
                                            );
                                        });
                                        let desc = page_desc(&shown.version, slot);
                                        if !desc.is_empty() {
                                            ui.add_space(4.0);
                                            ui.add(
                                                egui::Label::new(
                                                    egui::RichText::new(desc)
                                                        .size(12.0)
                                                        .color(palette_color(TEXT_FAINT)),
                                                )
                                                .wrap(),
                                            );
                                        }
                                    });
                                ui.add_space(10.0 + pad);
                            });
                        });
                    st.thumb_cell_h
                        .insert(slot, frame_resp.response.rect.height() - pad);
                    cell_rects.push((slot, frame_resp.response.rect));
                    if picking {
                        // .thumb-grid.picking: a 2px dashed accent outline per cell.
                        dashed_card_border(
                            ui.painter(),
                            frame_resp.response.rect.expand(2.0),
                            6.0,
                            ACCENT,
                            1.0,
                        );
                    }
                }
            });
            if row_start == 0 {
                ui.spacing_mut().item_spacing.y = saved_spacing_y;
            }
            ui.add_space(12.0);
        }
        // Idempotent restore: also covers page_count == 0,
        // where the loop never ran.
        ui.spacing_mut().item_spacing.y = saved_spacing_y;
        // Page drag resolution (the N4 showroom wall's
        // raw-pointer pattern): the press is tracked against the
        // THUMB rects — a press on the bind bar below a thumb
        // never arms a drag — movement past the threshold arms
        // it, the seam paints at the target CELL, the dragged
        // page's thumb floats with the pointer, and the drop
        // persists through the governed DetailAct. Escape cancels.
        if draggable {
            let pointer = ui.input(|i| i.pointer.interact_pos());
            let primary_down = ui.input(|i| i.pointer.primary_down());
            let primary_released = ui.input(|i| i.pointer.primary_released());
            if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                st.page_press = None;
                st.page_drag = None;
            }
            if let Some(pos) = pointer {
                if primary_down && st.page_drag.is_none() {
                    if st.page_press.is_none() {
                        if let Some((idx, _)) =
                            thumb_rects.iter().find(|(_, r)| r.contains(pos))
                        {
                            st.page_press = Some((*idx, pos));
                        }
                    }
                    if let Some((from, start)) = st.page_press {
                        if start.distance(pos) > 8.0 {
                            st.page_drag = Some(from);
                        }
                    }
                }
                if let Some(from) = st.page_drag {
                    let target = deck_drag_target(&cell_rects, pos);
                    let painter = ui.painter();
                    // Insertion seam at the target cell's left
                    // edge (or the last cell's right).
                    if let Some((_, r)) = cell_rects.get(target) {
                        let x = r.left() - 6.0;
                        painter.line_segment(
                            [egui::pos2(x, r.top()), egui::pos2(x, r.bottom())],
                            egui::Stroke::new(2.5, ACCENT),
                        );
                    } else if let Some((_, r)) = cell_rects.last() {
                        let x = r.right() + 6.0;
                        painter.line_segment(
                            [egui::pos2(x, r.top()), egui::pos2(x, r.bottom())],
                            egui::Stroke::new(2.5, ACCENT),
                        );
                    }
                    // The dragged page floats with the pointer.
                    let ghost = egui::Rect::from_min_size(
                        pos - egui::vec2(inner_w / 2.0, 10.0),
                        egui::vec2(inner_w, inner_w * 9.0 / 16.0),
                    );
                    if let Some(tex) = self.thumb(&version_id, from) {
                        painter.image(
                            tex.id(),
                            ghost,
                            egui::Rect::from_min_max(
                                egui::pos2(0.0, 0.0),
                                egui::pos2(1.0, 1.0),
                            ),
                            egui::Color32::from_rgba_unmultiplied(255, 255, 255, 205),
                        );
                    } else {
                        painter.rect_filled(ghost, 0.0, palette_color(HOVER_BG));
                    }
                    painter.rect_stroke(
                        ghost,
                        0.0,
                        egui::Stroke::new(1.5, ACCENT),
                        egui::StrokeKind::Middle,
                    );
                }
                if primary_released {
                    if let Some(from) = st.page_drag.take() {
                        let target = deck_drag_target(&cell_rects, pos);
                        // Move semantics mirror deck_move_page:
                        // the seam indexes the pre-move sequence.
                        let mut order: Vec<usize> = (0..page_count).collect();
                        if from < order.len() {
                            let moved = order.remove(from);
                            let mut at = target;
                            if at > from {
                                at -= 1;
                            }
                            at = at.min(order.len());
                            order.insert(at, moved);
                            if order.iter().enumerate().any(|(n, o)| n != *o) {
                                let pages = shown
                                    .version
                                    .get("pages")
                                    .and_then(Value::as_array)
                                    .cloned()
                                    .unwrap_or_default();
                                let page_ids: Vec<String> = order
                                    .iter()
                                    .map(|&old| {
                                        pages.get(old).map(|p| s(p, "id")).unwrap_or_default()
                                    })
                                    .collect();
                                // The pre-drag sequence, for the
                                // undo entry the run side records.
                                let old_page_ids: Vec<String> = pages
                                    .iter()
                                    .map(|p| s(p, "id"))
                                    .collect();
                                st.selected_page = Some(at);
                                // The panel addresses pages of
                                // the pre-reorder sequence; it
                                // closes with the drop.
                                st.bind_panel = None;
                                acts.push(DetailAct::ReorderPages {
                                    version_id: version_id.clone(),
                                    page_ids,
                                    old_page_ids,
                                });
                            }
                        }
                    }
                    st.page_press = None;
                }
            } else if !primary_down {
                st.page_press = None;
                st.page_drag = None;
            }
        }
        // The bind panel renders directly after the wall
        // (prototype @2603 concatenation order) — also while a
        // pick is running (SPEC §4: picking never hides it).
        if st.bind_panel.is_some() {
            self.unit_bind_panel_ui(ui, st, shown, &targets, &lang_codes, acts);
        }
    }

    /// The per-page bind bar (SPEC §4, bindBarHtml @2744): one
    /// chip per language of the full content-language set, in
    /// the .lang-chip form enlarged for the thumb bar (min-width
    /// 32, height 24), then the panel-open pill (.bind-open
    /// @683 — its CSS and handler exist in the prototype build
    /// but no element emits it there; the pill is restored here
    /// to its CSS spec so the panel is reachable, and the user
    /// ruled it kept). Chip click is the prototype's DIRECT
    /// TOGGLE (@5026): bound → unbind, missing → unmark, unbound
    /// → bind to the same-index page (an occupied target goes
    /// through the move-confirm overlay). When the same-index
    /// bind is impossible (no version in that language, or the
    /// page has no counterpart), the click opens the panel
    /// instead. While a pick runs the bar renders but is inert.
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
        picking: bool,
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
            if ui.add(chip).on_hover_text(tip).clicked() && !picking {
                match (target, state) {
                    (Some(_), Some(PageBindState::Bound(b))) => {
                        acts.push(DetailAct::Unbind {
                            binding_id: s(&b, "id"),
                            ok_msg: format!(
                                "已解绑：第 {} 页 · {}",
                                slot + 1,
                                lang_display_name(lang)
                            ),
                        });
                    }
                    (Some(_), Some(PageBindState::Missing(b))) => {
                        acts.push(DetailAct::Unbind {
                            binding_id: s(&b, "id"),
                            ok_msg: format!("已取消缺译标记（第 {} 页）。", slot + 1),
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
                .size(11.0)
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
        .min_size(egui::vec2(0.0, 24.0));
        if ui.add(pill).clicked() && !picking {
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
}

impl App {
    // ------------------------------------------------------------
    // Bind panel (SPEC §5, bindPanelHtml @2751): --panel fill,
    // 1px --line border, radius 6, 14px padding, directly below
    // the wall. Title, a segctl language switch whose labels
    // carry the bind status, the candidate grid (targetPagesFor
    // = the target rendition's latest version's pages), and the
    // action row. Candidate click follows tryPageBind: the
    // already-bound candidate only reports; an occupied one
    // goes through the move-confirm overlay; otherwise it binds
    // — and the panel stays OPEN until the bind actually lands
    // (it closes inside apply_page_binding, SPEC §5). When the
    // page is marked missing for the language, the candidate
    // area is replaced by the missing banner. When the target
    // language has NO version yet, the candidates are generated
    // from the source pages in the prototype's own form
    // (targetPagesFor's no-target branch) — but a client
    // binding must address a real target version, so clicking
    // one only reports that; see the REPORT （差 15).
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
                } else {
                    // Candidate grid: .bind-cands (prototype
                    // @687), repeat(auto-fill, minmax(170px, 1fr)),
                    // gap 8 — column count derived from the width
                    // with the same minmax semantics, rows laid
                    // out explicitly, card height following
                    // the wrapped text （差 28).
                    let saved_spacing = ui.spacing().item_spacing;
                    ui.spacing_mut().item_spacing = egui::vec2(8.0, 8.0);
                    let avail_w = ui.available_width();
                    let cols = (((avail_w + 8.0) / (170.0 + 8.0)).floor() as usize).max(1);
                    let card_w = (avail_w - (cols - 1) as f32 * 8.0) / cols as f32;
                    ui.add_space(10.0);
                    match &target {
                        Some(t) => {
                            let target_id = s(&t.version, "id");
                            let cand_pages = i(&t.version, "page_count").max(0) as usize;
                            for row_start in (0..cand_pages).step_by(cols) {
                                ui.horizontal(|ui| {
                                    for ci in row_start..(row_start + cols).min(cand_pages) {
                                        self.bind_candidate_ui(
                                            ui, st, shown, t, &target_id, pi, ci, card_w,
                                            acts,
                                        );
                                    }
                                });
                            }
                        }
                        None => {
                            // No version in this language yet:
                            // the prototype generates candidates
                            // from the SOURCE pages. They render
                            // in the same form; a click reports
                            // the honest state, because a client
                            // binding must address a real target
                            // version （差 15, REPORT).
                            for row_start in (0..page_count).step_by(cols) {
                                ui.horizontal(|ui| {
                                    for ci in row_start..(row_start + cols).min(page_count) {
                                        // Same .bind-cand form as the
                                        // real candidates, height
                                        // following the wrapped text
                                        // （差 28).
                                        let painter = ui.painter().clone();
                                        let wrap_w = (card_w - 20.0).max(40.0);
                                        let g1 = painter.layout(
                                            format!(
                                                "第 {} 页 · {}",
                                                ci + 1,
                                                page_title(&shown.version, ci)
                                            ),
                                            egui::FontId::new(
                                                13.0,
                                                egui::FontFamily::Proportional,
                                            ),
                                            palette_color(TEXT_MAIN),
                                            wrap_w,
                                        );
                                        let g2 = painter.layout(
                                            page_desc(&shown.version, ci),
                                            egui::FontId::new(
                                                11.5,
                                                egui::FontFamily::Proportional,
                                            ),
                                            palette_color(TEXT_FAINT),
                                            wrap_w,
                                        );
                                        let card_h =
                                            9.0 + g1.size().y + 3.0 + g2.size().y + 9.0;
                                        let (rect, resp) = ui.allocate_exact_size(
                                            egui::vec2(card_w, card_h),
                                            egui::Sense::click(),
                                        );
                                        let (fill, stroke) = if resp.hovered() {
                                            (
                                                palette_color(PANEL_BG),
                                                egui::Stroke::new(1.5, ACCENT),
                                            )
                                        } else {
                                            (
                                                palette_color(PANEL_BG),
                                                egui::Stroke::new(1.5, palette_color(LINE_34)),
                                            )
                                        };
                                        painter.rect_filled(rect, 6.0, fill);
                                        painter.rect_stroke(
                                            rect,
                                            6.0,
                                            stroke,
                                            egui::StrokeKind::Inside,
                                        );
                                        let g1_h = g1.size().y;
                                        painter.galley(
                                            egui::pos2(rect.min.x + 10.0, rect.min.y + 9.0),
                                            g1,
                                            palette_color(TEXT_MAIN),
                                        );
                                        painter.galley(
                                            egui::pos2(
                                                rect.min.x + 10.0,
                                                rect.min.y + 9.0 + g1_h + 3.0,
                                            ),
                                            g2,
                                            palette_color(TEXT_FAINT),
                                        );
                                        if resp.clicked() {
                                            self.msg = format!(
                                                "尚无{}版本，暂时没有可绑定的对应页。",
                                                lang_display_name(&lang)
                                            );
                                        }
                                    }
                                });
                            }
                        }
                    }
                    ui.spacing_mut().item_spacing = saved_spacing;
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
                                ok_msg: format!(
                                    "已解绑：第 {} 页 · {}",
                                    pi + 1,
                                    lang_display_name(&lang)
                                ),
                            });
                        }
                    }
                    if is_missing {
                        if btn_default(ui, "取消缺译标记").clicked() {
                            if let Some(PageBindState::Missing(b)) = &cur_state {
                                acts.push(DetailAct::Unbind {
                                    binding_id: s(b, "id"),
                                    ok_msg: format!(
                                        "已取消缺译标记（第 {} 页）。",
                                        pi + 1
                                    ),
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

    /// One real candidate card of the bind panel (the target
    /// rendition's latest version's pages): current binding in
    /// accent on accent-soft, occupation noted on the second
    /// line — whose text is the candidate page's own 说明，
    /// falling back to the SOURCE page's 说明 when the target
    /// page has none (prototype targetPagesFor: cp.d || p.d).
    #[allow(clippy::too_many_arguments)]
    fn bind_candidate_ui(
        &mut self,
        ui: &mut egui::Ui,
        st: &UnitState,
        shown: &UnitVersion,
        target: &UnitVersion,
        target_id: &str,
        pi: usize,
        ci: usize,
        card_w: f32,
        acts: &mut Vec<DetailAct>,
    ) {
        let cand = PageBindingRequest {
            unit_id: st.unit_id.clone(),
            from_version_id: s(&shown.version, "id"),
            from_slot: pi as i64,
            to_version_id: target_id.to_string(),
            to_slot: ci as i64,
        };
        let is_cur = exact_binding(&st.bindings, &cand).is_some();
        let owner = occupied_binding(&st.bindings, &cand).map(|b| i(&b, "from_slot"));
        // .bind-cand: 2px --line border (the current one accent
        // on accent-soft), radius 6, 9/10 padding, two lines —
        // and the card's height FOLLOWS the wrapped text
        // (DIFFERENCES 差 28): both lines are laid out with
        // wrapping at the inner width and the card is as tall
        // as they need, never a fixed 58 with overflow.
        let painter = ui.painter().clone();
        let wrap_w = (card_w - 20.0).max(40.0);
        let line1 = format!("第 {} 页 · {}", ci + 1, page_title(&target.version, ci));
        let g1 = painter.layout(
            line1,
            egui::FontId::new(13.0, egui::FontFamily::Proportional),
            palette_color(TEXT_MAIN),
            wrap_w,
        );
        let cand_desc = page_desc(&target.version, ci);
        let mut line2 = if cand_desc.is_empty() {
            page_desc(&shown.version, ci)
        } else {
            cand_desc
        };
        if is_cur {
            line2.push_str(" · 当前绑定");
        } else if let Some(owner_slot) = owner {
            line2.push_str(&format!(
                " · 已绑在中文第 {} 页，点选将移动绑定",
                owner_slot + 1
            ));
        }
        let g2 = painter.layout(
            line2,
            egui::FontId::new(11.5, egui::FontFamily::Proportional),
            palette_color(TEXT_FAINT),
            wrap_w,
        );
        let card_h = 9.0 + g1.size().y + 3.0 + g2.size().y + 9.0;
        let (rect, resp) = ui.allocate_exact_size(egui::vec2(card_w, card_h), egui::Sense::click());
        let hovered = resp.hovered();
        let (fill, stroke) = if is_cur {
            (SEL_BG, egui::Stroke::new(1.5, ACCENT))
        } else if hovered {
            (palette_color(PANEL_BG), egui::Stroke::new(1.5, ACCENT))
        } else {
            (
                palette_color(PANEL_BG),
                egui::Stroke::new(1.5, palette_color(LINE_34)),
            )
        };
        painter.rect_filled(rect, 6.0, fill);
        painter.rect_stroke(rect, 6.0, stroke, egui::StrokeKind::Inside);
        let g1_h = g1.size().y;
        painter.galley(
            egui::pos2(rect.min.x + 10.0, rect.min.y + 9.0),
            g1,
            palette_color(TEXT_MAIN),
        );
        painter.galley(
            egui::pos2(rect.min.x + 10.0, rect.min.y + 9.0 + g1_h + 3.0),
            g2,
            palette_color(TEXT_FAINT),
        );
        if resp.clicked() {
            if is_cur {
                self.msg = format!(
                    "第 {} 页已经绑在{}第 {} 页上了。",
                    pi + 1,
                    lang_display_name(&target.lang),
                    ci + 1
                );
            } else {
                // The panel is NOT closed here: it closes when
                // the bind lands (apply_page_binding) or when
                // the user 收起s it — an occupied candidate's
                // confirm overlay must find it still open
                // underneath (SPEC §5).
                acts.push(DetailAct::Bind(cand));
            }
        }
    }
}

impl App {
    // ------------------------------------------------------------
    // Embedded blocks (SPEC §6, kidsHtml @2597): rendered only
    // when the detail payload carries children. Each child is a
    // standard card: cover, title, the 内含 badge in the badges
    // row, a sub-page strip with ALL pages at equal flexible
    // widths, and the accent main action. Selection semantics
    // are the prototype's (SPEC §10.10's sibling rule): a single
    // click on the cover or the card body only SELECTS (the
    // frame's colour turns accent — colour only, no outer
    // ring; user ruling 2026-11-27); the drill is the main
    // button or a
    // cover double-click. Rows are chunked explicitly (the
    // Frame wrap finding).
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
        ui.add_space(4.0);
        let avail = ui.available_width();
        let cols = (((avail + 12.0) / (230.0 + 12.0)).floor() as usize).max(1);
        let cell_w = ((avail - 12.0 * (cols as f32 - 1.0)) / cols as f32).max(170.0);
        let inner_w = (cell_w - 2.0).max(120.0);
        if (inner_w - st.kid_card_w).abs() > 0.5 {
            st.kid_card_h.clear();
            st.kid_card_w = inner_w;
        }
        let mut drill: Option<String> = None;
        for row_start in (0..children.len()).step_by(cols) {
            let row_slice = &children[row_start..(row_start + cols).min(children.len())];
            let row_target = row_slice
                .iter()
                .filter_map(|child| {
                    let cid = child.get("unit").map(|u| s(u, "id")).unwrap_or_default();
                    st.kid_card_h.get(&cid)
                })
                .fold(0.0f32, |a, &h| a.max(h));
            ui.horizontal_top(|ui| {
                ui.spacing_mut().item_spacing.x = 12.0;
                for child in row_slice {
                    let cunit = child.get("unit").cloned().unwrap_or(Value::Null);
                    let child_id = s(&cunit, "id");
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
                    let selected = st.kid_sel.as_deref() == Some(child_id.as_str());
                    let own_natural = st.kid_card_h.get(&child_id).copied().unwrap_or(0.0);
                    let pad = (row_target - own_natural).max(0.0);
                    let mut action_rect = egui::Rect::NOTHING;
                    let frame_resp = egui::Frame::NONE
                        .fill(palette_color(PANEL_BG))
                        .stroke(egui::Stroke::new(
                            1.5,
                            if selected {
                                ACCENT
                            } else {
                                palette_color(LINE_34)
                            },
                        ))
                        .corner_radius(6)
                        .inner_margin(egui::Margin::same(0))
                        .show(ui, |ui| {
                            ui.vertical(|ui| {
                                ui.set_width(inner_w);
                                let resp = self.thumb_response_art(
                                    ui,
                                    &latest_id,
                                    0,
                                    inner_w,
                                    false,
                                    "",
                                    &s(&cunit, "name"),
                                );
                                if resp.clicked() {
                                    st.kid_sel = Some(child_id.clone());
                                }
                                if resp.double_clicked() {
                                    drill = Some(child_id.clone());
                                }
                                egui::Frame::NONE
                                    .inner_margin(egui::Margin {
                                        left: 12,
                                        right: 12,
                                        top: 10,
                                        bottom: 11,
                                    })
                                    .show(ui, |ui| {
                                        let title = ui.add(
                                            egui::Label::new(
                                                egui::RichText::new(s(&cunit, "name"))
                                                    .size(13.0)
                                                    .strong(),
                                            )
                                            .truncate()
                                            .sense(egui::Sense::click()),
                                        );
                                        if title.clicked() {
                                            st.kid_sel = Some(child_id.clone());
                                        }
                                        // .badges row: the 内含
                                        // chip rides directly
                                        // under the title.
                                        if nested > 0 {
                                            ui.horizontal_wrapped(|ui| {
                                                chip(
                                                    ui,
                                                    &format!("内含 {nested} 个子块"),
                                                    palette_color(ACCENT_DEEP),
                                                    SEL_BG,
                                                );
                                            });
                                        }
                                        // .std-sub: ALL of the child's
                                        // pages as wrapped mini bars under
                                        // a dashed rule with a minimum width
                                        // floor so multi-page blocks stay readable.
                                        let pages = latest
                                            .get("pages")
                                            .and_then(Value::as_array)
                                            .cloned()
                                            .unwrap_or_default();
                                        if !pages.is_empty() {
                                            ui.add_space(8.0);
                                            dashed_rule(ui);
                                            ui.add_space(8.0);
                                            let strip_w = (inner_w - 24.0).max(60.0);
                                            let per_row = (((strip_w + 5.0) / (72.0 + 5.0))
                                                .floor()
                                                as usize)
                                                .max(1)
                                                .min(pages.len());
                                            for sub_start in (0..pages.len()).step_by(per_row) {
                                                if sub_start > 0 {
                                                    ui.add_space(5.0);
                                                }
                                                let sub_end = (sub_start + per_row).min(pages.len());
                                                let sub_len = (sub_end - sub_start) as f32;
                                                let bar_w =
                                                    (strip_w - 5.0 * (sub_len - 1.0)) / sub_len;
                                                ui.horizontal(|ui| {
                                                    ui.spacing_mut().item_spacing.x = 5.0;
                                                    for (idx, page) in
                                                        pages[sub_start..sub_end].iter().enumerate()
                                                    {
                                                        let page_no = sub_start + idx + 1;
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
                                                                ui.set_width(
                                                                    (bar_w - 14.0).max(0.0),
                                                                );
                                                                ui.add(
                                                                    egui::Label::new(
                                                                        egui::RichText::new(
                                                                            format!(
                                                                                "{}. {}",
                                                                                page_no,
                                                                                s(page, "title")
                                                                            ),
                                                                        )
                                                                        .size(11.0)
                                                                        .strong(),
                                                                    )
                                                                    .truncate(),
                                                                );
                                                            });
                                                    }
                                                });
                                            }
                                        }
                                        ui.add_space(8.0 + pad);
                                        let btn_resp = ui.add_sized(
                                            egui::vec2(inner_w - 24.0, 30.0),
                                            egui::Button::new(
                                                egui::RichText::new("打开维护 · 点开下钻 →")
                                                    .size(12.0)
                                                    .strong()
                                                    .color(egui::Color32::WHITE),
                                            )
                                            .fill(ACCENT)
                                            .corner_radius(6),
                                        );
                                        action_rect = btn_resp.rect;
                                        if btn_resp.clicked() {
                                            drill = Some(child_id.clone());
                                        }
                                    });
                            });
                        });
                    st.kid_card_h
                        .insert(child_id.clone(), frame_resp.response.rect.height() - pad);
                    if ui.input(|i| i.pointer.primary_clicked()) {
                        if let Some(pos) = ui.input(|i| i.pointer.interact_pos()) {
                            if frame_resp.response.rect.contains(pos) && !action_rect.contains(pos)
                            {
                                st.kid_sel = Some(child_id.clone());
                            }
                        }
                    }
                }
            });
            ui.add_space(12.0);
        }
        if let Some(child_id) = drill {
            if !child_id.is_empty() {
                self.push_unit_detail(&child_id);
            }
        }
    }
}

/// Shared context for one history line's rendering.
struct HistCtx {
    row: RendRow,
    /// The version the line is using (USE_SEL ?? the current
    /// published head).
    using_id: Option<String>,
    /// The current published head version (first normal in
    /// payload order), for 新增 counts and derivations.
    head_normal: Option<Value>,
    /// Enabled history languages: en first, then the other
    /// renditions' languages.
    langs: Vec<String>,
    mounts: Vec<Value>,
    is_super: bool,
    my_name: String,
}

/// One history language's resolved state for one version:
/// an explicit mount wins; otherwise the derivation of
/// SPEC §10.7 (target rendition basis vs this version).
fn hist_lang_state(ctx: &HistCtx, detail: &Value, version: &Value, lang: &str) -> &'static str {
    let vid = s(version, "id");
    for m in &ctx.mounts {
        if s(m, "version_id") == vid && s(m, "lang") == lang {
            return match s(m, "state").as_str() {
                "synced" => "synced",
                "stale" => "stale",
                _ => "missing",
            };
        }
    }
    let target = detail
        .get("renditions")
        .and_then(Value::as_array)
        .and_then(|arr| {
            arr.iter().find(|r| {
                s(&r["rendition"], "lang") == lang
                    && s(&r["rendition"], "id") != ctx.row.rendition_id
            })
        });
    let Some(target) = target else {
        return "missing";
    };
    let is_current = ctx
        .head_normal
        .as_ref()
        .is_some_and(|h| s(h, "id") == vid);
    if !is_current {
        return "missing";
    }
    let target_latest = target.get("latest").cloned().unwrap_or(Value::Null);
    if target_latest.is_object()
        && i(&target_latest, "basis_revision") >= i(version, "basis_revision")
    {
        "synced"
    } else {
        "stale"
    }
}

impl App {
    // ------------------------------------------------------------
    // History (SPEC §7, histHtmlForBlk @2555): ONE line for the
    // shown rendition — the unnumbered working-draft card
    // always first, then the numbered cards in display order.
    // Card anatomy follows the final cascade (SPEC §10.2);
    // interactions follow §10.10 as amended by the user's
    // rulings: a card click only SELECTS (accent border + ring;
    // the wall never follows — the bottom bar opens the
    // selection), a thumbnail click selects the thumbnail, a
    // thumbnail double-click opens #406 at that page, a card
    // drag reorders the line (the working card's drag is the
    // 定号插入）. Head segments hide under width pressure in
    // the prototype's order (languages first, then badges).
    // ------------------------------------------------------------
    fn unit_history_ui(
        &mut self,
        ui: &mut egui::Ui,
        st: &mut UnitState,
        acts: &mut Vec<DetailAct>,
    ) {
        ui.add_space(20.0);
        ui.label(egui::RichText::new("版本管理").strong().size(16.0));
        ui.add_space(4.0);
        let rows = rendition_rows(&st.detail);
        let Some(row) = rows
            .iter()
            .find(|r| Some(&r.rendition_id) == st.selected_rendition.as_ref())
            .or_else(|| rows.first())
            .cloned()
        else {
            return;
        };
        let head_normal = row
            .versions
            .iter()
            .find(|v| s(v, "status") == "normal")
            .cloned();
        let using_id = row
            .use_sel
            .clone()
            .or_else(|| head_normal.as_ref().map(|v| s(v, "id")));
        let mut langs: Vec<String> = vec!["en".to_string()];
        for r in &rows {
            if r.rendition_id != row.rendition_id
                && r.lang != row.lang
                && !langs.contains(&r.lang)
            {
                langs.push(r.lang.clone());
            }
        }
        let ctx = HistCtx {
            using_id,
            head_normal,
            langs,
            mounts: st
                .detail
                .get("lang_mounts")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default(),
            is_super: self.role() == "super",
            my_name: self
                .user
                .as_ref()
                .and_then(|u| u.get("display_name"))
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            row,
        };
        if ctx.row.working.is_none() {
            st.selected_working_card = false;
        }
        let versions = display_versions(&ctx.row);
        let mut keys: Vec<String> = if ctx.row.working.is_some() {
            vec!["working".to_string()]
        } else {
            Vec::new()
        };
        keys.extend(versions.iter().map(|v| s(v, "id")));
        // Geometry: repeat(auto-fill, minmax(248px, 1fr)) with
        // a 300px cap; at most 640px wide the cards go full
        // width, one per row.
        let avail = ui.available_width();
        let (cols, card_w) = if avail <= 640.0 {
            (1, avail)
        } else {
            let c = (((avail + 12.0) / (248.0 + 12.0)).floor() as usize).max(1);
            let w = ((avail - 12.0 * (c as f32 - 1.0)) / c as f32).min(300.0);
            (c, w)
        };
        // H2 (visual pass 2026-11-26): measured heights are
        // only valid at the card width they were taken at.
        if (card_w - st.hist_card_w).abs() > 0.5 {
            st.hist_card_h.clear();
            st.hist_card_w = card_w;
        }
        // Uniform card height (user ruling 2026-11-27): the
        // prototype's .vh-list is align-items:stretch, but a
        // per-row max re-lays-out every card whenever the row
        // composition changes — publishing the draft removes its
        // card and the remaining cards jump to a new height. Use
        // ONE sticky max across the whole line: every card is the
        // same height, and the target never shrinks when the
        // draft card leaves (its measured height stays as the
        // floor). 0 = nothing measured yet, so cards render
        // naturally and are measured on this pass.
        let target_h = st
            .hist_card_h
            .values()
            .cloned()
            .fold(0.0f32, f32::max);
        let mut card_rects: Vec<(String, egui::Rect)> = Vec::new();
        let mut hot_rects: Vec<egui::Rect> = Vec::new();
        for row_start in (0..keys.len()).step_by(cols) {
            let row_keys = &keys[row_start..(row_start + cols).min(keys.len())];
            ui.horizontal_top(|ui| {
                ui.spacing_mut().item_spacing.x = 12.0;
                for key in row_keys {
                    let (rect, hots) = if key == "working" {
                        self.hist_working_card_ui(ui, st, &ctx, card_w, target_h, acts)
                    } else {
                        let version = versions
                            .iter()
                            .find(|v| s(v, "id") == *key)
                            .cloned()
                            .unwrap_or(Value::Null);
                        self.hist_version_card_ui(
                            ui,
                            st,
                            &ctx,
                            &version,
                            card_w,
                            target_h,
                            acts,
                        )
                    };
                    card_rects.push((key.clone(), rect));
                    hot_rects.extend(hots);
                }
            });
            ui.add_space(12.0);
        }
        self.hist_drag_resolve(ui, st, &ctx, &versions, &card_rects, &hot_rects, acts);
    }

    /// The unnumbered working-draft card. It exists only while
    /// the rendition has a real working row; publishing opens the
    /// version-type dialog instead of expanding the card.
    fn hist_working_card_ui(
        &mut self,
        ui: &mut egui::Ui,
        st: &mut UnitState,
        ctx: &HistCtx,
        card_w: f32,
        target_h: f32,
        acts: &mut Vec<DetailAct>,
    ) -> (egui::Rect, Vec<egui::Rect>) {
        let mut hots: Vec<egui::Rect> = Vec::new();
        // H2: pad to the line's uniform stretch target by the
        // amount this card's natural height falls short of it.
        let own_natural = st.hist_card_h.get("working").copied().unwrap_or(0.0);
        let pad = (target_h - own_natural).max(0.0);
        let Some(src) = ctx.row.working.clone() else {
            return (egui::Rect::NOTHING, hots);
        };
        let added = added_pages(&src, ctx.head_normal.as_ref());
        let base_semver = ctx
            .head_normal
            .as_ref()
            .map(|v| s(v, "semver"))
            .or_else(|| {
                ctx.row
                    .versions
                    .iter()
                    .find(|v| s(v, "status") != "working")
                    .map(|v| s(v, "semver"))
            })
            .unwrap_or_default();
        let selected = st.selected_working_card;
        let inner_w = (card_w - 22.0).max(160.0);
        // Card border (user ruling 2026-11-27, superseding
        // 2026-11-26): a constant 1.5px in every state — the
        // selected card changes the frame's COLOUR only, and
        // no outer ring is painted. The unnumbered working
        // card is never dashed (the prototype's .newver is
        // solid; dashes belong to numbered drafts / rejected).
        let frame_resp = egui::Frame::NONE
            .fill(palette_color(PANEL_BG))
            .stroke(egui::Stroke::new(
                1.5,
                if selected {
                    ACCENT
                } else {
                    palette_color(LINE_34)
                },
            ))
            .corner_radius(6)
            .inner_margin(egui::Margin::same(10))
            .show(ui, |ui| {
                ui.vertical(|ui| {
                    ui.set_width(inner_w);
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 6.0;
                        ui.spacing_mut().button_padding = egui::vec2(10.0, 2.0);
                        self.bigver_chip(ui, "未定号", false);
                        vh_badge(ui, "草稿", palette_color(CHIP_ORANGE_FG), CHIP_ORANGE_BG);
                        ui.with_layout(
                            egui::Layout::right_to_left(egui::Align::Center),
                            |ui| {
                                ui.spacing_mut().item_spacing.x = 6.0;
                                let pub_resp = vh_btn_primary(ui, "发布");
                                hots.push(pub_resp.rect);
                                if pub_resp.clicked() {
                                    acts.push(DetailAct::OpenDraftPublish(DraftPublishState {
                                        unit_id: st.unit_id.clone(),
                                        kind: DraftPublishKind::Working {
                                            working_id: Some(s(&src, "id")),
                                            rendition_id: ctx.row.rendition_id.clone(),
                                            head_id: ctx
                                                .row
                                                .latest
                                                .as_ref()
                                                .map(|v| s(v, "id"))
                                                .unwrap_or_default(),
                                            head_is_draft: ctx
                                                .row
                                                .latest
                                                .as_ref()
                                                .is_some_and(|v| s(v, "status") == "draft"),
                                            added,
                                        },
                                        base_semver: base_semver.clone(),
                                        bump: 1,
                                        note: String::new(),
                                        focus: true,
                                    }));
                                }
                            },
                        );
                    });
                // Thumbs: the working page set.
                self.hist_thumbs_ui(
                    ui,
                    st,
                    "working",
                    &src,
                    &s(&src, "id"),
                    inner_w,
                    true,
                    ctx.row.working.as_ref().map(|w| s(w, "id")),
                    &mut hots,
                );
                ui.add_space(8.0);
                let meta = if ctx.my_name.is_empty() {
                    "刚刚 · 我".to_string()
                } else {
                    format!("刚刚 · 我（{}）", ctx.my_name)
                };
                vh_caption_line(ui, &meta, palette_color(TEXT_FAINT));
                ui.add_space(2.0);
                vh_caption_line(ui, "未定号草稿", palette_color(TEXT_FAINT));
                // H2: absorb the stretch pad at the card's
                // foot — the box grows, the content stays
                // top-aligned (flex stretch semantics).
                if pad > 0.0 {
                    ui.add_space(pad);
                }
                });
            });
        st.hist_card_h.insert(
            "working".to_string(),
            frame_resp.response.rect.height() - pad,
        );
        (frame_resp.response.rect, hots)
    }

    /// One numbered history card (SPEC §10.2/§10.9).
    fn hist_version_card_ui(
        &mut self,
        ui: &mut egui::Ui,
        st: &mut UnitState,
        ctx: &HistCtx,
        version: &Value,
        card_w: f32,
        target_h: f32,
        acts: &mut Vec<DetailAct>,
    ) -> (egui::Rect, Vec<egui::Rect>) {
        let mut hots: Vec<egui::Rect> = Vec::new();
        let vid = s(version, "id");
        let status = s(version, "status");
        let is_draft = status == "draft";
        let is_rejected = status == "rejected";
        let is_recalled = status == "recalled";
        let is_cur = ctx.head_normal.as_ref().is_some_and(|h| s(h, "id") == vid);
        let is_using = ctx.using_id.as_deref() == Some(vid.as_str());
        let selected =
            !st.selected_working_card && st.selected_version.as_deref() == Some(vid.as_str());
        let inner_w = (card_w - 22.0).max(160.0);
        // H2: the stretch pad for this card (see the working
        // card's note).
        let own_natural = st.hist_card_h.get(&vid).copied().unwrap_or(0.0);
        let pad = (target_h - own_natural).max(0.0);
        // Card border (user ruling 2026-11-27, superseding
        // 2026-11-26): a constant 1.5px in every state; a
        // state may change the border's COLOUR only, and no
        // outer ring is painted. Numbered drafts wear the
        // prototype's amber DASHED frame (.draftv) and
        // rejected cards the rose dashed frame (.pubrej):
        // a dashed card's frame stroke stays transparent at
        // the same 1.5 width (identical geometry) and the
        // dashes are painted over the rect below; a selected
        // dashed card's dashes turn accent, like the solid
        // cards' colour-only selection.
        let dashed = is_draft || is_rejected;
        let stroke = egui::Stroke::new(
            1.5,
            if dashed {
                egui::Color32::TRANSPARENT
            } else if selected {
                ACCENT
            } else {
                palette_color(LINE_34)
            },
        );
        let frame_resp = egui::Frame::NONE
            .fill(palette_color(PANEL_BG))
            .stroke(stroke)
            .corner_radius(6)
            .inner_margin(egui::Margin::same(10))
            .show(ui, |ui| {
                ui.vertical(|ui| {
                    ui.set_width(inner_w);
                    // ---- head (one line, segments hide under
                // pressure: languages first, then badges) ----
                let editing_renum = st
                    .renum_edit
                    .as_ref()
                    .is_some_and(|(id, _)| *id == vid);
                let semver_label = format!("v{}", s(version, "semver"));
                let bigver_w = {
                    let g = ui.painter().layout_no_wrap(
                        semver_label.clone(),
                        egui::FontId::new(12.0, egui::FontFamily::Monospace),
                        palette_color(TEXT_MAIN),
                    );
                    g.size().x + 16.0
                };
                let right_w = {
                    let mut w = 0.0f32;
                    if ctx.is_super && !is_draft && !is_recalled {
                        w += 48.0;
                    }
                    if is_draft {
                        w += 118.0 + 48.0 + 6.0;
                    } else if !is_recalled && !is_rejected {
                        w += 48.0;
                    }
                    if ctx.is_super && !is_draft && !is_recalled && !is_rejected {
                        w += 6.0;
                    }
                    w
                };
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 6.0;
                    ui.spacing_mut().button_padding = egui::vec2(10.0, 2.0);
                    if editing_renum {
                        if let Some((_, text)) = &mut st.renum_edit {
                            let resp = ui.add(
                                egui::TextEdit::singleline(text).desired_width(92.0),
                            );
                            hots.push(resp.rect);
                            if st.renum_focus {
                                resp.request_focus();
                                st.renum_focus = false;
                            }
                            let esc = ui.input(|i| i.key_pressed(egui::Key::Escape));
                            if esc {
                                st.renum_edit = None;
                            } else if resp.lost_focus() {
                                if let Some((_, t)) = st.renum_edit.take() {
                                    let to = t.trim().trim_start_matches('v').to_string();
                                    let from = s(version, "semver");
                                    if !to.is_empty() && to != from {
                                        acts.push(DetailAct::RenumberVersion {
                                            version_id: vid.clone(),
                                            from,
                                            to,
                                        });
                                    }
                                }
                            }
                        }
                    } else {
                        self.bigver_chip(ui, &semver_label, is_cur);
                    }
                    // Width budget for the hide ladder using actual chip & button sizes.
                    let badges: Vec<(&str, egui::Color32, egui::Color32)> = {
                        let mut v = Vec::new();
                        if is_draft {
                            v.push(("草稿", palette_color(CHIP_ORANGE_FG), CHIP_ORANGE_BG));
                        }
                        if is_rejected {
                            v.push(("未通过", palette_color(ROSE), ROSE_SOFT_BG));
                        }
                        if is_recalled {
                            v.push(("已撤回", palette_color(ROSE), ROSE_SOFT_BG));
                        }
                        v
                    };
                    let show_langs = !is_draft && !ctx.langs.is_empty();
                    let badges_w = badges.len() as f32 * 56.0;
                    let langs_w = if show_langs {
                        ctx.langs.len() as f32 * 36.0
                    } else {
                        0.0
                    };
                    let head_left_w = if editing_renum { 96.0 } else { bigver_w };
                    let gap_pad = if right_w > 0.0 { 12.0 } else { 4.0 };
                    let hide_langs = head_left_w + badges_w + langs_w + right_w + gap_pad > inner_w;
                    let hide_badges = hide_langs && (head_left_w + badges_w + right_w + gap_pad > inner_w);
                    if !hide_badges {
                        for (text, fg, bg) in &badges {
                            vh_badge(ui, text, *fg, *bg);
                        }
                    }
                    if show_langs && !hide_langs {
                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing.x = 4.0;
                            ui.spacing_mut().button_padding = egui::vec2(6.0, 2.0);
                            for lang in &ctx.langs {
                                let state = hist_lang_state(ctx, &st.detail, version, lang);
                                let (fill, stroke, fg) = match state {
                                    "synced" => (
                                        ACCENT,
                                        egui::Stroke::new(1.0, ACCENT),
                                        egui::Color32::WHITE,
                                    ),
                                    "stale" => (
                                        egui::Color32::TRANSPARENT,
                                        egui::Stroke::NONE,
                                        palette_color(CHIP_ORANGE_FG),
                                    ),
                                    _ => (
                                        palette_color(HOVER_BG),
                                        egui::Stroke::new(1.0, palette_color(FIELD_BORDER)),
                                        palette_color(TEXT_FAINT),
                                    ),
                                };
                                let tip = match state {
                                    "synced" => format!(
                                        "{} · 已挂译文到该版 · 点击解除",
                                        lang_display_name(lang)
                                    ),
                                    "stale" => format!(
                                        "{} · 译文落后于该版 · 点击更新到该版",
                                        lang_display_name(lang)
                                    ),
                                    _ => format!(
                                        "{} · 该版未挂译文 · 点击挂上（基于该版）",
                                        lang_display_name(lang)
                                    ),
                                };
                                let btn = egui::Button::new(
                                    egui::RichText::new(lang.to_uppercase())
                                        .size(10.0)
                                        .strong()
                                        .color(fg),
                                )
                                .fill(fill)
                                .stroke(stroke)
                                .corner_radius(6)
                                .min_size(egui::vec2(32.0, 24.0));
                                let resp = ui.add(btn).on_hover_text(tip);
                                if state == "stale" {
                                    dashed_card_border(
                                        ui.painter(),
                                        resp.rect,
                                        6.0,
                                        palette_color(CHIP_ORANGE_FG),
                                        1.0,
                                    );
                                }
                                hots.push(resp.rect);
                                if resp.clicked() {
                                    let (next, toast) = match state {
                                        "missing" => (
                                            "synced",
                                            format!(
                                                "{}译文已挂到 {}。",
                                                lang_display_name(lang),
                                                s(version, "semver")
                                            ),
                                        ),
                                        "stale" => (
                                            "synced",
                                            format!(
                                                "{}译文已更新到 {}。",
                                                lang_display_name(lang),
                                                s(version, "semver")
                                            ),
                                        ),
                                        _ => (
                                            "missing",
                                            format!(
                                                "已解除 {} 的{}绑定。",
                                                s(version, "semver"),
                                                lang_display_name(lang)
                                            ),
                                        ),
                                    };
                                    acts.push(DetailAct::LangMount {
                                        version_id: vid.clone(),
                                        lang: lang.clone(),
                                        state: next,
                                        toast,
                                    });
                                }
                            }
                        });
                    }
                    // Right group: [primary/default actions]
                    // then 改号 at the right edge (added first
                    // in right-to-left order).
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.spacing_mut().item_spacing.x = 6.0;
                        if ctx.is_super && !is_draft && !is_recalled {
                            let resp = vh_btn(ui, "改号");
                            hots.push(resp.rect);
                            if resp.clicked() {
                                st.renum_edit =
                                    Some((vid.clone(), s(version, "semver")));
                                st.renum_focus = true;
                            }
                        }
                        if is_draft {
                            let drop_resp = vh_btn(ui, "放弃");
                            hots.push(drop_resp.rect);
                            if drop_resp.clicked() {
                                acts.push(DetailAct::DropVersion(vid.clone()));
                            }
                            let pub_resp = vh_btn_primary(ui, "发布为最新版本");
                            hots.push(pub_resp.rect);
                            if pub_resp.clicked() {
                                let added =
                                    added_pages(version, ctx.head_normal.as_ref());
                                let base_semver = ctx
                                    .head_normal
                                    .as_ref()
                                    .map(|v| s(v, "semver"))
                                    .filter(|v| !v.is_empty())
                                    .unwrap_or_else(|| s(version, "semver"));
                                acts.push(DetailAct::OpenDraftPublish(DraftPublishState {
                                    unit_id: st.unit_id.clone(),
                                    kind: DraftPublishKind::Numbered {
                                        version_id: vid.clone(),
                                        added,
                                    },
                                    base_semver,
                                    bump: 1,
                                    note: String::new(),
                                    focus: true,
                                }));
                            }
                        } else if !is_recalled && !is_rejected {
                            if is_using {
                                let resp = vh_btn_primary(ui, "使用");
                                hots.push(resp.rect);
                                if resp.clicked() {
                                    acts.push(DetailAct::UseSel {
                                        rendition_id: ctx.row.rendition_id.clone(),
                                        version_id: None,
                                        semver: String::new(),
                                    });
                                }
                            } else {
                                let resp = vh_btn(ui, "选用");
                                hots.push(resp.rect);
                                if resp.clicked() {
                                    acts.push(DetailAct::UseSel {
                                        rendition_id: ctx.row.rendition_id.clone(),
                                        version_id: Some(vid.clone()),
                                        semver: s(version, "semver"),
                                    });
                                }
                            }
                        }
                    });
                });
                // ---- thumbs ----
                self.hist_thumbs_ui(
                    ui,
                    st,
                    &vid,
                    version,
                    &vid,
                    inner_w,
                    false,
                    Some(vid.clone()),
                    &mut hots,
                );
                // ---- caption ----
                ui.add_space(8.0);
                vh_caption_line(
                    ui,
                    &format!(
                        "{} · {}",
                        fmt_time(i(version, "created_ms")),
                        s(version, "author")
                    ),
                    palette_color(TEXT_FAINT),
                );
                // Always reserve the second caption line (the draft
                // card's reference height), even when the version
                // carries no revision note.
                ui.add_space(2.0);
                vh_caption_line(ui, &s(version, "note"), palette_color(TEXT_FAINT));
                if is_rejected && !s(version, "pub_reason").is_empty() {
                    ui.add_space(2.0);
                    vh_caption_line(
                        ui,
                        &format!(
                            "未通过理由：{}（{} · {}）",
                            s(version, "pub_reason"),
                            s(version, "pub_by"),
                            fmt_time(i(version, "pub_at"))
                        ),
                        palette_color(ROSE),
                    );
                }
                // H2: absorb the stretch pad at the foot.
                if pad > 0.0 {
                    ui.add_space(pad);
                }
                });
            });
        if dashed {
            let dash_color = if selected {
                ACCENT
            } else if is_draft {
                palette_color(CHIP_ORANGE_FG)
            } else {
                palette_color(ROSE)
            };
            dashed_card_border(
                ui.painter(),
                frame_resp.response.rect,
                6.0,
                dash_color,
                1.5,
            );
        }
        st.hist_card_h
            .insert(vid, frame_resp.response.rect.height() - pad);
        (frame_resp.response.rect, hots)
    }

    /// The .vh-bigver chip: --panel3 fill, radius 6, 12px
    /// medium; the in-use version's chip is accent-filled.
    fn bigver_chip(&self, ui: &mut egui::Ui, text: &str, accent_fill: bool) {
        // The prototype's bigver uses a monospace numeric stack
        // with tabular figures （差 35): the client's closest
        // face is egui's Monospace family.
        let font = egui::FontId::new(12.0, egui::FontFamily::Monospace);
        let fg = if accent_fill {
            egui::Color32::WHITE
        } else {
            palette_color(TEXT_MAIN)
        };
        let galley = ui.painter().layout_no_wrap(text.to_string(), font, fg);
        let (rect, _) = ui.allocate_exact_size(
            egui::vec2(galley.size().x + 16.0, 24.0),
            egui::Sense::hover(),
        );
        ui.painter().rect_filled(
            rect,
            6.0,
            if accent_fill {
                ACCENT
            } else {
                palette_color(PRESSED_BG)
            },
        );
        ui.painter().galley(
            egui::pos2(rect.min.x + 8.0, rect.center().y - galley.size().y / 2.0),
            galley,
            fg,
        );
    }

    /// A history card's thumbnail strip (.vh-thumbs): the
    /// prototype's flex-wrap row — thumbs flex at a 120px
    /// floor and wrap (H1, visual pass 2026-11-26) — and a
    /// foot line under each (desc left, version right; the
    /// working card's foot is 草稿 alone). There is NO page
    /// pill over the thumb (H3): the prototype's {i} / {n}
    /// is slide-art text, and over a real rendered thumbnail
    /// the prototype form carries nothing — the black chip
    /// that stood here is removed; the fraction survives in
    /// the no-thumbnail art placeholder's eyebrow (T2). A
    /// single click selects the thumbnail and its card; a
    /// double-click opens #406 at that page.
    #[allow(clippy::too_many_arguments)]
    fn hist_thumbs_ui(
        &mut self,
        ui: &mut egui::Ui,
        st: &mut UnitState,
        card_key: &str,
        version: &Value,
        thumb_version_id: &str,
        inner_w: f32,
        working_card: bool,
        select_version_id: Option<String>,
        hots: &mut Vec<egui::Rect>,
    ) {
        let page_count = i(version, "page_count").max(0) as usize;
        if page_count == 0 {
            return;
        }
        ui.add_space(9.0);
        // H1: as many per row as fit at >=120px (+6 gap);
        // each row's thumbs then share the row width equally
        // (flex-grow), so a wrapped lone thumb spans the row.
        let per_row = (((inner_w + 6.0) / 126.0).floor() as usize)
            .max(1)
            .min(page_count);
        for row_start in (0..page_count).step_by(per_row) {
            if row_start > 0 {
                ui.add_space(6.0);
            }
            let row_end = (row_start + per_row).min(page_count);
            let row_len = (row_end - row_start) as f32;
            let thumb_w = (inner_w - 6.0 * (row_len - 1.0)) / row_len;
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 6.0;
                for p in row_start..row_end {
                    let thumb_sel = st
                        .hist_thumb_sel
                        .as_ref()
                        .is_some_and(|(k, idx)| k == card_key && *idx == p);
                    let thumb_inner_w = (thumb_w - 3.0).max(60.0);
                    let frame_resp = egui::Frame::NONE
                        .fill(palette_color(PANEL_BG))
                        .stroke(egui::Stroke::new(
                            1.5,
                            if thumb_sel {
                                ACCENT
                            } else {
                                palette_color(LINE_34)
                            },
                        ))
                        .corner_radius(6)
                        .inner_margin(egui::Margin::same(0))
                        .show(ui, |ui| {
                            let resp = self.thumb_response_art(
                                ui,
                                thumb_version_id,
                                p,
                                thumb_inner_w,
                                false,
                                &format!("{} / {}", p + 1, page_count),
                                &page_title(version, p),
                            );
                            if resp.clicked() {
                                st.hist_thumb_sel = Some((card_key.to_string(), p));
                                st.selected_working_card = working_card;
                                st.selected_version = select_version_id.clone();
                                st.selected_page = None;
                                st.bind_pick = None;
                            }
                            if resp.double_clicked() {
                                let v = version.clone();
                                self.open_light(st, &v, p);
                            }
                        });
                    hots.push(frame_resp.response.rect);
                }
            });
        }
    }

    /// History card pointer resolution: background clicks
    /// select, background double-clicks open #406 at page 0,
    /// and drags reorder the line (numbered cards) or number
    /// the working draft in (working card dropped on a version
    /// card). Presses that start on a card's interactive
    /// children (its hot rects) never arm anything here.
    #[allow(clippy::too_many_arguments)]
    fn hist_drag_resolve(
        &mut self,
        ui: &mut egui::Ui,
        st: &mut UnitState,
        ctx: &HistCtx,
        versions: &[Value],
        card_rects: &[(String, egui::Rect)],
        hot_rects: &[egui::Rect],
        acts: &mut Vec<DetailAct>,
    ) {
        let pointer = ui.input(|i| i.pointer.interact_pos());
        let primary_down = ui.input(|i| i.pointer.primary_down());
        let primary_pressed = ui.input(|i| i.pointer.primary_pressed());
        let primary_released = ui.input(|i| i.pointer.primary_released());
        if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
            st.vh_press = None;
            st.vh_drag = None;
        }
        let Some(pos) = pointer else {
            if !primary_down {
                st.vh_press = None;
                st.vh_drag = None;
            }
            return;
        };
        let in_hot = |p: egui::Pos2| hot_rects.iter().any(|r| r.contains(p));
        if primary_pressed && st.vh_drag.is_none() && !in_hot(pos) {
            if let Some((key, _)) = card_rects.iter().find(|(_, r)| r.contains(pos)) {
                st.vh_press = Some((key.clone(), pos));
            }
        }
        if primary_down {
            if let Some((key, start)) = st.vh_press.clone() {
                if start.distance(pos) > 8.0 {
                    st.vh_drag = Some(key);
                }
            }
        }
        // Drag visuals: a seam at the target card's edge for a
        // line reorder; a highlight over the target card for a
        // 定号插入 (the prototype's .dragover form).
        if let Some(drag_key) = st.vh_drag.clone() {
            let target = card_rects
                .iter()
                .find(|(k, r)| *k != drag_key && r.contains(pos));
            if let Some((tkey, trect)) = target {
                if drag_key == "working" {
                    ui.painter()
                        .rect_filled(*trect, 6.0, palette_color(CHIP_ORANGE_BG));
                    dashed_card_border(
                        ui.painter(),
                        *trect,
                        6.0,
                        palette_color(CHIP_ORANGE_FG),
                        1.0,
                    );
                    let _ = tkey;
                } else {
                    let before = pos.x < trect.center().x;
                    let x = if before { trect.left() - 6.0 } else { trect.right() + 6.0 };
                    ui.painter().line_segment(
                        [egui::pos2(x, trect.top()), egui::pos2(x, trect.bottom())],
                        egui::Stroke::new(2.5, ACCENT),
                    );
                }
            }
            // The dragged card's ghost follows the pointer.
            if let Some((_, drect)) = card_rects.iter().find(|(k, _)| *k == drag_key) {
                let ghost = egui::Rect::from_min_size(
                    pos - egui::vec2(30.0, 14.0),
                    egui::vec2(drect.width().min(220.0), 28.0),
                );
                ui.painter()
                    .rect_filled(ghost, 6.0, palette_color(PRESSED_BG));
                ui.painter().rect_stroke(
                    ghost,
                    6.0,
                    egui::Stroke::new(1.0, ACCENT),
                    egui::StrokeKind::Middle,
                );
                let label = if drag_key == "working" {
                    "未定号草稿".to_string()
                } else {
                    versions
                        .iter()
                        .find(|v| s(v, "id") == drag_key)
                        .map(|v| format!("v{}", s(v, "semver")))
                        .unwrap_or_default()
                };
                let galley = ui.painter().layout_no_wrap(
                    label,
                    egui::FontId::new(12.0, egui::FontFamily::Proportional),
                    palette_color(TEXT_MAIN),
                );
                ui.painter().galley(
                    ghost.min + egui::vec2(10.0, 7.0),
                    galley,
                    palette_color(TEXT_MAIN),
                );
            }
        }
        if primary_released {
            if let Some(drag_key) = st.vh_drag.take() {
                let target = card_rects
                    .iter()
                    .find(|(k, r)| *k != drag_key && r.contains(pos))
                    .cloned();
                if let Some((tkey, trect)) = target {
                    if drag_key == "working" {
                        if tkey != "working" {
                            let base = versions
                                .iter()
                                .find(|v| s(v, "id") == tkey);
                            if let Some(base) = base {
                                acts.push(DetailAct::InsertWorkingAt {
                                    working_id: ctx
                                        .row
                                        .working
                                        .as_ref()
                                        .map(|w| s(w, "id")),
                                    rendition_id: ctx.row.rendition_id.clone(),
                                    base_version_id: tkey.clone(),
                                    base_semver: s(base, "semver"),
                                });
                            }
                        }
                    } else {
                        // Line reorder over the display order
                        // (numbered cards only).
                        let mut order: Vec<String> =
                            versions.iter().map(|v| s(v, "id")).collect();
                        if let (Some(from), Some(tpos)) = (
                            order.iter().position(|id| *id == drag_key),
                            order.iter().position(|id| *id == tkey),
                        ) {
                            let moved = order.remove(from);
                            let after = pos.x >= trect.center().x;
                            let mut at = tpos;
                            if from < tpos {
                                at = tpos - 1 + if after { 1 } else { 0 };
                            } else if after {
                                at = tpos + 1;
                            }
                            at = at.min(order.len());
                            order.insert(at, moved);
                            let changed = order
                                .iter()
                                .enumerate()
                                .any(|(n, id)| versions.get(n).is_none_or(|v| s(v, "id") != *id));
                            if changed {
                                acts.push(DetailAct::ReorderLine {
                                    rendition_id: ctx.row.rendition_id.clone(),
                                    version_ids: order,
                                });
                            }
                        }
                    }
                }
                st.vh_press = None;
                return;
            }
            if let Some((key, start)) = st.vh_press.take() {
                if start.distance(pos) <= 8.0 {
                    if let Some((_, rect)) =
                        card_rects.iter().find(|(k, _)| *k == key)
                    {
                        if rect.contains(pos) && !in_hot(pos) {
                            // Select: the version this card stands for
                            // (working card keeps its own selection flag
                            // so mirror-state never double-highlights latest).
                            if key == "working" {
                                st.selected_working_card = true;
                                st.selected_version =
                                    ctx.row.working.as_ref().map(|w| s(w, "id"));
                            } else {
                                st.selected_working_card = false;
                                st.selected_version = Some(key.clone());
                            }
                            st.selected_page = None;
                            st.bind_pick = None;
                            // Double-click opens #406 at page 0
                            // with the card's own version.
                            let now = ui.input(|i| i.time);
                            let dbl = st
                                .vh_last_click
                                .as_ref()
                                .is_some_and(|(k, t)| *k == key && now - *t < 0.4);
                            if dbl {
                                let version = if key == "working" {
                                    ctx.row
                                        .working
                                        .clone()
                                        .or_else(|| ctx.row.latest.clone())
                                } else {
                                    versions
                                        .iter()
                                        .find(|v| s(v, "id") == key)
                                        .cloned()
                                };
                                if let Some(v) = version {
                                    self.open_light(st, &v, 0);
                                }
                                st.vh_last_click = None;
                            } else {
                                st.vh_last_click = Some((key.clone(), now));
                            }
                        }
                    }
                }
            }
        }
        if !primary_down && st.vh_drag.is_none() {
            // A press that never armed and never released inside
            // (scroll, window loss) dies quietly.
            if pointer.is_none() {
                st.vh_press = None;
            }
        }
    }
}

/// The history card head's badge form (H4, visual pass
/// 2026-11-26): the prototype's `.vh-card-head .badge` over
/// the final `.badge` cascade — 24px tall, 0/10 padding,
/// radius 6, 12px text at the badge weight 700. The generic
/// `chip` is the plain `.badge` form (20px, 11px) and stays
/// untouched for its other surfaces; this head form is
/// local to #101. The weight rides RichText::strong, the
/// client's standing approximation for the >=600 tier (G1).
fn vh_badge(ui: &mut egui::Ui, text: &str, fg: egui::Color32, bg: egui::Color32) {
    let font = egui::FontId::new(12.0, egui::FontFamily::Proportional);
    let galley = ui.painter().layout_no_wrap(text.to_string(), font, fg);
    let (rect, _) = ui.allocate_exact_size(
        egui::vec2(galley.size().x + 20.0, 24.0),
        egui::Sense::hover(),
    );
    ui.painter().rect_filled(rect, 6.0, palette_color(bg));
    ui.painter().galley(
        egui::pos2(rect.min.x + 10.0, rect.center().y - galley.size().y / 2.0),
        galley,
        fg,
    );
}

/// One history-card caption line at the card's 11.5px faint
/// weight. The line is allocated at a FIXED line height, so the
/// caption block has the same height on every card — the draft
/// card's two-line caption (time · author, then 未定号草稿) is
/// the reference (user ruling 2026-11-27: the card labels'
/// heights were inconsistent). The number of wrapped lines is
/// taken from the laid-out galley so long revision notes still
/// reserve their own lines.
fn vh_caption_line(ui: &mut egui::Ui, text: &str, color: egui::Color32) {
    let font = egui::FontId::new(11.5, egui::FontFamily::Proportional);
    let w = ui.available_width();
    let galley = ui.painter().layout(text.to_string(), font, color, w);
    let lines = galley.lines().count().max(1);
    let line_h = 15.5;
    let (rect, _) = ui.allocate_exact_size(
        egui::vec2(w, line_h * lines as f32),
        egui::Sense::hover(),
    );
    ui.painter().galley(rect.min, galley, color);
}

/// Hand-drawn dashed rounded-rect border (H5, visual pass
/// 2026-11-26): the prototype's draftv / pubrej history
/// cards carry a DASHED border (amber / rose). egui frame
/// strokes cannot dash, so a dashed card's frame keeps a
/// transparent stroke of the SAME width (identical
/// geometry) and the dashes are painted over its rect
/// afterwards — dashed_rule's segment technique carried
/// around a rounded path (5px on, 3.5px off). `width` is
/// the border width: history cards pass 1.5 (user ruling
/// 2026-11-27: card borders are a constant 1.5px in every
/// state); the chip / picking / drag-over outlines that
/// also use this helper keep their 1.0.
fn dashed_card_border(
    painter: &egui::Painter,
    rect: egui::Rect,
    radius: f32,
    color: egui::Color32,
    width: f32,
) {
    let rect = rect.shrink(width * 0.5);
    let rad = radius.min(rect.width() / 2.0).min(rect.height() / 2.0);
    let (min, max) = (rect.min, rect.max);
    let mut path: Vec<egui::Pos2> = Vec::new();
    // Clockwise from the top edge's left end: straight,
    // corner arc, straight, … closing back at the start.
    path.push(egui::pos2(min.x + rad, min.y));
    path.push(egui::pos2(max.x - rad, min.y));
    let corners = [
        (
            egui::pos2(max.x - rad, min.y + rad),
            -std::f32::consts::FRAC_PI_2,
        ),
        (egui::pos2(max.x - rad, max.y - rad), 0.0),
        (
            egui::pos2(min.x + rad, max.y - rad),
            std::f32::consts::FRAC_PI_2,
        ),
        (egui::pos2(min.x + rad, min.y + rad), std::f32::consts::PI),
    ];
    let straights = [
        egui::pos2(max.x, max.y - rad),
        egui::pos2(min.x + rad, max.y),
        egui::pos2(min.x, min.y + rad),
        egui::pos2(min.x + rad, min.y),
    ];
    for (idx, (c, a0)) in corners.iter().enumerate() {
        for k in 1..=6 {
            let a = a0 + (k as f32) * (std::f32::consts::FRAC_PI_2 / 6.0);
            path.push(*c + egui::vec2(rad * a.cos(), rad * a.sin()));
        }
        path.push(straights[idx]);
    }
    let stroke = egui::Stroke::new(width, color);
    let period = 8.5f32;
    let mut phase = 0.0f32;
    for w in 0..path.len() {
        let a = path[w];
        let b = path[(w + 1) % path.len()];
        let seg = b - a;
        let len = seg.length();
        if len <= 0.0 {
            continue;
        }
        let dir = seg / len;
        let mut t = 0.0f32;
        while t < len {
            let cyc = (phase + t) % period;
            let step = if cyc < 5.0 {
                (5.0 - cyc).min(len - t)
            } else {
                (period - cyc).min(len - t)
            };
            if cyc < 5.0 && step > 0.0 {
                painter.line_segment([a + dir * t, a + dir * (t + step)], stroke);
            }
            t += step.max(0.01);
        }
        phase = (phase + len) % period;
    }
}

/// The history card head's button form (prototype: .btn at the
/// card-head size — min-height 24, 12px text, 0/10 padding).
/// The label carries .strong: the prototype's .btn weight is
/// 600 (final cascade) and strong is the client's standing
/// approximation for that tier (G1, visual pass 2026-11-26).
fn vh_btn(ui: &mut egui::Ui, label: &str) -> egui::Response {
    ui.add(
        egui::Button::new(
            egui::RichText::new(label)
                .size(12.0)
                .strong()
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
                .strong()
                .color(egui::Color32::WHITE),
        )
        .fill(ACCENT)
        .corner_radius(6)
        .min_size(egui::vec2(0.0, 24.0)),
    )
}

impl App {
    /// Prototype confirm copy (SPEC §10.5): moving a binding
    /// asks first, naming the language, the target page, the
    /// source page it is bound from, and the destination.
    fn request_page_binding(&mut self, request: PageBindingRequest, bindings: &[Value]) {
        if exact_binding(bindings, &request).is_some() {
            self.msg = "页面已经绑定".into();
            return;
        }
        if let Some(owner) = occupied_binding(bindings, &request) {
            let lang = self.target_lang_name(
                &request.unit_id,
                &request.from_version_id,
                &request.to_version_id,
            );
            // The prototype's full confirm copy (DIFFERENCES
            // 差 41, verified against the HTML): it names the
            // language, the candidate page and its title, the
            // source page it is bound from, and the destination,
            // and says plainly this is not a silent steal. The
            // source language is the shown rendition's own (the
            // prototype hardcodes 中文 as its source).
            let (cand_title, src_lang) =
                if let ContentFrame::Unit(st) = self.content.stack.top() {
                    let cand = binding_targets(&st.detail, &request.from_version_id)
                        .iter()
                        .find(|t| s(&t.version, "id") == request.to_version_id)
                        .map(|t| page_title(&t.version, request.to_slot.max(0) as usize))
                        .unwrap_or_default();
                    let src = rendition_rows(&st.detail)
                        .iter()
                        .find(|r| {
                            r.versions.iter().any(|v| s(v, "id") == request.from_version_id)
                                || r.working
                                    .as_ref()
                                    .is_some_and(|w| s(w, "id") == request.from_version_id)
                        })
                        .map(|r| lang_display_name(&r.lang))
                        .unwrap_or_else(|| "中文".to_string());
                    (cand, src)
                } else {
                    (String::new(), "中文".to_string())
                };
            self.overlays.push(Overlay::Confirm(ConfirmState {
                title: "移动绑定？".into(),
                body: format!(
                    "{}第 {} 页「{}」已经绑在{}第 {} 页上。继续会把它从{}第 {} 页解开、改绑到{}第 {} 页——不是静默抢绑，请你确认。",
                    lang,
                    request.to_slot + 1,
                    cand_title,
                    src_lang,
                    i(&owner, "from_slot") + 1,
                    src_lang,
                    i(&owner, "from_slot") + 1,
                    src_lang,
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
        // Prototype success toast + the panel closes (the
        // pick-mode flow ends here; the bind bar returns).
        let lang = self.target_lang_name(
            &request.unit_id,
            &request.from_version_id,
            &request.to_version_id,
        );
        self.msg = format!(
            "已绑定：第 {} 页 · {} ↔ 第 {} 页。",
            request.from_slot + 1,
            lang,
            request.to_slot + 1
        );
        if let ContentFrame::Unit(st) = self.content.stack.top_mut() {
            if st.unit_id == request.unit_id {
                st.bind_panel = None;
                st.bind_pick = None;
            }
        }
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
                let lang =
                    self.target_lang_name(unit_id, from_version_id, to_version_id);
                self.msg = format!("已标注缺译：第 {} 页 · {}", from_slot + 1, lang);
                self.reload_unit_detail(unit_id);
                self.changed(Change::Units);
            }
            Err(e) => self.msg = format!("标缺译失败：{e}"),
        }
    }

    /// Render #406 stage slide (.big-slide): full-resolution real page
    /// rendered directly from the PPTX package (never reuses thumbnails).
    /// Returns true when the stage was double-clicked to launch live show.
    fn light_stage_ui(
        &mut self,
        ui: &mut egui::Ui,
        version: &Value,
        slot: usize,
        width: f32,
        eyebrow: &str,
        version_tag: &str,
    ) -> bool {
        let h = width * 9.0 / 16.0;
        let version_id = s(version, "id");
        let target_px = (width * ui.ctx().pixels_per_point()).round().max(1280.0) as u32;
        let (rect, resp) = ui.allocate_exact_size(egui::vec2(width, h), egui::Sense::click());
        let state = self.stage_slide(&version_id, slot, target_px);
        match state {
            ThumbState::Ready(tex) => {
                ui.painter().rect_filled(rect, 6.0, palette_color(PANEL_BG));
                let inner = rect.shrink(1.0);
                let ts = tex.size_vec2();
                let draw_rect = if ts.x > 0.0 && ts.y > 0.0 {
                    let scale = (inner.width() / ts.x).min(inner.height() / ts.y);
                    egui::Rect::from_center_size(inner.center(), ts * scale)
                } else {
                    inner
                };
                ui.painter().image(
                    tex.id(),
                    draw_rect,
                    egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                    egui::Color32::WHITE,
                );
                ui.painter().rect_stroke(
                    rect,
                    6.0,
                    egui::Stroke::new(1.0, palette_color(LINE_34)),
                    egui::StrokeKind::Inside,
                );
            }
            ThumbState::Loading => {
                let painter = ui.painter();
                painter.rect_filled(rect, 6.0, palette_color(HOVER_BG));
                painter.rect_stroke(
                    rect,
                    6.0,
                    egui::Stroke::new(1.0, palette_color(LINE_34)),
                    egui::StrokeKind::Inside,
                );
                painter.text(
                    rect.center(),
                    egui::Align2::CENTER_CENTER,
                    "正在渲染真实页面…",
                    egui::FontId::new(13.5, egui::FontFamily::Proportional),
                    palette_color(TEXT_WEAK),
                );
            }
            ThumbState::Failed(_) => {
                let painter = ui.painter();
                painter.rect_filled(rect, 6.0, palette_color(HOVER_BG));
                painter.rect_stroke(
                    rect,
                    6.0,
                    egui::Stroke::new(1.0, palette_color(LINE_34)),
                    egui::StrokeKind::Inside,
                );
                let inner = rect.shrink(24.0);
                let clip = painter.with_clip_rect(inner);
                let mut y = inner.min.y;
                if !eyebrow.is_empty() {
                    let g = clip.layout_no_wrap(
                        eyebrow.to_string(),
                        egui::FontId::new(12.0, egui::FontFamily::Proportional),
                        palette_color(TEXT_FAINT),
                    );
                    y += g.size().y + 8.0;
                    clip.galley(inner.min, g, palette_color(TEXT_FAINT));
                }
                let title = page_title(version, slot);
                if !title.is_empty() {
                    let title_size = (width * 0.032).clamp(20.0, 32.0);
                    let g = clip.layout(
                        title,
                        egui::FontId::new(title_size, egui::FontFamily::Proportional),
                        palette_color(TEXT_MAIN),
                        inner.width(),
                    );
                    let g_h = g.size().y;
                    clip.galley(egui::pos2(inner.min.x, y), g, palette_color(TEXT_MAIN));
                    y += g_h + 10.0;
                }
                let desc = page_desc(version, slot);
                if !desc.is_empty() && y < inner.max.y - 36.0 {
                    let g = clip.layout(
                        desc.clone(),
                        egui::FontId::new(14.5, egui::FontFamily::Proportional),
                        palette_color(TEXT_WEAK),
                        inner.width(),
                    );
                    clip.galley(egui::pos2(inner.min.x, y), g, palette_color(TEXT_WEAK));
                }
                let foot_h = 24.0;
                let rule_y = inner.max.y - foot_h - 8.0;
                if rule_y > inner.min.y + 40.0 {
                    painter.line_segment(
                        [
                            egui::pos2(inner.min.x, rule_y),
                            egui::pos2(inner.max.x, rule_y),
                        ],
                        egui::Stroke::new(1.0, palette_color(LINE_34)),
                    );
                }
                let foot_left = if desc.is_empty() {
                    format!("第 {} 页", slot + 1)
                } else {
                    desc
                };
                let fl = clip.layout_no_wrap(
                    foot_left,
                    egui::FontId::new(12.0, egui::FontFamily::Proportional),
                    palette_color(TEXT_FAINT),
                );
                clip.galley(
                    egui::pos2(inner.min.x, inner.max.y - fl.size().y),
                    fl,
                    palette_color(TEXT_FAINT),
                );
                if !version_tag.is_empty() {
                    let fr = clip.layout_no_wrap(
                        version_tag.to_string(),
                        egui::FontId::new(12.0, egui::FontFamily::Proportional),
                        palette_color(TEXT_FAINT),
                    );
                    clip.galley(
                        egui::pos2(inner.max.x - fr.size().x, inner.max.y - fr.size().y),
                        fr,
                        palette_color(TEXT_FAINT),
                    );
                }
            }
        }
        resp.on_hover_text("真实页面预览 · ←/→ 翻页 · 双击进入放映")
            .double_clicked()
    }

    pub(super) fn light_ui(&mut self, ui: &mut egui::Ui, mut light: LightState) -> Option<Overlay> {
        let mut request = None;
        let mut unbind = None;
        let mut start_pick = false;
        if light.targets.is_empty() {
            light.target_index = 0;
        } else {
            light.target_index = light.target_index.min(light.targets.len() - 1);
        }

        // Keyboard page navigation (←/→, PageUp/PageDown) matching #406 slide flips.
        if !ui.ctx().egui_wants_keyboard_input() {
            let page_count = i(&light.version, "page_count").max(0) as usize;
            if page_count > 1 {
                let prev = ui.input(|inp| {
                    inp.key_pressed(egui::Key::ArrowLeft) || inp.key_pressed(egui::Key::PageUp)
                });
                let next = ui.input(|inp| {
                    inp.key_pressed(egui::Key::ArrowRight) || inp.key_pressed(egui::Key::PageDown)
                });
                if prev {
                    light.slot = if light.slot == 0 {
                        page_count - 1
                    } else {
                        light.slot - 1
                    };
                    if let ContentFrame::Unit(st) = self.content.stack.top_mut() {
                        st.selected_page = Some(light.slot);
                    }
                } else if next {
                    light.slot = (light.slot + 1) % page_count;
                    if let ContentFrame::Unit(st) = self.content.stack.top_mut() {
                        st.selected_page = Some(light.slot);
                    }
                }
            }
        }

        // Resolve version eyebrow, version-policy footer, and optional Chinese original (#lbOrig).
        let ver_id = s(&light.version, "id");
        let page_count = i(&light.version, "page_count").max(1) as usize;
        let (unit_name, cur_lang, version_tag, zh_orig) =
            if let ContentFrame::Unit(st) = self.content.stack.top() {
                let uname = st
                    .detail
                    .get("unit")
                    .map(|u| s(u, "name"))
                    .unwrap_or_default();
                let rows = rendition_rows(&st.detail);
                let cur_row = rows.iter().find(|r| {
                    r.working.as_ref().is_some_and(|w| s(w, "id") == ver_id)
                        || r.versions.iter().any(|v| s(v, "id") == ver_id)
                });
                let lang = cur_row
                    .map(|r| r.lang.clone())
                    .unwrap_or_else(|| "zh".to_string());
                let semver = s(&light.version, "semver");
                let ver_txt = if semver.is_empty() {
                    "草稿".to_string()
                } else {
                    format!("v{semver}")
                };
                let policy_txt = match &st.entry_policy {
                    Some(EntryPolicyView::Locked { .. }) => "锁定版本",
                    _ => "跟随最新",
                };
                let tag = format!("{ver_txt} · {policy_txt}");
                let orig = if lang != "zh" {
                    rows.iter()
                        .find(|r| r.lang == "zh")
                        .and_then(|zr| zr.working.as_ref().or(zr.latest.as_ref()))
                        .and_then(|zv| {
                            let zvid = s(zv, "id");
                            let zpages = i(zv, "page_count").max(0) as usize;
                            if zpages == 0 {
                                return None;
                            }
                            let mapped_slot = light
                                .bindings
                                .iter()
                                .find(|b| {
                                    s(b, "from_version_id") == zvid
                                        && s(b, "to_version_id") == ver_id
                                        && i(b, "to_slot") == light.slot as i64
                                        && s(b, "kind") != "missing"
                                })
                                .map(|b| i(b, "from_slot").max(0) as usize)
                                .unwrap_or(light.slot)
                                .min(zpages - 1);
                            let zsem = s(zv, "semver");
                            let zlabel = if zsem.is_empty() {
                                "草稿".to_string()
                            } else {
                                format!("v{zsem}")
                            };
                            Some((
                                page_title(zv, mapped_slot),
                                page_desc(zv, mapped_slot),
                                page_note(zv, mapped_slot),
                                zlabel,
                            ))
                        })
                } else {
                    None
                };
                (uname, lang, tag, orig)
            } else {
                (String::new(), "zh".to_string(), String::new(), None)
            };

        let eyebrow = if cur_lang != "zh" {
            format!(
                "{} · {} / {} · {}",
                unit_name,
                light.slot + 1,
                page_count,
                cur_lang.to_uppercase()
            )
        } else if unit_name.is_empty() {
            format!("{} / {}", light.slot + 1, page_count)
        } else {
            format!("{} · {} / {}", unit_name, light.slot + 1, page_count)
        };

        let mut launch_present = false;

        // Render inside CentralPanel (.sheetpage #lightDlg inside #pageHost):
        // top bar, 268px left sidebar, and 46px bottom bar remain visible.
        egui::Frame::NONE
            .fill(palette_color(APP_BG))
            .inner_margin(egui::Margin {
                left: 22,
                right: 22,
                top: 16,
                bottom: 16,
            })
            .show(ui, |ui| {
                let avail_w = ui.available_width();
                let avail_h = ui.available_height();
                let side_width = 300.0;
                let left_width = (avail_w - 14.0 - side_width).max(260.0);
                let stage_width = left_width
                    .min(((avail_h - 140.0).max(220.0)) * 16.0 / 9.0)
                    .max(260.0);
                let stage_px = (stage_width * ui.ctx().pixels_per_point())
                    .round()
                    .max(1280.0) as u32;
                if light.slot + 1 < page_count {
                    let _ = self.stage_slide(&ver_id, light.slot + 1, stage_px);
                }
                if light.slot > 0 {
                    let _ = self.stage_slide(&ver_id, light.slot - 1, stage_px);
                }

                egui::ScrollArea::vertical()
                    .id_salt("light_406_body")
                    .show(ui, |ui| {
                        ui.horizontal_top(|ui| {
                            ui.spacing_mut().item_spacing.x = 14.0;
                            // ---- Left: .lb-main (stage + .note-box + optional .orig-box) ----
                            ui.vertical(|ui| {
                                ui.set_width(left_width);
                                if stage_width < left_width {
                                    ui.horizontal(|ui| {
                                        ui.add_space(((left_width - stage_width) / 2.0).max(0.0));
                                        if self.light_stage_ui(
                                            ui,
                                            &light.version,
                                            light.slot,
                                            stage_width,
                                            &eyebrow,
                                            &version_tag,
                                        ) {
                                            launch_present = true;
                                        }
                                    });
                                } else if self.light_stage_ui(
                                    ui,
                                    &light.version,
                                    light.slot,
                                    stage_width,
                                    &eyebrow,
                                    &version_tag,
                                ) {
                                    launch_present = true;
                                }
                                ui.add_space(10.0);
                                // Prototype .note-box: --panel2 fill, 12px radius, 12/14 padding.
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
                                        ui.vertical(|ui| {
                                            ui.set_width((left_width - 28.0).max(120.0));
                                            ui.label(
                                                egui::RichText::new("这一页的备注（讲稿）")
                                                    .size(13.5)
                                                    .strong(),
                                            );
                                            ui.add_space(4.0);
                                            let note = page_note(&light.version, light.slot);
                                            ui.add(
                                                egui::Label::new(
                                                    egui::RichText::new(if note.is_empty() {
                                                        "这一页暂无备注。"
                                                    } else {
                                                        &note
                                                    })
                                                    .size(13.5)
                                                    .color(palette_color(TEXT_WEAK)),
                                                )
                                                .wrap(),
                                            );
                                        });
                                    });
                                if let Some((zt, zd, zn, zver)) = &zh_orig {
                                    ui.add_space(10.0);
                                    egui::Frame::NONE
                                        .fill(palette_color(PANEL_BG))
                                        .stroke(egui::Stroke::new(1.0, palette_color(LINE_34)))
                                        .corner_radius(6)
                                        .inner_margin(egui::Margin::symmetric(12, 10))
                                        .show(ui, |ui| {
                                            ui.vertical(|ui| {
                                                ui.set_width((left_width - 26.0).max(120.0));
                                                ui.label(
                                                    egui::RichText::new(format!(
                                                        "中文原文对照（中文 {zver}）"
                                                    ))
                                                    .size(12.5)
                                                    .strong(),
                                                );
                                                ui.add_space(4.0);
                                                ui.add(
                                                    egui::Label::new(
                                                        egui::RichText::new(format!(
                                                            "中文原题：{zt}"
                                                        ))
                                                        .size(12.5)
                                                        .color(palette_color(TEXT_WEAK)),
                                                    )
                                                    .wrap(),
                                                );
                                                if !zd.is_empty() {
                                                    ui.add(
                                                        egui::Label::new(
                                                            egui::RichText::new(format!(
                                                                "中文说明：{zd}"
                                                            ))
                                                            .size(12.5)
                                                            .color(palette_color(TEXT_FAINT)),
                                                        )
                                                        .wrap(),
                                                    );
                                                }
                                                if !zn.is_empty() {
                                                    ui.add(
                                                        egui::Label::new(
                                                            egui::RichText::new(format!(
                                                                "中文讲稿：{zn}"
                                                            ))
                                                            .size(12.5)
                                                            .color(palette_color(TEXT_FAINT)),
                                                        )
                                                        .wrap(),
                                                    );
                                                }
                                            });
                                        });
                                }
                            });

                            // ---- Right: .lb-side (fixed 300px, direct on APP_BG) ----
                            ui.vertical(|ui| {
                                ui.set_width(side_width);
                                if light.targets.is_empty() {
                                    ui.label(
                                        egui::RichText::new("暂无其他语言版本")
                                            .size(13.0)
                                            .color(palette_color(TEXT_FAINT)),
                                    );
                                    return;
                                }
                                let target = light.targets[light.target_index].clone();
                                if light.targets.len() > 1 {
                                    let labels: Vec<String> = light
                                        .targets
                                        .iter()
                                        .map(|t| lang_display_name(&t.lang))
                                        .collect();
                                    let label_refs: Vec<&str> =
                                        labels.iter().map(|s| s.as_str()).collect();
                                    if let Some(next) = segmented(
                                        ui,
                                        &label_refs,
                                        light.target_index,
                                        SegStyle::Soft,
                                    ) {
                                        light.target_index = next;
                                    }
                                } else {
                                    ui.label(
                                        egui::RichText::new(lang_display_name(&target.lang))
                                            .strong()
                                            .size(14.5),
                                    );
                                }
                                ui.add_space(6.0);
                                let target = light.targets[light.target_index].clone();
                                let target_id = s(&target.version, "id");
                                let pages = i(&target.version, "page_count").max(0) as usize;
                                for slot in 0..pages {
                                    if slot > 0 {
                                        ui.add_space(6.0);
                                    }
                                    let candidate = PageBindingRequest {
                                        unit_id: light.unit_id.clone(),
                                        from_version_id: s(&light.version, "id"),
                                        from_slot: light.slot as i64,
                                        to_version_id: target_id.clone(),
                                        to_slot: slot as i64,
                                    };
                                    let exact = exact_binding(&light.bindings, &candidate);
                                    let occupied = occupied_binding(&light.bindings, &candidate);
                                    let mut cover_rect = egui::Rect::NOTHING;
                                    let mut unbind_rect = egui::Rect::NOTHING;
                                    let card_resp = standard_card_v(
                                        ui,
                                        exact.is_some(),
                                        None,
                                        true,
                                        CardVariant::Normal,
                                        |ui| {
                                            ui.set_width(side_width - 2.0);
                                            ui.spacing_mut().item_spacing.y = 0.0;
                                            let cover_resp = self.thumb_response_art(
                                                ui,
                                                &target_id,
                                                slot,
                                                side_width - 2.0,
                                                false,
                                                &format!(
                                                    "{} · {} / {}",
                                                    lang_display_name(&target.lang),
                                                    slot + 1,
                                                    pages
                                                ),
                                                &page_title(&target.version, slot),
                                            );
                                            cover_rect = cover_resp.rect;
                                            egui::Frame::NONE
                                                .inner_margin(egui::Margin {
                                                    left: 12,
                                                    right: 12,
                                                    top: 10,
                                                    bottom: 10,
                                                })
                                                .show(ui, |ui| {
                                                    ui.vertical(|ui| {
                                                        ui.set_width(side_width - 26.0);
                                                        ui.add(
                                                            egui::Label::new(
                                                                egui::RichText::new(page_title(
                                                                    &target.version,
                                                                    slot,
                                                                ))
                                                                .strong()
                                                                .size(13.0),
                                                            )
                                                            .truncate(),
                                                        );
                                                        let cand_desc =
                                                            page_desc(&target.version, slot);
                                                        if !cand_desc.is_empty()
                                                            || occupied.is_some()
                                                        {
                                                            ui.add_space(6.0);
                                                            ui.horizontal_wrapped(|ui| {
                                                                ui.spacing_mut().item_spacing =
                                                                    egui::vec2(5.0, 4.0);
                                                                if !cand_desc.is_empty() {
                                                                    chip(
                                                                        ui,
                                                                        &desc_line(&cand_desc, 26),
                                                                        palette_color(ACCENT_DEEP),
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
                                                });
                                        },
                                    );
                                    let card_rect = card_resp.response.rect;
                                    // Top-right corner × button on bound cards (.cache-corner > .cache-ic).
                                    if exact.is_some() && cover_rect.is_positive() {
                                        let btn_rect = egui::Rect::from_min_size(
                                            egui::pos2(
                                                cover_rect.max.x - 6.0 - 26.0,
                                                cover_rect.min.y + 6.0,
                                            ),
                                            egui::vec2(26.0, 26.0),
                                        );
                                        unbind_rect = btn_rect;
                                        let u_resp = ui.interact(
                                            btn_rect,
                                            ui.scope_id().with(("lb_unbind", &target_id, slot)),
                                            egui::Sense::click(),
                                        );
                                        let bg = if u_resp.hovered() {
                                            egui::Color32::from_rgba_unmultiplied(8, 10, 9, 210)
                                        } else {
                                            egui::Color32::from_rgba_unmultiplied(8, 10, 9, 150)
                                        };
                                        ui.painter().rect(
                                            btn_rect,
                                            6.0,
                                            bg,
                                            egui::Stroke::new(
                                                1.0,
                                                egui::Color32::from_rgba_unmultiplied(
                                                    255, 255, 255, 80,
                                                ),
                                            ),
                                            egui::StrokeKind::Inside,
                                        );
                                        ui.painter().text(
                                            btn_rect.center(),
                                            egui::Align2::CENTER_CENTER,
                                            "×",
                                            egui::FontId::proportional(14.0),
                                            egui::Color32::WHITE,
                                        );
                                        if u_resp.clicked() {
                                            unbind = exact.clone();
                                        }
                                        let _ = u_resp
                                            .on_hover_text("解除绑定")
                                            .on_hover_cursor(egui::CursorIcon::PointingHand);
                                    }
                                    // Whole-card click binds (unless click landed on ×).
                                    if card_rect.is_positive()
                                        && ui.input(|inp| inp.pointer.primary_clicked())
                                    {
                                        if let Some(pos) =
                                            ui.input(|inp| inp.pointer.interact_pos())
                                        {
                                            if card_rect.contains(pos)
                                                && !unbind_rect.contains(pos)
                                                && exact.is_none()
                                            {
                                                request = Some(candidate);
                                            }
                                        }
                                    }
                                }
                                ui.add_space(10.0);
                                // Prototype #lbGoBind: primary full-width button.
                                if ui
                                    .add_sized(
                                        egui::vec2(side_width, 30.0),
                                        egui::Button::new(
                                            egui::RichText::new(tr("绑定"))
                                                .size(12.5)
                                                .strong()
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
                light.targets = binding_targets(&st.detail, &s(&light.version, "id"));
            }
        }
        if let Some(request) = request {
            self.request_page_binding(request, &light.bindings);
            if let ContentFrame::Unit(st) = self.content.stack.top() {
                light.bindings = st.bindings.clone();
                light.targets = binding_targets(&st.detail, &s(&light.version, "id"));
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
        if launch_present {
            self.present_unit_version(&unit_name, &light.version, DcMode::Play, light.slot);
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

    #[test]
    fn header_does_not_collapse_scroll_area_below() {
        let ctx = egui::Context::default();
        let mut header_rect = egui::Rect::NOTHING;
        let mut scroll_avail = egui::Vec2::ZERO;
        let mut out = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::pos2(0.0, 0.0),
                    egui::vec2(1200.0, 800.0),
                )),
                ..Default::default()
            },
            |ctx_ui| {
                egui::CentralPanel::default().show(ctx_ui, |ui| {
                    header_rect = ui
                        .allocate_ui_with_layout(
                            egui::vec2(ui.available_width(), 28.0),
                            egui::Layout::left_to_right(egui::Align::BOTTOM).with_main_wrap(true),
                            |ui| {
                                ui.add(
                                    egui::Label::new(
                                        egui::RichText::new(
                                            "A complete infrastructure framework supporting battery circularity.",
                                        )
                                        .size(20.0)
                                        .strong(),
                                    )
                                    .wrap(),
                                );
                                ui.horizontal(|ui| {
                                    ui.label("当前 v1.0.0 · 跟随最新");
                                });
                            },
                        )
                        .response
                        .rect;
                    scroll_avail = ui.available_size();
                });
            },
        );
        out.textures_delta.clear();
        eprintln!("header_rect={header_rect:?}, scroll_avail={scroll_avail:?}");
        assert!(header_rect.max.y < 140.0, "header_rect={header_rect:?}");
        assert!(scroll_avail.y > 600.0, "scroll_avail={scroll_avail:?}");
    }

    #[test]
    fn history_card_stacks_head_thumbs_caption_vertically() {
        let ctx = egui::Context::default();
        let mut card_rect = egui::Rect::NOTHING;
        let mut head_rect = egui::Rect::NOTHING;
        let mut thumbs_rect = egui::Rect::NOTHING;
        let mut caption_rect = egui::Rect::NOTHING;
        let card_w = 280.0f32;
        let inner_w = card_w - 22.0;
        let mut out = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::pos2(0.0, 0.0),
                    egui::vec2(1200.0, 800.0),
                )),
                ..Default::default()
            },
            |ctx_ui| {
                egui::CentralPanel::default().show(ctx_ui, |ui| {
                    ui.horizontal_top(|ui| {
                        card_rect = egui::Frame::NONE
                            .stroke(egui::Stroke::new(1.0, egui::Color32::GRAY))
                            .inner_margin(egui::Margin::same(10))
                            .show(ui, |ui| {
                                ui.vertical(|ui| {
                                    ui.set_width(inner_w);
                                    head_rect = ui
                                        .horizontal(|ui| {
                                            ui.label("v1.0.0");
                                            ui.with_layout(
                                                egui::Layout::right_to_left(egui::Align::Center),
                                                |ui| {
                                                    let _ = ui.button("使用");
                                                },
                                            );
                                        })
                                        .response
                                        .rect;
                                    ui.add_space(9.0);
                                    thumbs_rect = ui
                                        .horizontal(|ui| {
                                            ui.allocate_exact_size(
                                                egui::vec2(inner_w, inner_w * 9.0 / 16.0),
                                                egui::Sense::hover(),
                                            );
                                        })
                                        .response
                                        .rect;
                                    ui.add_space(8.0);
                                    caption_rect = ui.label("2026-11-26 · 超级管理员").rect;
                                });
                            })
                            .response
                            .rect;
                    });
                });
            },
        );
        out.textures_delta.clear();
        assert!(card_rect.width() <= card_w + 1.0, "card_rect={card_rect:?}");
        assert!(
            head_rect.max.y < thumbs_rect.min.y,
            "head={head_rect:?}, thumbs={thumbs_rect:?}"
        );
        assert!(
            thumbs_rect.max.y < caption_rect.min.y,
            "thumbs={thumbs_rect:?}, caption={caption_rect:?}"
        );
    }
}

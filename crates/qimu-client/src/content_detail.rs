use super::*;

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

fn page_note(version: &Value, slot: usize) -> String {
    version
        .get("scripts")
        .and_then(Value::as_array)
        .and_then(|scripts| scripts.get(slot))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
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

    pub(super) fn unit_pages_ui(&mut self, ui: &mut egui::Ui, st: &mut UnitState) {
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
        ui.horizontal_wrapped(|ui| {
            for item in &versions {
                if ui
                    .selectable_label(
                        selected_id == item.rendition_id,
                        format!("{} · {}", item.form, item.lang),
                    )
                    .clicked()
                {
                    st.selected_rendition = Some(item.rendition_id.clone());
                    st.selected_page = None;
                    st.bind_pick = None;
                }
            }
        });
        let Some(selected) = versions
            .iter()
            .find(|item| item.rendition_id == selected_id)
            .cloned()
        else {
            return;
        };
        let version_id = s(&selected.version, "id");
        let page_count = i(&selected.version, "page_count").max(0) as usize;
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new("页面缩略图").strong());
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
        });
        ui.add_space(6.0);
        let targets = binding_targets(&st.detail, &version_id);
        let mut request = None;
        ui.horizontal_wrapped(|ui| {
            for slot in 0..page_count {
                ui.vertical(|ui| {
                    let response = self.thumb_response(
                        ui,
                        &version_id,
                        slot,
                        200.0,
                        st.selected_page == Some(slot),
                    );
                    if let Some(pick) = st.bind_pick.clone() {
                        if response.clicked() {
                            request = Some(PageBindingRequest {
                                unit_id: st.unit_id.clone(),
                                from_version_id: pick.from_version_id,
                                from_slot: pick.from_slot,
                                to_version_id: pick.to_version_id,
                                to_slot: slot as i64,
                            });
                        }
                    } else {
                        if response.clicked() {
                            st.selected_page = Some(slot);
                        }
                        if response.double_clicked() {
                            self.open_light(st, &selected.version, slot);
                        }
                    }
                    ui.label(egui::RichText::new(page_title(&selected.version, slot)).small());
                    ui.horizontal(|ui| {
                        for target in &targets {
                            let bound = st.bindings.iter().any(|binding| {
                                s(binding, "from_version_id") == version_id
                                    && i(binding, "from_slot") == slot as i64
                                    && s(binding, "to_version_id") == s(&target.version, "id")
                            });
                            let chip = egui::Button::new(
                                egui::RichText::new(target.lang.to_uppercase()).color(if bound {
                                    palette_color(CHIP_GREEN_FG)
                                } else {
                                    palette_color(TEXT_WEAK)
                                }),
                            )
                            .fill(if bound {
                                palette_color(CHIP_GREEN_BG)
                            } else {
                                palette_color(HOVER_BG)
                            })
                            .corner_radius(5);
                            if ui
                                .add(chip)
                                .on_hover_text(if bound {
                                    format!("{}已绑定", target.lang)
                                } else {
                                    format!("绑定{}对应页", target.lang)
                                })
                                .clicked()
                            {
                                st.bind_pick = Some(PageBindingPick {
                                    from_version_id: version_id.clone(),
                                    from_slot: slot as i64,
                                    to_version_id: s(&target.version, "id"),
                                    target_lang: target.lang.clone(),
                                });
                            }
                        }
                    });
                });
            }
        });
        if let Some(request) = request {
            st.bind_pick = None;
            self.request_page_binding(request, &st.bindings);
        }
        ui.add_space(10.0);
        ui.separator();
        ui.add_space(6.0);
    }

    pub(super) fn unit_bindings_ui(&mut self, ui: &mut egui::Ui, st: &mut UnitState) {
        ui.label(egui::RichText::new("语言绑定（逐页）").strong());
        let all_versions = unit_versions(&st.detail);
        if all_versions.len() < 2 {
            ui.label(egui::RichText::new("至少两个版本后可做逐页绑定").weak());
            return;
        }
        if !all_versions
            .iter()
            .any(|item| item.version["id"].as_str() == Some(st.bind_from.as_str()))
        {
            st.bind_from = s(&all_versions[0].version, "id");
            st.bind_from_slot = None;
        }
        if !all_versions
            .iter()
            .any(|item| item.version["id"].as_str() == Some(st.bind_to.as_str()))
        {
            st.bind_to = s(&all_versions[1].version, "id");
        }
        ui.horizontal(|ui| {
            ui.label("基准");
            let current = all_versions
                .iter()
                .find(|item| s(&item.version, "id") == st.bind_from)
                .map(UnitVersion::label)
                .unwrap_or_default();
            egui::ComboBox::from_id_salt("bind_from")
                .selected_text(current)
                .show_ui(ui, |ui| {
                    for item in &all_versions {
                        ui.selectable_value(
                            &mut st.bind_from,
                            s(&item.version, "id"),
                            item.label(),
                        );
                    }
                });
            ui.label("对应");
            let current = all_versions
                .iter()
                .find(|item| s(&item.version, "id") == st.bind_to)
                .map(UnitVersion::label)
                .unwrap_or_default();
            egui::ComboBox::from_id_salt("bind_to")
                .selected_text(current)
                .show_ui(ui, |ui| {
                    for item in &all_versions {
                        ui.selectable_value(&mut st.bind_to, s(&item.version, "id"), item.label());
                    }
                });
        });
        let from_pages = all_versions
            .iter()
            .find(|item| s(&item.version, "id") == st.bind_from)
            .map(|item| i(&item.version, "page_count").max(0) as usize)
            .unwrap_or(0);
        let to_pages = all_versions
            .iter()
            .find(|item| s(&item.version, "id") == st.bind_to)
            .map(|item| i(&item.version, "page_count").max(0) as usize)
            .unwrap_or(0);
        ui.label("基准页（点选）：");
        ui.horizontal_wrapped(|ui| {
            for slot in 0..from_pages {
                ui.vertical(|ui| {
                    let version_id = st.bind_from.clone();
                    let response = self.thumb_response(
                        ui,
                        &version_id,
                        slot,
                        84.0,
                        st.bind_from_slot == Some(slot as i64),
                    );
                    if response.clicked() {
                        st.bind_from_slot = Some(slot as i64);
                    }
                    if response.double_clicked() {
                        if let Some(item) = all_versions
                            .iter()
                            .find(|item| s(&item.version, "id") == version_id)
                        {
                            self.open_light(st, &item.version, slot);
                        }
                    }
                });
            }
        });
        let mut request = None;
        let mut unbind = None;
        ui.label("对应页（点「绑定/已绑定」切换）：");
        ui.horizontal_wrapped(|ui| {
            for slot in 0..to_pages {
                ui.vertical(|ui| {
                    let version_id = st.bind_to.clone();
                    self.thumb_ui(ui, &version_id, slot, 84.0);
                    let Some(from_slot) = st.bind_from_slot else {
                        ui.label("先选基准页");
                        return;
                    };
                    let candidate = PageBindingRequest {
                        unit_id: st.unit_id.clone(),
                        from_version_id: st.bind_from.clone(),
                        from_slot,
                        to_version_id: st.bind_to.clone(),
                        to_slot: slot as i64,
                    };
                    if let Some(binding) = exact_binding(&st.bindings, &candidate) {
                        if ui
                            .add(
                                egui::Button::new(
                                    egui::RichText::new("已绑定 ✓")
                                        .color(palette_color(CHIP_GREEN_FG)),
                                )
                                .fill(palette_color(CHIP_GREEN_BG))
                                .corner_radius(6),
                            )
                            .clicked()
                        {
                            unbind = Some((st.unit_id.clone(), s(&binding, "id")));
                        }
                    } else if ui.button("绑定").clicked() {
                        request = Some(candidate);
                    }
                });
            }
        });
        if let Some((unit_id, binding_id)) = unbind {
            self.delete_page_binding(&unit_id, &binding_id);
        }
        if let Some(request) = request {
            self.request_page_binding(request, &st.bindings);
        }
    }

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
                && i(&binding, "to_slot") == request.to_slot;
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
        match self.call_work(
            "POST",
            &format!("/v1/units/{unit_id}/page-bindings/{binding_id}/delete"),
            None,
        ) {
            Ok(_) => {
                self.msg = "已解除页面绑定".into();
                self.reload_unit_detail(unit_id);
            }
            Err(error) => self.msg = format!("解绑失败：{error}"),
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
                        let side_width = if screen.width() < 900.0 { 280.0 } else { 330.0 };
                        let left_width = (screen.width() - side_width - 54.0).max(260.0);
                        ui.horizontal_top(|ui| {
                            ui.add_space(18.0);
                            ui.vertical(|ui| {
                                ui.set_width(left_width);
                                let image_width = left_width
                                    .min((preview_screen.height() - 170.0).max(240.0) * 16.0 / 9.0);
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
                                egui::Frame::NONE
                                    .fill(palette_color(HOVER_BG))
                                    .corner_radius(6)
                                    .inner_margin(10.0)
                                    .show(ui, |ui| {
                                        ui.set_width(left_width - 20.0);
                                        ui.label(
                                            egui::RichText::new("这一页的备注（讲稿）").strong(),
                                        );
                                        let note = page_note(&light.version, light.slot);
                                        ui.label(if note.is_empty() { "—" } else { &note });
                                    });
                            });
                            ui.add_space(18.0);
                            egui::ScrollArea::vertical()
                                .max_height(preview_screen.height() - 36.0)
                                .show(ui, |ui| {
                                    ui.set_width(side_width);
                                    egui::Frame::NONE
                                        .fill(palette_color(HOVER_BG))
                                        .corner_radius(8.0)
                                        .inner_margin(12.0)
                                        .show(ui, |ui| {
                                            ui.set_width(side_width - 24.0);
                                            if light.targets.is_empty() {
                                                ui.label(
                                                    egui::RichText::new("暂无其他语言版本").weak(),
                                                );
                                                return;
                                            }
                                            ui.horizontal_wrapped(|ui| {
                                                for (index, target) in
                                                    light.targets.iter().enumerate()
                                                {
                                                    if ui
                                                        .selectable_label(
                                                            light.target_index == index,
                                                            target.lang.to_uppercase(),
                                                        )
                                                        .clicked()
                                                    {
                                                        light.target_index = index;
                                                    }
                                                }
                                            });
                                            let target = light.targets[light.target_index].clone();
                                            ui.label(format!(
                                                "{} · v{}",
                                                target.lang,
                                                s(&target.version, "semver")
                                            ));
                                            ui.add_space(6.0);
                                            let target_id = s(&target.version, "id");
                                            let pages =
                                                i(&target.version, "page_count").max(0) as usize;
                                            ui.horizontal_wrapped(|ui| {
                                                for slot in 0..pages {
                                                    let candidate = PageBindingRequest {
                                                        unit_id: light.unit_id.clone(),
                                                        from_version_id: s(&light.version, "id"),
                                                        from_slot: light.slot as i64,
                                                        to_version_id: target_id.clone(),
                                                        to_slot: slot as i64,
                                                    };
                                                    let exact =
                                                        exact_binding(&light.bindings, &candidate);
                                                    let occupied = occupied_binding(
                                                        &light.bindings,
                                                        &candidate,
                                                    );
                                                    ui.vertical(|ui| {
                                                        ui.set_width(132.0);
                                                        let response = self.thumb_response(
                                                            ui,
                                                            &target_id,
                                                            slot,
                                                            132.0,
                                                            exact.is_some(),
                                                        );
                                                        if response.clicked() && exact.is_none() {
                                                            request = Some(candidate);
                                                        }
                                                        ui.label(
                                                            egui::RichText::new(page_title(
                                                                &target.version,
                                                                slot,
                                                            ))
                                                            .small(),
                                                        );
                                                        if exact.is_some() {
                                                            if ui.small_button("× 解绑").clicked()
                                                            {
                                                                unbind = exact.clone();
                                                            }
                                                        } else if occupied.is_some() {
                                                            ui.label(
                                                                egui::RichText::new(
                                                                    "已被其他页占用",
                                                                )
                                                                .small()
                                                                .color(palette_color(
                                                                    CHIP_ORANGE_FG,
                                                                )),
                                                            );
                                                        }
                                                    });
                                                }
                                            });
                                            ui.add_space(10.0);
                                            if btn_primary(ui, tr("绑定")).clicked() {
                                                start_pick = true;
                                            }
                                        });
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
}

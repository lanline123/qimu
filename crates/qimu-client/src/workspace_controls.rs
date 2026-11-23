use super::*;
use std::cell::Cell;
use std::collections::{HashSet, VecDeque};
use std::fmt::Write as _;

thread_local! {
    pub(super) static LIGHT_THEME: Cell<bool> = const { Cell::new(false) };
    pub(super) static ENGLISH_UI: Cell<bool> = const { Cell::new(false) };
}

pub(super) fn palette_color(dark: egui::Color32) -> egui::Color32 {
    if !LIGHT_THEME.get() {
        return dark;
    }
    let rgb = if dark == APP_BG {
        0xf5f6f8
    } else if dark == PANEL_BG {
        0xffffff
    } else if dark == HOVER_BG {
        0xf1f2f5
    } else if dark == PRESSED_BG {
        0xe4e6ec
    } else if dark == SEPARATOR {
        0xe3e5ea
    } else if dark == FIELD_BORDER {
        0xd2d5de
    } else if dark == TEXT_MAIN {
        0x16161a
    } else if dark == TEXT_WEAK {
        0x6b6b78
    } else if dark == TEXT_FAINT {
        0x767681
    } else if dark == ACCENT_DEEP {
        0x1d4ed8
    } else if dark == CHIP_GREEN_FG {
        0x166534
    } else if dark == CHIP_ORANGE_FG {
        0xb45309
    } else if dark == ROSE {
        0xbe123c
    } else if dark == SEL_BG {
        0xe9efff
    } else {
        return dark;
    };
    egui::Color32::from_rgb((rgb >> 16) as u8, (rgb >> 8) as u8, rgb as u8)
}

pub(super) fn query_component(input: &str) -> String {
    let mut output = String::with_capacity(input.len());
    for byte in input.bytes() {
        if byte.is_ascii_alphanumeric() || b"-._~".contains(&byte) {
            output.push(char::from(byte));
        } else {
            let _ = write!(output, "%{byte:02X}");
        }
    }
    output
}

enum SearchTarget {
    Index(String),
    Unit(String),
    Deck(Value),
}

impl SearchTarget {
    fn caption(&self) -> &'static str {
        match self {
            Self::Index(_) => "索引",
            Self::Unit(_) => "内容块",
            Self::Deck(_) => "演示稿",
        }
    }
}

#[derive(Default)]
pub(super) struct WorkspaceControls {
    search_text: String,
    focus_search: bool,
    search_open: bool,
    search_hits: Vec<(String, SearchTarget)>,
    search_error: Option<String>,
    messages_open: bool,
    messages: VecDeque<String>,
    unread: usize,
}

impl WorkspaceControls {
    pub(super) fn record_message(&mut self, message: &str) {
        if !message.is_empty() {
            self.messages.push_front(message.into());
            self.messages.truncate(30);
            if !self.messages_open {
                self.unread = (self.unread + 1).min(30);
            }
        }
    }
}

fn menu_item(ui: &mut egui::Ui, label: &str) -> bool {
    if ui.button(tr(label)).clicked() {
        ui.close();
        true
    } else {
        false
    }
}

impl App {
    pub(super) fn workspace_edit_menu(&mut self, ui: &mut egui::Ui) {
        ui.menu_button(tr("编辑"), |ui| {
            if menu_item(ui, "全局搜索") {
                self.workspace_controls.focus_search = true;
            }
            if menu_item(ui, "刷新") {
                if self.user.is_some() {
                    self.index_search_tags.clear();
                    self.refresh_all();
                } else {
                    self.refresh_guest();
                }
            }
        });
    }

    pub(super) fn workspace_context_menus(&mut self, ui: &mut egui::Ui) {
        ui.menu_button(tr("索引"), |ui| {
            if menu_item(ui, "索引总览") {
                self.navigate_to(Page::Index);
                self.index.stack.pop_to(0);
            }
            if menu_item(ui, "新建索引") {
                self.act_new_index();
            }
            if let IndexFrame::Detail(st) = self.index.stack.top_mut() {
                if menu_item(ui, "派生关系") {
                    st.view = IndexDetailView::Lineage;
                    self.navigate_to(Page::Index);
                }
            }
        });
        ui.menu_button(tr("演示"), |ui| {
            if menu_item(ui, "演示厅") {
                self.navigate_to(Page::Showroom);
                self.showroom.zone = "plaza".into();
                self.showroom.stack.pop_to(0);
            }
        });
        ui.menu_button(tr("组织"), |ui| {
            let sections = self.settings_sections();
            for (section, label) in sections.into_iter().enumerate() {
                if matches!(label, "个人资料" | "组织管理" | "内容组与授权" | "审批队列")
                    && menu_item(ui, label)
                {
                    self.navigate_to(Page::Settings);
                    self.settings.section = section;
                }
            }
            if self.user.is_none() && menu_item(ui, "登录") {
                self.login_open = true;
            }
        });
        ui.menu_button(tr("窗口"), |ui| {
            if menu_item(ui, "最小化") {
                ui.ctx()
                    .send_viewport_cmd(egui::ViewportCommand::Minimized(true));
            }
            let maximized = ui.input(|input| input.viewport().maximized.unwrap_or(false));
            if menu_item(
                ui,
                if maximized {
                    "还原窗口"
                } else {
                    "最大化"
                },
            ) {
                ui.ctx()
                    .send_viewport_cmd(egui::ViewportCommand::Maximized(!maximized));
            }
            let fullscreen = ui.input(|input| input.viewport().fullscreen.unwrap_or(false));
            if menu_item(ui, if fullscreen { "退出全屏" } else { "全屏" }) {
                ui.ctx()
                    .send_viewport_cmd(egui::ViewportCommand::Fullscreen(!fullscreen));
            }
        });
        if ui.button(tr("设置")).clicked() {
            self.navigate_to(Page::Settings);
        }
    }

    pub(super) fn workspace_preferences(&mut self, ui: &mut egui::Ui) {
        if ui
            .button(egui::RichText::new("◐").size(16.0))
            .on_hover_text(tr("切换主题"))
            .clicked()
        {
            let light = !LIGHT_THEME.get();
            LIGHT_THEME.set(light);
            apply_modern_style(ui.ctx());
            save_client_settings(json!({ "theme": if light { "light" } else { "dark" } }));
            ui.ctx().request_repaint();
        }
        let unread = self.workspace_controls.unread;
        let label = if unread == 0 {
            tr("消息").into()
        } else {
            format!("{} {unread}", tr("消息"))
        };
        if ui.button(label).on_hover_text(tr("消息中心")).clicked() {
            self.workspace_controls.messages_open = !self.workspace_controls.messages_open;
            self.workspace_controls.unread = 0;
        }
        ui.horizontal(|ui| {
            if let Some(next) = segmented(
                ui,
                &["English", "中文"],
                usize::from(!ENGLISH_UI.get()),
                SegStyle::Solid,
            ) {
                ENGLISH_UI.set(next == 0);
                save_client_settings(
                    json!({ "locale": if next == 0 { "en-us" } else { "zh-hans" } }),
                );
                ui.ctx().request_repaint();
            }
        });
    }

    pub(super) fn workspace_search_box(&mut self, ui: &mut egui::Ui) {
        let width = if ui.ctx().content_rect().width() < 1120.0 {
            142.0
        } else {
            222.0
        };
        let response = ui.add(
            egui::TextEdit::singleline(&mut self.workspace_controls.search_text)
                .desired_width(width)
                .hint_text(tr("搜索页面、标签、维护人…")),
        );
        if std::mem::take(&mut self.workspace_controls.focus_search) {
            response.request_focus();
        }
        let enter = (response.has_focus() || response.lost_focus())
            && ui.input(|input| input.key_pressed(egui::Key::Enter));
        let submitted = ui.button(tr("搜索")).clicked();
        if enter || submitted {
            self.run_workspace_search();
        }
    }

    fn run_workspace_search(&mut self) {
        let query = self.workspace_controls.search_text.trim();
        if query.is_empty() {
            self.workspace_controls.search_open = false;
            return;
        }
        let lower = query.to_lowercase();
        let matches = |text: &str| text.to_lowercase().contains(&lower);
        let has_field = |record: &Value, fields: &[&str]| {
            fields
                .iter()
                .any(|key| record[key].as_str().is_some_and(&matches))
        };
        let mut hits = Vec::new();
        for item in &self.indexes {
            let id = s(item, "id");
            if has_field(item, &["name", "maintainer", "id"])
                || self.index_search_tags.get(&id).is_some_and(|tags| {
                    tags.iter()
                        .any(|tag| matches(tag) || matches(&format!("v{tag}")))
                })
            {
                hits.push((s(item, "name"), SearchTarget::Index(id)));
            }
        }
        for item in &self.showroom_live {
            if has_field(item, &["title", "intro", "created_by", "id"]) {
                hits.push((s(item, "title"), SearchTarget::Deck(item.clone())));
            }
        }
        let mut units = HashSet::new();
        for item in &self.units {
            let unit = &item["unit"];
            if has_field(unit, &["name", "maintainer", "description", "id"]) {
                let id = s(unit, "id");
                units.insert(id.clone());
                hits.push((s(unit, "name"), SearchTarget::Unit(id)));
            }
        }
        self.workspace_controls.search_error = None;
        if self.user.is_some() {
            match self.call(
                "GET",
                &format!("/v1/search?q={}", query_component(query)),
                None,
            ) {
                Ok(response) => {
                    for hit in response["hits"].as_array().into_iter().flatten() {
                        let unit = &hit["unit"];
                        let id = s(unit, "id");
                        if units.insert(id.clone()) {
                            hits.push((s(unit, "name"), SearchTarget::Unit(id)));
                        }
                    }
                }
                Err(error) => self.workspace_controls.search_error = Some(error),
            }
        }
        self.workspace_controls.search_hits = hits;
        self.workspace_controls.search_open = true;
    }

    pub(super) fn workspace_control_windows(&mut self, ctx: &egui::Context) {
        let controls = &mut self.workspace_controls;
        if controls.search_open {
            let mut selected = None;
            egui::Window::new(tr("搜索结果"))
                .id(egui::Id::unique("workspace-search-results"))
                .open(&mut controls.search_open)
                .default_pos(egui::pos2(210.0, 102.0))
                .default_width(430.0)
                .show(ctx, |ui| {
                    if let Some(error) = &controls.search_error {
                        ui.label(format!("{} · {error}", tr("页面全文搜索失败")));
                    }
                    if controls.search_hits.is_empty() {
                        ui.label(tr("没有匹配结果"));
                    }
                    egui::ScrollArea::vertical()
                        .max_height(392.0)
                        .show(ui, |ui| {
                            for (position, (title, target)) in
                                controls.search_hits.iter().enumerate()
                            {
                                ui.push_id(position, |ui| {
                                    if ui
                                        .button(format!("{} · {title}", tr(target.caption())))
                                        .clicked()
                                    {
                                        selected = Some(position);
                                    }
                                });
                            }
                        });
                });
            if let Some(position) = selected {
                let (_, target) = controls.search_hits.swap_remove(position);
                controls.search_open = false;
                match target {
                    SearchTarget::Index(id) => {
                        self.navigate_to(Page::Index);
                        self.open_index_detail_as(&id, "main".into());
                    }
                    SearchTarget::Unit(id) => {
                        self.navigate_to(Page::Content);
                        self.close_unit_detail();
                        self.open_unit_detail(&id);
                    }
                    SearchTarget::Deck(item) => {
                        self.navigate_to(Page::Showroom);
                        self.open_showroom_deck(&item);
                    }
                }
            }
        }
        let controls = &mut self.workspace_controls;
        if controls.messages_open {
            let mut inbox = false;
            egui::Window::new(tr("消息中心"))
                .id(egui::Id::unique("workspace-messages"))
                .open(&mut controls.messages_open)
                .anchor(egui::Align2::RIGHT_TOP, egui::vec2(-12.0, 68.0))
                .default_width(330.0)
                .show(ctx, |ui| {
                    if self.user.is_some()
                        && ui
                            .button(format!("{} · {}", tr("收到的分享"), self.inbox.len()))
                            .clicked()
                    {
                        inbox = true;
                    }
                    if controls.messages.is_empty() {
                        ui.label(tr("本次会话暂无消息"));
                    }
                    egui::ScrollArea::vertical()
                        .max_height(310.0)
                        .show(ui, |ui| {
                            for message in &controls.messages {
                                ui.label(message);
                                ui.separator();
                            }
                        });
                });
            if inbox {
                controls.messages_open = false;
                self.navigate_to(Page::Showroom);
                self.showroom.zone = "inbox".into();
                self.showroom.stack.pop_to(0);
            }
        }
    }
}

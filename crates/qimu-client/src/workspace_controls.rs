use super::*;
use crate::account_menu::{AccountMenu, AccountMenuAction, AccountMenuEntry};
use crate::server_menu::{ServerMenu, ServerMenuAction, ServerMenuEntry};
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
    } else if dark == CARD_BG {
        0xffffff
    } else if dark == PANEL_BG {
        0xffffff
    } else if dark == HOVER_BG {
        0xf1f2f5
    } else if dark == FIELD_BG {
        0xf1f2f5
    } else if dark == CARD_BORDER {
        0xe3e5ea
    } else if dark == PRESSED_BG {
        0xe4e6ec
    } else if dark == SEPARATOR {
        0xe3e5ea
    } else if dark == FIELD_BORDER {
        0xd2d5de
    } else if dark == TEXT_MAIN {
        0x000000
    } else if dark == TEXT_WEAK {
        0x373740
    } else if dark == TEXT_FAINT {
        0x55555f
    } else if dark == ACCENT_DEEP {
        0x1d4ed8
    } else if dark == CHIP_GREEN_BG {
        0xdcfce7
    } else if dark == CHIP_ORANGE_BG {
        0xfef3c7
    } else if dark == CHIP_BLUE_BG {
        0xdbeafe
    } else if dark == MINE_BG {
        0xeaf0fe
    } else if dark == ROSE_SOFT_BG {
        0xffe4e6
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

struct WorkspaceMessage {
    body: String,
    at: f64,
}

#[derive(Default)]
pub(super) struct WorkspaceControls {
    search_text: String,
    focus_search: bool,
    search_open: bool,
    search_hits: Vec<(String, SearchTarget)>,
    search_error: Option<String>,
    messages: VecDeque<WorkspaceMessage>,
    pub(super) unread: usize,
    server_menu: ServerMenu,
    account_menu: AccountMenu,
}

impl WorkspaceControls {
    pub(super) fn record_message(&mut self, message: &str, now: f64, mark_unread: bool) {
        if !message.is_empty() {
            self.messages.push_front(WorkspaceMessage {
                body: message.into(),
                at: now,
            });
            self.messages.truncate(30);
            if mark_unread {
                self.unread = (self.unread + 1).min(30);
            }
        }
    }

    /// Either toolbar popup menu (account / server) is open.
    /// A popup menu owns the user's attention while it is up,
    /// so global shortcuts (the deckbar Delete key) stand down
    /// until it closes.
    pub(super) fn menu_open(&self) -> bool {
        self.account_menu.is_open() || self.server_menu.is_open()
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
        ui.menu_button(egui::RichText::new(tr("编辑")).size(12.0), |ui| {
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
        ui.menu_button(egui::RichText::new(tr("索引")).size(12.0), |ui| {
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
        ui.menu_button(egui::RichText::new(tr("演示")).size(12.0), |ui| {
            if menu_item(ui, "演示厅") {
                self.navigate_to(Page::Showroom);
                self.showroom.zone = "plaza".into();
                self.showroom.stack.pop_to(0);
            }
        });
        ui.menu_button(egui::RichText::new(tr("组织")).size(12.0), |ui| {
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
        ui.menu_button(egui::RichText::new(tr("窗口")).size(12.0), |ui| {
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

    pub(super) fn workspace_account_button(&mut self, ui: &mut egui::Ui) {
        use craft_ui::buttons::{ButtonVisuals, IconButton, IconButtonStyle};
        let name = self
            .user
            .as_ref()
            .map(|user| s(user, "display_name"))
            .filter(|name| !name.is_empty())
            .unwrap_or_default();
        let text = if self.user.is_some() {
            if name.is_empty() {
                self.role()
            } else {
                format!("{name} · {}", self.role())
            }
        } else if self.anon_active() {
            tr("匿名账户").to_string()
        } else {
            tr("未登录").to_string()
        };
        let font = egui::FontId::proportional(12.0);
        let full_label = ui.painter().layout_no_wrap(
            text.clone(),
            font.clone(),
            palette_color(TEXT_WEAK),
        );
        // The button hugs its label (10px of padding per side,
        // the measure the server button below also uses) between
        // the 92px floor and the 204px cap in the width formula.
        // A label too wide for the cap is ellipsized to fit
        // instead of running into the clip rect mid-glyph, and
        // the hover then carries the full text so a long name
        // stays readable.
        let (label, truncated) = if full_label.size().x + 20.0 <= 204.0 {
            (full_label, false)
        } else {
            let mut shown = text.clone();
            let galley = loop {
                let candidate = ui.painter().layout_no_wrap(
                    format!("{shown}…"),
                    font.clone(),
                    palette_color(TEXT_WEAK),
                );
                if candidate.size().x + 20.0 <= 204.0 || shown.is_empty() {
                    break candidate;
                }
                shown.pop();
            };
            (galley, true)
        };
        let hover_text = if truncated {
            text.clone()
        } else {
            tr("账户信息").to_string()
        };
        let rest = ButtonVisuals {
            fill: egui::Color32::TRANSPARENT,
            stroke: egui::Stroke::NONE,
            icon: palette_color(TEXT_WEAK),
        };
        let hovered = ButtonVisuals {
            fill: palette_color(PRESSED_BG),
            icon: palette_color(TEXT_MAIN),
            ..rest
        };
        let style = IconButtonStyle {
            normal: rest,
            hovered,
            pressed: Some(hovered),
            selected: hovered,
            corner_radius: egui::CornerRadius::same(6),
            focus_stroke: egui::Stroke::new(1.0, ACCENT),
        };
        let width = (label.size().x + 20.0).clamp(92.0, 204.0);
        if IconButton::new(tr("账户信息"), egui::vec2(width, 24.0), &style)
            .selected(self.workspace_controls.account_menu.is_open())
            .show(ui, |ui, rect, tint| {
                ui.painter()
                    .with_clip_rect(rect)
                    .galley_with_override_text_color(
                        egui::pos2(
                            rect.center().x - label.size().x / 2.0,
                            rect.center().y - label.size().y / 2.0,
                        ),
                        label.clone(),
                        tint,
                    );
            })
            .on_hover_text(hover_text)
            .clicked()
        {
            self.workspace_controls.server_menu.close();
            self.workspace_controls
                .account_menu
                .set_open(!self.workspace_controls.account_menu.is_open());
        }
    }

    pub(super) fn workspace_server_button(&mut self, ui: &mut egui::Ui) {
        use craft_ui::buttons::{ButtonVisuals, IconButton, IconButtonStyle};
        let placeholder = tr("暂无服务器");
        let active_name = self
            .active
            .and_then(|index| self.conns.get(index))
            .filter(|conn| {
                !conn.anon && conn.token.is_some() && conn.status == ConnStatus::Connected
            })
            .map(|conn| conn.name.clone());
        let name = active_name.unwrap_or_else(|| placeholder.to_string());
        let mut widest: Option<(&str, f32)> = None;
        for conn in self.conns.iter().filter(|conn| {
            !conn.anon && conn.token.is_some() && conn.status == ConnStatus::Connected
        }) {
            let width = ui
                .painter()
                .layout_no_wrap(
                    conn.name.clone(),
                    egui::FontId::proportional(12.0),
                    palette_color(TEXT_WEAK),
                )
                .size()
                .x;
            if widest.is_none_or(|(_, current)| width > current) {
                widest = Some((&conn.name, width));
            }
        }
        let label = ui.painter().layout_no_wrap(
            name.clone(),
            egui::FontId::proportional(12.0),
            palette_color(TEXT_WEAK),
        );
        let rest = ButtonVisuals {
            fill: egui::Color32::TRANSPARENT,
            stroke: egui::Stroke::NONE,
            icon: palette_color(TEXT_WEAK),
        };
        let hovered = ButtonVisuals {
            fill: palette_color(PRESSED_BG),
            icon: palette_color(TEXT_MAIN),
            ..rest
        };
        let style = IconButtonStyle {
            normal: rest,
            hovered,
            pressed: Some(hovered),
            selected: hovered,
            corner_radius: egui::CornerRadius::same(6),
            focus_stroke: egui::Stroke::new(1.0, ACCENT),
        };
        let text_width = widest.map_or(label.size().x, |(_, width)| width);
        let width = (text_width + 20.0).max(72.0);
        if IconButton::new(tr("切换服务器"), egui::vec2(width, 24.0), &style)
            .selected(self.workspace_controls.server_menu.is_open())
            .show(ui, |ui, rect, tint| {
                ui.painter()
                    .with_clip_rect(rect)
                    .galley_with_override_text_color(
                        egui::pos2(rect.left() + 10.0, rect.center().y - label.size().y / 2.0),
                        label.clone(),
                        tint,
                    );
            })
            .on_hover_text(format!("{} · {name}", tr("切换服务器")))
            .clicked()
        {
            self.workspace_controls.account_menu.close();
            self.workspace_controls
                .server_menu
                .set_open(!self.workspace_controls.server_menu.is_open());
        }
    }

    pub(super) fn workspace_preferences(&mut self, ui: &mut egui::Ui) {
        use craft_ui::buttons::{ButtonVisuals, IconButton, IconButtonStyle};
        let icons = ChromeIcons;
        let rest = ButtonVisuals {
            fill: egui::Color32::TRANSPARENT,
            stroke: egui::Stroke::NONE,
            icon: palette_color(ICON_TINT),
        };
        let hovered = ButtonVisuals {
            fill: palette_color(PRESSED_BG),
            icon: palette_color(TEXT_MAIN),
            ..rest
        };
        let style = IconButtonStyle {
            normal: rest,
            hovered,
            pressed: Some(hovered),
            selected: hovered,
            corner_radius: egui::CornerRadius::same(6),
            focus_stroke: egui::Stroke::new(1.0, ACCENT),
        };
        // The theme glyph is the one chrome icon with a large
        // filled area (theme.svg's half-disc). Icon textures
        // rasterize pure white (SvgStyle current_color #ffffff,
        // painted on transparency by the egui_extras/resvg
        // loader) and the draw tint multiplies through, so the
        // shared style's ICON_TINT — which is TEXT_MAIN, pure
        // white — renders the half-disc as a solid white block
        // on the dark menu bar (the reported white background).
        // The prototype's menu icons sit at a muted tone, so the
        // theme button gets its own glyph colors: TEXT_WEAK at
        // rest, brightening to TEXT_MAIN on hover. The glyph
        // file and all other icons are unchanged.
        let theme_hovered = ButtonVisuals {
            fill: palette_color(PRESSED_BG),
            stroke: egui::Stroke::NONE,
            icon: palette_color(TEXT_MAIN),
        };
        let theme_style = IconButtonStyle {
            normal: ButtonVisuals {
                fill: egui::Color32::TRANSPARENT,
                stroke: egui::Stroke::NONE,
                icon: palette_color(TEXT_WEAK),
            },
            hovered: theme_hovered,
            pressed: Some(theme_hovered),
            selected: theme_hovered,
            corner_radius: egui::CornerRadius::same(6),
            focus_stroke: egui::Stroke::new(1.0, ACCENT),
        };
        if IconButton::new(tr("切换主题"), egui::vec2(29.0, 24.0), &theme_style)
            .show(ui, |ui, rect, tint| {
                icons.paint(ui, ChromeIcon::Theme, rect.center(), 15.0, tint)
            })
            .on_hover_text(tr("切换主题"))
            .clicked()
        {
            let light = !LIGHT_THEME.get();
            LIGHT_THEME.set(light);
            apply_modern_style(ui.ctx());
            save_client_settings(json!({ "theme": if light { "light" } else { "dark" } }));
            ui.ctx().request_repaint();
        }

        let label = ui.painter().layout_no_wrap(
            tr("消息").into(),
            egui::FontId::proportional(12.0),
            palette_color(TEXT_WEAK),
        );
        let unread = self.workspace_controls.unread;
        let count = (unread > 0).then(|| {
            ui.painter().layout_no_wrap(
                unread.to_string(),
                egui::FontId::proportional(10.5),
                egui::Color32::WHITE,
            )
        });
        let count_width = count.as_ref().map_or(0.0, |count| count.size().x + 15.0);
        let width = 35.0 + label.size().x + count_width;
        if IconButton::new(tr("消息中心"), egui::vec2(width, 24.0), &style)
            .selected(self.page == Page::Messages)
            .show(ui, |ui, rect, tint| {
                let y = rect.center().y;
                icons.paint(
                    ui,
                    ChromeIcon::Bell,
                    egui::pos2(rect.left() + 14.5, y),
                    15.0,
                    tint,
                );
                let x = rect.left() + 28.0;
                ui.painter().galley_with_override_text_color(
                    egui::pos2(x, y - label.size().y / 2.0),
                    label.clone(),
                    tint,
                );
                if let Some(count) = count {
                    let x = x + label.size().x + 3.0;
                    let bubble = egui::Rect::from_min_size(
                        egui::pos2(x, y - 7.5),
                        egui::vec2(count_width - 3.0, 15.0),
                    );
                    ui.painter().rect_filled(bubble, 255.0, palette_color(ROSE));
                    ui.painter().galley(
                        bubble.center() - count.size() / 2.0,
                        count,
                        egui::Color32::WHITE,
                    );
                }
            })
            .on_hover_text(if unread == 0 {
                tr("消息中心").to_owned()
            } else {
                format!("{} · {unread}", tr("消息中心"))
            })
            .clicked()
        {
            // The badge is the mirror's real unread count now:
            // opening the page no longer marks anything read —
            // read state flips per card, on the card's own
            // click (user ruling 2026-11-25).
            self.navigate_to(Page::Messages);
        }

        egui::Frame::NONE
            .fill(palette_color(FIELD_BG))
            .stroke(egui::Stroke::new(1.0, palette_color(FIELD_BORDER)))
            .corner_radius(6)
            .inner_margin(2)
            .show(ui, |ui| {
                ui.horizontal_centered(|ui| {
                    ui.spacing_mut().item_spacing.x = 2.0;
                    ui.spacing_mut().interact_size.y = 22.0;
                    ui.spacing_mut().button_padding = egui::vec2(10.0, 3.0);
                    for (english, label) in [(true, "English"), (false, "中文")] {
                        let selected = ENGLISH_UI.get() == english;
                        let button = egui::Button::new(
                            egui::RichText::new(label)
                                .size(11.5)
                                .strong()
                                .color(if selected {
                                    palette_color(TEXT_MAIN)
                                } else {
                                    palette_color(TEXT_WEAK)
                                }),
                        )
                        .fill(if selected {
                            palette_color(PRESSED_BG)
                        } else {
                            egui::Color32::TRANSPARENT
                        })
                        .stroke(egui::Stroke::NONE)
                        .corner_radius(6);
                        if ui.add(button).clicked() && !selected {
                            ENGLISH_UI.set(english);
                            save_client_settings(
                                json!({ "locale": if english { "en-us" } else { "zh-hans" } }),
                            );
                            ui.ctx().request_repaint();
                        }
                    }
                });
            });
    }

    pub(super) fn workspace_search_box(&mut self, ui: &mut egui::Ui) {
        let width = (ui.ctx().content_rect().width() - 650.0).clamp(110.0, 250.0);
        let response = ui.add_sized(
            [width, 26.0],
            egui::TextEdit::singleline(&mut self.workspace_controls.search_text)
                .desired_width(width)
                .align(egui::Align2::LEFT_CENTER)
                .hint_text(tr("搜索页面、标签、维护人…")),
        );
        if std::mem::take(&mut self.workspace_controls.focus_search) {
            response.request_focus();
        }
        if (response.has_focus() || response.lost_focus())
            && ui.input(|input| input.key_pressed(egui::Key::Enter))
        {
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
        if self.user.is_some() || self.local.is_some() {
            match self.call_work(
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
        // F7: with a local library open, the call above is
        // answered by the local face alone, so the organization's
        // full-text hits never arrived. While connected, merge
        // the server's hits in too (the server applies its own
        // visibility filtering), labelled as organization results
        // so their source is explicit; local hits win id ties.
        // With no local library the call above already read the
        // server and this pass deduplicates to a no-op.
        if self.org_online() {
            if let Ok(response) = self.call(
                "GET",
                &format!("/v1/search?q={}", query_component(query)),
                None,
            ) {
                for hit in response["hits"].as_array().into_iter().flatten() {
                    let unit = &hit["unit"];
                    let id = s(unit, "id");
                    if !id.is_empty() && units.insert(id.clone()) {
                        hits.push((
                            format!("{}（组织）", s(unit, "name")),
                            SearchTarget::Unit(id),
                        ));
                    }
                }
            }
        }
        self.workspace_controls.search_hits = hits;
        self.workspace_controls.search_open = true;
    }

    pub(super) fn workspace_control_windows(&mut self, ctx: &egui::Context) {
        if self.workspace_controls.account_menu.is_open() {
            let display_name = self
                .user
                .as_ref()
                .map(|user| s(user, "display_name"))
                .filter(|name| !name.is_empty())
                .unwrap_or_else(|| {
                    if self.user.is_some() {
                        s(self.user.as_ref().unwrap(), "username")
                    } else {
                        tr("匿名账户").to_string()
                    }
                });
            let username = self
                .user
                .as_ref()
                .map(|user| s(user, "username"))
                .unwrap_or_default();
            let role = if self.user.is_some() {
                self.role()
            } else {
                String::new()
            };
            let server = self.active_server_label();
            let email = if self.user.is_some() {
                self.settings.profile.get("email").cloned().unwrap_or_default()
            } else {
                String::new()
            };
            let phone = if self.user.is_some() {
                self.settings.profile.get("phone").cloned().unwrap_or_default()
            } else {
                String::new()
            };
            let entry = AccountMenuEntry {
                name: &display_name,
                role: &role,
                username: &username,
                email: &email,
                phone: &phone,
                server: &server,
                signed_in: self.user.is_some(),
            };
            let controls = &mut self.workspace_controls;
            if let Some(action) =
                controls
                    .account_menu
                    .show(ctx, egui::Id::unique("workspace-account-menu"), &entry)
            {
                controls.account_menu.close();
                match action {
                    AccountMenuAction::OpenProfile => {
                        if let Some(section) = self
                            .settings_sections()
                            .iter()
                            .position(|label| *label == "个人资料")
                        {
                            self.navigate_to(Page::Settings);
                            self.settings.section = section;
                        }
                    }
                    AccountMenuAction::Logout => self.logout(),
                    AccountMenuAction::Login => self.login_open = true,
                }
            }
        }
        if self.workspace_controls.server_menu.is_open() {
            let entries: Vec<_> = self
                .conns
                .iter()
                .enumerate()
                .map(|(index, conn)| {
                    let detail = if conn.anon {
                        format!("{} · 匿名", conn.url)
                    } else if conn.username.is_empty() {
                        conn.url.clone()
                    } else {
                        format!("{} · {}", conn.url, conn.username)
                    };
                    let (status, color) = match conn.status {
                        ConnStatus::Connected => ("已连接", palette_color(CHIP_GREEN_FG)),
                        ConnStatus::Anonymous => ("匿名连接", ACCENT),
                        ConnStatus::Failed => ("连接失败", palette_color(ROSE)),
                        ConnStatus::LoggedOut if conn.anon => {
                            ("匿名已断开", palette_color(TEXT_WEAK))
                        }
                        ConnStatus::LoggedOut => ("未登录", palette_color(TEXT_WEAK)),
                    };
                    ServerMenuEntry {
                        name: &conn.name,
                        detail,
                        status,
                        status_color: color,
                        active: self.active == Some(index),
                    }
                })
                .collect();
            let controls = &mut self.workspace_controls;
            if let Some(action) =
                controls
                    .server_menu
                    .show(ctx, egui::Id::unique("workspace-server-menu"), &entries)
            {
                controls.server_menu.close();
                match action {
                    ServerMenuAction::AddServer => {
                        self.navigate_to(Page::Settings);
                        self.settings.section = 0;
                    }
                    ServerMenuAction::Select(index) => self.activate(index),
                    ServerMenuAction::Disconnect(index) => self.disconnect_conn(index),
                }
            }
        }
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
    }

    /// The message center (full version, user ruling
    /// 2026-11-25): renders the LOCAL MIRROR list (App.messages)
    /// filtered by the sidebar directory's kind selection. An
    /// unread card wears its kind's frame color over the kind's
    /// soft fill; clicking the card marks it read in the mirror
    /// and the frame disappears (per-card read — opening the
    /// page marks nothing). The per-card 删除 button removes
    /// the local row (server-origin rows are tombstoned in
    /// core, so a later pull cannot resurrect them). Actions
    /// are collected during the render and applied after it,
    /// the dialog pattern.
    pub(super) fn page_messages(&mut self, ui: &mut egui::Ui) {
        let list: Vec<Value> = self
            .messages
            .iter()
            .filter(|m| match &self.msg_filter {
                None => true,
                Some(k) => s(m, "kind") == *k,
            })
            .cloned()
            .collect();
        let mut read_id: Option<String> = None;
        let mut delete_id: Option<String> = None;
        egui::Frame::NONE
            .inner_margin(egui::Margin::symmetric(22, 20))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.label(
                    egui::RichText::new(tr("消息中心"))
                        .size(20.0)
                        .strong()
                        .color(palette_color(TEXT_MAIN)),
                );
                ui.add_space(12.0);
                if list.is_empty() {
                    return;
                }
                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        for message in &list {
                            let id = s(message, "id");
                            let kind = s(message, "kind");
                            let unread = i(message, "read") == 0;
                            let (fill, stroke) = if unread {
                                (
                                    palette_color(msg_kind_soft(&kind)),
                                    egui::Stroke::new(
                                        1.5,
                                        palette_color(msg_kind_color(&kind)),
                                    ),
                                )
                            } else {
                                (
                                    palette_color(CARD_BG),
                                    egui::Stroke::new(1.0, palette_color(CARD_BORDER)),
                                )
                            };
                            let frame = egui::Frame::NONE
                                .fill(fill)
                                .stroke(stroke)
                                .corner_radius(6)
                                .inner_margin(egui::Margin::symmetric(16, 12))
                                .show(ui, |ui| {
                                    ui.set_width(ui.available_width());
                                    ui.horizontal(|ui| {
                                        ui.label(
                                            egui::RichText::new(s(message, "title"))
                                                .size(14.0)
                                                .strong()
                                                .color(palette_color(TEXT_MAIN)),
                                        );
                                        ui.with_layout(
                                            egui::Layout::right_to_left(egui::Align::Center),
                                            |ui| {
                                                if ui
                                                    .add(
                                                        egui::Button::new(
                                                            egui::RichText::new(tr("删除"))
                                                                .size(11.5)
                                                                .color(palette_color(
                                                                    TEXT_WEAK,
                                                                )),
                                                        )
                                                        .frame(false),
                                                    )
                                                    .clicked()
                                                {
                                                    delete_id = Some(id.clone());
                                                }
                                                ui.label(
                                                    egui::RichText::new(msg_age(i(
                                                        message,
                                                        "created_ms",
                                                    )))
                                                    .size(11.5)
                                                    .color(palette_color(TEXT_WEAK)),
                                                );
                                                ui.label(
                                                    egui::RichText::new(msg_kind_label(&kind))
                                                        .size(11.5)
                                                        .color(palette_color(msg_kind_color(
                                                            &kind,
                                                        ))),
                                                );
                                            },
                                        );
                                    });
                                    let body = s(message, "body");
                                    if !body.is_empty() {
                                        ui.add_space(6.0);
                                        ui.label(
                                            egui::RichText::new(body)
                                                .size(12.5)
                                                .color(palette_color(TEXT_WEAK)),
                                        );
                                    }
                                });
                            if unread
                                && frame
                                    .response
                                    .interact(egui::Sense::click())
                                    .clicked()
                            {
                                read_id = Some(id.clone());
                            }
                            ui.add_space(14.0);
                        }
                    });
            });
        if let Some(id) = delete_id {
            self.msg_delete(&id);
        } else if let Some(id) = read_id {
            self.msg_mark_read(&id);
        }
    }
}

/// Wall-clock age for a message row (created_ms): the mirror
/// stores real epoch milliseconds, unlike the old in-memory
/// history's egui-runtime seconds. Older than a day, the date
/// itself is shown (YYYY-MM-DD, UTC+8 — the product's locale).
fn msg_age(created_ms: i64) -> String {
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(created_ms);
    let seconds = ((now_ms - created_ms) / 1000).max(0);
    match seconds {
        0..=59 => tr("刚刚").to_string(),
        60..=3599 => format!("{} {}", seconds / 60, tr("分钟前")),
        3600..=86399 => format!("{} {}", seconds / 3600, tr("小时前")),
        _ => {
            // Days-from-epoch → civil date (Howard Hinnant's
            // algorithm), shifted to UTC+8.
            let days = ((created_ms + 8 * 3600_000) / 86_400_000) + 719_468;
            let era = if days >= 0 { days } else { days - 146_096 } / 146_097;
            let doe = days - era * 146_097;
            let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
            let y = yoe + era * 400;
            let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
            let mp = (5 * doy + 2) / 153;
            let d = doy - (153 * mp + 2) / 5 + 1;
            let m = if mp < 10 { mp + 3 } else { mp - 9 };
            let y = if m <= 2 { y + 1 } else { y };
            format!("{y:04}-{m:02}-{d:02}")
        }
    }
}

//! Persistent desktop chrome and workspace slots shared by every top-level page.

use super::*;

const SIDEBAR_WIDTH: f32 = 268.0;

impl Page {
    const ALL: [Self; 6] = [
        Self::Home,
        Self::Showroom,
        Self::Index,
        Self::Content,
        Self::Trash,
        Self::Settings,
    ];

    pub(super) fn label(self) -> &'static str {
        match self {
            Self::Home => "主页",
            Self::Showroom => "演示厅",
            Self::Content => "内容页",
            Self::Index => "索引页",
            Self::Trash => "回收站",
            Self::Settings => "设置",
        }
    }

    pub(super) fn icon(self) -> ChromeIcon {
        match self {
            Self::Home => ChromeIcon::Home,
            Self::Showroom => ChromeIcon::Showroom,
            Self::Index => ChromeIcon::Index,
            Self::Content => ChromeIcon::Content,
            Self::Trash => ChromeIcon::Trash,
            Self::Settings => ChromeIcon::Settings,
        }
    }

}

impl App {
    pub(super) fn workspace_ui(&mut self, ui: &mut egui::Ui) {
        self.shell_chrome(ui);
        self.shell_sidebar(ui);
        self.page_footer(ui);
        egui::CentralPanel::default()
            .frame(egui::Frame::NONE.fill(palette_color(APP_BG)))
            .show(ui, |ui| {
                ui.push_id(self.page.label(), |ui| match self.page {
                    Page::Home => self.page_home(ui),
                    Page::Showroom => self.sr_main_body(ui),
                    Page::Content => self.ct_main_body(ui),
                    Page::Index => self.ix_main_body(ui),
                    Page::Trash => self.page_trash(ui),
                    Page::Settings => self.st_main_body(ui),
                });
            });
        self.workspace_control_windows(ui.ctx());
    }

    pub(super) fn navigate_to(&mut self, page: Page) {
        self.page = page;
        self.deep_source = None;
        self.review.jump_source = None;
    }

    fn shell_sidebar(&mut self, ui: &mut egui::Ui) {
        let page = self.page;
        egui::Panel::left("workspace_sidebar")
            .exact_size(SIDEBAR_WIDTH)
            .resizable(false)
            .frame(egui::Frame::NONE.fill(palette_color(PANEL_BG)))
            .show(ui, |ui| {
                let action_height = match page {
                    Page::Home => 166.0,
                    Page::Index => 90.0,
                    Page::Showroom | Page::Content | Page::Trash => 52.0,
                    Page::Settings => 0.0,
                };
                if action_height > 0.0 {
                    egui::Panel::bottom("sidebar_actions")
                        .exact_size(action_height)
                        .frame(
                            egui::Frame::NONE
                                .fill(palette_color(PANEL_BG))
                                .inner_margin(egui::Margin::symmetric(12, 8)),
                        )
                        .show(ui, |ui| {
                            ui.spacing_mut().item_spacing.y = 6.0;
                            self.sidebar_actions(ui, page);
                        });
                }
                egui::Frame::NONE.inner_margin(12).show(ui, |ui| {
                    egui::ScrollArea::vertical()
                        .id_salt(page.label())
                        .show(ui, |ui| {
                            ui.set_width(ui.available_width());
                            ui.spacing_mut().item_spacing.y = 6.0;
                            match page {
                                Page::Home => self.home_nav_body(ui),
                                Page::Showroom => self.sr_nav_body(ui),
                                Page::Content => self.ct_nav_body(ui),
                                Page::Index => self.ix_nav_body(ui),
                                Page::Trash => self.trash_nav_body(ui),
                                Page::Settings => self.st_nav_body(ui),
                            }
                        });
                });
            });
    }

    fn sidebar_actions(&mut self, ui: &mut egui::Ui, page: Page) {
        let actions: &[(&str, &str)] = match page {
            Page::Home => &[
                ("new", "新建索引"),
                ("sample", "打开示例项目"),
                ("import", "导入素材…"),
                ("template", "从模板新建"),
            ],
            Page::Index => &[("new", "新建索引")],
            _ => &[],
        };
        for &(key, label) in actions {
            let (fill, foreground) = if key == "new" {
                (ACCENT, egui::Color32::WHITE)
            } else {
                (palette_color(HOVER_BG), palette_color(TEXT_MAIN))
            };
            let button = egui::Button::new(egui::RichText::new(tr(label)).color(foreground))
                .fill(fill)
                .corner_radius(6);
            if ui.add_sized([ui.available_width(), 32.0], button).clicked() {
                self.run_home_action(Some(key));
            }
        }
        let can_add_folder = matches!(page, Page::Content | Page::Index)
            || (page == Page::Showroom && self.showroom.zone == "plaza");
        if can_add_folder
            && self.small_icon_button(ui, ChromeIcon::FolderPlus, tr("新建文件夹"))
        {
            self.folder_edit = Some(FolderEdit {
                target: None,
                parent: None,
                text: "新文件夹".into(),
            });
            self.folder_edit_focus = true;
        }
        if page == Page::Trash
            && btn_default_enabled(
                ui,
                self.token.is_some() && !self.trash.items.is_empty(),
                tr("清空回收站"),
            )
            .clicked()
        {
            self.confirm_empty_trash();
        }
    }

    fn shell_chrome(&mut self, ui: &mut egui::Ui) {
        // ==== Main-window chrome: menu bar 32 + toolbar 36 = 68px.
        // (User ruling 2026-11-24: no in-app title bar — the HTML
        // prototype's .titlebar was a web-page imitation of a
        // desktop title bar; the client window uses the OS title
        // bar, which already shows the app title and the window
        // controls. The version string lives in the About dialog
        // and the update section; the current page is marked in
        // the toolbar tabs.)
        // no status bar — the message channel floats as a toast.
        // The menu bar sits on --panel, the toolbar on --bg, each
        // closed by a 1px --line (drawn by hand — the panels' own
        // separator lines stay off).
        // ====
        // Row 1 — menu bar (.menubar-row): the text menus on the
        // left; on the right, the session cluster in the
        // prototype's slots — the server and the user as quiet
        // icon-btn text, sign-in as the .pill-primary, sign-out as
        // an icon tool.
        egui::Panel::top("menu_row")
            .exact_size(32.0)
            .frame(
                egui::Frame::NONE
                    .fill(palette_color(PANEL_BG))
                    .inner_margin(egui::Margin::symmetric(10, 0)),
            )
            .show_separator_line(false)
            .show(ui, |ui| {
                egui::MenuBar::new().ui(ui, |ui| {
                    ui.menu_button(egui::RichText::new(tr("文件")).size(12.0), |ui| {
                        ui.set_min_width(160.0);
                        if ui.button(tr("新建索引")).clicked() {
                            self.act_new_index();
                            ui.close();
                        }
                        if ui.button(tr("导入 PPT")).clicked() {
                            self.navigate_to(Page::Content);
                            self.act_import_ppt();
                            ui.close();
                        }
                        ui.separator();
                        if self.user.is_some() {
                            if ui.button(tr("退出登录")).clicked() {
                                self.logout();
                                ui.close();
                            }
                        } else if ui.button(tr("登录")).clicked() {
                            self.login_open = true;
                            ui.close();
                        }
                        if ui.button(tr("退出")).clicked() {
                            ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
                            ui.close();
                        }
                    });
                    self.workspace_edit_menu(ui);
                    ui.menu_button(egui::RichText::new(tr("视图")).size(12.0), |ui| {
                        ui.set_min_width(160.0);
                        for page in Page::ALL {
                            if ui.button(tr(page.label())).clicked() {
                                self.navigate_to(page);
                                ui.close();
                            }
                        }
                    });
                    self.workspace_context_menus(ui);
                    ui.menu_button(egui::RichText::new(tr("帮助")).size(12.0), |ui| {
                        ui.set_min_width(160.0);
                        if ui.button(tr("检查更新")).clicked() {
                            self.check_lan_update(true);
                            ui.close();
                        }
                        if ui.button(tr("关于启幕")).clicked() {
                            self.about_open = true;
                            ui.close();
                        }
                    });
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        self.workspace_preferences(ui);
                        if self.user.is_some() {
                            if self.tool_slot(ui, false, tr("退出"), ChromeIcon::Power) {
                                self.logout();
                            }
                        } else if ui
                            .add(
                                egui::Button::new(
                                    egui::RichText::new(tr("登录"))
                                        .size(12.5)
                                        .color(egui::Color32::WHITE),
                                )
                                .fill(ACCENT)
                                .stroke(egui::Stroke::NONE)
                                .corner_radius(255)
                                .min_size(egui::vec2(0.0, 26.0)),
                            )
                            .clicked()
                        {
                            self.login_open = true;
                        }
                        ui.add_space(6.0);
                        let user_text = if self.user.is_some() {
                            let name = self
                                .user
                                .as_ref()
                                .map(|u| s(u, "display_name"))
                                .unwrap_or_default();
                            format!("{name} · {}", self.role())
                        } else if self.anon_active() {
                            format!("匿名 · 已连接 {}", self.active_server_label())
                        } else {
                            tr("未登录").to_string()
                        };
                        let user_label = egui::Label::new(egui::RichText::new(&user_text).size(12.0).color(palette_color(TEXT_WEAK)))
                            .truncate();
                        ui.add_sized([108.0, 24.0], user_label).on_hover_text(user_text);
                        ui.add_space(6.0);
                        // Current server: click cycles through the other
                        // connected servers, or opens 服务器 settings.
                        let srv_label = craft_i18n::fmt(
                            tr("服务器：{name}"),
                            &[("name", &self.active_server_label())],
                        );
                        if ui
                            .add_sized(
                                [if ui.ctx().content_rect().width() < 1120.0 { 92.0 } else { 156.0 }, 24.0],
                                egui::Button::new(
                                    egui::RichText::new(&srv_label).size(12.0).color(palette_color(TEXT_WEAK)),
                                )
                                .fill(egui::Color32::TRANSPARENT)
                                .stroke(egui::Stroke::NONE)
                                .corner_radius(6)
                                .truncate(),
                            )
                            .on_hover_text(srv_label)
                            .clicked()
                        {
                            let connected: Vec<usize> = (0..self.conns.len())
                                .filter(|&i| self.conns[i].status == ConnStatus::Connected)
                                .collect();
                            if connected.len() > 1 {
                                let cur = self.active.unwrap_or(usize::MAX);
                                let pos = connected.iter().position(|&i| i == cur).unwrap_or(0);
                                let next = connected[(pos + 1) % connected.len()];
                                self.activate(next);
                            } else {
                                self.navigate_to(Page::Settings);
                                self.settings.section = 0;
                            }
                        }
                    });
                });
                let r = ui.max_rect();
                ui.painter().hline(
                    r.x_range(),
                    r.max.y - 0.5,
                    egui::Stroke::new(1.0, palette_color(SEPARATOR)),
                );
            });
        // Prototype toolbar: page navigation, then context-specific tools.
        egui::Panel::top("tool_row")
            .exact_size(36.0)
            .show_separator_line(false)
            .frame(
                egui::Frame::NONE
                    .fill(palette_color(HEADER_BG))
                    .inner_margin(egui::Margin {
                        left: 10,
                        right: 10,
                        top: 0,
                        bottom: 0,
                    }),
            )
            .show(ui, |ui| {
                ui.horizontal_centered(|ui| {
                    ui.spacing_mut().item_spacing.x = 3.0;
                    for page in Page::ALL {
                        let tab = ChromeIcons.tool(ui, page.icon(), page.label(), true, self.page == page, false);
                        if page == Page::Index && tab.double_clicked() {
                            self.index.stack.pop_to(0);
                        }
                        if tab.clicked() {
                            self.navigate_to(page);
                        }
                    }
                    tool_vsep(ui);
                    self.workspace_search_box(ui);
                    // Page tool slots: only actions the page already
                    // has (the pages keep their in-body buttons; these
                    // call the same methods) — showroom refreshes,
                    // content imports, index creates.
                    match self.page {
                        Page::Showroom => {
                            if self.tool_slot(ui, false, tr("刷新"), ChromeIcon::Refresh) {
                                if self.user.is_some() {
                                    self.refresh_all();
                                } else {
                                    self.refresh_guest();
                                }
                            }
                        }
                        Page::Content => {
                            if self.tool_slot(ui, false, tr("智能分块导入"), ChromeIcon::Split)
                            {
                                self.act_import_split();
                            }
                            ui.add_space(2.0);
                            if self.tool_slot(ui, false, tr("导入 PPT"), ChromeIcon::Import) {
                                self.act_import_ppt();
                            }
                            ui.add_space(2.0);
                            if self.tool_slot(
                                ui,
                                false,
                                tr("粘贴新建"),
                                ChromeIcon::Clipboard,
                            ) {
                                self.create_unit_from_paste();
                            }
                        }
                        Page::Index => {
                            if self.tool_slot(ui, false, tr("新建索引"), ChromeIcon::NewIndex)
                            {
                                self.act_new_index();
                            }
                        }
                        Page::Home | Page::Trash => {}
                        Page::Settings => {}
                    }
                    self.index_toolbar(ui);
                });
                let r = ui.max_rect();
                ui.painter().hline(
                    r.x_range(),
                    r.max.y - 0.5,
                    egui::Stroke::new(1.0, palette_color(SEPARATOR)),
                );
            });
    }

    fn page_footer(&mut self, ui: &mut egui::Ui) {
        egui::Panel::bottom("deck_bar")
            .exact_size(46.0)
            .frame(egui::Frame::NONE.fill(palette_color(PANEL_BG)))
            .show_separator_line(false)
            .show(ui, |ui| {
                let r = ui.max_rect();
                ui.painter().hline(
                    r.x_range(),
                    r.min.y + 0.5,
                    egui::Stroke::new(1.0, palette_color(SEPARATOR)),
                );
                ui.horizontal_centered(|ui| {
                    ui.add_space(22.0);
                    let pid = match self.page {
                        Page::Home => "#1",
                        Page::Showroom => match self.showroom.zone.as_str() {
                            "library" => "#3",
                            "inbox" => "#4",
                            "groups" => "#5",
                            _ => "#2",
                        },
                        Page::Content => "#6",
                        Page::Index => "#7",
                        Page::Settings => "#8",
                        Page::Trash => "#9",
                    };
                    let (badge, _) =
                        ui.allocate_exact_size(egui::vec2(34.0, 26.0), egui::Sense::hover());
                    ui.painter().rect(
                        badge,
                        6.0,
                        palette_color(HOVER_BG),
                        egui::Stroke::new(1.0, palette_color(FIELD_BORDER)),
                        egui::StrokeKind::Inside,
                    );
                    ui.painter().text(
                        badge.center(),
                        egui::Align2::CENTER_CENTER,
                        pid,
                        egui::FontId::proportional(11.5),
                        palette_color(TEXT_WEAK),
                    );
                    if self.page == Page::Showroom
                        && matches!(self.showroom.stack.top(), SrFrame::Deck(_))
                    {
                        if self.tool_slot(ui, false, "返回上一层", ChromeIcon::Back) {
                            self.back_from_showroom_detail();
                        }
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.add_space(22.0);
                        for (label, url) in [
                            (
                                "内容索引规范",
                                "https://github.com/lanline123/qimu/blob/main/docs/产品规格.md",
                            ),
                            ("帮助文档", "https://github.com/lanline123/qimu#readme"),
                            ("项目主页", "https://github.com/lanline123/qimu"),
                        ] {
                            ui.hyperlink_to(
                                egui::RichText::new(tr(label)).size(11.5).color(palette_color(TEXT_WEAK)),
                                url,
                            );
                        }
                        ui.label(egui::RichText::new(tr("社区")).size(11.5).strong());
                    });
                });
            });
    }
}

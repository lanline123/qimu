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
                // Community foot (prototype .sp-foot): created
                // before the action panel so it hugs the sidebar's
                // bottom edge; the per-page actions stack above it.
                egui::Panel::bottom("sidebar_community")
                    .frame(egui::Frame::NONE.fill(palette_color(PANEL_BG)))
                    .show_separator_line(false)
                    .show(ui, |ui| {
                        let r = ui.max_rect();
                        ui.painter().hline(
                            r.x_range(),
                            r.min.y + 0.5,
                            egui::Stroke::new(1.0, palette_color(SEPARATOR)),
                        );
                        egui::Frame::NONE
                            .inner_margin(egui::Margin {
                                left: 14,
                                right: 14,
                                top: 10,
                                bottom: 12,
                            })
                            .show(ui, |ui| {
                                ui.spacing_mut().item_spacing.y = 2.0;
                                self.sidebar_community(ui);
                            });
                    });
                let action_height = match page {
                    Page::Home => 166.0,
                    Page::Index => 90.0,
                    Page::Content => 96.0,
                    Page::Showroom | Page::Trash => 52.0,
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

    /// Sidebar community foot (prototype .sp-foot): a small
    /// section title over three full-width icon links, pinned to
    /// the sidebar's bottom edge on every page. The links open the
    /// project's public pages in the browser (the prototype's
    /// click was a placeholder toast; the client opens the real
    /// addresses the old bottom-bar cluster used).
    fn sidebar_community(&mut self, ui: &mut egui::Ui) {
        ui.label(
            egui::RichText::new(tr("社区"))
                .size(10.5)
                .strong()
                .color(palette_color(TEXT_WEAK)),
        );
        ui.add_space(2.0);
        for (icon, label, url) in [
            (
                ChromeIcon::Doc,
                "帮助文档",
                "https://github.com/lanline123/qimu#readme",
            ),
            (
                ChromeIcon::Globe,
                "启幕 · 官网",
                "https://github.com/lanline123/qimu",
            ),
            (
                ChromeIcon::Link,
                "内容索引规范",
                "https://github.com/lanline123/qimu/blob/main/docs/产品规格.md",
            ),
        ] {
            self.community_link(ui, icon, label, url);
        }
    }

    /// One .sp-link row: 15px glyph + 12.5px label, muted at rest,
    /// accent on hover, the whole 28px row clickable.
    fn community_link(&mut self, ui: &mut egui::Ui, icon: ChromeIcon, label: &str, url: &str) {
        let (rect, resp) = ui.allocate_exact_size(
            egui::vec2(ui.available_width(), 28.0),
            egui::Sense::click(),
        );
        let hovered = resp.hovered();
        let text_color = if hovered {
            ACCENT
        } else {
            palette_color(TEXT_WEAK)
        };
        let glyph_color = if hovered {
            ACCENT
        } else {
            palette_color(ICON_TINT)
        };
        ChromeIcons.paint(
            ui,
            icon,
            egui::pos2(rect.min.x + 2.0 + 7.5, rect.center().y),
            15.0,
            glyph_color,
        );
        ui.painter().text(
            egui::pos2(rect.min.x + 2.0 + 15.0 + 9.0, rect.center().y),
            egui::Align2::LEFT_CENTER,
            tr(label),
            egui::FontId::proportional(12.5),
            text_color,
        );
        if resp.clicked() {
            ui.ctx().open_url(egui::OpenUrl::new_tab(url));
        }
        let _ = resp.on_hover_cursor(egui::CursorIcon::PointingHand);
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
        if page == Page::Content {
            let open = self.content.cache_open;
            let label = if open {
                tr("正在管理 · 点此返回")
            } else {
                tr("本地缓存")
            };
            let button = egui::Button::new(
                egui::RichText::new(label).color(egui::Color32::WHITE),
            )
            .fill(ACCENT)
            .corner_radius(6);
            if ui.add_sized([ui.available_width(), 32.0], button).clicked() {
                self.content.stack.pop_to(0);
                self.content.cache_open = !self.content.cache_open;
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
                // horizontal_centered allocates the panel's
                // full 32px and centers its child vertically:
                // MenuBar's own strip is only as tall as its
                // tallest control and would otherwise sit at
                // the top of the row, leaving every menu label
                // and right-cluster control riding high.
                ui.horizontal_centered(|ui| {
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

    /// Bottom-bar action group (prototype #deckBar .deckbar-acts):
    /// resident icon buttons, separator-grouped, present on every
    /// page; a button whose target does not exist in the current
    /// context is greyed and inert — the same availability model
    /// as the toolbar's two resident groups. The full-set ruling
    /// (2026-11-24: the bottom-bar buttons are the user's picked
    /// set, none may be dropped) keeps every prototype slot:
    /// slots whose feature is not built yet render as
    /// DECKBAR-PLACEHOLDER entries (see deckbar_placeholder) and
    /// light up one by one as their features land. Called inside
    /// the footer's right-to-left
    /// layout, so buttons are added right-to-left: sdClose first,
    /// sdShare last.
    fn deckbar_actions(&mut self, ui: &mut egui::Ui) {
        ui.spacing_mut().item_spacing.x = 6.0;
        // Snapshot of the open deck, cloned out of the frame: the
        // handlers below need &mut self.
        let (item, pages, from, view, cached) = match self.showroom.stack.top() {
            SrFrame::Deck(st) => {
                let cache_key = format!("sr_{}", s(&st.item, "id"));
                let cached = self
                    .cache_entries
                    .iter()
                    .any(|e| s(e, "key") == cache_key);
                (
                    Some(st.item.clone()),
                    Some(st.pages.clone()),
                    st.selected_page.unwrap_or(0),
                    st.view,
                    cached,
                )
            }
            _ => (None, None, 0, DeckView::Thumbnails, false),
        };
        let deck_open = item.is_some();
        // Editor snapshot for sdBlockSave / sdBlockPublish: the
        // two buttons exist only while the page editor is the
        // topmost overlay (prototype: hidden outside the editing
        // state), lit by the editor's own dirty flag — they do
        // not follow the group's grey state.
        let (editing, edit_dirty) = match self.overlays.last() {
            Some(Overlay::PageEdit(pe)) => (true, pe.dirty()),
            _ => (false, false),
        };
        // sdClose — the global back button. It keeps its own
        // availability and never follows the group's grey state.
        let back_label = self.deckbar_back_label();
        if ChromeIcons
            .tool(
                ui,
                ChromeIcon::Back,
                back_label.unwrap_or("返回上一层"),
                back_label.is_some(),
                false,
                true,
            )
            .clicked()
        {
            self.deckbar_back();
        }
        tool_vsep(ui);
        // sdPresent — the accent main button (the tool button's
        // selected form is its accent-filled state).
        if ChromeIcons
            .tool(ui, ChromeIcon::Play, "开始演示", deck_open, true, true)
            .clicked()
        {
            let (item, pages) = (item.clone().unwrap(), pages.clone().unwrap());
            if !self.present_via_deckcraft(
                &s(&item, "id"),
                &s(&item, "title"),
                DcMode::Play,
                from,
            ) {
                self.start_presentation(ui, &item, &pages);
            }
        }
        if ChromeIcons
            .tool(ui, ChromeIcon::Presenter, "演示者视图", deck_open, false, true)
            .clicked()
        {
            let (item, pages) = (item.clone().unwrap(), pages.clone().unwrap());
            if !self.present_via_deckcraft(
                &s(&item, "id"),
                &s(&item, "title"),
                DcMode::Presenter,
                from,
            ) {
                self.start_presentation(ui, &item, &pages);
            }
        }
        if ChromeIcons
            .tool(
                ui,
                ChromeIcon::Notes,
                "备注视图",
                deck_open,
                view == DeckView::Notes,
                true,
            )
            .clicked()
        {
            if let SrFrame::Deck(st) = self.showroom.stack.top_mut() {
                st.view = DeckView::Notes;
            }
        }
        if ChromeIcons
            .tool(
                ui,
                ChromeIcon::Thumbs,
                "缩略视图",
                deck_open,
                view == DeckView::Thumbnails,
                true,
            )
            .clicked()
        {
            if let SrFrame::Deck(st) = self.showroom.stack.top_mut() {
                st.view = DeckView::Thumbnails;
            }
        }
        if ChromeIcons
            .tool(ui, ChromeIcon::Reading, "普通视图", deck_open, false, true)
            .clicked()
        {
            let item = item.clone().unwrap();
            if !self.present_via_deckcraft(
                &s(&item, "id"),
                &s(&item, "title"),
                DcMode::Reading,
                from,
            ) {
                self.present_showroom(&s(&item, "id"), &s(&item, "title"));
            }
        }
        tool_vsep(ui);
        if ChromeIcons
            .tool(ui, ChromeIcon::Pdf, "导出 PDF", deck_open, false, true)
            .clicked()
        {
            let item = item.clone().unwrap();
            self.export_showroom_pdf(&item, "slides", None);
        }
        if ChromeIcons
            .tool(ui, ChromeIcon::Export, "导出 PPTX", deck_open, false, true)
            .clicked()
        {
            let item = item.clone().unwrap();
            self.export_showroom_pptx(&item);
        }
        // sdPreview — the #212 script screen. In the client that
        // screen is the deck's own 讲稿 (Notes) view — the deck
        // page's segmented 讲稿 tab — so the button lands there;
        // it carries no mode-selected state of its own.
        if ChromeIcons
            .tool(ui, ChromeIcon::Eye, "预览 / 讲稿视图", deck_open, false, true)
            .clicked()
        {
            if let SrFrame::Deck(st) = self.showroom.stack.top_mut() {
                st.view = DeckView::Notes;
            }
        }
        tool_vsep(ui);
        if editing {
            // DECKBAR-PLACEHOLDER sdBlockPublish — the save half
            // is real (sdBlockSave, below) but the publish half
            // has no truthful target from the editor: the
            // client's only publish-to-showroom path is
            // index-granular (push_index_to_showroom — POST
            // /v1/showroom with a source_index_id taken from the
            // index detail), the editor context carries no index
            // id (PageEditState holds a version id only), and the
            // server offers no unit/version-granular submission.
            // Placeholder until that link exists; the package
            // report declares the gap in full.
            self.deckbar_placeholder(
                ui,
                ChromeIcon::BlockPublish,
                "保存并发布到演示厅（建设中）",
            );
            // sdBlockSave — the page editor's own save driven
            // from the bar; one save implementation, not two
            // (see deckbar_save_block / App::save_page_edit).
            if ChromeIcons
                .tool(
                    ui,
                    ChromeIcon::BlockSave,
                    "保存到索引",
                    edit_dirty,
                    false,
                    true,
                )
                .clicked()
            {
                self.deckbar_save_block();
            }
        }
        // sdDel — wired only where a clean delete target with an
        // existing handler exists: the open content block (the
        // unit detail's own 删除内容块 path). Everywhere else it
        // greys with the group, as in the prototype.
        let unit_open = self.page == Page::Content
            && matches!(self.content.stack.top(), ContentFrame::Unit(_));
        if ChromeIcons
            .tool(
                ui,
                ChromeIcon::Trash,
                "删除选中的内容块",
                unit_open,
                false,
                true,
            )
            .clicked()
        {
            self.deckbar_delete_unit();
        }
        // DECKBAR-PLACEHOLDER — the PPT clipboard family and
        // 组合 (user ruling 2026-11-24: placeholders now, lit up
        // one by one as the features land; the clipboard feature
        // itself — capture / store / paste-as-blocks — is not
        // built yet). Registered here and in deckbar_placeholder;
        // grep DECKBAR-PLACEHOLDER for the full registry.
        self.deckbar_placeholder(
            ui,
            ChromeIcon::Combine,
            "组合：将选中的多个内容块组合（建设中）",
        );
        self.deckbar_placeholder(
            ui,
            ChromeIcon::Cut,
            "剪切：将选中的页剪切到剪贴板（建设中）",
        );
        self.deckbar_placeholder(
            ui,
            ChromeIcon::CopyPpt,
            "复制：将选中的页复制到剪贴板（建设中）",
        );
        self.deckbar_placeholder(
            ui,
            ChromeIcon::PasteGroup,
            "整组成块：剪贴板内容整组粘贴为一个内容块（建设中）",
        );
        self.deckbar_placeholder(
            ui,
            ChromeIcon::PasteSplit,
            "逐页成块：剪贴板每页各成一个内容块（建设中）",
        );
        tool_vsep(ui);
        // sdCache — the prototype's sdPull slot (T6 ruling
        // 2026-11-24: the slot is 缓存, not 收进我的 — the
        // prototype itself was wrong). Accent-filled while the
        // open deck sits in the local cache (the same cache-entry
        // probe the deck header uses); clicking (re)caches through
        // the deck header's own cache handler.
        if ChromeIcons
            .tool(ui, ChromeIcon::Cache, "缓存到本地", deck_open, cached, true)
            .clicked()
        {
            let item = item.clone().unwrap();
            self.cache_showroom_deck(&item);
        }
        // sdShare — group head (T7 ruling 2026-11-24: kept).
        // Sharing in the current phase is export-style only, so
        // the button produces the deck's share file through the
        // same export handler as the deck header's 导出.
        if ChromeIcons
            .tool(ui, ChromeIcon::Share, "分享给…", deck_open, false, true)
            .clicked()
        {
            let item = item.clone().unwrap();
            self.export_showroom_pptx(&item);
        }
    }

    /// DECKBAR-PLACEHOLDER — a bottom-bar slot whose feature is
    /// not built yet. The full-set ruling (2026-11-24) keeps
    /// every slot of the user's picked set: a placeholder renders
    /// in the group's disabled form (muted glyph, no fill) with a
    /// tooltip that states the slot's purpose, and lights up by
    /// replacing its deckbar_placeholder call with the real
    /// handler when the feature lands. It is painted by hand
    /// rather than through the disabled tool button because a
    /// disabled scope swallows hover — the tooltip must still
    /// work. Registry: sdPasteSplit, sdPasteGroup, sdCopyPpt,
    /// sdCutPpt, sdCombine, sdBlockPublish (in deckbar_actions).
    fn deckbar_placeholder(&mut self, ui: &mut egui::Ui, icon: ChromeIcon, label: &str) {
        let (rect, resp) =
            ui.allocate_exact_size(egui::vec2(30.0, 28.0), egui::Sense::hover());
        let glyph = palette_color(TEXT_WEAK).gamma_multiply(0.5);
        ChromeIcons.paint(ui, icon, rect.center(), 16.0, glyph);
        resp.on_hover_text(tr(label));
    }

    /// sdBlockSave: run the page editor's own save from the
    /// bottom bar. The editor overlay owns the edit state; the
    /// save itself is App::save_page_edit — the identical code
    /// path as the editor footer's 保存并发布新版本 — so the bump
    /// rule, payload and follow-up refresh exist exactly once.
    /// The overlay is taken out of the stack for the call (the
    /// save needs &mut App while the state borrows it) and put
    /// back when the editor must stay open.
    fn deckbar_save_block(&mut self) {
        let Some(pos) = self
            .overlays
            .iter()
            .rposition(|o| matches!(o, Overlay::PageEdit(_)))
        else {
            return;
        };
        if !matches!(self.overlays.last(), Some(Overlay::PageEdit(_))) {
            return; // a confirm dialog sits over the editor
        }
        let overlay = self.overlays.remove(pos);
        let Overlay::PageEdit(pe) = &overlay else {
            self.overlays.insert(pos, overlay);
            return;
        };
        if !pe.dirty() || !self.save_page_edit(pe) {
            self.overlays.insert(pos, overlay);
        }
    }

    /// sdClose's label for the current state (prototype
    /// updateBarAvail): None on the home page, where the button
    /// greys out entirely.
    fn deckbar_back_label(&self) -> Option<&'static str> {
        match self.page {
            Page::Home => None,
            Page::Showroom => match self.showroom.stack.top() {
                SrFrame::Deck(st) => Some(if st.preview_open {
                    "返回稿件详情"
                } else {
                    "关闭条目详情"
                }),
                SrFrame::Wall => Some("回主页"),
            },
            Page::Content => match self.content.stack.top() {
                ContentFrame::Unit(_) => Some("返回上一层"),
                ContentFrame::Wall(_) => Some("回主页"),
            },
            Page::Index => match self.index.stack.top() {
                IndexFrame::Overview(_) => Some("回主页"),
                _ => Some("返回上一层"),
            },
            Page::Trash => match self.trash.stack.top() {
                TrashFrame::Preview(_) => Some("返回上一层"),
                TrashFrame::List(_) => Some("回主页"),
            },
            Page::Settings => {
                if self
                    .settings_sections()
                    .get(self.settings.section)
                    .copied()
                    == Some("审批队列")
                    && matches!(self.review.stack.top(), ReviewFrame::Detail(_))
                {
                    Some("返回上一层")
                } else {
                    Some("回主页")
                }
            }
        }
    }

    /// sdClose's action: each page's own existing back path —
    /// never a second mechanism. Drill frames close through the
    /// same flags/pops their in-page back controls use; at a
    /// page's root frame the button returns home (the prototype's
    /// 回主页 state).
    fn deckbar_back(&mut self) {
        match self.page {
            Page::Home => {}
            Page::Showroom => {
                if matches!(self.showroom.stack.top(), SrFrame::Deck(_)) {
                    self.back_from_showroom_detail();
                } else {
                    self.navigate_to(Page::Home);
                }
            }
            Page::Content => {
                if let ContentFrame::Unit(st) = self.content.stack.top_mut() {
                    st.closed = true;
                    self.refresh_all();
                } else {
                    self.navigate_to(Page::Home);
                }
            }
            Page::Index => {
                // Tag first: acting on the frame must not happen
                // while a stack borrow is alive.
                let top = match self.index.stack.top() {
                    IndexFrame::Detail(_) => 1,
                    IndexFrame::Create(_) => 2,
                    IndexFrame::Overview(_) => 0,
                };
                match top {
                    1 => {
                        if let IndexFrame::Detail(st) = self.index.stack.top_mut() {
                            st.closed = true;
                        }
                    }
                    2 => {
                        if let IndexFrame::Create(st) = self.index.stack.top_mut() {
                            st.closed = true;
                        }
                    }
                    _ => self.navigate_to(Page::Home),
                }
            }
            Page::Trash => {
                if matches!(self.trash.stack.top(), TrashFrame::Preview(_)) {
                    self.trash.stack.pop();
                } else {
                    self.navigate_to(Page::Home);
                }
            }
            Page::Settings => {
                if self
                    .settings_sections()
                    .get(self.settings.section)
                    .copied()
                    == Some("审批队列")
                    && matches!(self.review.stack.top(), ReviewFrame::Detail(_))
                {
                    self.review.stack.pop();
                } else {
                    self.navigate_to(Page::Home);
                }
            }
        }
    }

    /// sdDel's one wired context: delete the open content block
    /// through the unit detail's own path (trash_add + close).
    fn deckbar_delete_unit(&mut self) {
        let uid = match self.content.stack.top() {
            ContentFrame::Unit(st) => {
                let unit = st.detail.get("unit").cloned().unwrap_or(json!({}));
                s(&unit, "id")
            }
            _ => return,
        };
        if uid.is_empty() {
            return;
        }
        if let ContentFrame::Unit(st) = self.content.stack.top_mut() {
            st.closed = true;
        }
        self.trash_add("unit", &uid);
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
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.add_space(22.0);
                        self.deckbar_actions(ui);
                    });
                });
            });
    }
}

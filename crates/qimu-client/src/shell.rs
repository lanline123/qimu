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
            Self::Content => "页面",
            Self::Index => "索引页",
            Self::Trash => "回收站",
            Self::Settings => "设置",
            Self::Messages => "消息中心",
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
            Self::Messages => ChromeIcon::Bell,
        }
    }
}

/// One resolved page identity (see App::page_identity): the
/// number the chip shows, the page name its hover reads, and
/// the path chain the menu row prints. `no` is None for the
/// client-only pages the user has not numbered — those show
/// their name alone until he rules otherwise (provisional
/// default, phase 1).
pub(super) struct PageIdentity {
    pub(super) no: Option<String>,
    pub(super) name: String,
    pub(super) path: Vec<String>,
}

impl PageIdentity {
    fn new(no: Option<&str>, name: String, path: Vec<String>) -> Self {
        Self {
            no: no.map(str::to_owned),
            name,
            path,
        }
    }

    /// A dialog or client-only window takes the chip over by
    /// name alone: the number falls away, the path stays the
    /// underlying page's.
    fn name_only(&mut self, name: &str) {
        self.no = None;
        self.name = name.to_string();
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
                    Page::Messages => self.page_messages(ui),
                });
            });
        self.workspace_control_windows(ui.ctx());
        self.prototype_dialogs_ui(ui.ctx());
    }

    pub(super) fn navigate_to(&mut self, page: Page) {
        self.overlays.retain(|o| !matches!(o, Overlay::Light(_)));
        self.page = page;
        self.deep_source = None;
        self.review.jump_source = None;
        // Entering a board lands on its 全部 selection (user
        // ruling 2026-11-24): the showroom zones and the two
        // folder-filtered pages reset their sidebar selection.
        // Only the selection resets — each page's drill stack
        // and frame state survive page switches as before, and
        // returning from a drill-down restores its frame, so
        // this never fires mid-drill (drill returns pop frames,
        // they do not navigate). Deep jumps that set self.page
        // directly carry their own target and are exempt.
        match page {
            Page::Showroom => {
                self.showroom.folder = None;
                self.showroom.library_view = 0;
                self.showroom.share_source = 0;
            }
            Page::Index => {
                self.index.overview_folder = None;
            }
            Page::Content => {
                self.content.folder_filter = None;
                // Navigation never carries a pending wall
                // scroll: the reset above changes the selection
                // only (user ruling 2026-11-26, direction B).
                self.content.scroll_intent = None;
            }
            _ => {}
        }
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
                // Single-button pages size to the button (32px +
                // the panel's 16px of vertical margin + a breath):
                // taller boxes left dead space above the community
                // foot (user ruling 2026-11-24).
                let action_height = match page {
                    Page::Home => 166.0,
                    Page::Index => 52.0,
                    Page::Content => 52.0,
                    Page::Showroom | Page::Trash => 52.0,
                    // Message center: the 清除消息 button takes
                    // the same single-button sizing as the other
                    // one-button pages.
                    Page::Messages => 52.0,
                    Page::Settings => 0.0,
                };
                if action_height > 0.0 {
                    egui::Panel::bottom("sidebar_actions")
                        .exact_size(action_height)
                        // No separator line above the actions block
                        // (user ruling 2026-11-24, all sidebars): the
                        // only line here is the community foot's own.
                        .show_separator_line(false)
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
                                Page::Messages => self.msg_nav_body(ui),
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
        let (rect, resp) =
            ui.allocate_exact_size(egui::vec2(ui.available_width(), 28.0), egui::Sense::click());
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
            let button = egui::Button::new(egui::RichText::new(label).color(egui::Color32::WHITE))
                .fill(ACCENT)
                .corner_radius(6);
            if ui.add_sized([ui.available_width(), 32.0], button).clicked() {
                self.content.stack.pop_to(0);
                self.content.cache_open = !self.content.cache_open;
            }
        }
        if page == Page::Trash {
            // Aligned with the 本地缓存 sidebar action above (user
            // ruling 2026-11-24): same accent-filled form, white
            // label, radius 6, full-width 32px — not the neutral
            // btn_default form. The empty-trash gate keeps its old
            // semantics, just carried by add_enabled_ui instead of
            // btn_default_enabled.
            let button = egui::Button::new(
                egui::RichText::new(tr("清空回收站")).color(egui::Color32::WHITE),
            )
            .fill(ACCENT)
            .corner_radius(6);
            let resp = ui
                .add_enabled_ui(!self.trash.items.is_empty(), |ui| {
                    ui.add_sized([ui.available_width(), 32.0], button)
                })
                .inner;
            if resp.clicked() {
                self.confirm_empty_trash();
            }
        }
        if page == Page::Messages {
            // 清除消息 (user ruling 2026-11-25): the same
            // accent-filled sidebar action form as 清空回收站
            // above; it opens the three-mode clear dialog
            // (App::msg_clear_window). Greyed when there is
            // nothing to clear, by the same add_enabled_ui gate.
            let button = egui::Button::new(
                egui::RichText::new(tr("清除消息")).color(egui::Color32::WHITE),
            )
            .fill(ACCENT)
            .corner_radius(6);
            let resp = ui
                .add_enabled_ui(!self.messages.is_empty(), |ui| {
                    ui.add_sized([ui.available_width(), 32.0], button)
                })
                .inner;
            if resp.clicked() {
                self.msg_clear_open = true;
            }
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
        // The frame's one page-identity resolution: the menu-row
        // path text below reads from it, and so does the
        // deck-bar page chip (page_identity is the client's
        // single resolver — no site computes its own page
        // number or name).
        let crumb_path = self.page_identity().path.join(" · ");
        // Row 1 — menu bar (.menubar-row): the text menus on the
        // left; on the right, the session cluster in the
        // prototype's slots — the server and account as quiet
        // text controls, with account details and logout in a card.
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
                        })
                        .response
                        .on_hover_text(tr("文件"))
                        .on_disabled_hover_text(tr("文件"));
                        self.workspace_edit_menu(ui);
                        ui.menu_button(egui::RichText::new(tr("视图")).size(12.0), |ui| {
                            ui.set_min_width(160.0);
                            for page in Page::ALL {
                                if ui.button(tr(page.label())).clicked() {
                                    self.navigate_to(page);
                                    ui.close();
                                }
                            }
                        })
                        .response
                        .on_hover_text(tr("视图"))
                        .on_disabled_hover_text(tr("视图"));
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
                        })
                        .response
                        .on_hover_text(tr("帮助"))
                        .on_disabled_hover_text(tr("帮助"));
                        // 申请公开 (user ruling 2026-11-27): the
                        // menu bar's entry to the organization
                        // chain — the only publish action that
                        // needs the server. Always clickable;
                        // the push dialog and the submission
                        // chain behind it carry the gates
                        // (online / identity / pending package),
                        // and with no index in context the click
                        // prompts instead of acting.
                        if ui
                            .button(egui::RichText::new(tr("申请公开")).size(12.0))
                            .clicked()
                        {
                            self.apply_public();
                        }
                        // Page path text (prototype #crumbView):
                        // in the prototype it rides the title bar
                        // after the brand ("启幕 · 索引总览 ·
                        // 公司介绍 · 编辑"); the client dropped
                        // the in-app title bar (ruling 2026-11-24),
                        // so it is transposed into the menu row,
                        // after the menus — the menus keep their
                        // seats no matter how long the path gets.
                        // Display only this phase (clickable path
                        // is a phase-2 question).
                        ui.add_space(10.0);
                        ui.label(
                            egui::RichText::new(format!("启幕 · {crumb_path}"))
                                .size(12.0)
                                .color(palette_color(TEXT_WEAK)),
                        );
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            self.workspace_preferences(ui);
                            ui.add_space(6.0);
                            self.workspace_account_button(ui);
                            ui.add_space(6.0);
                            self.workspace_server_button(ui);
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
                        // No Settings tab in the tool row (user
                        // ruling 2026-11-24). Settings stays
                        // reachable through the View menu and the
                        // menu-row entries, which iterate Page::ALL
                        // unfiltered.
                        if page == Page::Settings {
                            continue;
                        }
                        let tab = ChromeIcons.tool(
                            ui,
                            page.icon(),
                            page.label(),
                            true,
                            self.page == page,
                            false,
                        );
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
                            // 智能分块导入 is the AI-analysis split
                            // (user ruling 2026-11-24): not wired
                            // yet, so the slot renders greyed and
                            // inert until the AI proposer lands.
                            // act_import_split / propose_blocks
                            // stay in place for that feature.
                            self.tool_slot_placeholder(
                                ui,
                                tr("智能分块导入（AI 即将上线）"),
                                ChromeIcon::Split,
                            );
                            ui.add_space(2.0);
                            if self.tool_slot(ui, false, tr("导入 PPT"), ChromeIcon::Import) {
                                self.act_import_ppt();
                            }
                            ui.add_space(2.0);
                            if self.tool_slot(ui, false, tr("粘贴新建"), ChromeIcon::Clipboard)
                            {
                                self.create_unit_from_paste();
                            }
                        }
                        Page::Index => {
                            if self.tool_slot(ui, false, tr("新建索引"), ChromeIcon::NewIndex) {
                                self.act_new_index();
                            }
                        }
                        Page::Home | Page::Trash | Page::Messages => {}
                        Page::Settings => {}
                    }
                    self.index_toolbar(ui);
                    // Lazy import split status (2026-11-25):
                    // renders only while a deck is still
                    // splitting — progress + the turbo toggle —
                    // and removes itself when the last deck
                    // finishes (see App::lazy_status_slot).
                    self.lazy_status_slot(ui);
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
    /// placeholder entries (see deckbar_placeholder) and
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
        // View target (views unification, user ruling
        // 2026-11-25): the bottom-bar views （播放 / 普通视图 /
        // 演示者视图） are ONE action — "open an index" — so
        // they light on any detail whose object resolves to at
        // least one page, not only on an open showroom deck: the
        // open index detail (its branch head's resolved page
        // count) and the open content block (the version its
        // pages wall is showing) join the showroom deck here.
        // The tabs beside the trio （备注视图 / 缩略视图 /
        // 预览·讲稿） keep the deck frame as their target — they
        // re-slice an open deck, they do not open anything.
        enum BarViewTarget {
            Showroom,
            Index,
            Unit {
                title: String,
                version: Value,
                from: usize,
            },
        }
        let view_target: Option<BarViewTarget> = if deck_open {
            Some(BarViewTarget::Showroom)
        } else if self.page == Page::Index {
            match self.index.stack.top() {
                IndexFrame::Detail(d) => {
                    let pages: i64 = d
                        .detail
                        .get("resolved_entries")
                        .and_then(Value::as_array)
                        .map(|entries| {
                            entries
                                .iter()
                                .map(|e| {
                                    let total = e
                                        .get("resolved")
                                        .map(|r| i(r, "page_count"))
                                        .unwrap_or(0);
                                    match e
                                        .get("entry")
                                        .and_then(|en| en.get("excerpt_slots"))
                                        .and_then(Value::as_array)
                                    {
                                        Some(slots) if !slots.is_empty() => {
                                            (slots.len() as i64).min(total)
                                        }
                                        _ => total,
                                    }
                                })
                                .sum()
                        })
                        .unwrap_or(0);
                    (pages >= 1).then_some(BarViewTarget::Index)
                }
                _ => None,
            }
        } else if self.page == Page::Content {
            match self.content.stack.top() {
                ContentFrame::Unit(st) => {
                    super::content_detail::resolve_unit_bar_version(st).and_then(
                        |(t, non_latest)| {
                            let page_count = i(&t.version, "page_count");
                            if page_count < 1 {
                                return None;
                            }
                            let unit = st.detail.get("unit");
                            let name = unit.map(|u| s(u, "name")).unwrap_or_default();
                            // A picked history version or working draft names itself
                            // in the show title, so what is playing is never
                            // mistaken for the latest published version.
                            let title = if non_latest {
                                let ver_tag = if s(&t.version, "status") == "working"
                                    || s(&t.version, "semver").is_empty()
                                {
                                    "草稿".to_string()
                                } else {
                                    format!("v{}", s(&t.version, "semver"))
                                };
                                format!("{name} · {ver_tag}")
                            } else {
                                name
                            };
                            Some(BarViewTarget::Unit {
                                title,
                                version: t.version,
                                from: st
                                    .selected_page
                                    .unwrap_or(0)
                                    .min(page_count as usize - 1),
                            })
                        },
                    )
                }
                _ => None,
            }
        } else {
            None
        };
        let views_open = view_target.is_some();
        // Editor snapshot for sdBlockSave / sdBlockPublish: the
        // two buttons exist only while the page editor is the
        // topmost overlay (prototype: hidden outside the editing
        // state), lit by the editor's own dirty flag — they do
        // not follow the group's grey state.
        let (editing, edit_dirty) = match self.overlays.last() {
            Some(Overlay::PageEdit(pe)) => (true, pe.dirty()),
            _ => (false, false),
        };
        // PPT clipboard snapshot: the content wall's selection
        // drives copy / cut / combine / ungroup and the
        // clear-selection slot, the clipboard's own content
        // drives the two pastes. (The wall once had its own
        // selection strip; the 2026-11-25 consolidation ruling
        // removed it — this group is the one entry point.) The
        // wall target set: the ctrl-click multi set when
        // non-empty, else the anchor selection as a one-block
        // set. Copy targets, in order: the wall selection, the
        // open deck, a plaza-selected deck
        // (prototype: 演示厅复制选中整套稿的全部页).
        let (wall_targets, wall_multi): (Vec<String>, Vec<String>) = if self.page == Page::Content {
            match self.content.stack.top() {
                ContentFrame::Wall(w) => {
                    let targets = if !w.multi.is_empty() {
                        w.multi.clone()
                    } else if let Some(sel) = &w.selected {
                        vec![sel.clone()]
                    } else {
                        Vec::new()
                    };
                    (targets, w.multi.clone())
                }
                ContentFrame::Unit(st) => {
                    let targets = if st.unit_id.is_empty() {
                        Vec::new()
                    } else {
                        vec![st.unit_id.clone()]
                    };
                    (targets, Vec::new())
                }
            }
        } else {
            (Vec::new(), Vec::new())
        };
        // Index-detail entry selection (user ruling
        // 2026-11-27): while the index detail is the open
        // frame, its entry wall's effective selection is a
        // deck-bar target set of its own — the count, clear,
        // delete, cut and copy slots act on it.
        let entry_targets: Vec<usize> = if self.page == Page::Index {
            self.idx_entry_targets()
        } else {
            Vec::new()
        };
        let has_clip = self.ppt_clip.is_some();
        let can_combine = wall_multi.len() >= 2;
        // 解除组合 target: exactly one effective wall target
        // (the multi set, or the anchor selection as a one-block
        // set — the same target set copy/cut act on) whose
        // current version runs to more than one page. A
        // multi-selection of two or more, a single-page block,
        // or a block with no readable version all grey it.
        let ungroup_target: Option<String> = if matches!(self.content.stack.top(), ContentFrame::Wall(_))
            && wall_targets.len() == 1
        {
            let uid = wall_targets[0].clone();
            match self.unit_latest_page_count(&uid) {
                Some(n) if n > 1 => Some(uid),
                _ => None,
            }
        } else {
            None
        };
        let plaza_item: Option<Value> =
            if self.page == Page::Showroom && self.showroom.zone == "plaza" {
                self.showroom.selected_card.as_ref().and_then(|id| {
                    self.showroom_live
                        .iter()
                        .find(|it| s(it, "id") == *id)
                        .cloned()
                })
            } else {
                None
            };
        // sdInfo target (T5): the same finished-deck targets
        // the group acts on — the open deck first (with its
        // loaded page count), else the plaza-selected deck
        // (not loaded: no page count). No target, no light:
        // the button greys with the group, exactly as the
        // prototype's syncBarState rules sdInfo (lit only when
        // a viewable object exists).
        let info_target: Option<(Value, Option<usize>)> = match (&item, &pages) {
            (Some(it), Some(pgs)) => Some((
                it.clone(),
                if pgs.is_empty() { None } else { Some(pgs.len()) },
            )),
            _ => plaza_item.clone().map(|it| (it, None)),
        };
        let copy_deck: Option<Value> = item.clone().or(plaza_item);
        // sdClose — the global back button. It keeps its own
        // availability and never follows the group's grey state.
        // Its glyph is the redrawn back arrow in the theme
        // accent (user ruling 2026-11-26, this button only —
        // tool_accent); availability, label and action are the
        // plain tool button's.
        let back_label = self.deckbar_back_label();
        if ChromeIcons
            .tool_accent(
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
        if self.page == Page::Content && self.content_light_open() {
            if ChromeIcons
                .tool(ui, ChromeIcon::Doc, "编辑这一页", true, false, true)
                .clicked()
            {
                self.open_light_editor();
            }
        }
        tool_vsep(ui);
        // sdPresent — the accent main button (the tool button's
        // selected form is its accent-filled state). The trio
        // dispatches on the view target: the showroom deck keeps
        // its exact path (pinned export → the unified opener,
        // built-in presenter behind it); an index or a content
        // block resolves through its own resolver into the same
        // opener (present_index_views / present_unit_version).
        if ChromeIcons
            .tool(ui, ChromeIcon::Play, "开始演示", views_open, true, true)
            .clicked()
        {
            match &view_target {
                Some(BarViewTarget::Showroom) => {
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
                Some(BarViewTarget::Unit {
                    title,
                    version,
                    from,
                }) => {
                    let (title, version, from) =
                        (title.clone(), version.clone(), *from);
                    self.present_unit_version(&title, &version, DcMode::Play, from);
                }
                Some(BarViewTarget::Index) => {
                    self.present_index_views(DcMode::Play);
                }
                None => {}
            }
        }
        if ChromeIcons
            .tool(
                ui,
                ChromeIcon::Presenter,
                "演示者视图",
                views_open,
                false,
                true,
            )
            .clicked()
        {
            match &view_target {
                Some(BarViewTarget::Showroom) => {
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
                Some(BarViewTarget::Unit {
                    title,
                    version,
                    from,
                }) => {
                    let (title, version, from) =
                        (title.clone(), version.clone(), *from);
                    self.present_unit_version(&title, &version, DcMode::Presenter, from);
                }
                Some(BarViewTarget::Index) => {
                    self.present_index_views(DcMode::Presenter);
                }
                None => {}
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
            .tool(ui, ChromeIcon::Reading, "普通视图", views_open, false, true)
            .clicked()
        {
            match &view_target {
                Some(BarViewTarget::Showroom) => {
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
                Some(BarViewTarget::Unit {
                    title,
                    version,
                    from,
                }) => {
                    let (title, version, from) =
                        (title.clone(), version.clone(), *from);
                    self.present_unit_version(&title, &version, DcMode::Reading, from);
                }
                Some(BarViewTarget::Index) => {
                    self.present_index_views(DcMode::Reading);
                }
                None => {}
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
        // sdPreview — the #212 script screen (prototype
        // presentDlg): a full-page 讲稿视图 pushed over the deck
        // frame, NOT the deck's in-detail 讲稿 tier — that tier
        // is the 备注视图 button's target (prototype sdDeckNotes)
        // and the detail's own segmented control. The present
        // frame carries its own header actions, so this button
        // has no mode-selected state of its own.
        if ChromeIcons
            .tool(
                ui,
                ChromeIcon::Eye,
                "预览 / 讲稿视图",
                deck_open,
                false,
                true,
            )
            .clicked()
        {
            self.open_showroom_present();
        }
        tool_vsep(ui);
        if editing {
            // sdBlockPublish — the editor's save plus the
            // publish half (deckbar_publish_block): when the
            // saved block feeds exactly one local index whose
            // published entries resolve it to the version the
            // save just produced, that index is submitted
            // through the P3 chain. Lit by the same dirty flag
            // as sdBlockSave (the editor snapshot above): the
            // button is the save+publish combo, and
            // re-publishing an already-saved version stays with
            // the index detail's push dialog.
            if ChromeIcons
                .tool(
                    ui,
                    ChromeIcon::BlockPublish,
                    "保存并申请公开",
                    edit_dirty,
                    false,
                    true,
                )
                .clicked()
            {
                self.deckbar_publish_block();
            }
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
        if !editing && self.page == Page::Index {
            // sdBlockPublish in the index context (user
            // rulings 2026-11-27): while an index page is
            // showing, the button is ALWAYS clickable here —
            // it runs the LOCAL submit （提交到演示）: the
            // detail submits its own index, the overview its
            // selected index, and an overview with nothing
            // selected prompts instead of greying
            // (button-state standard: clickable + prompt beats
            // disabled). The organization chain lives in the
            // menu bar's 申请公开， not here.
            if ChromeIcons
                .tool(
                    ui,
                    ChromeIcon::BlockPublish,
                    "提交到演示",
                    true,
                    false,
                    true,
                )
                .clicked()
            {
                self.deckbar_submit_indexes();
            }
        }
        // The selection / PPT-clipboard group (2026-11-25
        // consolidation ruling): the content wall's own
        // selection strip is gone — this group is the only
        // entry for the selection count, clear-selection,
        // delete, the two combines, ungroup, cut / copy and
        // the two pastes, all calling the same clip_* methods
        // the strip called. Availability follows the
        // prototype's syncBarState table, adapted to the
        // client's clipboard: the pastes light while the
        // clipboard holds pages, copy lights on any copy
        // target (it does not follow a group grey-out), cut
        // is wall-only (a showroom deck is not your source),
        // the combines need a wall multi-selection of two or
        // more, ungroup needs exactly one multi-page target.
        // Cut and the combines clear the multi set afterwards.
        // The bar lays out right-to-left, so the slots are
        // added in reverse of their reading order — read
        // visually (left to right) the group is: 已选计数 ·
        // 清空选择 · 删除 · 组合为内容块 · 组合为索引 ·
        // 解除组合 · 剪切 · 复制 · 整组成块 · 逐页成块.
        if ChromeIcons
            .tool(
                ui,
                ChromeIcon::PasteSplit,
                "逐页成块：剪贴板每页各成一个内容块",
                has_clip,
                false,
                true,
            )
            .clicked()
        {
            self.clip_paste(true);
        }
        if ChromeIcons
            .tool(
                ui,
                ChromeIcon::PasteGroup,
                "整组成块：剪贴板各页合为一个多页组块",
                has_clip,
                false,
                true,
            )
            .clicked()
        {
            self.clip_paste(false);
        }
        if ChromeIcons
            .tool(
                ui,
                ChromeIcon::CopyPpt,
                "复制到 PPT 剪贴板",
                !wall_targets.is_empty() || copy_deck.is_some() || !entry_targets.is_empty(),
                false,
                true,
            )
            .clicked()
        {
            if !entry_targets.is_empty() {
                self.clip_copy_entries(&entry_targets, false);
            } else if !wall_targets.is_empty() {
                self.clip_copy_units(&wall_targets, false);
            } else if let Some(deck) = &copy_deck {
                self.clip_copy_deck(deck);
            }
        }
        if ChromeIcons
            .tool(
                ui,
                ChromeIcon::Cut,
                "剪切到 PPT 剪贴板",
                !wall_targets.is_empty() || !entry_targets.is_empty(),
                false,
                true,
            )
            .clicked()
        {
            if !entry_targets.is_empty() {
                // Entry cut: the fragment lands in the
                // clipboard first, then the entries leave the
                // source index (clip_copy_entries commits the
                // removal). Selection clears on full success,
                // the same audit-N3 rule as the wall cut below.
                if self.clip_copy_entries(&entry_targets, true) {
                    if let IndexFrame::Detail(d) = self.index.stack.top_mut() {
                        d.entry_anchor = None;
                        d.entry_sel.clear();
                    }
                }
            } else if self.clip_copy_units(&wall_targets, true) {
                match self.content.stack.top_mut() {
                    ContentFrame::Wall(w) => {
                        w.multi.clear();
                    }
                    ContentFrame::Unit(st) => {
                        st.closed = true;
                    }
                }
            }
        }
        // 解除组合 — the inverse of 组合为内容块, sitting
        // right beside the two combine slots: the one
        // selected multi-page block is split back into
        // single-page blocks in its own folder and the
        // original goes to the recycle bin (App::clip_ungroup_unit).
        if ChromeIcons
            .tool(
                ui,
                ChromeIcon::Split,
                "解除组合：将多页内容块拆为单页块",
                ungroup_target.is_some(),
                false,
                true,
            )
            .clicked()
        {
            if let Some(uid) = ungroup_target.clone() {
                if self.clip_ungroup_unit(&uid) {
                    if let ContentFrame::Wall(w) = self.content.stack.top_mut() {
                        w.multi.clear();
                        if w.selected.as_deref() == Some(uid.as_str()) {
                            w.selected = None;
                        }
                    }
                }
            }
        }
        if ChromeIcons
            .tool(
                ui,
                ChromeIcon::NewIndex,
                "组合为索引：将选中的多个内容块组合为一个新索引",
                can_combine,
                false,
                true,
            )
            .clicked()
        {
            if self.clip_combine_index(&wall_multi) {
                if let ContentFrame::Wall(w) = self.content.stack.top_mut() {
                    w.multi.clear();
                }
            }
        }
        if ChromeIcons
            .tool(
                ui,
                ChromeIcon::Combine,
                "组合为内容块：将选中的多个内容块组合为一个新内容块",
                can_combine,
                false,
                true,
            )
            .clicked()
        {
            if self.clip_combine_units(&wall_multi) {
                if let ContentFrame::Wall(w) = self.content.stack.top_mut() {
                    w.multi.clear();
                }
            }
        }
        // sdDel — wired on every page (user ruling 2026-11-25):
        // whatever object the current context has selected is
        // the target — a showroom entry (open deck or plaza
        // card), an index (open detail or overview selection),
        // wall blocks (单页 and 内容块 alike), or, in the open
        // block's detail, the clicked history version when one
        // is selected and the block itself otherwise. It greys
        // only where nothing is selected — the one objectively
        // unexecutable state. One routing exception (user
        // ruling 2026-11-27): in the showroom 「我的」 zone a
        // selected library bookmark makes this same button
        // un-bookmark instead of trash — the tip below names
        // that, so the shared icon never reads as deleting
        // someone else's deck.
        let del_targets = self.deckbar_delete_targets();
        // An open index detail with entries selected deletes
        // the ENTRIES (they leave the index through a commit;
        // the blocks stay in the library) — it takes precedence
        // over the frame's index-level target.
        let (del_tip, del_enabled) = if !entry_targets.is_empty() {
            (
                format!("删除选中的 {} 个条目", entry_targets.len()),
                true,
            )
        } else {
            (
                match del_targets.len() {
                    0 => "删除".to_string(),
                    // A library target un-bookmarks, it does
                    // not delete the deck: the tip says so
                    // verbatim.
                    1 if del_targets[0].kind == "library" => {
                        format!("移出我的库「{}」", del_targets[0].label)
                    }
                    1 => format!("删除「{}」", del_targets[0].label),
                    n => format!("删除选中的 {n} 项"),
                },
                !del_targets.is_empty(),
            )
        };
        if ChromeIcons
            .tool(ui, ChromeIcon::Trash, &del_tip, del_enabled, false, true)
            .clicked()
        {
            if !entry_targets.is_empty() {
                self.idx_remove_selected_entries();
            } else {
                self.deckbar_delete_selection(del_targets);
            }
        }
        // 清空选择 — directly in front of 删除 (added after it
        // in this right-to-left layout): drops the wall's
        // multi-selection only; no data is touched. Grey
        // while nothing is multi-selected.
        if ChromeIcons
            .tool(
                ui,
                ChromeIcon::Refresh,
                "清空选择",
                !wall_multi.is_empty() || !entry_targets.is_empty(),
                false,
                true,
            )
            .clicked()
        {
            if let ContentFrame::Wall(w) = self.content.stack.top_mut() {
                w.multi.clear();
            }
            if let IndexFrame::Detail(d) = self.index.stack.top_mut() {
                d.entry_anchor = None;
                d.entry_sel.clear();
            }
        }
        // 已选计数 — a status label, not a button: the one
        // member of the group allowed to appear and disappear
        // (full-set ruling covers buttons). It follows the
        // wall multi-selection exactly as the removed wall
        // strip's count did; the index entry selection reads
        // in 条目.
        if !wall_multi.is_empty() {
            ui.label(
                egui::RichText::new(format!("已选 {} 个内容块", wall_multi.len()))
                    .color(palette_color(TEXT_WEAK)),
            );
        } else if !entry_targets.is_empty() {
            ui.label(
                egui::RichText::new(format!("已选 {} 个条目", entry_targets.len()))
                    .color(palette_color(TEXT_WEAK)),
            );
        }
        tool_vsep(ui);
        // sdInfo — the deck's metadata panel (T5 ruling
        // 2026-11-24: defined, not deleted). Its slot sits in
        // the first group beside sdCache: the prototype's
        // syncBarState pairs sdInfo with the sdPull slot, and
        // share / cache / info are the group's three
        // about-this-deck actions. Availability = a finished
        // deck target exists (open or plaza-selected); the
        // panel itself is App::open_deck_info.
        if ChromeIcons
            .tool(
                ui,
                ChromeIcon::Info,
                "演示稿信息",
                info_target.is_some(),
                false,
                true,
            )
            .clicked()
        {
            let (it, pgs) = info_target.clone().unwrap();
            self.open_deck_info(&it, pgs);
        }
        // sdCache — the prototype's sdPull slot (T6 ruling
        // 2026-11-24: the slot is 缓存, not a save-to-library
        // action — the prototype itself was wrong). Accent-filled
        // while the open deck sits in the local cache (the same
        // cache-entry probe the deck header uses); clicking
        // (re)caches through the deck header's own cache handler.
        if ChromeIcons
            .tool(ui, ChromeIcon::Cache, "缓存到本地", deck_open, cached, true)
            .clicked()
        {
            let item = item.clone().unwrap();
            self.cache_showroom_deck(&item);
        }
        // sdShare: enabled whenever a deck is open, like
        // sdExport/sdExportPdf — the share dialog's 链接 form
        // is export-style and works offline and without login
        // (user ruling 2026-11-24); its server forms gate
        // themselves at their send point.
        if ChromeIcons
            .tool(ui, ChromeIcon::Share, "分享给…", deck_open, false, true)
            .clicked()
        {
            let item = item.clone().unwrap();
            self.open_share_dialog(&item);
        }
    }

    /// Placeholder slot — a bottom-bar slot whose feature is
    /// not built yet. The full-set ruling (2026-11-24) keeps
    /// every slot of the user's picked set: a placeholder renders
    /// in the group's disabled form (muted glyph, no fill) with a
    /// tooltip that states the slot's purpose, and lights up by
    /// replacing its deckbar_placeholder call with the real
    /// handler when the feature lands. It is painted by hand
    /// rather than through the disabled tool button because a
    /// disabled scope swallows hover — the tooltip must still
    /// work. Registry: currently empty — sdPasteSplit,
    /// sdPasteGroup, sdCopyPpt, sdCutPpt and sdCombine lit up
    /// with the clipboard package, and sdBlockPublish, the
    /// last entry, lit up with the P3 submission chain
    /// (deckbar_publish_block). The helper stays for any
    /// future slot.
    fn deckbar_placeholder(&mut self, ui: &mut egui::Ui, icon: ChromeIcon, label: &str) {
        let (rect, resp) = ui.allocate_exact_size(egui::vec2(30.0, 28.0), egui::Sense::hover());
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
        if !pe.dirty() || self.save_page_edit(pe).is_none() {
            self.overlays.insert(pos, overlay);
        }
    }

    /// sdBlockPublish: the editor's save (the identical
    /// App::save_page_edit sdBlockSave runs) followed by the
    /// publish half P3 §8 step 3 specified — in the local
    /// library, find the indexes whose main-head entries carry
    /// this block; exactly one, and its published entries
    /// resolving this block to the version the save just
    /// produced, submits that index through the same
    /// App::push_index_to_showroom the index detail's push
    /// dialog ends in. Zero or several candidate indexes, an
    /// unpublished index, a published set that does not carry
    /// the block, or one still locked to an older version all
    /// stop after the save with a message that says so — the
    /// button never claims a publish that did not happen.
    fn deckbar_publish_block(&mut self) {
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
        if !pe.dirty() {
            self.overlays.insert(pos, overlay);
            return;
        }
        let Some(new_version_id) = self.save_page_edit(pe) else {
            // Save failed; its message stands and the editor
            // stays open, exactly as sdBlockSave leaves it.
            self.overlays.insert(pos, overlay);
            return;
        };
        // Save succeeded — the editor closes (the editor
        // footer's own save semantics) and the publish half
        // runs against the message the save left.
        let saved_msg = self.msg.clone();
        self.publish_saved_block(pe, &new_version_id, &saved_msg);
    }

    /// The deck bar's index-context submit (user rulings
    /// 2026-11-27): builds the target set — the detail's own
    /// index, or the overview's selected index — and runs the
    /// shared LOCAL submit （提交到演示）. Always clickable
    /// while an index page is showing; an empty target set
    /// prompts inside the shared action.
    fn deckbar_submit_indexes(&mut self) {
        let targets: Vec<(String, String)> = match self.index.stack.top() {
            IndexFrame::Detail(d) => {
                let name = self
                    .indexes
                    .iter()
                    .find(|x| s(x, "id") == d.index_id)
                    .map(|x| s(x, "name"))
                    .unwrap_or_default();
                vec![(d.index_id.clone(), name)]
            }
            IndexFrame::Overview(ov) => match &ov.selected {
                Some(id) => {
                    let name = self
                        .indexes
                        .iter()
                        .find(|x| s(x, "id") == *id)
                        .map(|x| s(x, "name"))
                        .unwrap_or_default();
                    vec![(id.clone(), name)]
                }
                None => Vec::new(),
            },
            _ => Vec::new(),
        };
        self.submit_indexes_to_local_showroom(&targets);
    }

    /// 提交到演示 (user ruling 2026-11-27): the shared LOCAL
    /// submit action — the index tool row's Push button runs
    /// it for its one target, the deck bar for its target set.
    /// Each target is pinned into this library's own showroom
    /// list （本地演示） through LocalStore; no network, no
    /// login. Failures name their index and do not stop the
    /// rest; the summary reports the split. An empty target
    /// set prompts — the buttons stay clickable throughout
    /// (button-state standard).
    pub(super) fn submit_indexes_to_local_showroom(
        &mut self,
        targets: &[(String, String)],
    ) {
        if targets.is_empty() {
            self.msg = "请先选择要提交的索引".into();
            return;
        }
        let mut ok = 0usize;
        let mut failed: Vec<String> = Vec::new();
        {
            let Some(local) = &self.local else {
                self.msg = "本地库未打开，无法提交到演示".into();
                return;
            };
            for (id, name) in targets {
                match local.submit_index_to_local_showroom(id) {
                    Ok(_) => ok += 1,
                    Err(e) => failed.push(format!("「{name}」{e}")),
                }
            }
        }
        if ok > 0 {
            self.refresh_local();
        }
        if failed.is_empty() {
            if targets.len() == 1 {
                self.msg = format!("已提交到演示：{}（本地演示）", targets[0].1);
            } else {
                self.msg = format!("已提交 {ok} 个索引到演示（本地演示）");
            }
        } else if ok == 0 {
            self.msg = format!("提交到演示失败：{}", failed.join("；"));
        } else {
            self.msg = format!(
                "已提交 {ok}/{} 个索引到演示；失败：{}",
                targets.len(),
                failed.join("；")
            );
        }
    }

    /// 申请公开 (user ruling 2026-11-27): the menu-bar entry
    /// to the organization chain — submit the current /
    /// selected index for admin review through the push
    /// dialog (the 申请公开 dialog) and push_index_to_showroom.
    /// This is the one publish action that needs the server;
    /// the dialog chain carries the existing gates (online,
    /// identity, pending package, governance), so this entry
    /// never greys — with no index in context it prompts.
    fn apply_public(&mut self) {
        let target: Option<(String, String)> = if self.page == Page::Index {
            match self.index.stack.top() {
                IndexFrame::Detail(d) => Some((
                    d.index_id.clone(),
                    self.indexes
                        .iter()
                        .find(|x| s(x, "id") == d.index_id)
                        .map(|x| s(x, "name"))
                        .unwrap_or_default(),
                )),
                IndexFrame::Overview(ov) => ov.selected.clone().map(|id| {
                    let name = self
                        .indexes
                        .iter()
                        .find(|x| s(x, "id") == id)
                        .map(|x| s(x, "name"))
                        .unwrap_or_default();
                    (id, name)
                }),
                _ => None,
            }
        } else {
            None
        };
        match target {
            Some((id, title)) => self.open_push_dialog(&id, &title),
            None => {
                self.msg = if self.page == Page::Index {
                    "请先选择要申请公开的索引".into()
                } else {
                    "申请公开：请先打开一个索引".into()
                };
            }
        }
    }

    /// The publish half of sdBlockPublish (see
    /// deckbar_publish_block): the candidate search runs in
    /// the local library, the submission's home turf — an
    /// organization-side block has no package to build here
    /// and keeps the index detail's road.
    fn publish_saved_block(&mut self, pe: &PageEditState, new_version_id: &str, saved_msg: &str) {
        if pe.unit_id.is_empty() {
            self.msg = format!("{saved_msg}；提交未成：缺少内容块上下文");
            return;
        }
        let Some(local) = &self.local else {
            self.msg = format!("{saved_msg}；提交未成：本地库未打开");
            return;
        };
        let hits = local.indexes_with_unit_on_main(&pe.unit_id);
        match hits.len() {
            0 => {
                self.msg = format!(
                    "{saved_msg}；提交未成：本地库中没有包含此内容块的索引（先加入索引，或到索引详情提交）"
                );
            }
            1 => {
                let (index_id, index_name) = hits.into_iter().next().unwrap();
                self.publish_saved_block_to_index(
                    pe,
                    new_version_id,
                    saved_msg,
                    &index_id,
                    &index_name,
                );
            }
            _ => {
                self.msg = format!(
                    "{saved_msg}；提交未成：此内容块属于多个索引，请到索引详情选择提交"
                );
            }
        }
    }

    /// sdBlockPublish, the one-candidate step: the candidate's
    /// published set (LocalStore::push_preview — the same
    /// resolution the push dialog shows) must carry this block
    /// and resolve it to the version the save produced; then
    /// the submission itself is push_index_to_showroom with
    /// the preview's tag and pins, and only that call's own
    /// success verdict turns the message into 已保存并提交.
    fn publish_saved_block_to_index(
        &mut self,
        pe: &PageEditState,
        new_version_id: &str,
        saved_msg: &str,
        index_id: &str,
        index_name: &str,
    ) {
        let Some(local) = &self.local else {
            self.msg = format!("{saved_msg}；提交未成：本地库未打开");
            return;
        };
        let preview = match local.push_preview(index_id, "main") {
            Some(Ok(preview)) => preview,
            Some(Err(e)) => {
                self.msg = format!("{saved_msg}；提交未成：{e}");
                return;
            }
            None => {
                self.msg = format!("{saved_msg}；提交未成：索引不在本地库中");
                return;
            }
        };
        let Some(tag_id) = preview["published_tag_id"].as_str().map(str::to_owned) else {
            self.msg = format!(
                "{saved_msg}；提交未成：索引「{index_name}」尚未发布版本（请先在索引详情发布）"
            );
            return;
        };
        let empty: Vec<Value> = Vec::new();
        let rows = preview["current_entries"].as_array().unwrap_or(&empty);
        let mut unit_rows = 0usize;
        let mut resolves_to_new = false;
        let mut locked = false;
        for row in rows {
            if s(&row["entry"], "unit_id") != pe.unit_id {
                continue;
            }
            unit_rows += 1;
            if s(&row["resolved"], "id") == new_version_id {
                resolves_to_new = true;
            }
            if row["entry"]["policy"].get("locked").is_some() {
                locked = true;
            }
        }
        if unit_rows == 0 {
            self.msg = format!(
                "{saved_msg}；提交未成：此内容块不在索引「{index_name}」的发布版中（请先在索引详情重新发布）"
            );
            return;
        }
        if !resolves_to_new {
            self.msg = if locked {
                format!(
                    "{saved_msg}；提交未成：索引「{index_name}」仍锁定旧版本（请先在索引详情更新条目）"
                )
            } else {
                format!(
                    "{saved_msg}；提交未成：索引「{index_name}」发布版解析到的不是刚保存的新版本"
                )
            };
            return;
        }
        let pins = match super::prototype_dialogs::preview_pins(rows) {
            Ok(pins) => pins,
            Err(e) => {
                self.msg = format!("{saved_msg}；提交未成：{e}");
                return;
            }
        };
        self.push_index_to_showroom(index_id, index_name, &tag_id, &pins, pe.note.trim());
        if self.msg == "已提交，等待审核" {
            self.msg = format!("{saved_msg}，并已申请公开，等待审核");
        } else {
            let why = self.msg.clone();
            if why != saved_msg {
                self.msg = format!("{saved_msg}；提交未成：{why}");
            }
        }
    }

    fn content_light_open(&self) -> bool {
        self.page == Page::Content
            && self
                .overlays
                .iter()
                .any(|overlay| matches!(overlay, Overlay::Light(_)))
    }

    /// The client's single page-identity resolution (page
    /// identity layer, phase 1): the current page + frame stack
    /// + key state in, (page number, page name, path chain) out.
    /// The deck-bar page chip and the menu-row path text both
    /// read from here; nothing else in the client computes a
    /// page number or name of its own.
    ///
    /// Numbering follows the prototype's map (orient audit
    /// §三): top pages 1–9, content blocks 101+ by wall order,
    /// showroom decks unified per page kind (detail #211, notes
    /// #212 — same page, same number, whatever deck is open),
    /// full-page overlays 401+. Two user rulings override the
    /// prototype: the index detail is split off the overview as
    /// #7c–#7g by its five views (2026-11-27; the prototype
    /// keeps both at #7 and leans on the path text), and the
    /// chip sits at the bottom bar's left end (2026-11-27,
    /// final seat — a content-column header strip tried
    /// earlier the same day was superseded and removed).
    pub(super) fn page_identity(&self) -> PageIdentity {
        let mut id = self.page_frame_identity();
        // Full-page overlays and dialogs resolve first
        // (prototype currentPageNo order: dialog stack → drill
        // stack → top page). The small modal dialogs carry no
        // number of their own in the prototype map — they take
        // the chip over by name only (provisional).
        if let Some(top) = self.overlays.last() {
            match top {
                Overlay::Light(_) => {
                    id.no = Some("406".to_string());
                    id.name = "大图预览".to_string();
                    id.path.push("大图预览".to_string());
                }
                Overlay::PageEdit(_) => {
                    id.no = Some("407".to_string());
                    id.name = "编辑这一页".to_string();
                    id.path.push("编辑这一页".to_string());
                }
                Overlay::Present(st) => {
                    let view = if st.speaker {
                        "演讲者视图"
                    } else {
                        "放映页"
                    };
                    id.no = Some(if st.speaker { "214" } else { "213" }.to_string());
                    id.name = format!("演示厅 · {} · {view}", st.title);
                    id.path.push(view.to_string());
                }
                Overlay::AllVersions { .. } => id.name_only("全部历史版本"),
                Overlay::PickTemplate(_) => id.name_only("从模板新建"),
                Overlay::DeckInfo(_) => id.name_only("演示稿信息"),
                Overlay::InsertOrder(_) => id.name_only("调整插入顺序"),
                Overlay::DraftPublish(_) => id.name_only("发布草稿"),
                Overlay::Confirm(st) => id.name_only(&st.title),
            }
        } else if self.workspace_controls.search_open {
            // The search-results window floats above the page;
            // a client-only window, so it takes the chip by
            // name only and the path stays the page's own.
            id.name_only("搜索结果");
        }
        id
    }

    /// Identity of the frame the user is actually looking at,
    /// before the overlay layer has its say.
    fn page_frame_identity(&self) -> PageIdentity {
        match self.page {
            Page::Home => PageIdentity::new(Some("1"), "主页".into(), vec!["主页".into()]),
            Page::Showroom => match self.showroom.stack.top() {
                SrFrame::Present(st) => {
                    let title = s(&st.item, "title");
                    PageIdentity::new(
                        Some("212"),
                        format!("演示厅 · {title} · 讲稿视图"),
                        vec!["演示厅".into(), title, "讲稿视图".into()],
                    )
                }
                SrFrame::Deck(st) => {
                    let title = s(&st.item, "title");
                    let mut path =
                        vec!["演示厅".to_string(), title.clone(), "稿件详情".to_string()];
                    if st.preview_open {
                        path.push("单页预览".to_string());
                    }
                    PageIdentity::new(Some("211"), format!("演示厅 · {title} · 详情页"), path)
                }
                SrFrame::Wall => {
                    let (no, zone) = match self.showroom.zone.as_str() {
                        "library" => ("3", "我的"),
                        "inbox" => ("4", "分享"),
                        "groups" => ("5", "群组"),
                        _ => ("2", "公共"),
                    };
                    PageIdentity::new(
                        Some(no),
                        format!("演示厅 · {zone}"),
                        vec!["演示厅".into(), zone.into()],
                    )
                }
            },
            Page::Content => {
                let units: Vec<&UnitState> = self
                    .content
                    .stack
                    .frames
                    .iter()
                    .filter_map(|f| match f {
                        ContentFrame::Unit(st) => Some(st),
                        ContentFrame::Wall(_) => None,
                    })
                    .collect();
                if let Some(top) = units.last() {
                    let top_name = self.unit_frame_name(top);
                    let name = if units.len() > 1 {
                        let parent = self.unit_frame_name(units[units.len() - 2]);
                        format!("子块 · {top_name}（属「{parent}」） · 下钻页")
                    } else {
                        format!("内容块 · {top_name} · 下钻页")
                    };
                    let mut path = vec!["页面".to_string()];
                    path.extend(units.iter().map(|st| self.unit_frame_name(st)));
                    PageIdentity::new(Some(&self.unit_page_no(&top.unit_id)), name, path)
                } else if self.content.cache_open {
                    PageIdentity::new(
                        Some("6b"),
                        "本地缓存".into(),
                        vec!["页面".into(), "本地缓存".into()],
                    )
                } else {
                    PageIdentity::new(Some("6"), "页面 · 目录".into(), vec!["页面".into()])
                }
            }
            Page::Index => match self.index.stack.top() {
                IndexFrame::Detail(st) => {
                    // User ruling 2026-11-27: the detail is NOT
                    // the overview's #7 — it splits by view along
                    // the figcraft census word list (#7c–#7g).
                    let (no, view) = match st.view {
                        IndexDetailView::Wall => ("7c", "条目图墙"),
                        IndexDetailView::Commits => ("7d", "提交图"),
                        IndexDetailView::Versions => ("7e", "历史版本"),
                        IndexDetailView::Exports => ("7f", "导出记录"),
                        IndexDetailView::Lineage => ("7g", "派生关系"),
                    };
                    let mut iname = s(&st.detail["index"], "name");
                    if iname.is_empty() {
                        iname = self
                            .indexes
                            .iter()
                            .find(|x| s(x, "id") == st.index_id)
                            .map(|x| s(x, "name"))
                            .unwrap_or_else(|| st.index_id.clone());
                    }
                    PageIdentity::new(
                        Some(no),
                        format!("索引页 · {iname} · {view}"),
                        vec!["索引总览".into(), iname, view.into()],
                    )
                }
                IndexFrame::Create(_) => PageIdentity::new(
                    None,
                    "新建索引".into(),
                    vec!["索引总览".into(), "新建索引".into()],
                ),
                IndexFrame::Overview(_) => {
                    if self.index.overview_controls.relationships {
                        PageIdentity::new(
                            Some("7b"),
                            "索引页 · 派生关系".into(),
                            vec!["索引总览".into(), "派生关系".into()],
                        )
                    } else {
                        PageIdentity::new(Some("7"), "索引页".into(), vec!["索引总览".into()])
                    }
                }
            },
            Page::Trash => match self.trash.stack.top() {
                TrashFrame::Preview(st) => PageIdentity::new(
                    None,
                    "回收站预览".into(),
                    vec!["回收站".into(), st.title.clone()],
                ),
                TrashFrame::List(_) => {
                    PageIdentity::new(Some("9"), "回收站".into(), vec!["回收站".into()])
                }
            },
            Page::Settings => {
                let sections = self.settings_sections();
                let section = sections
                    .get(self.settings.section)
                    .copied()
                    .unwrap_or_default();
                if section == "审批队列" {
                    match self.review.stack.top() {
                        ReviewFrame::Detail(d) => {
                            let title = self
                                .find_showroom_item(&d.item_id)
                                .map(|it| s(&it, "title"))
                                .filter(|t| !t.is_empty())
                                .unwrap_or_else(|| d.item_id.clone());
                            PageIdentity::new(
                                None,
                                "审批详情".into(),
                                vec!["设置".into(), "审批队列".into(), title],
                            )
                        }
                        ReviewFrame::Submission(st) => {
                            let sub = st
                                .manifest
                                .as_ref()
                                .map(|m| m["submission"].clone())
                                .unwrap_or(Value::Null);
                            let mut title = s(&sub, "title");
                            if title.is_empty() {
                                title = st.sub_id.clone();
                            }
                            PageIdentity::new(
                                None,
                                "投稿查看".into(),
                                vec!["设置".into(), "审批队列".into(), title],
                            )
                        }
                        ReviewFrame::Queue(_) => PageIdentity::new(
                            Some("8"),
                            "设置".into(),
                            vec!["设置".into(), "审批队列".into()],
                        ),
                    }
                } else {
                    PageIdentity::new(
                        Some("8"),
                        "设置".into(),
                        vec!["设置".into(), section.into()],
                    )
                }
            }
            Page::Messages => {
                PageIdentity::new(None, "消息中心".into(), vec!["消息中心".into()])
            }
        }
    }

    /// A drilled block's display name, from its own frame
    /// payload (the units cache is only the fallback — a deep
    /// jump can open a block the wall list never loaded).
    fn unit_frame_name(&self, st: &UnitState) -> String {
        let name = s(&st.detail["unit"], "name");
        if !name.is_empty() {
            return name;
        }
        self.unit_name(&st.unit_id)
    }

    /// A drilled block's page number: 101 + its position in
    /// the wall's unit list — the prototype numbers blocks by
    /// their flattened wall order the same way. A block the
    /// list does not hold (deep jump before the wall loaded)
    /// takes the prototype's own unnumbered-drill fallback,
    /// #199.
    fn unit_page_no(&self, unit_id: &str) -> String {
        for (pos, item) in self.units.iter().enumerate() {
            if s(&item["unit"], "id") == unit_id {
                return (101 + pos).to_string();
            }
        }
        "199".to_string()
    }

    /// sdClose's label for the current state (prototype
    /// updateBarAvail): None on the home page, where the button
    /// greys out entirely.
    fn deckbar_back_label(&self) -> Option<&'static str> {
        match self.page {
            Page::Home => None,
            Page::Showroom => match self.showroom.stack.top() {
                SrFrame::Present(_) => Some("关闭当前整页视图"),
                SrFrame::Deck(st) => Some(if st.preview_open {
                    "返回稿件详情"
                } else {
                    "关闭条目详情"
                }),
                SrFrame::Wall => Some("回主页"),
            },
            Page::Content => match self.content.stack.top() {
                ContentFrame::Unit(_) => Some(if self.content_light_open() {
                    // Prototype updateBarAvail: while a full-page
                    // view (#406) tops the stack, sdClose reads
                    // 关闭当前整页视图 — the close still lands
                    // back on the detail frame (T4).
                    "关闭当前整页视图"
                } else if self.content.stack.depth() > 2 {
                    "返回上一层"
                } else {
                    "退回内容长卷"
                }),
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
                if self.settings_sections().get(self.settings.section).copied() == Some("审批队列")
                    && matches!(
                        self.review.stack.top(),
                        ReviewFrame::Detail(_) | ReviewFrame::Submission(_)
                    )
                {
                    Some("返回上一层")
                } else {
                    Some("回主页")
                }
            }
            Page::Messages => Some("回主页"),
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
                if matches!(self.showroom.stack.top(), SrFrame::Present(_)) {
                    // Close the 讲稿视图 frame: pop back to the
                    // deck frame it covered, folding in any
                    // scripts saved while it was open.
                    let notes = match self.showroom.stack.top() {
                        SrFrame::Present(pv) => {
                            pv.pages.iter().map(|p| p.note.clone()).collect::<Vec<_>>()
                        }
                        _ => Vec::new(),
                    };
                    self.showroom.stack.pop();
                    self.sync_present_notes(notes);
                } else if matches!(self.showroom.stack.top(), SrFrame::Deck(_)) {
                    self.back_from_showroom_detail();
                } else {
                    self.navigate_to(Page::Home);
                }
            }
            Page::Content => {
                if let Some(pos) = self
                    .overlays
                    .iter()
                    .rposition(|o| matches!(o, Overlay::Light(_)))
                {
                    self.overlays.remove(pos);
                } else if let ContentFrame::Unit(st) = self.content.stack.top_mut() {
                    // Pop only — the breadcrumb return in
                    // unit_detail does the same. Mutations made
                    // inside the detail mark their slices dirty
                    // (phase 1), so the wall is already current;
                    // the refresh_all that used to run here was
                    // the #101 return lag (2026-11-25). This is
                    // the §3.4 deckbar_back re-evaluation the
                    // phase-1 design scheduled, settled by the
                    // real-machine report: the full sweep goes,
                    // the dirty-set consumption stays.
                    st.closed = true;
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
                if self.settings_sections().get(self.settings.section).copied() == Some("审批队列")
                    && matches!(
                        self.review.stack.top(),
                        ReviewFrame::Detail(_) | ReviewFrame::Submission(_)
                    )
                {
                    self.review.stack.pop();
                } else {
                    self.navigate_to(Page::Home);
                }
            }
            Page::Messages => self.navigate_to(Page::Home),
        }
    }

    /// Keyboard Delete (user report 2026-11-25: with cards
    /// selected the key did nothing — it was never wired
    /// anywhere in the client). Runs the deckbar sdDel path
    /// verbatim — same `deckbar_delete_targets`, same
    /// `deckbar_delete_selection` — so the key inherits the
    /// button's whole semantics: the target kinds (a library
    /// bookmark un-bookmarks instead of trashing), trash
    /// categorisation, the pin-guarded version refusal, the
    /// spoken result line. Three gates, all must pass:
    /// - a plain Delete press, first press only (key repeats
    ///   are ignored: holding the key down must not chain from
    ///   a selected history version into its owning block),
    ///   and no Ctrl/Alt/Cmd chord;
    /// - no widget holds focus — the page editor undo/redo
    ///   idiom (`memory.focused()`): a focused TextEdit keeps
    ///   Delete for its own text, so renaming a folder or
    ///   typing in a search box can never delete a card;
    /// - no overlay or dialog is open
    ///   (`delete_key_dialog_open`): Delete must never reach
    ///   through a confirm, the login window or a share
    ///   dialog at the wall behind it.
    /// With no selection the targets list is empty and the
    /// key is a silent no-op — exactly the button's grey
    /// state. Backspace is deliberately not mapped: on
    /// Windows Delete is the delete key, and Backspace stays
    /// unclaimed.
    pub(super) fn deckbar_delete_key(&mut self, ctx: &egui::Context) {
        let pressed = ctx.input(|i| {
            i.events.iter().any(|e| match e {
                egui::Event::Key {
                    key,
                    pressed,
                    repeat,
                    modifiers,
                    ..
                } => {
                    *key == egui::Key::Delete
                        && *pressed
                        && !*repeat
                        && !modifiers.ctrl
                        && !modifiers.alt
                        && !modifiers.command
                }
                _ => false,
            })
        });
        if !pressed {
            return;
        }
        if ctx.memory(|m| m.focused().is_some()) {
            return;
        }
        if self.delete_key_dialog_open() {
            return;
        }
        // Index-detail entry selection takes the key first:
        // Delete removes the selected entries from the index
        // (the sdDel slot's precedence, same here).
        if self.page == Page::Index && !self.idx_entry_targets().is_empty() {
            ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Delete));
            self.idx_remove_selected_entries();
            return;
        }
        let targets = self.deckbar_delete_targets();
        if targets.is_empty() {
            return;
        }
        ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Delete));
        self.deckbar_delete_selection(targets);
    }

    /// Keyboard Ctrl+Z → the session undo stack (user ruling
    /// 2026-11-25). Same three gates as the Delete key, for the
    /// same reasons:
    /// - a plain Ctrl+Z press, first press only (key repeats
    ///   are ignored: holding the chord down must not burn
    ///   through the whole stack in one press), no Shift (that
    ///   chord is redo's spelling — `redo_key` below claims it,
    ///   and this handler must never fire for it), no Alt/Cmd;
    /// - no widget holds focus — a focused TextEdit keeps
    ///   Ctrl+Z for its own text buffer, and the page editor's
    ///   own undo keeps working the same way (its overlay also
    ///   trips the dialog gate below);
    /// - no overlay or dialog is open (`delete_key_dialog_open`,
    ///   shared verbatim): undo must never reach through a
    ///   confirm or the login window at the wall behind it.
    /// An empty stack is a silent no-op. On a hit the key is
    /// consumed so nothing downstream re-reads the press.
    pub(super) fn undo_key(&mut self, ctx: &egui::Context) {
        let pressed = ctx.input(|i| {
            i.events.iter().any(|e| match e {
                egui::Event::Key {
                    key,
                    pressed,
                    repeat,
                    modifiers,
                    ..
                } => {
                    *key == egui::Key::Z
                        && *pressed
                        && !*repeat
                        && modifiers.ctrl
                        && !modifiers.shift
                        && !modifiers.alt
                        && !modifiers.command
                }
                _ => false,
            })
        });
        if !pressed {
            return;
        }
        if ctx.memory(|m| m.focused().is_some()) {
            return;
        }
        if self.delete_key_dialog_open() {
            return;
        }
        if self.undo_stack.is_empty() {
            return;
        }
        ctx.input_mut(|i| i.consume_key(egui::Modifiers::CTRL, egui::Key::Z));
        self.do_undo();
    }

    /// Keyboard Ctrl+Y (and its alias Ctrl+Shift+Z) → the
    /// session redo stack (user ruling 2026-11-25). The gates
    /// are `undo_key`'s three, verbatim and for the same
    /// reasons: first press only with repeats ignored, no
    /// widget focus (a focused TextEdit keeps both chords for
    /// its own text buffer — egui's TextEdit handles Ctrl+Y /
    /// Ctrl+Shift+Z as text redo itself), and no overlay or
    /// dialog open. The chord set is disjoint from undo's:
    /// Ctrl+Y differs by key, Ctrl+Shift+Z by the Shift that
    /// `undo_key` excludes, so neither handler can fire for
    /// the other's press (regression-checked: plain Ctrl+Z
    /// still undoes, Ctrl+Shift+Z never undoes). An empty
    /// redo stack is a silent no-op; on a hit the press is
    /// consumed with the exact modifiers it arrived with.
    pub(super) fn redo_key(&mut self, ctx: &egui::Context) {
        let chord: Option<(egui::Modifiers, egui::Key)> = ctx.input(|i| {
            let mut hit = None;
            for e in &i.events {
                let egui::Event::Key {
                    key,
                    pressed,
                    repeat,
                    modifiers,
                    ..
                } = e
                else {
                    continue;
                };
                if !*pressed || *repeat || modifiers.alt || modifiers.command {
                    continue;
                }
                if *key == egui::Key::Y && modifiers.ctrl && !modifiers.shift {
                    hit = Some((egui::Modifiers::CTRL, egui::Key::Y));
                    break;
                }
                if *key == egui::Key::Z && modifiers.ctrl && modifiers.shift {
                    hit = Some((
                        egui::Modifiers {
                            ctrl: true,
                            shift: true,
                            ..Default::default()
                        },
                        egui::Key::Z,
                    ));
                    break;
                }
            }
            hit
        });
        let Some((mods, key)) = chord else {
            return;
        };
        if ctx.memory(|m| m.focused().is_some()) {
            return;
        }
        if self.delete_key_dialog_open() {
            return;
        }
        if self.redo_stack.is_empty() {
            return;
        }
        ctx.input_mut(|i| i.consume_key(mods, key));
        self.do_redo();
    }

    /// Whether an in-progress pointer/keyboard interaction owns
    /// Escape this frame: a press or armed drag on a wall or in a
    /// detail wall, or a page-binding pick in flight. Each of
    /// these has its own Esc consumer in the render pass (the
    /// drag-arming code clears its press/drag state, the detail
    /// clears its pick) — the app-level back key must stand down
    /// while any of them is live, or one Esc would both cancel
    /// the interaction AND pop the frame behind it.
    fn esc_interaction_active(&self) -> bool {
        if self.card_press.is_some() || self.folder_press.is_some() {
            return true;
        }
        if let ContentFrame::Unit(st) = self.content.stack.top() {
            if st.page_press.is_some()
                || st.page_drag.is_some()
                || st.vh_press.is_some()
                || st.vh_drag.is_some()
                || st.bind_pick.is_some()
                || st.renum_edit.is_some()
                || st.meta_edit.is_some()
            {
                return true;
            }
        }
        if let SrFrame::Deck(st) = self.showroom.stack.top() {
            if st.page_press.is_some() || st.page_drag.is_some() {
                return true;
            }
        }
        false
    }

    /// Keyboard Escape → the deckbar sdClose path (user ruling
    /// 2026-11-26: Esc and the bottom bar's back button are one
    /// action). On a hit it runs `deckbar_back()` verbatim — the
    /// button's own per-page back path — so the key inherits the
    /// button's whole semantics, and its availability is the
    /// button's: `deckbar_back_label()` returning None (the Home
    /// root) makes the key a silent no-op, exactly the button's
    /// grey state. The return standard's other half — Esc closes
    /// an open overlay first — is NOT duplicated here: the
    /// overlay stack already owns Esc in that scene
    /// (`overlay_stack_ui`, with the page editor's unsaved
    /// guard and the presenter's end_present), so the dialog
    /// gate below stands this handler down whenever any overlay
    /// or modal window is up, and the two paths can never fire
    /// on the same press. Gates, all must pass:
    /// - a plain Escape press, first press only, no modifiers
    ///   (repeats ignored: holding Esc must not walk the whole
    ///   drill stack in one hold);
    /// - no widget holds focus — the established idiom of the
    ///   Delete/undo keys: a focused TextEdit keeps Escape for
    ///   its own cancel (folder rename, the detail's meta and
    ///   renumber edits all cancel in place), and egui's blur
    ///   on Escape must not be misread as anything else;
    /// - no overlay or dialog is open (`delete_key_dialog_open`,
    ///   shared verbatim: the overlay stack, the login window —
    ///   whose own Esc closes it — the delete confirm, the
    ///   share dialogs, the toolbar menus);
    /// - no drag/press/pick interaction is live
    ///   (`esc_interaction_active`): those cancel themselves
    ///   with this same press in the render pass.
    /// On a hit the press is consumed, so no later reader in
    /// the frame can re-interpret it.
    pub(super) fn deckbar_back_key(&mut self, ctx: &egui::Context) {
        let pressed = ctx.input(|i| {
            i.events.iter().any(|e| match e {
                egui::Event::Key {
                    key,
                    pressed,
                    repeat,
                    modifiers,
                    ..
                } => {
                    *key == egui::Key::Escape
                        && *pressed
                        && !*repeat
                        && !modifiers.ctrl
                        && !modifiers.alt
                        && !modifiers.command
                        && !modifiers.shift
                }
                _ => false,
            })
        });
        if !pressed {
            return;
        }
        if ctx.memory(|m| m.focused().is_some()) {
            return;
        }
        if self.delete_key_dialog_open() {
            return;
        }
        if self.esc_interaction_active() {
            return;
        }
        if self.deckbar_back_label().is_none() {
            return;
        }
        ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape));
        self.deckbar_back();
    }

    /// Any overlay or standalone dialog that must shield the
    /// wall from the keyboard Delete shortcut. The overlay
    /// stack covers Confirm / Light / PickTemplate / PageEdit /
    /// DeckInfo / Present / AllVersions; the rest are the
    /// App-level windows and modals drawn beside it, plus the
    /// index detail's pending more-action modal and the
    /// toolbar popup menus. The search-results window is
    /// deliberately NOT a shield: it is a persistent companion
    /// the wall stays fully interactive under, so selecting a
    /// card and pressing Delete keeps working while it floats.
    pub(super) fn delete_key_dialog_open(&self) -> bool {
        if !self.overlays.is_empty()
            || self.login_open
            || self.about_open
            || self.folder_sync_open
            || self.msg_clear_open
            || self.must_change_pw
            || self.folder_delete_confirm.is_some()
            || self.share_dialog.is_some()
            || self.pull_dialog.is_some()
            || self.push_dialog.is_some()
            || self.workspace_controls.menu_open()
        {
            return true;
        }
        match self.index.stack.top() {
            IndexFrame::Detail(st) => st.more_action.is_some(),
            _ => false,
        }
    }

    /// sdDel's targets for the current context (user ruling
    /// 2026-11-25: clickable whenever anything is selected, on
    /// any page). Resolution per page:
    /// - Content, block detail open: the clicked history
    ///   version when one is selected (deleting it deletes THAT
    ///   version), else the open block itself.
    /// - Content, wall: the multi-selection, else the anchor
    ///   card — 单页 and 内容块 are both "unit" targets.
    ///   Unsplit lazy wrappers never appear (their cards take
    ///   no selection; filtered again here so a stale selection
    ///   can never name one).
    /// - Index: the open detail's index, else the overview's
    ///   selected card.
    /// - Showroom: the open deck's entry, else the plaza wall's
    ///   selected card.
    /// Everywhere else (home, trash, settings, messages):
    /// nothing — the button greys.
    fn deckbar_delete_targets(&self) -> Vec<DelTarget> {
        match self.page {
            Page::Content => match self.content.stack.top() {
                ContentFrame::Unit(st) => {
                    let unit = st.detail.get("unit").cloned().unwrap_or(json!({}));
                    let uid = s(&unit, "id");
                    if uid.is_empty() {
                        return Vec::new();
                    }
                    let name = s(&unit, "name");
                    if let Some((vid, ver_label)) =
                        super::content_detail::selected_version_delete_target(st)
                    {
                        return vec![DelTarget {
                            kind: "version",
                            id: vid,
                            label: format!("{name} · {ver_label}"),
                        }];
                    }
                    vec![DelTarget {
                        kind: "unit",
                        id: uid,
                        label: name,
                    }]
                }
                ContentFrame::Wall(w) => {
                    let ids: Vec<String> = if !w.multi.is_empty() {
                        w.multi.clone()
                    } else if let Some(sel) = &w.selected {
                        vec![sel.clone()]
                    } else {
                        Vec::new()
                    };
                    ids.into_iter()
                        .filter(|id| !self.lazy_unit_page.contains_key(id))
                        .map(|id| {
                            let label = self
                                .units
                                .iter()
                                .find_map(|it| {
                                    let u = it.get("unit")?;
                                    (s(u, "id") == id).then(|| s(u, "name"))
                                })
                                .unwrap_or_default();
                            DelTarget {
                                kind: "unit",
                                id,
                                label,
                            }
                        })
                        .collect()
                }
            },
            Page::Index => match self.index.stack.top() {
                IndexFrame::Detail(d) => {
                    let label = self
                        .indexes
                        .iter()
                        .find(|x| s(x, "id") == d.index_id)
                        .map(|x| s(x, "name"))
                        .unwrap_or_default();
                    vec![DelTarget {
                        kind: "index",
                        id: d.index_id.clone(),
                        label,
                    }]
                }
                IndexFrame::Overview(ov) => match &ov.selected {
                    Some(id) => {
                        let label = self
                            .indexes
                            .iter()
                            .find(|x| s(x, "id") == *id)
                            .map(|x| s(x, "name"))
                            .unwrap_or_default();
                        vec![DelTarget {
                            kind: "index",
                            id: id.clone(),
                            label,
                        }]
                    }
                    None => Vec::new(),
                },
                _ => Vec::new(),
            },
            Page::Showroom => match self.showroom.stack.top() {
                SrFrame::Deck(st) => {
                    let id = s(&st.item, "id");
                    if id.is_empty() {
                        Vec::new()
                    } else {
                        vec![DelTarget {
                            kind: "showroom",
                            id,
                            label: s(&st.item, "title"),
                        }]
                    }
                }
                SrFrame::Present(pv) => {
                    // The 讲稿视图 sheet sits on top of the deck
                    // frame and carries that deck's item: the
                    // open deck is still the current object, so
                    // the target is the deck itself, exactly as
                    // in the Deck arm above. (The matching
                    // cleanup in deckbar_delete_selection
                    // collapses the stack to the wall, so the
                    // buried deck frame never resurfaces.)
                    let id = s(&pv.item, "id");
                    if id.is_empty() {
                        Vec::new()
                    } else {
                        vec![DelTarget {
                            kind: "showroom",
                            id,
                            label: s(&pv.item, "title"),
                        }]
                    }
                }
                SrFrame::Wall => match &self.showroom.selected_card {
                    Some(id) => {
                        // 「我的」 zone split (user ruling
                        // 2026-11-27): a selected card that is
                        // one of my library bookmarks is a
                        // LIBRARY target, not a showroom one —
                        // the bottom-bar delete un-bookmarks it
                        // (deckbar_delete_selection POSTs
                        // /v1/library/{lid}/delete), it does NOT
                        // trash the deck itself: the deck is a
                        // showroom entry, my library row is only
                        // a bookmark of it. The zone check is
                        // load-bearing — the same item id
                        // selected on the plaza wall stays a
                        // showroom target. The target id is the
                        // item id (the selected_card value); the
                        // library row id is resolved from
                        // library_refs at execution time.
                        if self.showroom.zone == "library" {
                            if let Some(e) =
                                self.library_refs.iter().find(|e| s(e, "item_id") == *id)
                            {
                                return vec![DelTarget {
                                    kind: "library",
                                    id: id.clone(),
                                    label: s(e, "title"),
                                }];
                            }
                        }
                        let label = self
                            .showroom_live
                            .iter()
                            .find(|it| s(it, "id") == *id)
                            .map(|it| s(it, "title"))
                            .unwrap_or_default();
                        vec![DelTarget {
                            kind: "showroom",
                            id: id.clone(),
                            label,
                        }]
                    }
                    None => Vec::new(),
                },
            },
            _ => Vec::new(),
        }
    }

    /// sdDel: run the batch (App::trash_add_many), then apply
    /// the context cleanup for what actually went away — open
    /// frames whose object was deleted close, selections shed
    /// the dead ids, an open detail whose history version was
    /// deleted reloads without it — refresh once, scoped to the
    /// deleted kinds, and report: one line for the successes,
    /// every failure named with its reason (a pin-guarded
    /// version refuses, the rest still delete).
    ///
    /// Library targets (kind "library", user ruling
    /// 2026-11-27) split off BEFORE the trash batch:
    /// un-bookmarking is a server bookmark delete, not a trash
    /// move — it never reaches trash_add_many (no /v1/trash
    /// row, nothing lands in the recycle bin, the deck itself
    /// is untouched and stays on the plaza wall). Each one
    /// resolves its library row id from library_refs by item
    /// id and replays the exact call the card's removed
    /// 「移除」 button used to make. Library successes also
    /// stay out of the undo recording below: a bookmark
    /// removal has no local trash row to snapshot and no
    /// wired inverse (re-adding is the pull dialog's job) —
    /// the same class as the organization showroom rows that
    /// drop out inside record_trash_undo.
    fn deckbar_delete_selection(&mut self, targets: Vec<DelTarget>) {
        if targets.is_empty() {
            return;
        }
        let (library_targets, targets): (Vec<DelTarget>, Vec<DelTarget>) =
            targets.into_iter().partition(|t| t.kind == "library");
        let mut library_succeeded: Vec<DelTarget> = Vec::new();
        let mut library_failures: Vec<(DelTarget, String)> = Vec::new();
        for t in &library_targets {
            let lid = self
                .library_refs
                .iter()
                .find(|e| s(e, "item_id") == t.id)
                .map(|e| s(e, "id"))
                .unwrap_or_default();
            if lid.is_empty() {
                library_failures.push((t.clone(), "条目已不在我的库中".to_string()));
                continue;
            }
            match self.call(
                "POST",
                &format!("/v1/library/{lid}/delete"),
                Some(json!({})),
            ) {
                Ok(_) => {
                    if self.showroom.selected_card.as_deref() == Some(t.id.as_str()) {
                        self.showroom.selected_card = None;
                    }
                    library_succeeded.push(t.clone());
                }
                Err(e) => library_failures.push((t.clone(), e)),
            }
        }
        if !library_succeeded.is_empty() {
            self.reread_library();
        }
        let failures = self.trash_add_many(&targets);
        let failed: std::collections::HashSet<&str> =
            failures.iter().map(|(t, _)| t.id.as_str()).collect();
        let succeeded: Vec<&DelTarget> = targets
            .iter()
            .filter(|t| !failed.contains(t.id.as_str()))
            .collect();
        // Undo (Ctrl+Z): the batch records as ONE entry — the
        // locally-held rows among the successes (organization
        // showroom rows live on the server and drop out inside
        // record_trash_undo).
        if !succeeded.is_empty() {
            let pairs: Vec<(&str, &str)> = succeeded
                .iter()
                .map(|t| (t.kind, t.id.as_str()))
                .collect();
            let undo_label = if succeeded.len() == 1 {
                format!("删除「{}」", succeeded[0].label)
            } else {
                format!("删除 {} 项", succeeded.len())
            };
            self.record_trash_undo(undo_label, &pairs);
        }
        let mut version_gone = false;
        for t in &succeeded {
            match t.kind {
                "version" => {
                    version_gone = true;
                    if let ContentFrame::Unit(st) = self.content.stack.top_mut() {
                        if st.selected_version.as_deref() == Some(t.id.as_str())
                            || st.selected_working_card
                        {
                            st.selected_version = None;
                            st.selected_working_card = false;
                            st.selected_page = None;
                            st.bind_pick = None;
                            st.bind_panel = None;
                        }
                    }
                }
                "unit" => {
                    if let ContentFrame::Unit(st) = self.content.stack.top_mut() {
                        if st.unit_id == t.id {
                            st.closed = true;
                        }
                    }
                    if let ContentFrame::Wall(w) = self.content.stack.root_mut() {
                        w.multi.retain(|id| id != &t.id);
                        if w.selected.as_deref() == Some(t.id.as_str()) {
                            w.selected = None;
                        }
                    }
                }
                "index" => {
                    if let IndexFrame::Detail(st) = self.index.stack.top_mut() {
                        if st.index_id == t.id {
                            st.closed = true;
                        }
                    }
                    if let IndexFrame::Overview(ov) = self.index.stack.root_mut() {
                        if ov.selected.as_deref() == Some(t.id.as_str()) {
                            ov.selected = None;
                        }
                    }
                }
                "showroom" => {
                    // The 讲稿视图 sheet may top the stack with
                    // the deck frame buried beneath it: if the
                    // sheet's deck is the deleted one, collapse
                    // the stack to the wall — the buried deck
                    // frame must not resurface rendering a
                    // trashed entry. (NavStack exposes no
                    // below-top access; pop_to is the close.
                    // The match reads the item id out first,
                    // same borrow dance as deckbar_back.)
                    let present_is_target = match self.showroom.stack.top() {
                        SrFrame::Present(pv) => s(&pv.item, "id") == t.id,
                        _ => false,
                    };
                    if present_is_target {
                        self.showroom.stack.pop_to(0);
                    }
                    if let SrFrame::Deck(st) = self.showroom.stack.top_mut() {
                        if s(&st.item, "id") == t.id {
                            st.closed = true;
                        }
                    }
                    if self.showroom.selected_card.as_deref() == Some(t.id.as_str()) {
                        self.showroom.selected_card = None;
                    }
                }
                _ => {}
            }
        }
        if !succeeded.is_empty() {
            let kinds: Vec<&str> = succeeded.iter().map(|t| t.kind).collect();
            self.refresh_after_trash_delete(&kinds);
            if version_gone {
                if let ContentFrame::Unit(st) = self.content.stack.top() {
                    let uid = st.unit_id.clone();
                    self.reload_unit_detail(&uid);
                }
            }
        }
        // Result lines: the trash family's wording is the
        // long-standing one below, kept verbatim; the library
        // family gets its own line (nothing went to the
        // recycle bin, so it must not claim so). Batches from
        // deckbar_delete_targets are homogeneous in practice —
        // the lines simply join if one ever mixes.
        let mut lines: Vec<String> = Vec::new();
        if !targets.is_empty() || !failures.is_empty() {
            match (succeeded.len(), failures.len()) {
                (1, 0) => lines.push("已移入回收站，可随时恢复".to_string()),
                (n, 0) => lines.push(format!("已移入回收站 {n} 项，可随时恢复")),
                (0, _) => {
                    let list = failures
                        .iter()
                        .map(|(t, e)| format!("「{}」：{e}", t.label))
                        .collect::<Vec<_>>()
                        .join("；");
                    lines.push(format!("删除失败：{list}"));
                }
                (n, _) => {
                    let list = failures
                        .iter()
                        .map(|(t, e)| format!("「{}」：{e}", t.label))
                        .collect::<Vec<_>>()
                        .join("；");
                    lines.push(format!("已移入回收站 {n} 项；未能删除：{list}"));
                }
            }
        }
        match (library_succeeded.len(), library_failures.len()) {
            (0, 0) => {}
            (1, 0) => lines.push(format!("已移出我的库「{}」", library_succeeded[0].label)),
            (n, 0) => lines.push(format!("已移出我的库 {n} 项")),
            (0, _) => {
                let list = library_failures
                    .iter()
                    .map(|(t, e)| format!("「{}」：{e}", t.label))
                    .collect::<Vec<_>>()
                    .join("；");
                lines.push(format!("移出我的库失败：{list}"));
            }
            (n, _) => {
                let list = library_failures
                    .iter()
                    .map(|(t, e)| format!("「{}」：{e}", t.label))
                    .collect::<Vec<_>>()
                    .join("；");
                lines.push(format!("已移出我的库 {n} 项；未能移出：{list}"));
            }
        }
        if !lines.is_empty() {
            self.say(lines.join("；"));
        }
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
                    // Page-number chip (prototype #pageIdChip):
                    // the deck bar's first element, at its left
                    // end, ahead of the clipboard chip and the
                    // action group. Its number and name come
                    // from page_identity(), the single resolver.
                    // (User ruling 2026-11-27, final seat: the
                    // bottom bar's left. A content-column header
                    // strip tried earlier the same day was
                    // superseded; the strip is gone.)
                    // Prototype #pageIdChip form: a quiet badge
                    // reading "#N"; pages the user has not
                    // numbered (client-only pages, provisional
                    // default) read their page name instead.
                    let identity = self.page_identity();
                    let text = match &identity.no {
                        Some(no) => format!("#{no}"),
                        None => identity.name.clone(),
                    };
                    let font = egui::FontId::proportional(11.5);
                    let galley = ui.painter().layout_no_wrap(
                        text.clone(),
                        font.clone(),
                        palette_color(TEXT_WEAK),
                    );
                    let (badge, resp) = ui.allocate_exact_size(
                        egui::vec2(galley.size().x + 16.0, 26.0),
                        egui::Sense::hover(),
                    );
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
                        text,
                        font,
                        palette_color(TEXT_WEAK),
                    );
                    match &identity.no {
                        Some(no) => {
                            resp.on_hover_text(format!("当前页面编号 {no} · {}", identity.name))
                        }
                        None => resp.on_hover_text(format!("当前页 · {}", identity.name)),
                    };
                    // PPT clipboard chip (prototype #deckBar's
                    // left-side tray chip): shown only while the
                    // clipboard holds pages; the text is the
                    // clipboard package's clip_badge().
                    if let Some(clip) = self.clip_badge() {
                        ui.add_space(8.0);
                        let font = egui::FontId::proportional(11.5);
                        let galley = ui.painter().layout_no_wrap(
                            clip.clone(),
                            font.clone(),
                            palette_color(TEXT_WEAK),
                        );
                        let (chip, _) = ui.allocate_exact_size(
                            egui::vec2(galley.size().x + 16.0, 26.0),
                            egui::Sense::hover(),
                        );
                        ui.painter().rect(
                            chip,
                            6.0,
                            palette_color(HOVER_BG),
                            egui::Stroke::new(1.0, palette_color(FIELD_BORDER)),
                            egui::StrokeKind::Inside,
                        );
                        ui.painter().text(
                            chip.center(),
                            egui::Align2::CENTER_CENTER,
                            clip,
                            font,
                            palette_color(TEXT_WEAK),
                        );
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.add_space(22.0);
                        self.deckbar_actions(ui);
                    });
                });
            });
    }
}

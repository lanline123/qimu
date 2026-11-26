//! `#8` 设置（Page::Settings）视图实现：按角色分区的本机偏好、个人资料、
//! 组织与授权、公司全局配置、内容语言、AI 翻译与术语库、缓存策略及备份。

use super::*;

fn role_label(role: &str) -> &'static str {
    match role {
        "super" => "超级管理员",
        "dept_admin" => "部门管理员",
        _ => "普通成员",
    }
}

fn cap_label(cap: &str) -> &'static str {
    match cap {
        "view" => "查看",
        "use" => "使用",
        "publish" => "发布",
        "manage" => "管理",
        other => {
            let _ = other;
            "查看"
        }
    }
}

pub(super) fn cache_policy_mode(policy: &Value) -> &'static str {
    match policy.get("mode").and_then(|v| v.as_str()) {
        Some("manual") => "manual",
        Some("forbid") => "forbid",
        Some("allow") => "allow",
        _ => {
            if policy.get("allowed").and_then(|v| v.as_bool()) == Some(false) {
                "forbid"
            } else {
                "allow"
            }
        }
    }
}

pub(super) fn cache_policy_label(mode: &str) -> &'static str {
    match mode {
        "manual" => "仅手动缓存",
        "forbid" => "禁止本地缓存",
        _ => "允许被动缓存",
    }
}

pub(super) fn can_enable_auto_cache(policy: &Value) -> bool {
    cache_policy_mode(policy) != "forbid"
}

fn parse_languages_list(raw: &str) -> Vec<String> {
    let mut out = Vec::new();
    for part in raw.split(',') {
        let trimmed = part.trim();
        if !trimmed.is_empty() && !out.iter().any(|x: &String| x == trimmed) {
            out.push(trimmed.to_string());
        }
    }
    if out.is_empty() {
        out.push("中文".into());
    }
    out
}

fn parse_company_materials(raw: &str) -> Vec<(String, String)> {
    if raw.trim().is_empty() {
        return vec![
            (
                "公司介绍 PDF".into(),
                "company-intro.pdf · 用于对外汇报附件".into(),
            ),
            ("品牌规范".into(), "brand-guidelines.pdf · VI / Logo 使用规范".into()),
            ("资质证书".into(), "营业执照与行业资质扫描件".into()),
        ];
    }
    if let Ok(arr) = serde_json::from_str::<Vec<Value>>(raw) {
        let list: Vec<(String, String)> = arr
            .iter()
            .filter_map(|v| {
                let name = s(v, "name");
                if name.trim().is_empty() {
                    None
                } else {
                    Some((name, s(v, "note")))
                }
            })
            .collect();
        if !list.is_empty() {
            return list;
        }
    }
    Vec::new()
}

fn encode_company_materials(items: &[(String, String)]) -> String {
    let list: Vec<Value> = items
        .iter()
        .map(|(n, note)| json!({ "name": n, "note": note }))
        .collect();
    serde_json::to_string(&list).unwrap_or_else(|_| "[]".into())
}

fn settings_card(ui: &mut egui::Ui, add_contents: impl FnOnce(&mut egui::Ui)) {
    egui::Frame::NONE
        .fill(palette_color(CARD_BG))
        .stroke(egui::Stroke::new(1.0, palette_color(CARD_BORDER)))
        .corner_radius(egui::CornerRadius::same(10))
        .inner_margin(egui::Margin::same(16))
        .show(ui, |ui| {
            ui.set_width(ui.available_width().min(900.0));
            add_contents(ui);
        });
}

fn section_title(ui: &mut egui::Ui, title: &str, subtitle: &str) {
    ui.label(
        egui::RichText::new(tr(title))
            .strong()
            .size(15.5)
            .color(palette_color(TEXT_MAIN)),
    );
    if !subtitle.is_empty() {
        ui.add_space(3.0);
        ui.label(
            egui::RichText::new(tr(subtitle))
                .size(12.0)
                .color(palette_color(TEXT_WEAK)),
        );
    }
    ui.add_space(10.0);
}

fn readonly_admin_notice(ui: &mut egui::Ui) {
    ui.horizontal(|ui| {
        badge(ui, tr("由超级管理员维护"), BadgeTone::Warn);
        ui.label(
            egui::RichText::new(tr("部门管理员当前只读查看，无权修改公司级全局配置。"))
                .size(12.0)
                .color(palette_color(TEXT_WEAK)),
        );
    });
    ui.add_space(8.0);
}

pub(super) fn sections_for_context(signed_in: bool, role: &str) -> Vec<&'static str> {
    if !signed_in {
        return vec!["个人偏好", "演示偏好", "离线缓存", "服务器"];
    }
    let mut sections: Vec<&'static str> =
        vec!["个人偏好", "演示偏好", "离线缓存", "服务器", "个人资料", "我的提交"];
    if role != "member" {
        sections.push("组织管理");
        sections.push("内容组与授权");
        sections.push("审批队列");
    }
    if role == "super" {
        sections.push("高级功能与备份");
    }
    if role != "member" {
        sections.extend(["公司公共资料", "内容语言", "AI 翻译", "离线缓存策略"]);
    }
    sections
}

impl App {
    /// 按登录身份与权限裁剪的 `#8` 设置分区列表（§2.6）。
    pub(super) fn settings_sections(&self) -> Vec<&'static str> {
        sections_for_context(self.user.is_some(), &self.role())
    }

    fn select_settings_section(&mut self, name: &str) {
        let sections = self.settings_sections();
        if let Some(pos) = sections.iter().position(|x| *x == name) {
            self.settings.section = pos;
        }
    }

    pub(super) fn st_nav_body(&mut self, ui: &mut egui::Ui) {
        let sections = self.settings_sections();
        if self.settings.section >= sections.len() {
            self.settings.section = 0;
        }
        let current = sections[self.settings.section];
        ui.label(
            egui::RichText::new(tr("设置分区"))
                .size(13.0)
                .strong()
                .color(palette_color(TEXT_MAIN)),
        );
        ui.add_space(4.0);
        let groups: &[(&str, &[(&str, ChromeIcon)])] = &[
            (
                "本机与偏好",
                &[
                    ("个人偏好", ChromeIcon::Theme),
                    ("演示偏好", ChromeIcon::Presenter),
                    ("离线缓存", ChromeIcon::Download),
                ],
            ),
            (
                "账号与提交",
                &[
                    ("服务器", ChromeIcon::Globe),
                    ("个人资料", ChromeIcon::Clipboard),
                    ("我的提交", ChromeIcon::Clipboard),
                ],
            ),
            (
                "组织与审批",
                &[
                    ("组织管理", ChromeIcon::Lineage),
                    ("内容组与授权", ChromeIcon::Directory),
                    ("审批队列", ChromeIcon::Active),
                ],
            ),
            (
                "公司全局设置",
                &[
                    ("高级功能与备份", ChromeIcon::Clipboard),
                    ("公司公共资料", ChromeIcon::Home),
                    ("内容语言", ChromeIcon::All),
                    ("AI 翻译", ChromeIcon::Split),
                    ("离线缓存策略", ChromeIcon::Download),
                ],
            ),
        ];
        for (caption, items) in groups {
            let visible: Vec<(&str, ChromeIcon)> = items
                .iter()
                .copied()
                .filter(|(name, _)| sections.iter().any(|x| *x == *name))
                .collect();
            if visible.is_empty() {
                continue;
            }
            if !caption.is_empty() {
                ui.add_space(6.0);
                ui.label(
                    egui::RichText::new(tr(caption))
                        .size(12.0)
                        .color(palette_color(TEXT_WEAK)),
                );
            }
            for (name, icon) in visible {
                let resp = tree_row(ui, current == name, 0.0, Some(icon), tr(name), 12.5);
                if name == "审批队列" {
                    let pending = self.showroom_pending.len() + self.submissions_pending.len();
                    let warn = pending > 0;
                    let fg = if warn {
                        palette_color(CHIP_ORANGE_FG)
                    } else {
                        palette_color(TEXT_WEAK)
                    };
                    let font = egui::FontId::new(10.5, egui::FontFamily::Proportional);
                    let galley = ui.painter().layout_no_wrap(pending.to_string(), font, fg);
                    let w = galley.size().x + 10.0;
                    let rect = egui::Rect::from_min_size(
                        egui::pos2(resp.rect.max.x - w - 8.0, resp.rect.center().y - 8.0),
                        egui::vec2(w, 16.0),
                    );
                    ui.painter().galley(
                        egui::pos2(
                            rect.center().x - galley.size().x / 2.0,
                            rect.center().y - galley.size().y / 2.0,
                        ),
                        galley,
                        fg,
                    );
                }
                if resp.clicked() {
                    if let Some(pos) = sections.iter().position(|x| *x == name) {
                        self.settings.section = pos;
                    }
                }
            }
        }
        if sections.iter().any(|x| *x == "组织管理") {
            let depts = self
                .org
                .get("departments")
                .and_then(|d| d.as_array())
                .cloned()
                .unwrap_or_default();
            let user_count = self
                .org
                .get("users")
                .and_then(|u| u.as_array())
                .map_or(0, |a| a.len());
            let mut jump_org: Option<Option<String>> = None;
            ui.add_space(10.0);
            egui::CollapsingHeader::new(
                egui::RichText::new(format!("{} · {} 人", tr("公司组织架构"), user_count))
                    .size(12.5),
            )
            .default_open(false)
            .show(ui, |ui| {
                if depts.is_empty() {
                    ui.label(
                        egui::RichText::new(tr("暂无部门"))
                            .size(12.0)
                            .color(palette_color(TEXT_WEAK)),
                    );
                }
                for d in &depts {
                    let did = s(d, "id");
                    if tree_row(
                        ui,
                        self.settings.org_sel_dept.as_deref() == Some(did.as_str()),
                        0.0,
                        Some(ChromeIcon::Directory),
                        &s(d, "name"),
                        12.5,
                    )
                    .clicked()
                    {
                        jump_org = Some(Some(did));
                    }
                }
                ui.add_space(4.0);
                let add_sub = egui::Button::new(
                    egui::RichText::new(tr("＋ 在所选部门下新建子部门"))
                        .size(12.5)
                        .color(egui::Color32::WHITE),
                )
                .fill(ACCENT)
                .corner_radius(6);
                if ui
                    .add_sized([ui.available_width(), 30.0], add_sub)
                    .clicked()
                {
                    jump_org = Some(self.settings.org_sel_dept.clone());
                }
                if self.role() == "super" {
                    let add_root = egui::Button::new(
                        egui::RichText::new(tr("＋ 新建一级部门"))
                            .size(12.5)
                            .color(palette_color(TEXT_MAIN)),
                    )
                    .fill(palette_color(HOVER_BG))
                    .corner_radius(6);
                    if ui
                        .add_sized([ui.available_width(), 30.0], add_root)
                        .clicked()
                    {
                        jump_org = Some(None);
                    }
                }
            });
            if let Some(dept_sel) = jump_org {
                self.settings.org_sel_dept = dept_sel;
                self.select_settings_section("组织管理");
            }
        }
    }

    pub(super) fn st_main_body(&mut self, ui: &mut egui::Ui) {
        let sections = self.settings_sections();
        if self.settings.section >= sections.len() {
            self.settings.section = 0;
        }
        let current = sections[self.settings.section];
        egui::Frame::NONE
            .fill(palette_color(APP_BG))
            .inner_margin(egui::Margin {
                left: 22,
                right: 22,
                top: 20,
                bottom: 16,
            })
            .show(ui, |ui| {
                ui.set_min_size(ui.available_size());
                egui::ScrollArea::vertical().show(ui, |ui| {
                    ui.heading(tr(current));
                    ui.add_space(14.0);
                    match current {
                        "个人偏好" => self.settings_personal_prefs(ui),
                        "演示偏好" => self.settings_present_prefs(ui),
                        "离线缓存" => self.settings_offline_cache(ui),
                        "服务器" => self.settings_servers(ui),
                        "个人资料" => self.settings_profile(ui),
                        "我的提交" => self.my_submissions_body(ui),
                        "组织管理" => self.settings_org(ui),
                        "内容组与授权" => self.settings_grants(ui),
                        "审批队列" => self.review_queue_body(ui),
                        "高级功能与备份" => self.settings_advanced_and_backup(ui),
                        "公司公共资料" => self.settings_company_profile(ui),
                        "内容语言" => self.settings_languages(ui),
                        "AI 翻译" => self.settings_ai(ui),
                        "离线缓存策略" => self.settings_cache_policy(ui),
                        _ => {}
                    }
                });
            });
    }

    fn settings_personal_prefs(&mut self, ui: &mut egui::Ui) {
        settings_card(ui, |ui| {
            section_title(ui, "外观与语言", "控制本机客户端的主题外观与界面显示语言。");
            egui::Grid::new("personal_prefs_grid")
                .num_columns(2)
                .spacing([18.0, 12.0])
                .show(ui, |ui| {
                    ui.label(
                        egui::RichText::new(tr("界面主题"))
                            .size(13.0)
                            .color(palette_color(TEXT_WEAK)),
                    );
                    let cur_theme = usize::from(LIGHT_THEME.get());
                    if let Some(sel) =
                        segmented(ui, &["深色模式", "浅色模式"], cur_theme, SegStyle::Soft)
                    {
                        let light = sel == 1;
                        LIGHT_THEME.set(light);
                        apply_modern_style(ui.ctx());
                        save_client_settings(
                            json!({ "theme": if light { "light" } else { "dark" } }),
                        );
                        ui.ctx().request_repaint();
                        self.say(if light {
                            "已切换为浅色模式"
                        } else {
                            "已切换为深色模式"
                        });
                    }
                    ui.end_row();

                    ui.label(
                        egui::RichText::new(tr("界面语言"))
                            .size(13.0)
                            .color(palette_color(TEXT_WEAK)),
                    );
                    let cur_lang = usize::from(ENGLISH_UI.get());
                    if let Some(sel) =
                        segmented(ui, &["中文", "English"], cur_lang, SegStyle::Soft)
                    {
                        let english = sel == 1;
                        ENGLISH_UI.set(english);
                        save_client_settings(
                            json!({ "locale": if english { "en-us" } else { "zh-hans" } }),
                        );
                        ui.ctx().request_repaint();
                        self.say(if english {
                            "Interface language switched to English"
                        } else {
                            "界面语言已切换为中文"
                        });
                    }
                    ui.end_row();
                });
        });

        ui.add_space(12.0);
        settings_card(ui, |ui| {
            section_title(
                ui,
                "个人通讯录",
                "把常打交道的同事加入常用联系人，分享演示稿时一步直达。",
            );
            let mut remove_idx: Option<usize> = None;
            ui.horizontal_wrapped(|ui| {
                if self.my_contacts.is_empty() {
                    ui.label(
                        egui::RichText::new(tr("还没有常用联系人。"))
                            .size(12.0)
                            .color(palette_color(TEXT_WEAK)),
                    );
                } else {
                    for (idx, name) in self.my_contacts.iter().enumerate() {
                        egui::Frame::NONE
                            .fill(palette_color(HOVER_BG))
                            .stroke(egui::Stroke::new(1.0, palette_color(CARD_BORDER)))
                            .corner_radius(12)
                            .inner_margin(egui::Margin::symmetric(10, 4))
                            .show(ui, |ui| {
                                ui.horizontal(|ui| {
                                    ui.label(
                                        egui::RichText::new(name)
                                            .size(12.5)
                                            .color(palette_color(TEXT_MAIN)),
                                    );
                                    if ui
                                        .add(
                                            egui::Button::new(
                                                egui::RichText::new("×")
                                                    .size(12.0)
                                                    .color(palette_color(TEXT_WEAK)),
                                            )
                                            .frame(false),
                                        )
                                        .on_hover_text(tr("移除联系人"))
                                        .clicked()
                                    {
                                        remove_idx = Some(idx);
                                    }
                                });
                            });
                    }
                }
            });
            if let Some(idx) = remove_idx {
                let removed = self.my_contacts.remove(idx);
                save_client_settings(json!({ "my_contacts": self.my_contacts }));
                self.say(format!("已从个人通讯录移除「{removed}」"));
            }
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                ui.add_sized(
                    [260.0, 28.0],
                    egui::TextEdit::singleline(&mut self.settings.contact_input)
                        .hint_text(tr("输入同事姓名或账号添加")),
                );
                if ui.button(tr("添加到通讯录")).clicked() {
                    let name = self.settings.contact_input.trim().to_string();
                    if name.is_empty() {
                        self.say("请先输入联系人名称");
                    } else if self.my_contacts.iter().any(|x| x == &name) {
                        self.say(format!("「{name}」已在个人通讯录中"));
                    } else {
                        self.my_contacts.push(name.clone());
                        self.settings.contact_input.clear();
                        save_client_settings(json!({ "my_contacts": self.my_contacts }));
                        self.say(format!("已把「{name}」加入个人通讯录"));
                    }
                }
                if self.user.is_some() && self.role() != "member" {
                    let label = if self.settings.contact_picker_open {
                        "收起公司通讯录"
                    } else {
                        "从公司通讯录添加"
                    };
                    if ui.button(tr(label)).clicked() {
                        self.settings.contact_picker_open = !self.settings.contact_picker_open;
                        if self.settings.contact_picker_open && !self.settings.org_contacts_loaded {
                            self.settings.org_contacts_loaded = true;
                            if let Ok(resp) = self.call("GET", "/v1/org/contacts", None) {
                                self.settings.org_contacts =
                                    resp["users"].as_array().cloned().unwrap_or_default();
                            }
                        }
                    }
                }
            });
            if self.settings.contact_picker_open {
                ui.add_space(8.0);
                let mut add_person: Option<String> = None;
                let mut rem_person: Option<String> = None;
                ui.horizontal_wrapped(|ui| {
                    if self.settings.org_contacts.is_empty() {
                        ui.label(
                            egui::RichText::new(tr("暂无可添加的公司成员（需要组织连接且有权限）。"))
                                .size(11.5)
                                .color(palette_color(TEXT_WEAK)),
                        );
                    }
                    for c in &self.settings.org_contacts {
                        let dname = s(c, "display_name");
                        let uname = s(c, "username");
                        let label = if dname.is_empty() { uname } else { dname };
                        if label.is_empty() {
                            continue;
                        }
                        let active = self.my_contacts.iter().any(|x| x == &label);
                        let btn_text = if active {
                            format!("✓ {label}")
                        } else {
                            format!("＋ {label}")
                        };
                        if ui.selectable_label(active, btn_text).clicked() {
                            if active {
                                rem_person = Some(label);
                            } else {
                                add_person = Some(label);
                            }
                        }
                    }
                });
                if let Some(p) = add_person {
                    self.my_contacts.push(p.clone());
                    save_client_settings(json!({ "my_contacts": self.my_contacts }));
                    self.say(format!("已把「{p}」加入个人通讯录"));
                } else if let Some(p) = rem_person {
                    self.my_contacts.retain(|x| x != &p);
                    save_client_settings(json!({ "my_contacts": self.my_contacts }));
                    self.say(format!("已从个人通讯录移除「{p}」"));
                }
            }
        });

        ui.add_space(12.0);
        settings_card(ui, |ui| {
            section_title(ui, "关于与更新", "");
            ui.horizontal(|ui| {
                ui.label(
                    egui::RichText::new(format!("启幕桌面端 v{}", env!("CARGO_PKG_VERSION")))
                        .size(13.0)
                        .color(palette_color(TEXT_MAIN)),
                );
                if ui.button(tr("检查局域网更新")).clicked() {
                    self.check_lan_update(true);
                }
            });
            if let Some(note) = &self.upd_note {
                ui.add_space(4.0);
                ui.label(
                    egui::RichText::new(note.clone())
                        .size(12.0)
                        .color(palette_color(ACCENT_DEEP)),
                );
            }
        });
    }

    fn settings_present_prefs(&mut self, ui: &mut egui::Ui) {
        settings_card(ui, |ui| {
            section_title(
                ui,
                "放映打开方式",
                "播放、普通视图、演示者视图打开稿件时使用的呈现引擎。",
            );
            ui.horizontal(|ui| {
                for (val, label) in [
                    (PresentEngine::Builtin, "内置放映"),
                    (PresentEngine::DeckCraft, "DeckCraft"),
                    (PresentEngine::PowerPoint, "PowerPoint"),
                ] {
                    if ui.radio(self.present_engine == val, label).clicked()
                        && self.present_engine != val
                    {
                        self.present_engine = val;
                        save_client_settings(json!({ "present_engine": val.key() }));
                        self.say(format!("放映打开方式已设为「{label}」"));
                    }
                }
            });
            ui.add_space(6.0);
            ui.label(
                egui::RichText::new(
                    "「内置放映」在启幕内放映（默认）；「DeckCraft」用 DeckCraft 桌面应用打开；「PowerPoint」把稿件写到缓存目录并用 PowerPoint 打开，播放与演示者视图会自动开始放映。所选应用不可用时会明确提示，不会自动改用其他方式。",
                )
                .size(11.5)
                .color(palette_color(TEXT_WEAK)),
            );
            ui.add_space(10.0);
            if ui
                .checkbox(&mut self.dc_auto_quit, tr("放映结束自动关闭 DeckCraft"))
                .changed()
            {
                save_client_settings(json!({ "deckcraft_auto_quit": self.dc_auto_quit }));
            }
        });

        ui.add_space(12.0);
        settings_card(ui, |ui| {
            section_title(
                ui,
                "撤回步数",
                "Ctrl+Z 撤回 / Ctrl+Y 重做的操作步数（仅限本机库内操作）。",
            );
            ui.horizontal(|ui| {
                ui.label(tr("保留历史步数"));
                let resp = ui.add(egui::DragValue::new(&mut self.undo_limit).range(0..=200));
                ui.label(tr("步"));
                if resp.changed() {
                    save_client_settings(json!({ "undo_limit": self.undo_limit }));
                    let limit = self.undo_limit.max(0) as usize;
                    while self.undo_stack.len() > limit {
                        self.undo_stack.remove(0);
                    }
                    while self.redo_stack.len() > limit {
                        self.redo_stack.remove(0);
                    }
                    self.say(if self.undo_limit <= 0 {
                        "撤回已关闭".to_string()
                    } else {
                        format!("撤回步数已设为 {} 步", self.undo_limit)
                    });
                }
            });
            ui.add_space(6.0);
            ui.label(
                egui::RichText::new(
                    "移动、并入、删除、组合、粘贴与文件夹操作等可按 Ctrl+Z 逐步撤回，按 Ctrl+Y（或 Ctrl+Shift+Z）重做刚撤回的操作（本次运行内有效）。设为 0 关闭撤回与重做；调小步数会立即丢弃最早的记录。",
                )
                .size(11.5)
                .color(palette_color(TEXT_WEAK)),
            );
        });
    }

    fn settings_offline_cache(&mut self, ui: &mut egui::Ui) {
        settings_card(ui, |ui| {
            section_title(
                ui,
                "我的离线缓存",
                "管理本机自动缓存开关及已用容量；个人开关不得突破管理员策略上限。",
            );
            let policy_mode = cache_policy_mode(&self.cache_policy);
            let allow_auto = can_enable_auto_cache(&self.cache_policy);
            let cap_mb = self
                .cache_policy
                .get("max_mb")
                .and_then(|v| v.as_i64())
                .unwrap_or(500);
            let used_bytes: i64 = self.cache_entries.iter().map(|e| i(e, "bytes")).sum();
            let used_mb = used_bytes as f64 / 1_048_576.0;
            let pct = if cap_mb > 0 {
                (used_bytes as f64 / (cap_mb as f64 * 1024.0 * 1024.0)).clamp(0.0, 1.0) as f32
            } else {
                0.0
            };

            ui.horizontal(|ui| {
                ui.label(
                    egui::RichText::new(format!(
                        "管理员策略：{}",
                        cache_policy_label(policy_mode)
                    ))
                    .size(12.5)
                    .color(palette_color(TEXT_WEAK)),
                );
                badge(
                    ui,
                    &format!("上限 {} MB", cap_mb),
                    if policy_mode == "forbid" {
                        BadgeTone::Bad
                    } else {
                        BadgeTone::Info
                    },
                );
            });
            ui.add_space(8.0);
            let bar_w = ui.available_width().min(560.0);
            let (track, _) = ui.allocate_exact_size(egui::vec2(bar_w, 8.0), egui::Sense::hover());
            ui.painter()
                .rect_filled(track, 4.0, palette_color(HOVER_BG));
            if pct > 0.0 {
                let fill_rect =
                    egui::Rect::from_min_size(track.min, egui::vec2(track.width() * pct, 8.0));
                ui.painter().rect_filled(fill_rect, 4.0, ACCENT);
            }
            ui.add_space(6.0);
            ui.label(
                egui::RichText::new(format!(
                    "已缓存 {} 条 · 本地已用 {:.1} MB / 上限 {} MB",
                    self.cache_entries.len(),
                    used_mb,
                    cap_mb
                ))
                .size(12.0)
                .color(palette_color(TEXT_WEAK)),
            );
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                let toggle_label = if self.cache_auto && allow_auto {
                    "关闭自动缓存"
                } else {
                    "开启自动缓存我用过的稿件"
                };
                if btn_default_enabled(ui, allow_auto, tr(toggle_label)).clicked() {
                    self.cache_auto = !self.cache_auto;
                    save_client_settings(json!({ "auto_cache": self.cache_auto }));
                    self.say(if self.cache_auto {
                        if policy_mode == "manual" {
                            "已开启自动缓存偏好（当前管理员策略为「仅手动缓存」，打开稿件时不会被动触发）"
                        } else {
                            "已开启自动缓存我用过的稿件"
                        }
                    } else {
                        "已关闭自动缓存"
                    });
                }
                if !self.cache_entries.is_empty() && btn_danger(ui, tr("清空本地缓存")).clicked()
                {
                    let keys: Vec<String> =
                        self.cache_entries.iter().map(|e| s(e, "key")).collect();
                    for k in keys {
                        self.remove_cache(&k);
                    }
                    self.say("已清空本机全部离线缓存");
                }
            });
            if !allow_auto {
                ui.add_space(4.0);
                ui.label(
                    egui::RichText::new(tr("管理员已禁止本地离线缓存，无法开启自动缓存。"))
                        .size(11.5)
                        .color(palette_color(ROSE)),
                );
            }
        });

        ui.add_space(12.0);
        settings_card(ui, |ui| {
            self.cache_path_ui(ui);
        });

        ui.add_space(12.0);
        settings_card(ui, |ui| {
            section_title(
                ui,
                "本地库备份",
                "把本机库（qimu.db 与内容文件）完整复制到选定目录作为离线冷备。",
            );
            ui.horizontal(|ui| {
                if btn_primary(ui, tr("选择目录立即备份本地库…")).clicked() {
                    if let Some(dir) = rfd::FileDialog::new().pick_folder() {
                        let ts = qimu_core::auth::now_ms();
                        let dest = dir.join(format!("qimu-local-backup-{ts}"));
                        match self.local.as_ref() {
                            Some(local) => match local.backup_to(&dest) {
                                Ok(files) => {
                                    let note = format!(
                                        "备份完成：{}（含 {} 个文件）",
                                        dest.display(),
                                        files
                                    );
                                    self.settings.local_backup_note = note.clone();
                                    save_client_settings(json!({ "last_local_backup": note }));
                                    self.say("本地库已备份");
                                }
                                Err(e) => {
                                    self.settings.local_backup_note = format!("备份失败：{e}");
                                    self.say(format!("本地库备份失败：{e}"));
                                }
                            },
                            None => self.say("本地库尚未打开，无法备份"),
                        }
                    }
                }
            });
            if self.settings.local_backup_note.is_empty() {
                if let Some(saved) = client_settings()
                    .get("last_local_backup")
                    .and_then(|v| v.as_str())
                {
                    self.settings.local_backup_note = saved.to_string();
                }
            }
            if !self.settings.local_backup_note.is_empty() {
                ui.add_space(6.0);
                ui.label(
                    egui::RichText::new(&self.settings.local_backup_note)
                        .size(11.5)
                        .color(palette_color(TEXT_WEAK)),
                );
            }
        });
    }

    /// 缓存路径（本机设置）、本地库诊断、数据目录搬迁与旧库处置。
    pub(super) fn cache_path_ui(&mut self, ui: &mut egui::Ui) {
        ui.label(
            egui::RichText::new("缓存路径（本机设置）")
                .strong()
                .size(15.5)
                .color(palette_color(TEXT_MAIN)),
        );
        ui.add_space(4.0);
        ui.label(
            egui::RichText::new(
                "离线缓存的加密快照保存在这个目录，装在本机的所有账号共用；与管理员下发的缓存策略无关。",
            )
            .size(11.5)
            .color(palette_color(TEXT_WEAK)),
        );
        ui.add_space(8.0);
        let effective = cache_dir();
        let total_bytes: u64 = dir_manifest(&effective).iter().map(|(_, n)| n).sum();
        ui.label(
            egui::RichText::new(format!(
                "当前生效：{} · {} 条缓存 · {:.1} MB",
                effective.display(),
                self.cache_entries.len(),
                total_bytes as f64 / 1_048_576.0
            ))
            .size(11.5)
            .color(palette_color(TEXT_WEAK)),
        );
        let user_set = client_settings()
            .get("cache_dir")
            .and_then(|v| v.as_str())
            .is_some_and(|s| !s.trim().is_empty());
        if !user_set {
            if let Some(p) = platform_cache_resolved() {
                if p == effective {
                    ui.label(
                        egui::RichText::new(
                            "未手动指定路径：当前使用平台默认位置（Windows 优先 D 盘、macOS 优先非系统卷）。",
                        )
                        .size(11.5)
                        .color(palette_color(TEXT_WEAK)),
                    );
                }
            }
        }
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            ui.add_sized(
                [420.0, 30.0],
                egui::TextEdit::singleline(&mut self.cache_dir_input),
            );
            if ui.button("保存路径").clicked() {
                let p = std::path::PathBuf::from(self.cache_dir_input.trim());
                if p.as_os_str().is_empty() {
                    self.msg = "缓存路径不能为空".into();
                } else {
                    match std::fs::create_dir_all(&p) {
                        Ok(()) => {
                            save_client_settings(json!({ "cache_dir": p.to_string_lossy() }));
                            self.cache_entries = load_cache_index();
                            self.msg = "缓存路径已保存，新缓存将写入新目录".into();
                        }
                        Err(e) => self.msg = format!("缓存路径不可用：{e}"),
                    }
                }
            }
            if ui.button("恢复默认").clicked() {
                save_client_settings(json!({ "cache_dir": Value::Null }));
                self.cache_dir_input = default_cache_dir().to_string_lossy().into_owned();
                self.cache_entries = load_cache_index();
                self.msg = "缓存路径已恢复默认".into();
            }
        });
        ui.add_space(12.0);
        ui.label(
            egui::RichText::new("本地库")
                .strong()
                .size(14.0)
                .color(palette_color(TEXT_MAIN)),
        );
        ui.add_space(4.0);
        let local_diag = match &self.local {
            Some(local) => {
                let (units, folders, indexes) = local.counts();
                format!(
                    "已打开：{} · 所有者 {} · 内容块 {} · 目录 {} · 索引 {}",
                    local.root.display(),
                    local.owner.display_name,
                    units,
                    folders,
                    indexes
                )
            }
            None => match &self.local_open_error {
                Some(e) => format!(
                    "本地库打开失败：{e}（库根 {}）",
                    local_store::library_dir().display()
                ),
                None => format!(
                    "本地库打开失败（原因见控制台日志）；库根解析为 {}",
                    local_store::library_dir().display()
                ),
            },
        };
        ui.label(
            egui::RichText::new(local_diag)
                .size(11.5)
                .color(palette_color(TEXT_WEAK)),
        );
        ui.add_space(12.0);
        ui.label(
            egui::RichText::new("数据目录")
                .strong()
                .size(14.0)
                .color(palette_color(TEXT_MAIN)),
        );
        ui.add_space(4.0);
        ui.label(
            egui::RichText::new(
                "本地库（内容块、索引、页面与演示数据）存放的目录。改选新目录后，本次运行继续使用当前目录不受影响；软件会在下次启动时把当前库完整复制到新目录并核对内容，核对通过才切换，失败则继续使用当前目录并给出提示。新目录里已有其他启幕库时不会覆盖或合并，保存会被拒绝。",
            )
            .size(11.5)
            .color(palette_color(TEXT_WEAK)),
        );
        ui.add_space(4.0);
        let resolution = local_store::library_resolution();
        let origin_note = match resolution.origin {
            local_store::LibraryOrigin::Bootstrap => "来自配置文件",
            local_store::LibraryOrigin::LegacySetting => "来自既有设置",
            local_store::LibraryOrigin::Env => "来自环境变量 QIMU_DATA_DIR",
            local_store::LibraryOrigin::Standard => "标准目录",
            local_store::LibraryOrigin::Migrated => "本次启动由旧版库搬迁而来",
            local_store::LibraryOrigin::FallbackSource => "旧库搬迁未成，暂按原目录使用",
        };
        let current_root = self
            .local
            .as_ref()
            .map(|l| l.root.display().to_string())
            .unwrap_or_else(|| resolution.dir.display().to_string());
        ui.label(
            egui::RichText::new(format!(
                "当前生效：{}（{}）；配置文件：{}",
                current_root,
                origin_note,
                local_store::bootstrap_path().display()
            ))
            .size(11.5)
            .color(palette_color(TEXT_WEAK)),
        );
        if let Some(pend) = local_store::read_bootstrap().pending_relocate {
            ui.label(
                egui::RichText::new(format!(
                    "已登记下次启动搬迁到：{}（重启软件后执行）",
                    pend.to
                ))
                .size(11.5)
                .color(palette_color(ACCENT_DEEP)),
            );
        }
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            ui.add_sized(
                [420.0, 30.0],
                egui::TextEdit::singleline(&mut self.data_dir_input),
            );
            if ui.button("选择目录…").clicked() {
                if let Some(p) = rfd::FileDialog::new().pick_folder() {
                    self.data_dir_input = p.to_string_lossy().into_owned();
                }
            }
        });
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            if ui.button("保存并搬迁").clicked() {
                let target = std::path::PathBuf::from(self.data_dir_input.trim());
                if target.as_os_str().is_empty() {
                    self.msg = "数据目录不能为空".into();
                } else {
                    self.apply_data_dir_change(target);
                }
            }
            if ui.button("恢复默认目录").clicked() {
                self.apply_data_dir_change(local_store::standard_library_dir());
            }
        });
        ui.add_space(12.0);
        ui.label(
            egui::RichText::new("旧库处置（升级搬迁后）")
                .strong()
                .size(14.0)
                .color(palette_color(TEXT_MAIN)),
        );
        ui.add_space(4.0);
        let policy = local_store::read_bootstrap().policy();
        ui.horizontal(|ui| {
            for (val, label) in [
                (local_store::OldLibraryPolicy::Ask, "询问"),
                (local_store::OldLibraryPolicy::Delete, "删除"),
                (local_store::OldLibraryPolicy::Keep, "不删除"),
            ] {
                if ui.radio(policy == val, label).clicked() && policy != val {
                    let mut b = local_store::read_bootstrap();
                    b.old_library_policy = Some(val);
                    local_store::write_bootstrap(&b);
                    self.msg = format!("旧库处置已设为「{label}」");
                }
            }
        });
        ui.add_space(4.0);
        ui.label(
            egui::RichText::new(
                "升级时旧库被搬到标准目录、且新库核对通过并启用之后，怎么处置原来的旧库：「询问」每次弹窗问你（默认）；「删除」直接删；「不删除」一直保留。搬迁未成功时旧库就是正在使用的库，任何设置下都不会删。",
            )
            .size(11.5)
            .color(palette_color(TEXT_WEAK)),
        );
    }

    pub(super) fn apply_data_dir_change(&mut self, target: std::path::PathBuf) {
        let current = self
            .local
            .as_ref()
            .map(|l| l.root.clone())
            .unwrap_or_else(local_store::library_dir);
        if target == current {
            self.say("所选目录就是当前数据目录，无需搬迁");
            return;
        }
        if target.starts_with(&current) || current.starts_with(&target) {
            self.say("新目录不能位于当前数据目录之内，也不能包含当前数据目录");
            return;
        }
        if target.exists() && !target.is_dir() {
            self.say("所选路径是一个文件，不能用作数据目录");
            return;
        }
        if local_store::probe_library(&target)
            .map(|c| c.has_content())
            .unwrap_or(false)
        {
            self.say("新目录里已有另一个启幕库，不会覆盖或合并；请换一个空目录");
            return;
        }
        let mut b = local_store::read_bootstrap();
        b.pending_relocate = Some(local_store::PendingRelocate {
            from: current.to_string_lossy().into_owned(),
            to: target.to_string_lossy().into_owned(),
        });
        local_store::write_bootstrap(&b);
        self.say(format!(
            "已登记：下次启动时把数据目录从 {} 搬迁到 {}；搬迁在启动时执行并核对内容，失败则继续使用当前目录。本次运行不受影响",
            current.display(),
            target.display()
        ));
    }

    fn settings_profile(&mut self, ui: &mut egui::Ui) {
        let mut save_clicked = false;
        settings_card(ui, |ui| {
            section_title(
                ui,
                "个人资料（对外绑定字段）",
                "封面 / 尾页中的个人占位字段；导出「个人版」时自动替换为本人信息。",
            );
            ui.horizontal_top(|ui| {
                ui.vertical(|ui| {
                    egui::Grid::new("profile_grid")
                        .num_columns(2)
                        .spacing([14.0, 12.0])
                        .show(ui, |ui| {
                            for (key, label, hint) in [
                                ("name", "姓名", "汇报人姓名"),
                                ("title", "职务", "例如：客户经理"),
                                ("english_name", "英文名", "例如：James Dai"),
                                ("phone", "手机号", "联系电话"),
                                ("email", "邮箱", "工作邮箱"),
                                ("wechat", "微信二维码", "微信号或二维码说明"),
                                ("linkedin", "领英 LinkedIn", "主页链接（选填）"),
                                ("x", "X (Twitter)", "@账号（选填）"),
                                ("facebook", "Facebook", "主页链接（选填）"),
                                ("instagram", "Instagram", "@账号（选填）"),
                                ("youtube", "YouTube", "频道链接（选填）"),
                                ("tiktok", "TikTok", "@账号（选填）"),
                                ("weibo", "微博", "账号名称（选填）"),
                            ] {
                                ui.label(
                                    egui::RichText::new(label)
                                        .size(12.5)
                                        .color(palette_color(TEXT_WEAK)),
                                );
                                let v = self
                                    .settings
                                    .profile
                                    .entry(key.to_string())
                                    .or_default();
                                ui.add_sized(
                                    [320.0, 30.0],
                                    egui::TextEdit::singleline(v)
                                        .hint_text(hint)
                                        .margin(egui::vec2(8.0, 6.0)),
                                );
                                ui.end_row();
                            }
                            ui.label("");
                            if btn_primary(ui, tr("保存资料")).clicked() {
                                save_clicked = true;
                            }
                            ui.end_row();
                        });
                });
                ui.add_space(28.0);
                ui.vertical(|ui| {
                    ui.label(egui::RichText::new("个人绑定预览").strong().size(15.0));
                    ui.add_space(8.0);
                    let p = &self.settings.profile;
                    let get = |k: &str| p.get(k).cloned().unwrap_or_default();
                    let dname = self
                        .user
                        .as_ref()
                        .map(|u| s(u, "display_name"))
                        .unwrap_or_default();
                    let pname = if get("name").is_empty() {
                        dname
                    } else {
                        get("name")
                    };
                    egui::Frame::NONE
                        .fill(egui::Color32::WHITE)
                        .stroke(egui::Stroke::new(1.0, palette_color(CARD_BORDER)))
                        .corner_radius(egui::CornerRadius::same(8))
                        .inner_margin(egui::Margin::same(14))
                        .show(ui, |ui| {
                            ui.set_min_size(egui::vec2(290.0, 175.0));
                            let (bar, _) = ui
                                .allocate_exact_size(egui::vec2(140.0, 8.0), egui::Sense::hover());
                            ui.painter().rect_filled(
                                bar,
                                0.0,
                                egui::Color32::from_rgb(0x1F, 0x38, 0x64),
                            );
                            ui.add_space(10.0);
                            let eng = get("english_name");
                            let head = if eng.is_empty() {
                                format!("汇报人：{} · {}", pname, get("title"))
                            } else {
                                format!("汇报人：{} ({}) · {}", pname, eng, get("title"))
                            };
                            ui.label(
                                egui::RichText::new(head)
                                    .size(11.5)
                                    .strong()
                                    .color(egui::Color32::from_rgb(0x22, 0x22, 0x22)),
                            );
                            ui.add_space(4.0);
                            ui.label(
                                egui::RichText::new(format!("{} · {}", get("phone"), get("email")))
                                    .size(11.0)
                                    .color(egui::Color32::from_rgb(0x55, 0x55, 0x55)),
                            );
                            let social: Vec<String> = [
                                ("LinkedIn", "linkedin"),
                                ("X", "x"),
                                ("微信", "wechat"),
                            ]
                            .iter()
                            .filter_map(|(k, fk)| {
                                let val = get(fk);
                                (!val.is_empty()).then(|| format!("{k}: {val}"))
                            })
                            .collect();
                            if !social.is_empty() {
                                ui.add_space(4.0);
                                ui.label(
                                    egui::RichText::new(social.join(" · "))
                                        .size(10.5)
                                        .color(egui::Color32::from_rgb(0x66, 0x77, 0x88)),
                                );
                            }
                            ui.add_space(10.0);
                            let (img, _) = ui
                                .allocate_exact_size(egui::vec2(84.0, 56.0), egui::Sense::hover());
                            ui.painter().rect_filled(
                                img,
                                4.0,
                                egui::Color32::from_rgb(0xD8, 0xE0, 0xEA),
                            );
                            ui.painter().text(
                                img.center(),
                                egui::Align2::CENTER_CENTER,
                                if get("wechat").is_empty() {
                                    "二维码占位"
                                } else {
                                    "微信名片"
                                },
                                egui::FontId::proportional(10.0),
                                egui::Color32::from_rgb(0x47, 0x55, 0x69),
                            );
                        });
                });
            });
        });

        if save_clicked {
            let fields: Value = json!(self.settings.profile);
            match self.call("PUT", "/v1/me/profile", Some(json!({ "fields": fields }))) {
                Ok(_) => self.say("个人资料已保存"),
                Err(e) => self.msg = e,
            }
        }

        ui.add_space(12.0);
        settings_card(ui, |ui| {
            section_title(
                ui,
                "修改登录密码",
                "修改当前账号密码；修改成功后会自动退出其他设备上的登录会话。",
            );
            let anon = self.anon_active();
            if anon {
                ui.label(
                    egui::RichText::new(tr("匿名连接无需设置或修改密码。"))
                        .size(12.0)
                        .color(palette_color(TEXT_WEAK)),
                );
            } else {
                egui::Grid::new("pw_change_grid")
                    .num_columns(2)
                    .spacing([14.0, 10.0])
                    .show(ui, |ui| {
                        ui.label(tr("当前密码"));
                        ui.add_sized(
                            [260.0, 28.0],
                            egui::TextEdit::singleline(&mut self.settings.pw_old).password(true),
                        );
                        ui.end_row();
                        ui.label(tr("新密码"));
                        ui.add_sized(
                            [260.0, 28.0],
                            egui::TextEdit::singleline(&mut self.settings.pw_new)
                                .password(true)
                                .hint_text(tr("至少 8 位")),
                        );
                        ui.end_row();
                        ui.label(tr("确认新密码"));
                        ui.add_sized(
                            [260.0, 28.0],
                            egui::TextEdit::singleline(&mut self.settings.pw_new2).password(true),
                        );
                        ui.end_row();
                    });
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    if btn_default(ui, tr("确认修改密码")).clicked()
                        && self.require_org_online("修改密码")
                    {
                        if self.settings.pw_new.chars().count() < 8 {
                            self.settings.pw_status = "新密码至少需要 8 位".into();
                        } else if self.settings.pw_new != self.settings.pw_new2 {
                            self.settings.pw_status = "两次输入的新密码不一致".into();
                        } else {
                            match self.call(
                                "POST",
                                "/v1/me/password",
                                Some(json!({
                                    "current_password": self.settings.pw_old,
                                    "new_password": self.settings.pw_new,
                                })),
                            ) {
                                Ok(_) => {
                                    self.settings.pw_old.clear();
                                    self.settings.pw_new.clear();
                                    self.settings.pw_new2.clear();
                                    self.settings.pw_status = "密码已修改".into();
                                    self.say("密码已修改");
                                }
                                Err(e) => {
                                    self.settings.pw_status = format!("修改失败：{e}");
                                }
                            }
                        }
                    }
                    if !self.settings.pw_status.is_empty() {
                        ui.label(
                            egui::RichText::new(&self.settings.pw_status)
                                .size(12.0)
                                .color(if self.settings.pw_status.starts_with("修改失败")
                                    || self.settings.pw_status.contains("不一致")
                                    || self.settings.pw_status.contains("至少")
                                {
                                    palette_color(ROSE)
                                } else {
                                    palette_color(CHIP_GREEN_FG)
                                }),
                        );
                    }
                });
            }
        });
    }

    fn settings_org(&mut self, ui: &mut egui::Ui) {
        let depts = self
            .org
            .get("departments")
            .and_then(|d| d.as_array())
            .cloned()
            .unwrap_or_default();
        let users = self
            .org
            .get("users")
            .and_then(|d| d.as_array())
            .cloned()
            .unwrap_or_default();
        let is_super = self.role() == "super";
        let my_dept_id = self.user.as_ref().map(|u| s(u, "dept_id")).unwrap_or_default();

        let dept_name_of = |id: &str| -> String {
            if id.is_empty() {
                return "全公司（未分部门）".into();
            }
            depts
                .iter()
                .find(|d| s(d, "id") == id)
                .map(|d| s(d, "name"))
                .unwrap_or_else(|| id.to_string())
        };

        let dept_within_client = |dept_id: &str, ancestor_id: &str| -> bool {
            if ancestor_id.is_empty() {
                return false;
            }
            let mut cur = dept_id.to_string();
            for _ in 0..=depts.len() {
                if cur == ancestor_id {
                    return true;
                }
                let next = depts
                    .iter()
                    .find(|d| s(d, "id") == cur)
                    .and_then(|d| d["parent_id"].as_str())
                    .map(|x| x.to_string());
                match next {
                    Some(p) => cur = p,
                    None => break,
                }
            }
            false
        };

        // Scope filter for dept_admin: limit to own dept subtree
        let visible_depts: Vec<Value> = if is_super {
            depts.clone()
        } else {
            depts
                .iter()
                .filter(|d| dept_within_client(&s(d, "id"), &my_dept_id))
                .cloned()
                .collect()
        };
        if !is_super && !my_dept_id.is_empty() {
            let valid_sel = self
                .settings
                .org_sel_dept
                .as_deref()
                .is_some_and(|id| dept_within_client(id, &my_dept_id));
            if !valid_sel {
                self.settings.org_sel_dept = Some(my_dept_id.clone());
            }
        }

        // Ordered tree walk
        let mut tree_rows: Vec<(Value, usize)> = Vec::new();
        fn walk_depts(
            all: &[Value],
            parent: Option<&str>,
            depth: usize,
            out: &mut Vec<(Value, usize)>,
        ) {
            for d in all {
                let p = d["parent_id"].as_str();
                if p == parent {
                    out.push((d.clone(), depth));
                    walk_depts(all, Some(&s(d, "id")), depth + 1, out);
                }
            }
        }
        if is_super {
            walk_depts(&depts, None, 0, &mut tree_rows);
            for d in &depts {
                if !tree_rows.iter().any(|(x, _)| s(x, "id") == s(d, "id")) {
                    tree_rows.push((d.clone(), 0));
                }
            }
        } else {
            for d in &visible_depts {
                let mut depth = 0;
                let mut cur = s(d, "id");
                while cur != my_dept_id && depth < 8 {
                    if let Some(p) = depts
                        .iter()
                        .find(|x| s(x, "id") == cur)
                        .and_then(|x| x["parent_id"].as_str())
                    {
                        cur = p.to_string();
                        depth += 1;
                    } else {
                        break;
                    }
                }
                tree_rows.push((d.clone(), depth));
            }
        }

        settings_card(ui, |ui| {
            section_title(
                ui,
                "部门组织树",
                if is_super {
                    "选择部门即可查看其成员、重命名或在所选部门下新建子部门。"
                } else {
                    "部门管理员可在本人管辖部门及子部门内维护组织。"
                },
            );
            if is_super
                && tree_row(
                    ui,
                    self.settings.org_sel_dept.is_none(),
                    0.0,
                    Some(ChromeIcon::Home),
                    &format!("全公司（共 {} 人）", users.len()),
                    13.0,
                )
                .clicked()
            {
                self.settings.org_sel_dept = None;
                self.settings.dept_rename.clear();
            }
            for (d, depth) in &tree_rows {
                let did = s(d, "id");
                let dname = s(d, "name");
                let direct_cnt = users.iter().filter(|u| s(u, "dept_id") == did).count();
                let total_cnt = users
                    .iter()
                    .filter(|u| dept_within_client(&s(u, "dept_id"), &did))
                    .count();
                let selected = self.settings.org_sel_dept.as_deref() == Some(did.as_str());
                let row_label = if total_cnt > direct_cnt {
                    format!("{dname}（直属 {direct_cnt} · 含下级 {total_cnt}）")
                } else {
                    format!("{dname}（{direct_cnt} 人）")
                };
                if tree_row(
                    ui,
                    selected,
                    *depth as f32 * 16.0,
                    Some(ChromeIcon::Directory),
                    &row_label,
                    12.5,
                )
                .clicked()
                {
                    self.settings.org_sel_dept = Some(did);
                    self.settings.dept_rename = dname;
                }
            }
            ui.add_space(10.0);
            if let Some(sel_id) = self.settings.org_sel_dept.clone() {
                if self.settings.dept_rename.is_empty() {
                    self.settings.dept_rename = dept_name_of(&sel_id);
                }
                ui.horizontal(|ui| {
                    ui.label(tr("重命名选中部门"));
                    ui.add_sized(
                        [200.0, 26.0],
                        egui::TextEdit::singleline(&mut self.settings.dept_rename),
                    );
                    if btn_default_enabled(
                        ui,
                        !self.settings.dept_rename.trim().is_empty(),
                        tr("保存部门名"),
                    )
                    .clicked()
                        && self.require_org_online("重命名部门")
                    {
                        match self.call(
                            "POST",
                            &format!("/v1/org/departments/{sel_id}/rename"),
                            Some(json!({ "name": self.settings.dept_rename.trim() })),
                        ) {
                            Ok(_) => {
                                self.say("部门已重命名");
                                self.refresh_all();
                            }
                            Err(e) => self.msg = e,
                        }
                    }
                });
                ui.add_space(6.0);
            }
            ui.horizontal(|ui| {
                let target_note = match &self.settings.org_sel_dept {
                    Some(pid) => format!("在「{}」下建子部门", dept_name_of(pid)),
                    None => "新建一级部门".into(),
                };
                ui.label(target_note);
                ui.add_sized(
                    [200.0, 26.0],
                    egui::TextEdit::singleline(&mut self.settings.dept_name)
                        .hint_text(tr("输入新部门名称")),
                );
                let can_create_dept = !self.settings.dept_name.trim().is_empty()
                    && (is_super || self.settings.org_sel_dept.is_some());
                if btn_default_enabled(ui, can_create_dept, tr("添加部门")).clicked()
                    && self.require_org_online("添加部门")
                {
                    let body = match &self.settings.org_sel_dept {
                        Some(pid) => json!({
                            "name": self.settings.dept_name.trim(),
                            "parent_id": pid,
                        }),
                        None => json!({ "name": self.settings.dept_name.trim() }),
                    };
                    match self.call("POST", "/v1/org/departments", Some(body)) {
                        Ok(_) => {
                            self.settings.dept_name.clear();
                            self.say("部门已添加");
                            self.refresh_all();
                        }
                        Err(e) => self.msg = e,
                    }
                }
            });
        });

        ui.add_space(12.0);
        settings_card(ui, |ui| {
            let scope_title = match &self.settings.org_sel_dept {
                Some(did) => format!("成员列表 · {}", dept_name_of(did)),
                None => "成员列表 · 全公司".into(),
            };
            section_title(
                ui,
                &scope_title,
                "包含账号、姓名、所属部门、系统角色与启用状态。",
            );
            let filtered_users: Vec<Value> = users
                .iter()
                .filter(|u| match &self.settings.org_sel_dept {
                    Some(did) => dept_within_client(&s(u, "dept_id"), did),
                    None => true,
                })
                .cloned()
                .collect();

            if filtered_users.is_empty() {
                ui.label(
                    egui::RichText::new(tr("该范围暂无成员。"))
                        .size(12.0)
                        .color(palette_color(TEXT_WEAK)),
                );
            }

            let mut toggle_disable: Option<(String, bool)> = None;
            let mut begin_edit_user: Option<Value> = None;
            for u in &filtered_users {
                let uid = s(u, "id");
                let uname = s(u, "username");
                let dname = s(u, "display_name");
                let urole = s(u, "role");
                let udept = s(u, "dept_id");
                let disabled = u.get("disabled").and_then(|v| v.as_bool()) == Some(true);
                egui::Frame::NONE
                    .fill(palette_color(HOVER_BG))
                    .corner_radius(6)
                    .inner_margin(egui::Margin::symmetric(10, 7))
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.label(
                                egui::RichText::new(format!("{dname} ({uname})"))
                                    .strong()
                                    .size(12.5)
                                    .color(palette_color(TEXT_MAIN)),
                            );
                            badge(
                                ui,
                                role_label(&urole),
                                match urole.as_str() {
                                    "super" => BadgeTone::Warn,
                                    "dept_admin" => BadgeTone::Info,
                                    _ => BadgeTone::Neutral,
                                },
                            );
                            ui.label(
                                egui::RichText::new(dept_name_of(&udept))
                                    .size(11.5)
                                    .color(palette_color(TEXT_WEAK)),
                            );
                            if disabled {
                                badge(ui, "已停用", BadgeTone::Bad);
                            }
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    if is_super && uid != self.my_id {
                                        if ui
                                            .button(if disabled { "启用" } else { "停用" })
                                            .clicked()
                                        {
                                            toggle_disable = Some((uid.clone(), !disabled));
                                        }
                                        if ui.button("调整部门 / 角色").clicked() {
                                            begin_edit_user = Some(u.clone());
                                        }
                                    }
                                },
                            );
                        });
                    });
                ui.add_space(4.0);
            }
            if let Some((uid, next_disabled)) = toggle_disable {
                if self.require_org_online(if next_disabled { "停用账号" } else { "启用账号" })
                {
                    match self.call(
                        "POST",
                        &format!("/v1/org/users/{uid}/disable"),
                        Some(json!({ "disabled": next_disabled })),
                    ) {
                        Ok(_) => {
                            self.say(if next_disabled {
                                "账号已停用"
                            } else {
                                "账号已启用"
                            });
                            self.refresh_all();
                        }
                        Err(e) => self.msg = e,
                    }
                }
            }
            if let Some(u) = begin_edit_user {
                self.settings.edit_user_id = s(&u, "id");
                self.settings.edit_user_dept = s(&u, "dept_id");
                self.settings.edit_user_role = s(&u, "role");
            }

            if is_super && !self.settings.edit_user_id.is_empty() {
                let target_label = users
                    .iter()
                    .find(|u| s(u, "id") == self.settings.edit_user_id)
                    .map(|u| format!("{} ({})", s(u, "display_name"), s(u, "username")))
                    .unwrap_or_else(|| self.settings.edit_user_id.clone());
                ui.add_space(8.0);
                egui::Frame::NONE
                    .fill(palette_color(CARD_BG))
                    .stroke(egui::Stroke::new(1.0, ACCENT))
                    .corner_radius(6)
                    .inner_margin(egui::Margin::same(10))
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.label(format!("正在调整：{target_label}"));
                            egui::ComboBox::from_id_salt("edit_user_dept")
                                .selected_text(dept_name_of(&self.settings.edit_user_dept))
                                .show_ui(ui, |ui| {
                                    ui.selectable_value(
                                        &mut self.settings.edit_user_dept,
                                        String::new(),
                                        "无（未分配部门）",
                                    );
                                    for d in &depts {
                                        ui.selectable_value(
                                            &mut self.settings.edit_user_dept,
                                            s(d, "id"),
                                            s(d, "name"),
                                        );
                                    }
                                });
                            egui::ComboBox::from_id_salt("edit_user_role")
                                .selected_text(role_label(&self.settings.edit_user_role))
                                .show_ui(ui, |ui| {
                                    for (rk, rl) in [
                                        ("member", "普通成员"),
                                        ("dept_admin", "部门管理员"),
                                        ("super", "超级管理员"),
                                    ] {
                                        ui.selectable_value(
                                            &mut self.settings.edit_user_role,
                                            rk.into(),
                                            rl,
                                        );
                                    }
                                });
                            if btn_primary(ui, "保存调整").clicked()
                                && self.require_org_online("调整成员部门与角色")
                            {
                                let uid = self.settings.edit_user_id.clone();
                                let dept_arg = if self.settings.edit_user_dept.trim().is_empty() {
                                    Value::Null
                                } else {
                                    json!(self.settings.edit_user_dept)
                                };
                                match self.call(
                                    "POST",
                                    &format!("/v1/org/users/{uid}/update"),
                                    Some(json!({
                                        "dept_id": dept_arg,
                                        "role": self.settings.edit_user_role,
                                    })),
                                ) {
                                    Ok(_) => {
                                        self.settings.edit_user_id.clear();
                                        self.say("成员部门与角色已更新");
                                        self.refresh_all();
                                    }
                                    Err(e) => self.msg = e,
                                }
                            }
                            if btn_weak(ui, "取消").clicked() {
                                self.settings.edit_user_id.clear();
                            }
                        });
                    });
            }

            ui.add_space(12.0);
            ui.separator();
            ui.add_space(10.0);
            ui.label(egui::RichText::new("创建新成员账号").strong().size(13.5));
            ui.add_space(6.0);
            if !is_super {
                self.settings.user_role_pick = "member".into();
                if self.settings.user_dept_pick.is_empty() {
                    self.settings.user_dept_pick = my_dept_id.clone();
                }
            } else if self.settings.user_role_pick.is_empty() {
                self.settings.user_role_pick = "member".into();
            }
            egui::Grid::new("create_user_grid")
                .num_columns(4)
                .spacing([10.0, 8.0])
                .show(ui, |ui| {
                    ui.label("登录账号");
                    ui.add_sized(
                        [160.0, 26.0],
                        egui::TextEdit::singleline(&mut self.settings.user_name)
                            .hint_text("例如：zhangsan"),
                    );
                    ui.label("显示姓名");
                    ui.add_sized(
                        [160.0, 26.0],
                        egui::TextEdit::singleline(&mut self.settings.user_display)
                            .hint_text("例如：张三"),
                    );
                    ui.end_row();
                    ui.label("初始密码");
                    ui.add_sized(
                        [160.0, 26.0],
                        egui::TextEdit::singleline(&mut self.settings.user_password)
                            .password(true)
                            .hint_text("至少 8 位"),
                    );
                    ui.label("所属部门");
                    egui::ComboBox::from_id_salt("new_user_dept")
                        .selected_text(dept_name_of(&self.settings.user_dept_pick))
                        .show_ui(ui, |ui| {
                            if is_super {
                                ui.selectable_value(
                                    &mut self.settings.user_dept_pick,
                                    String::new(),
                                    "无（全公司根）",
                                );
                            }
                            for d in &visible_depts {
                                ui.selectable_value(
                                    &mut self.settings.user_dept_pick,
                                    s(d, "id"),
                                    s(d, "name"),
                                );
                            }
                        });
                    ui.end_row();
                    if is_super {
                        ui.label("系统角色");
                        egui::ComboBox::from_id_salt("new_user_role")
                            .selected_text(role_label(&self.settings.user_role_pick))
                            .show_ui(ui, |ui| {
                                for (rk, rl) in [
                                    ("member", "普通成员"),
                                    ("dept_admin", "部门管理员"),
                                    ("super", "超级管理员"),
                                ] {
                                    ui.selectable_value(
                                        &mut self.settings.user_role_pick,
                                        rk.into(),
                                        rl,
                                    );
                                }
                            });
                        ui.label("");
                        ui.label("");
                        ui.end_row();
                    }
                });
            ui.add_space(8.0);
            let can_create = !self.settings.user_name.trim().is_empty()
                && !self.settings.user_display.trim().is_empty()
                && self.settings.user_password.chars().count() >= 8;
            if btn_primary(ui, tr("创建账号"))
                .on_disabled_hover_text("需填账号、姓名与至少 8 位密码")
                .clicked()
            {
                if !can_create {
                    self.say("请填写账号、姓名，并设置至少 8 位密码");
                } else if self.require_org_online("创建账号") {
                    let dept_arg = if self.settings.user_dept_pick.trim().is_empty() {
                        Value::Null
                    } else {
                        json!(self.settings.user_dept_pick)
                    };
                    match self.call(
                        "POST",
                        "/v1/org/users",
                        Some(json!({
                            "username": self.settings.user_name.trim(),
                            "display_name": self.settings.user_display.trim(),
                            "password": self.settings.user_password,
                            "dept_id": dept_arg,
                            "role": if is_super { self.settings.user_role_pick.as_str() } else { "member" },
                        })),
                    ) {
                        Ok(_) => {
                            self.settings.user_name.clear();
                            self.settings.user_display.clear();
                            self.settings.user_password.clear();
                            self.say("账号已创建");
                            self.refresh_all();
                        }
                        Err(e) => self.msg = e,
                    }
                }
            }
        });
    }

    fn settings_grants(&mut self, ui: &mut egui::Ui) {
        let is_super = self.role() == "super";
        if !self.settings.grants_loaded {
            self.settings.grants_loaded = true;
            if let Ok(v) = self.call("GET", "/v1/grants", None) {
                self.grants = v
                    .get("grants")
                    .and_then(|g| g.as_array())
                    .cloned()
                    .unwrap_or_default();
            }
        }
        let depts = self
            .org
            .get("departments")
            .and_then(|d| d.as_array())
            .cloned()
            .unwrap_or_default();
        let users = self
            .org
            .get("users")
            .and_then(|d| d.as_array())
            .cloned()
            .unwrap_or_default();
        if self.settings.grant_group.is_empty() {
            if let Some(first) = self.groups.first() {
                self.settings.grant_group = s(first, "id");
            }
        }
        if self.settings.grant_subject_type.is_empty() {
            self.settings.grant_subject_type = "dept".into();
        }
        if self.settings.grant_capability.is_empty() {
            self.settings.grant_capability = "view".into();
        }

        let subject_label = |stype: &str, sid: &str| -> String {
            if stype == "dept" {
                let dname = depts
                    .iter()
                    .find(|d| s(d, "id") == sid)
                    .map(|d| s(d, "name"))
                    .unwrap_or_else(|| sid.to_string());
                format!("部门 · {dname}")
            } else {
                let uname = users
                    .iter()
                    .find(|u| s(u, "id") == sid)
                    .map(|u| format!("{} ({})", s(u, "display_name"), s(u, "username")))
                    .unwrap_or_else(|| sid.to_string());
                format!("个人 · {uname}")
            }
        };

        settings_card(ui, |ui| {
            section_title(ui, "内容组", "内容块与索引按内容组隔离；部门默认向下继承授权。");
            if !is_super {
                readonly_admin_notice(ui);
            }
            ui.horizontal_wrapped(|ui| {
                for g in &self.groups {
                    badge(ui, &s(g, "name"), BadgeTone::Info);
                }
            });
            if is_super {
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    ui.label("新内容组");
                    ui.add_sized(
                        [220.0, 26.0],
                        egui::TextEdit::singleline(&mut self.settings.group_name)
                            .hint_text("例如：研发与方案"),
                    );
                    if btn_default_enabled(
                        ui,
                        !self.settings.group_name.trim().is_empty(),
                        tr("添加内容组"),
                    )
                    .clicked()
                        && self.require_org_online("添加内容组")
                    {
                        match self.call(
                            "POST",
                            "/v1/groups",
                            Some(json!({ "name": self.settings.group_name.trim() })),
                        ) {
                            Ok(_) => {
                                self.settings.group_name.clear();
                                self.say("内容组已添加");
                                self.refresh_all();
                            }
                            Err(e) => self.msg = e,
                        }
                    }
                });
            }
        });

        ui.add_space(12.0);
        settings_card(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(
                    egui::RichText::new(tr("内容组授权清单"))
                        .strong()
                        .size(15.5)
                        .color(palette_color(TEXT_MAIN)),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button(tr("刷新授权列表")).clicked() {
                        match self.call("GET", "/v1/grants", None) {
                            Ok(v) => {
                                self.grants = v
                                    .get("grants")
                                    .and_then(|g| g.as_array())
                                    .cloned()
                                    .unwrap_or_default();
                            }
                            Err(e) => self.msg = e,
                        }
                    }
                });
            });
            ui.add_space(8.0);
            if self.grants.is_empty() {
                ui.label(
                    egui::RichText::new(tr("暂无授权记录。"))
                        .size(12.0)
                        .color(palette_color(TEXT_WEAK)),
                );
            }
            for g in &self.grants {
                let gname = self.group_name(&s(g, "group_id"));
                let stype = s(g, "subject_type");
                let sid = s(g, "subject_id");
                let cap = s(g, "capability");
                ui.horizontal(|ui| {
                    badge(ui, &gname, BadgeTone::Neutral);
                    ui.label("→");
                    ui.label(subject_label(&stype, &sid));
                    badge(ui, cap_label(&cap), BadgeTone::Ok);
                });
            }
            if is_super {
                ui.add_space(10.0);
                ui.separator();
                ui.add_space(8.0);
                ui.horizontal_wrapped(|ui| {
                    egui::ComboBox::from_id_salt("grant_group")
                        .selected_text(if self.settings.grant_group.is_empty() {
                            "选择内容组".into()
                        } else {
                            self.group_name(&self.settings.grant_group)
                        })
                        .show_ui(ui, |ui| {
                            for g in &self.groups {
                                ui.selectable_value(
                                    &mut self.settings.grant_group,
                                    s(g, "id"),
                                    s(g, "name"),
                                );
                            }
                        });
                    let prev_type = self.settings.grant_subject_type.clone();
                    egui::ComboBox::from_id_salt("grant_st")
                        .selected_text(if self.settings.grant_subject_type == "user" {
                            "个人"
                        } else {
                            "部门"
                        })
                        .show_ui(ui, |ui| {
                            ui.selectable_value(
                                &mut self.settings.grant_subject_type,
                                "dept".into(),
                                "部门",
                            );
                            ui.selectable_value(
                                &mut self.settings.grant_subject_type,
                                "user".into(),
                                "个人",
                            );
                        });
                    if self.settings.grant_subject_type != prev_type {
                        self.settings.grant_subject_id.clear();
                    }
                    let picked_text = if self.settings.grant_subject_id.is_empty() {
                        if self.settings.grant_subject_type == "user" {
                            "选择成员".into()
                        } else {
                            "选择部门".into()
                        }
                    } else {
                        subject_label(
                            &self.settings.grant_subject_type,
                            &self.settings.grant_subject_id,
                        )
                    };
                    egui::ComboBox::from_id_salt("grant_subject_pick")
                        .selected_text(picked_text)
                        .show_ui(ui, |ui| {
                            if self.settings.grant_subject_type == "user" {
                                for u in &users {
                                    ui.selectable_value(
                                        &mut self.settings.grant_subject_id,
                                        s(u, "id"),
                                        format!("{} ({})", s(u, "display_name"), s(u, "username")),
                                    );
                                }
                            } else {
                                for d in &depts {
                                    ui.selectable_value(
                                        &mut self.settings.grant_subject_id,
                                        s(d, "id"),
                                        s(d, "name"),
                                    );
                                }
                            }
                        });
                    egui::ComboBox::from_id_salt("grant_cap")
                        .selected_text(cap_label(&self.settings.grant_capability))
                        .show_ui(ui, |ui| {
                            for (v, l) in [
                                ("view", "查看"),
                                ("use", "使用"),
                                ("publish", "发布"),
                                ("manage", "管理"),
                            ] {
                                ui.selectable_value(
                                    &mut self.settings.grant_capability,
                                    v.into(),
                                    l,
                                );
                            }
                        });
                    if btn_default_enabled(
                        ui,
                        !self.settings.grant_subject_id.trim().is_empty()
                            && !self.settings.grant_group.is_empty(),
                        tr("添加授权"),
                    )
                    .clicked()
                        && self.require_org_online("添加授权")
                    {
                        match self.call(
                            "POST",
                            "/v1/grants",
                            Some(json!({
                                "group_id": self.settings.grant_group,
                                "subject_type": self.settings.grant_subject_type,
                                "subject_id": self.settings.grant_subject_id,
                                "capability": self.settings.grant_capability,
                            })),
                        ) {
                            Ok(_) => {
                                self.say("已添加授权");
                                self.settings.grants_loaded = false;
                            }
                            Err(e) => self.msg = e,
                        }
                    }
                });
            }
        });
    }

    fn settings_advanced_and_backup(&mut self, ui: &mut egui::Ui) {
        if !self.settings.sub_ttl_loaded {
            self.settings.sub_ttl_loaded = true;
            if let Ok(v) = self.call("GET", "/v1/settings/submissions", None) {
                if let Some(d) = v.get("pending_ttl_days").and_then(|x| x.as_i64()) {
                    self.settings.sub_ttl_days = d;
                }
                if let Some(b) = v.get("self_maint_enabled").and_then(|x| x.as_bool()) {
                    self.settings.pub_self_maint = b;
                }
                if let Some(m) = v.get("default_publish_mode").and_then(|x| x.as_str()) {
                    self.settings.pub_default_mode = m.to_string();
                }
                if let Some(m) = v.get("minor_edit_mode").and_then(|x| x.as_str()) {
                    self.settings.pub_minor_mode = m.to_string();
                }
            }
        }
        if !self.settings.server_backup_loaded {
            self.settings.server_backup_loaded = true;
            if let Ok(v) = self.call("GET", "/v1/admin/backup", None) {
                self.settings.server_backup_info = Some(v);
            }
        }

        settings_card(ui, |ui| {
            section_title(
                ui,
                "高级功能 · 成员自维护与投稿策略",
                "控制成员自维护与申请公开总开关、默认审核策略及待审投稿过期保留期限。",
            );
            ui.checkbox(
                &mut self.settings.pub_self_maint,
                tr("开启成员自维护与申请公开功能"),
            );
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                ui.label(tr("默认公开方式"));
                let mode_idx = usize::from(self.settings.pub_default_mode == "auto");
                if let Some(sel) =
                    segmented(ui, &["需要审批", "免审直发"], mode_idx, SegStyle::Soft)
                {
                    self.settings.pub_default_mode =
                        if sel == 1 { "auto" } else { "review" }.into();
                }
                ui.add_space(16.0);
                ui.label(tr("非实质修改"));
                let minor_idx = usize::from(self.settings.pub_minor_mode == "direct");
                if let Some(sel) =
                    segmented(ui, &["跟随审批", "免审直改"], minor_idx, SegStyle::Soft)
                {
                    self.settings.pub_minor_mode =
                        if sel == 1 { "direct" } else { "follow" }.into();
                }
            });
            ui.add_space(12.0);
            ui.horizontal(|ui| {
                ui.label(tr("待审投稿保留天数"));
                ui.add(egui::DragValue::new(&mut self.settings.sub_ttl_days).range(0..=3650));
                ui.label(tr("天"));
                if btn_primary(ui, tr("保存全局与投稿设置")).clicked()
                    && self.require_org_online("保存投稿设置")
                {
                    match self.call(
                        "PUT",
                        "/v1/settings/submissions",
                        Some(json!({
                            "pending_ttl_days": self.settings.sub_ttl_days,
                            "self_maint_enabled": self.settings.pub_self_maint,
                            "default_publish_mode": self.settings.pub_default_mode,
                            "minor_edit_mode": self.settings.pub_minor_mode,
                        })),
                    ) {
                        Ok(_) => self.say("全局高级功能与投稿设置已保存"),
                        Err(e) => self.msg = e,
                    }
                }
            });
            ui.add_space(6.0);
            ui.label(
                egui::RichText::new(
                    "待审投稿超过保留天数仍未审批的，服务器会自动清理其暂存文件，提交记录标记为「已过期」（记录保留，提交人可重新提交）。设为 0 表示永不过期。",
                )
                .size(11.5)
                .color(palette_color(TEXT_WEAK)),
            );
        });

        ui.add_space(12.0);
        settings_card(ui, |ui| {
            section_title(
                ui,
                "备份与主机",
                "对组织服务器数据库与文件目录执行一致性快照备份（存于服务器数据目录 backups/ 下）。",
            );
            if let Some(info) = &self.settings.server_backup_info {
                let host = s(info, "host_name");
                let data_dir = s(info, "data_dir");
                let last_ms = i(info, "last_at_ms");
                let last_path = s(info, "last_path");
                let last_files = i(info, "last_files");
                ui.label(
                    egui::RichText::new(format!("主机名：{host} · 数据目录：{data_dir}"))
                        .size(12.0)
                        .color(palette_color(TEXT_WEAK)),
                );
                if last_ms > 0 {
                    ui.label(
                        egui::RichText::new(format!(
                            "最近服务器备份：{} · 路径 {}（含 {} 个文件）",
                            fmt_time(last_ms),
                            last_path,
                            last_files
                        ))
                        .size(11.5)
                        .color(palette_color(CHIP_GREEN_FG)),
                    );
                } else {
                    ui.label(
                        egui::RichText::new(tr("尚未通过界面触发过服务器备份。"))
                            .size(11.5)
                            .color(palette_color(TEXT_WEAK)),
                    );
                }
            }
            ui.add_space(8.0);
            if btn_default(ui, tr("立即备份服务器一次")).clicked()
                && self.require_org_online("立即备份服务器")
            {
                match self.call("POST", "/v1/admin/backup", Some(json!({}))) {
                    Ok(v) => {
                        let path = s(&v, "path");
                        let files = i(&v, "files");
                        self.settings.server_backup_info = Some(json!({
                            "host_name": s(&v, "host_name"),
                            "data_dir": self.settings.server_backup_info.as_ref().map(|x| s(x, "data_dir")).unwrap_or_default(),
                            "last_at_ms": i(&v, "at_ms"),
                            "last_path": path,
                            "last_files": files,
                        }));
                        self.say("服务器备份已完成");
                    }
                    Err(e) => self.msg = e,
                }
            }
        });
    }

    fn settings_company_profile(&mut self, ui: &mut egui::Ui) {
        let is_super = self.role() == "super";
        let users = self
            .org
            .get("users")
            .and_then(|d| d.as_array())
            .cloned()
            .unwrap_or_default();

        settings_card(ui, |ui| {
            section_title(
                ui,
                "公司公共资料",
                "超级管理员维护；导出「公用版」时带入封面 / 尾页联系区。",
            );
            if !is_super {
                readonly_admin_notice(ui);
            }
            egui::Grid::new("company_grid")
                .num_columns(2)
                .spacing([14.0, 10.0])
                .show(ui, |ui| {
                    for (key, label, hint) in [
                        ("company_name", "公司名称", "例如：示例科技有限公司"),
                        ("phone", "公司电话", "例如：400-800-2026"),
                        ("website", "公司官网", "例如：www.example.com"),
                        ("address", "公司地址", "办公地址"),
                        ("gongzhonghao", "微信公众号", "公众号名称"),
                        ("qr_note", "二维码说明", "公用版导出时带入的二维码说明"),
                    ] {
                        ui.label(label);
                        let v = self
                            .settings
                            .company
                            .entry(key.to_string())
                            .or_default();
                        ui.add_enabled(
                            is_super,
                            egui::TextEdit::singleline(v)
                                .desired_width(340.0)
                                .hint_text(hint),
                        );
                        ui.end_row();
                    }
                });

            ui.add_space(12.0);
            ui.label(egui::RichText::new("其他公司材料").strong().size(13.5));
            ui.add_space(4.0);
            let mut materials = parse_company_materials(
                self.settings
                    .company
                    .get("materials_json")
                    .map(String::as_str)
                    .unwrap_or(""),
            );
            let mut remove_mat: Option<usize> = None;
            for (idx, (mname, mnote)) in materials.iter().enumerate() {
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new(mname).strong().size(12.5));
                    ui.label(
                        egui::RichText::new(mnote)
                            .size(11.5)
                            .color(palette_color(TEXT_WEAK)),
                    );
                    if is_super && ui.button("移除").clicked() {
                        remove_mat = Some(idx);
                    }
                });
            }
            if let Some(idx) = remove_mat {
                materials.remove(idx);
                self.settings
                    .company
                    .insert("materials_json".into(), encode_company_materials(&materials));
            }
            if is_super {
                ui.horizontal(|ui| {
                    ui.add_sized(
                        [150.0, 26.0],
                        egui::TextEdit::singleline(&mut self.settings.company_mat_name)
                            .hint_text("材料名称"),
                    );
                    ui.add_sized(
                        [210.0, 26.0],
                        egui::TextEdit::singleline(&mut self.settings.company_mat_note)
                            .hint_text("文件或版本说明"),
                    );
                    if btn_default_enabled(
                        ui,
                        !self.settings.company_mat_name.trim().is_empty(),
                        "添加材料",
                    )
                    .clicked()
                    {
                        materials.push((
                            self.settings.company_mat_name.trim().to_string(),
                            self.settings.company_mat_note.trim().to_string(),
                        ));
                        self.settings
                            .company
                            .insert("materials_json".into(), encode_company_materials(&materials));
                        self.settings.company_mat_name.clear();
                        self.settings.company_mat_note.clear();
                    }
                });
            }

            ui.add_space(12.0);
            let caretaker = self
                .settings
                .company
                .get("caretaker")
                .cloned()
                .unwrap_or_default();
            ui.label(
                egui::RichText::new(format!(
                    "公司资料管理员：{}",
                    if caretaker.is_empty() {
                        "未指定（由超管直接维护）"
                    } else {
                        &caretaker
                    }
                ))
                .strong()
                .size(13.0),
            );
            if is_super && !users.is_empty() {
                ui.add_space(4.0);
                ui.horizontal_wrapped(|ui| {
                    if ui
                        .selectable_label(caretaker.is_empty(), "不指定")
                        .clicked()
                    {
                        self.settings.company.insert("caretaker".into(), String::new());
                    }
                    for u in &users {
                        if u.get("disabled").and_then(|v| v.as_bool()) == Some(true) {
                            continue;
                        }
                        let dname = s(u, "display_name");
                        if dname.is_empty() {
                            continue;
                        }
                        let active = caretaker == dname;
                        if ui
                            .selectable_label(
                                active,
                                if active {
                                    format!("✓ {dname}")
                                } else {
                                    dname.clone()
                                },
                            )
                            .clicked()
                        {
                            self.settings.company.insert("caretaker".into(), dname);
                        }
                    }
                });
            }

            if is_super {
                ui.add_space(14.0);
                if btn_primary(ui, tr("保存公司资料")).clicked()
                    && self.require_org_online("保存公司资料")
                {
                    let fields: Value = json!(self.settings.company);
                    match self.call("PUT", "/v1/company-profile", Some(json!({ "fields": fields })))
                    {
                        Ok(_) => self.say("公司公共资料已保存"),
                        Err(e) => self.msg = e,
                    }
                }
            }
        });
    }

    fn settings_languages(&mut self, ui: &mut egui::Ui) {
        let is_super = self.role() == "super";
        let mut langs = parse_languages_list(&self.settings.languages_text);
        let mut save_langs = false;

        settings_card(ui, |ui| {
            section_title(
                ui,
                "内容语言清单",
                "内容块逐页绑定与整页语言切换的可选目标语言均取自这里；移除语言的既有绑定记录保留，恢复后重新可见。",
            );
            if !is_super {
                readonly_admin_notice(ui);
            }
            let mut remove_idx: Option<usize> = None;
            for (idx, lang) in langs.iter().enumerate() {
                let is_zh = matches!(lang.as_str(), "中文" | "简体中文" | "zh");
                egui::Frame::NONE
                    .fill(palette_color(HOVER_BG))
                    .corner_radius(6)
                    .inner_margin(egui::Margin::symmetric(10, 7))
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.label(
                                egui::RichText::new(lang)
                                    .strong()
                                    .size(13.0)
                                    .color(palette_color(TEXT_MAIN)),
                            );
                            ui.label(
                                egui::RichText::new(if is_zh {
                                    "原文语言 · 逐页绑定的出发点"
                                } else {
                                    "已启用 · 块内绑定与整页切换可选"
                                })
                                .size(11.5)
                                .color(palette_color(TEXT_WEAK)),
                            );
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    if is_zh {
                                        badge(ui, "原文", BadgeTone::Ok);
                                    } else {
                                        if is_super && ui.button("移除").clicked() {
                                            remove_idx = Some(idx);
                                        }
                                        badge(ui, "已启用", BadgeTone::Info);
                                    }
                                },
                            );
                        });
                    });
                ui.add_space(4.0);
            }
            if let Some(idx) = remove_idx {
                let removed = langs.remove(idx);
                self.settings.languages_text = langs.join(", ");
                save_langs = true;
                self.say(format!("已从清单移除「{removed}」"));
            }

            if is_super {
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    ui.add_sized(
                        [220.0, 28.0],
                        egui::TextEdit::singleline(&mut self.settings.lang_input)
                            .hint_text("输入语言名称，如：越南语"),
                    );
                    if btn_primary(ui, tr("添加到清单")).clicked() {
                        let nm = self.settings.lang_input.trim().to_string();
                        if nm.is_empty() {
                            self.say("先输入要添加的语言名称");
                        } else if langs.iter().any(|x| x == &nm) {
                            self.say(format!("「{nm}」已在清单中"));
                        } else {
                            langs.push(nm.clone());
                            self.settings.languages_text = langs.join(", ");
                            self.settings.lang_input.clear();
                            save_langs = true;
                            self.say(format!("已加入「{nm}」"));
                        }
                    }
                });
                let presets: Vec<&str> = ["英语", "日语", "法语", "德语", "韩语", "西班牙语", "葡萄牙语", "俄语", "阿拉伯语"]
                    .into_iter()
                    .filter(|p| !langs.iter().any(|x| x == p))
                    .collect();
                if !presets.is_empty() {
                    ui.add_space(8.0);
                    ui.horizontal_wrapped(|ui| {
                        for p in presets {
                            if ui.button(format!("＋ {p}")).clicked() {
                                langs.push(p.to_string());
                                self.settings.languages_text = langs.join(", ");
                                save_langs = true;
                                self.say(format!("已加入「{p}」"));
                            }
                        }
                    });
                }
            }
        });

        if save_langs && self.require_org_online("保存内容语言") {
            match self.call(
                "PUT",
                "/v1/settings/languages",
                Some(json!({ "languages": langs })),
            ) {
                Ok(_) => self.refresh_all(),
                Err(e) => self.msg = e,
            }
        }
    }

    fn settings_ai(&mut self, ui: &mut egui::Ui) {
        let is_super = self.role() == "super";
        if !self.settings.ai_loaded {
            self.settings.ai_loaded = true;
            if is_super {
                if let Ok(v) = self.call("GET", "/v1/settings/ai", None) {
                    self.settings.ai_provider = s(&v, "provider");
                    self.settings.ai_endpoint = s(&v, "endpoint");
                    self.settings.ai_model = s(&v, "model");
                    self.settings.ai_has_key =
                        v.get("has_key").and_then(|x| x.as_bool()).unwrap_or(false);
                }
            }
            if let Ok(v) = self.call("GET", "/v1/settings/glossary", None) {
                if let Some(pairs) = v.get("pairs").and_then(|x| x.as_array()) {
                    self.settings.glossary_text = pairs
                        .iter()
                        .map(|pr| format!("{}={}", s(pr, "src"), s(pr, "dst")))
                        .collect::<Vec<_>>()
                        .join("\n");
                }
            }
        }

        settings_card(ui, |ui| {
            section_title(
                ui,
                "AI 翻译服务",
                "译文一律先生成草稿待人工校对，不会自动发布或覆盖现有绑定。",
            );
            if !is_super {
                readonly_admin_notice(ui);
            }
            if let Some(st) = &self.ai_status {
                let prov = s(st, "provider");
                let configured = st.get("configured").and_then(|v| v.as_bool()) == Some(true);
                let model = s(st, "model");
                ui.horizontal(|ui| {
                    badge(
                        ui,
                        if configured {
                            "服务已配置就绪"
                        } else {
                            "当前使用本地占位 (stub)"
                        },
                        if configured {
                            BadgeTone::Ok
                        } else {
                            BadgeTone::Warn
                        },
                    );
                    ui.label(
                        egui::RichText::new(if model.is_empty() {
                            format!("提供方：{prov}")
                        } else {
                            format!("提供方：{prov} · 模型：{model}")
                        })
                        .size(12.0)
                        .color(palette_color(TEXT_WEAK)),
                    );
                    if ui.button(tr("刷新状态")).clicked() {
                        if let Ok(v) = self.call("GET", "/v1/ai/status", None) {
                            self.ai_status = Some(v);
                            self.say("AI 服务状态已刷新");
                        }
                    }
                });
                ui.add_space(10.0);
            }

            if is_super {
                egui::Grid::new("ai_grid")
                    .num_columns(2)
                    .spacing([14.0, 10.0])
                    .show(ui, |ui| {
                        ui.label("提供方");
                        egui::ComboBox::from_id_salt("ai_provider")
                            .selected_text(self.settings.ai_provider.clone())
                            .show_ui(ui, |ui| {
                                ui.selectable_value(
                                    &mut self.settings.ai_provider,
                                    "stub".into(),
                                    "stub（本地占位）",
                                );
                                ui.selectable_value(
                                    &mut self.settings.ai_provider,
                                    "openai-compat".into(),
                                    "OpenAI 兼容接口",
                                );
                            });
                        ui.end_row();
                        ui.label("接口地址");
                        ui.add_sized(
                            [340.0, 26.0],
                            egui::TextEdit::singleline(&mut self.settings.ai_endpoint)
                                .hint_text("https://api.openai.com"),
                        );
                        ui.end_row();
                        ui.label("模型");
                        ui.add_sized(
                            [340.0, 26.0],
                            egui::TextEdit::singleline(&mut self.settings.ai_model)
                                .hint_text("例如：gpt-4o-mini"),
                        );
                        ui.end_row();
                        ui.label("API 密钥");
                        ui.add_sized(
                            [340.0, 26.0],
                            egui::TextEdit::singleline(&mut self.settings.ai_key).password(true),
                        );
                        ui.end_row();
                    });
                ui.label(
                    egui::RichText::new(if self.settings.ai_has_key {
                        "密钥：已设置（留空保存则不更换）"
                    } else {
                        "密钥：未设置"
                    })
                    .small()
                    .color(palette_color(TEXT_WEAK)),
                );
                ui.add_space(8.0);
                if btn_primary(ui, tr("保存 AI 设置")).clicked()
                    && self.require_org_online("保存 AI 设置")
                {
                    let mut body = json!({
                        "provider": self.settings.ai_provider,
                        "endpoint": self.settings.ai_endpoint,
                        "model": self.settings.ai_model,
                    });
                    if !self.settings.ai_key.is_empty() {
                        body["api_key"] = json!(self.settings.ai_key);
                    }
                    match self.call("PUT", "/v1/settings/ai", Some(body)) {
                        Ok(_) => {
                            if !self.settings.ai_key.is_empty() {
                                self.settings.ai_has_key = true;
                                self.settings.ai_key.clear();
                            }
                            if let Ok(st) = self.call("GET", "/v1/ai/status", None) {
                                self.ai_status = Some(st);
                            }
                            self.say("AI 设置已保存");
                        }
                        Err(e) => self.msg = e,
                    }
                }
            }
        });

        ui.add_space(12.0);
        settings_card(ui, |ui| {
            let pair_count = self
                .settings
                .glossary_text
                .lines()
                .filter(|l| l.contains('='))
                .count();
            section_title(
                ui,
                &format!("公司术语库（共 {pair_count} 条）"),
                "每行一条：源词=译词；若希望专有名词不译，可写成「源词=源词」。翻译时强制校验参照。",
            );
            if is_super {
                ui.horizontal(|ui| {
                    ui.add_sized(
                        [150.0, 26.0],
                        egui::TextEdit::singleline(&mut self.settings.glossary_src)
                            .hint_text("源词（如：启幕）"),
                    );
                    ui.label("=");
                    ui.add_sized(
                        [150.0, 26.0],
                        egui::TextEdit::singleline(&mut self.settings.glossary_dst)
                            .hint_text("译词（如：Qimu）"),
                    );
                    if btn_default_enabled(
                        ui,
                        !self.settings.glossary_src.trim().is_empty()
                            && !self.settings.glossary_dst.trim().is_empty(),
                        tr("＋ 加入术语表"),
                    )
                    .clicked()
                    {
                        let line = format!(
                            "{}={}",
                            self.settings.glossary_src.trim(),
                            self.settings.glossary_dst.trim()
                        );
                        if !self.settings.glossary_text.is_empty()
                            && !self.settings.glossary_text.ends_with('\n')
                        {
                            self.settings.glossary_text.push('\n');
                        }
                        self.settings.glossary_text.push_str(&line);
                        self.settings.glossary_src.clear();
                        self.settings.glossary_dst.clear();
                    }
                });
                ui.add_space(8.0);
                ui.add_sized(
                    [480.0, 140.0],
                    egui::TextEdit::multiline(&mut self.settings.glossary_text),
                );
                ui.add_space(8.0);
                if btn_primary(ui, tr("保存术语表")).clicked()
                    && self.require_org_online("保存术语表")
                {
                    let pairs: Vec<Value> = self
                        .settings
                        .glossary_text
                        .lines()
                        .filter_map(|l| {
                            let (a, b) = l.split_once('=')?;
                            let a = a.trim();
                            let b = b.trim();
                            if a.is_empty() || b.is_empty() {
                                return None;
                            }
                            Some(json!({ "src": a, "dst": b }))
                        })
                        .collect();
                    match self.call(
                        "PUT",
                        "/v1/settings/glossary",
                        Some(json!({ "pairs": pairs })),
                    ) {
                        Ok(_) => self.say("术语表已保存"),
                        Err(e) => self.msg = e,
                    }
                }
            } else {
                ui.add_enabled(
                    false,
                    egui::TextEdit::multiline(&mut self.settings.glossary_text)
                        .desired_width(480.0)
                        .desired_rows(6),
                );
            }
        });
    }

    fn settings_cache_policy(&mut self, ui: &mut egui::Ui) {
        let is_super = self.role() == "super";
        settings_card(ui, |ui| {
            section_title(
                ui,
                "离线缓存总策略（服务器下发）",
                "控制全公司终端的缓存模式与本地容量上限；个人自动缓存开关不得突破此策略。",
            );
            if !is_super {
                readonly_admin_notice(ui);
            }
            let cur_mode = cache_policy_mode(&self.cache_policy).to_string();
            let mut next_mode = cur_mode.clone();
            let mut max_mb = self
                .cache_policy
                .get("max_mb")
                .and_then(|v| v.as_i64())
                .unwrap_or(500);

            ui.horizontal(|ui| {
                ui.label(tr("缓存模式"));
                for (val, label) in [
                    ("allow", "允许被动缓存"),
                    ("manual", "仅手动缓存"),
                    ("forbid", "禁止本地缓存"),
                ] {
                    if ui
                        .add_enabled(is_super, egui::RadioButton::new(next_mode == val, tr(label)))
                        .clicked()
                    {
                        next_mode = val.to_string();
                    }
                }
            });
            ui.add_space(10.0);
            ui.horizontal(|ui| {
                ui.label(tr("容量上限（MB）"));
                let dv = ui.add_enabled(
                    is_super,
                    egui::DragValue::new(&mut max_mb).range(0..=102_400),
                );
                let changed_cap = dv.changed();
                let mode_changed = next_mode != cur_mode;
                if is_super
                    && (btn_primary(ui, tr("保存策略")).clicked() || mode_changed)
                    && self.require_org_online("保存缓存策略")
                {
                    let allowed = next_mode != "forbid";
                    match self.call(
                        "PUT",
                        "/v1/settings/cache-policy",
                        Some(json!({
                            "allowed": allowed,
                            "mode": next_mode,
                            "max_mb": max_mb
                        })),
                    ) {
                        Ok(_) => {
                            self.cache_policy = json!({
                                "allowed": allowed,
                                "mode": next_mode,
                                "max_mb": max_mb
                            });
                            if !allowed && self.cache_auto {
                                self.cache_auto = false;
                                save_client_settings(json!({ "auto_cache": false }));
                            }
                            self.say("离线缓存策略已保存");
                        }
                        Err(e) => self.msg = e,
                    }
                } else if changed_cap {
                    self.cache_policy["max_mb"] = json!(max_mb);
                }
            });
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cache_policy_mode_handles_legacy_and_explicit_modes() {
        assert_eq!(
            cache_policy_mode(&json!({ "allowed": true, "max_mb": 500 })),
            "allow"
        );
        assert_eq!(
            cache_policy_mode(&json!({ "allowed": false, "max_mb": 500 })),
            "forbid"
        );
        assert_eq!(
            cache_policy_mode(&json!({ "allowed": true, "mode": "manual", "max_mb": 500 })),
            "manual"
        );
        assert!(!can_enable_auto_cache(&json!({ "allowed": false })));
        assert!(can_enable_auto_cache(&json!({
            "allowed": true,
            "mode": "manual"
        })));
    }

    #[test]
    fn parse_languages_list_dedupes_and_defaults() {
        assert_eq!(
            parse_languages_list("中文, 英语, 中文, 法语"),
            vec!["中文", "英语", "法语"]
        );
        assert_eq!(parse_languages_list("  "), vec!["中文"]);
    }

    #[test]
    fn sections_for_context_respects_role_boundaries() {
        let anon = sections_for_context(false, "member");
        assert_eq!(anon, vec!["个人偏好", "演示偏好", "离线缓存", "服务器"]);

        let member = sections_for_context(true, "member");
        assert!(member.contains(&"个人资料"));
        assert!(member.contains(&"个人偏好"));
        assert!(!member.contains(&"组织管理"));
        assert!(!member.contains(&"公司公共资料"));

        let dept = sections_for_context(true, "dept_admin");
        assert!(dept.contains(&"组织管理"));
        assert!(dept.contains(&"公司公共资料"));
        assert!(!dept.contains(&"高级功能与备份"));

        let sup = sections_for_context(true, "super");
        assert!(sup.contains(&"高级功能与备份"));
        assert!(sup.contains(&"离线缓存策略"));
        assert!(sup.contains(&"AI 翻译"));
    }

    #[test]
    fn encode_and_parse_company_materials_roundtrips() {
        let orig = vec![
            ("品牌手册".to_string(), "v2.pdf".to_string()),
            ("资质认证".to_string(), "2026".to_string()),
        ];
        let json_str = encode_company_materials(&orig);
        assert_eq!(parse_company_materials(&json_str), orig);
    }
}

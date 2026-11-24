use super::{
    palette_color, tr, ACCENT, CARD_BORDER, FIELD_BG, PANEL_BG, ROSE, TEXT_MAIN, TEXT_WEAK,
};
use eframe::egui;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum AccountMenuAction {
    OpenProfile,
    Logout,
    Login,
}

pub(super) struct AccountMenuEntry<'a> {
    pub name: &'a str,
    pub role: &'a str,
    pub username: &'a str,
    pub email: &'a str,
    pub phone: &'a str,
    pub server: &'a str,
    pub signed_in: bool,
}

#[derive(Default)]
pub(super) struct AccountMenu {
    open: bool,
}

impl AccountMenu {
    pub fn is_open(&self) -> bool {
        self.open
    }

    pub fn set_open(&mut self, open: bool) {
        self.open = open;
    }

    pub fn close(&mut self) {
        self.open = false;
    }

    pub fn show(
        &mut self,
        ctx: &egui::Context,
        id: impl Into<egui::Id>,
        entry: &AccountMenuEntry<'_>,
    ) -> Option<AccountMenuAction> {
        if !self.open {
            return None;
        }

        let mut action = None;
        egui::Window::new(
            egui::RichText::new(tr("账户信息"))
                .size(13.5)
                .strong()
                .color(palette_color(TEXT_MAIN)),
        )
        .id(id.into())
        .open(&mut self.open)
        .anchor(egui::Align2::RIGHT_TOP, egui::vec2(-12.0, 38.0))
        .default_width(326.0)
        .resizable(false)
        .movable(false)
        .collapsible(false)
        .frame(
            egui::Frame::NONE
                .fill(palette_color(PANEL_BG))
                .stroke(egui::Stroke::new(1.0, palette_color(CARD_BORDER)))
                .corner_radius(10)
                .inner_margin(14),
        )
        .title_frame(
            egui::Frame::NONE
                .fill(palette_color(PANEL_BG))
                .inner_margin(egui::Margin::symmetric(12, 7)),
        )
        .show(ctx, |ui| {
            ui.horizontal(|ui| {
                let (avatar, _) =
                    ui.allocate_exact_size(egui::vec2(42.0, 42.0), egui::Sense::hover());
                ui.painter()
                    .rect_filled(avatar, 10.0, palette_color(FIELD_BG));
                let initial = entry.name.chars().next().unwrap_or('·');
                ui.painter().text(
                    avatar.center(),
                    egui::Align2::CENTER_CENTER,
                    initial,
                    egui::FontId::proportional(18.0),
                    palette_color(TEXT_MAIN),
                );
                ui.add_space(11.0);
                ui.vertical(|ui| {
                    ui.label(
                        egui::RichText::new(entry.name)
                            .size(15.0)
                            .strong()
                            .color(palette_color(TEXT_MAIN)),
                    );
                    if !entry.role.is_empty() {
                        ui.label(
                            egui::RichText::new(entry.role)
                                .size(11.0)
                                .color(palette_color(TEXT_WEAK)),
                        );
                    }
                });
            });
            ui.add_space(12.0);
            ui.separator();
            ui.add_space(7.0);
            for (label, value) in [
                (tr("用户名"), entry.username),
                (tr("邮箱"), entry.email),
                (tr("手机"), entry.phone),
                (tr("服务器"), entry.server),
            ] {
                if value.is_empty() {
                    continue;
                }
                ui.horizontal(|ui| {
                    ui.add_sized(
                        [58.0, 22.0],
                        egui::Label::new(
                            egui::RichText::new(label)
                                .size(11.5)
                                .color(palette_color(TEXT_WEAK)),
                        ),
                    );
                    ui.add_sized(
                        [ui.available_width(), 22.0],
                        egui::Label::new(
                            egui::RichText::new(value)
                                .size(12.0)
                                .color(palette_color(TEXT_MAIN)),
                        )
                        .truncate(),
                    )
                    .on_hover_text(value);
                });
            }
            ui.add_space(12.0);
            if entry.signed_in {
                if ui
                    .add_sized(
                        [ui.available_width(), 30.0],
                        egui::Button::new(tr("编辑个人资料")),
                    )
                    .clicked()
                {
                    action = Some(AccountMenuAction::OpenProfile);
                }
                ui.add_space(6.0);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui
                        .add(
                            egui::Button::new(
                                egui::RichText::new(tr("退出登录")).color(palette_color(ROSE)),
                            )
                            .stroke(egui::Stroke::new(1.0, palette_color(CARD_BORDER))),
                        )
                        .clicked()
                    {
                        action = Some(AccountMenuAction::Logout);
                    }
                });
            } else if ui
                .add_sized(
                    [ui.available_width(), 30.0],
                    egui::Button::new(egui::RichText::new(tr("登录")).color(egui::Color32::WHITE))
                        .fill(ACCENT)
                        .stroke(egui::Stroke::NONE),
                )
                .clicked()
            {
                action = Some(AccountMenuAction::Login);
            }
        });
        action
    }
}

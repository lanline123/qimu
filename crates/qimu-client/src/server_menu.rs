use super::{
    palette_color, tr, ACCENT_DEEP, CARD_BORDER, HOVER_BG, PANEL_BG, SEL_BG, TEXT_MAIN, TEXT_WEAK,
};
use eframe::egui;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum ServerMenuAction {
    Select(usize),
    Disconnect(usize),
    AddServer,
}

pub(super) struct ServerMenuEntry<'a> {
    pub name: &'a str,
    pub detail: String,
    pub status: &'a str,
    pub status_color: egui::Color32,
    pub active: bool,
}

#[derive(Default)]
pub(super) struct ServerMenu {
    open: bool,
}

impl ServerMenu {
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
        entries: &[ServerMenuEntry<'_>],
    ) -> Option<ServerMenuAction> {
        if !self.open {
            return None;
        }

        let mut action = None;
        egui::Window::new(
            egui::RichText::new(tr("切换服务器"))
                .size(13.5)
                .strong()
                .color(palette_color(TEXT_MAIN)),
        )
        .id(id.into())
        .open(&mut self.open)
        .anchor(egui::Align2::RIGHT_TOP, egui::vec2(-12.0, 38.0))
        .default_width(320.0)
        .resizable(false)
        .movable(false)
        .collapsible(false)
        .frame(
            egui::Frame::NONE
                .fill(palette_color(PANEL_BG))
                .stroke(egui::Stroke::new(1.0, palette_color(CARD_BORDER)))
                .corner_radius(10)
                .inner_margin(10),
        )
        .title_frame(
            egui::Frame::NONE
                .fill(palette_color(PANEL_BG))
                .inner_margin(egui::Margin::symmetric(10, 6)),
        )
        .show(ctx, |ui| {
            if entries.is_empty() {
                ui.label(tr("暂无保存的服务器"));
            }
            for (index, entry) in entries.iter().enumerate() {
                let (rect, response) = ui.allocate_exact_size(
                    egui::vec2(ui.available_width(), 52.0),
                    egui::Sense::click(),
                );
                if entry.active || response.hovered() {
                    ui.painter().rect_filled(
                        rect,
                        6.0,
                        palette_color(if entry.active { SEL_BG } else { HOVER_BG }),
                    );
                }
                ui.painter().text(
                    egui::pos2(rect.left() + 10.0, rect.top() + 8.0),
                    egui::Align2::LEFT_TOP,
                    entry.name,
                    egui::FontId::proportional(12.0),
                    palette_color(if entry.active { ACCENT_DEEP } else { TEXT_MAIN }),
                );
                ui.painter().text(
                    egui::pos2(rect.left() + 10.0, rect.top() + 31.0),
                    egui::Align2::LEFT_TOP,
                    &entry.detail,
                    egui::FontId::proportional(10.5),
                    palette_color(TEXT_WEAK),
                );
                ui.painter().text(
                    egui::pos2(rect.right() - 10.0, rect.top() + 10.0),
                    egui::Align2::RIGHT_TOP,
                    entry.status,
                    egui::FontId::proportional(10.5),
                    entry.status_color,
                );
                if response.clicked() {
                    action = Some(ServerMenuAction::Select(index));
                }
                ui.add_space(4.0);
            }
            ui.separator();
            ui.horizontal(|ui| {
                if ui.button(tr("添加服务器…")).clicked() {
                    action = Some(ServerMenuAction::AddServer);
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui
                        .add_enabled(
                            entries.iter().any(|entry| entry.active),
                            egui::Button::new(tr("断开当前服务器")),
                        )
                        .clicked()
                    {
                        action = entries
                            .iter()
                            .position(|entry| entry.active)
                            .map(ServerMenuAction::Disconnect);
                    }
                });
            });
        });
        action
    }
}

//! Toolbar and menu glyphs from the HTML prototype, stored as a white alpha atlas.
use super::*;

#[derive(Clone, Copy)]
pub(super) enum ChromeIcon {
    Active,
    History,
    All,
    Directory,
    Lineage,
    Push,
    Publish,
    Merge,
    More,
    Bell,
    Theme,
}

#[derive(Clone, Copy)]
pub(super) struct ChromeIcons(egui::TextureId);

impl App {
    pub(super) fn chrome_icons(&mut self, ui: &egui::Ui) -> ChromeIcons {
        static ATLAS: std::sync::LazyLock<&[u8]> = std::sync::LazyLock::new(|| {
            icon_png(include_str!("../assets/icons/chrome-icons.png.b64"))
        });
        ChromeIcons(self.tex_icon(ui, "chrome-icons", *ATLAS))
    }
}

impl ChromeIcons {
    pub(super) fn paint(
        self,
        ui: &egui::Ui,
        icon: ChromeIcon,
        center: egui::Pos2,
        size: f32,
        tint: egui::Color32,
    ) {
        let tile = icon as usize as f32;
        let uv = egui::Rect::from_min_max(
            egui::pos2(tile / 11.0, 0.0),
            egui::pos2((tile + 1.0) / 11.0, 1.0),
        );
        ui.painter().image(
            self.0,
            egui::Rect::from_center_size(center, egui::Vec2::splat(size)),
            uv,
            tint,
        );
    }

    pub(super) fn tool(
        self,
        ui: &mut egui::Ui,
        icon: ChromeIcon,
        label: &str,
        enabled: bool,
        selected: bool,
        editor: bool,
    ) -> egui::Response {
        use craft_ui::buttons::{ButtonVisuals, IconButton, IconButtonStyle};
        let normal = ButtonVisuals {
            fill: palette_color(FIELD_BG),
            stroke: egui::Stroke::new(1.0, palette_color(FIELD_BORDER)),
            icon: palette_color(ICON_TINT),
        };
        let style = IconButtonStyle {
            normal,
            hovered: ButtonVisuals {
                fill: palette_color(PRESSED_BG),
                icon: ACCENT,
                ..normal
            },
            pressed: None,
            selected: ButtonVisuals {
                fill: ACCENT,
                stroke: egui::Stroke::NONE,
                icon: egui::Color32::WHITE,
            },
            corner_radius: egui::CornerRadius::same(7),
            focus_stroke: egui::Stroke::new(1.0, ACCENT),
        };
        let label = tr(label);
        ui.add_enabled_ui(enabled, |ui| {
            let width = if editor { 30.0 } else { 28.0 };
            IconButton::new(label, egui::vec2(width, 28.0), &style)
                .selected(enabled && selected)
                .show(ui, |ui, rect, tint| {
                    self.paint(ui, icon, rect.center(), 16.0, tint)
                })
        })
        .inner
        .on_hover_text(label)
    }
}

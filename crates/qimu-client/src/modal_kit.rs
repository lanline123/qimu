use super::*;

pub(super) struct ModalSpec<'a> {
    id: egui::Id,
    title: &'a str,
    subtitle: Option<&'a str>,
    icon: ChromeIcon,
    width: f32,
}

impl<'a> ModalSpec<'a> {
    pub(super) fn new(id: egui::Id, title: &'a str, icon: ChromeIcon) -> Self {
        Self {
            id,
            title,
            subtitle: None,
            icon,
            width: 560.0,
        }
    }

    pub(super) fn subtitle(mut self, subtitle: &'a str) -> Self {
        self.subtitle = Some(subtitle);
        self
    }

    pub(super) fn width(mut self, width: f32) -> Self {
        self.width = width;
        self
    }
}

pub(super) fn show<T>(
    ctx: &egui::Context,
    spec: ModalSpec<'_>,
    content: impl FnOnce(&mut egui::Ui) -> T,
) -> egui::ModalResponse<T> {
    egui::Modal::new(spec.id)
        .backdrop_color(egui::Color32::from_black_alpha(150))
        .frame(
            egui::Frame::new()
                .fill(palette_color(PANEL_BG))
                .stroke(egui::Stroke::new(1.0, palette_color(FIELD_BORDER)))
                .corner_radius(16)
                .inner_margin(egui::Margin::same(20))
                .shadow(egui::Shadow {
                    offset: [0, 16],
                    blur: 36,
                    spread: 0,
                    color: egui::Color32::from_black_alpha(90),
                }),
        )
        .show(ctx, |ui| {
            ui.set_width(spec.width);
            ui.horizontal(|ui| {
                let (icon_rect, _) =
                    ui.allocate_exact_size(egui::vec2(42.0, 42.0), egui::Sense::hover());
                ui.painter()
                    .rect_filled(icon_rect, 10, palette_color(ACCENT_SOFT));
                ChromeIcons.paint(ui, spec.icon, icon_rect.center(), 20.0, ACCENT_DEEP);
                ui.add_space(12.0);
                ui.vertical(|ui| {
                    ui.spacing_mut().item_spacing.y = 3.0;
                    ui.label(
                        egui::RichText::new(spec.title)
                            .strong()
                            .size(19.0)
                            .color(palette_color(TEXT_MAIN)),
                    );
                    if let Some(subtitle) = spec.subtitle {
                        ui.add(
                            egui::Label::new(
                                egui::RichText::new(subtitle)
                                    .size(12.0)
                                    .color(palette_color(TEXT_WEAK)),
                            )
                            .wrap(),
                        );
                    }
                });
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let (rect, response) =
                        ui.allocate_exact_size(egui::vec2(30.0, 30.0), egui::Sense::click());
                    if response.hovered() {
                        ui.painter().rect_filled(rect, 7, palette_color(ICON_HOVER));
                    }
                    ui.painter().text(
                        rect.center(),
                        egui::Align2::CENTER_CENTER,
                        "×",
                        egui::FontId::proportional(20.0),
                        palette_color(TEXT_WEAK),
                    );
                    if response.clicked() {
                        ui.close();
                    }
                });
            });
            ui.add_space(16.0);
            content(ui)
        })
}

pub(super) fn section_label(ui: &mut egui::Ui, text: &str) {
    ui.label(
        egui::RichText::new(text)
            .strong()
            .size(12.5)
            .color(palette_color(TEXT_MAIN)),
    );
}

pub(super) fn choice_row(ui: &mut egui::Ui, title: &str, subtitle: &str, selected: bool) -> bool {
    ui.push_id(ui.scope_id().with((title, subtitle)), |ui| {
        let frame = egui::Frame::new()
            .fill(if selected {
                palette_color(SEL_BG)
            } else {
                palette_color(FIELD_BG)
            })
            .stroke(egui::Stroke::new(
                1.0,
                if selected {
                    ACCENT
                } else {
                    palette_color(FIELD_BORDER)
                },
            ))
            .corner_radius(8)
            .inner_margin(egui::Margin::symmetric(11, 9))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    let (dot, _) =
                        ui.allocate_exact_size(egui::vec2(20.0, 20.0), egui::Sense::hover());
                    let center = dot.center();
                    ui.painter().circle_stroke(
                        center,
                        7.0,
                        egui::Stroke::new(
                            1.5,
                            if selected {
                                ACCENT
                            } else {
                                palette_color(TEXT_WEAK)
                            },
                        ),
                    );
                    if selected {
                        ui.painter().circle_filled(center, 4.0, ACCENT);
                    }
                    ui.add_space(9.0);
                    ui.vertical(|ui| {
                        ui.spacing_mut().item_spacing.y = 1.0;
                        ui.label(
                            egui::RichText::new(title)
                                .strong()
                                .size(12.5)
                                .color(palette_color(TEXT_MAIN)),
                        );
                        if !subtitle.is_empty() {
                            ui.label(
                                egui::RichText::new(subtitle)
                                    .size(11.0)
                                    .color(palette_color(TEXT_WEAK)),
                            );
                        }
                    });
                });
            });
        let response = ui.interact(
            frame.response.rect,
            ui.scope_id().with("choice-row"),
            egui::Sense::click(),
        );
        if response.hovered() {
            ui.painter().rect_stroke(
                frame.response.rect,
                8,
                egui::Stroke::new(1.0, ACCENT_DEEP),
                egui::StrokeKind::Inside,
            );
        }
        response.clicked()
    })
    .inner
}

pub(super) fn footer(ui: &mut egui::Ui, primary_label: &str, primary_enabled: bool) -> bool {
    ui.add_space(14.0);
    ui.separator();
    ui.add_space(10.0);
    let mut submitted = false;
    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
        ui.spacing_mut().item_spacing.x = 8.0;
        let primary = ui
            .add_enabled_ui(primary_enabled, |ui| {
                ui.add_sized(
                    [104.0, 32.0],
                    egui::Button::new(
                        egui::RichText::new(primary_label)
                            .strong()
                            .size(12.5)
                            .color(if primary_enabled {
                                egui::Color32::WHITE
                            } else {
                                palette_color(TEXT_WEAK)
                            }),
                    )
                    .fill(if primary_enabled {
                        ACCENT
                    } else {
                        palette_color(FIELD_BORDER)
                    })
                    .corner_radius(7),
                )
            })
            .inner;
        submitted = primary.clicked();
        if ui
            .add_sized([76.0, 32.0], egui::Button::new(tr("取消")).corner_radius(7))
            .clicked()
        {
            ui.close();
        }
    });
    submitted
}

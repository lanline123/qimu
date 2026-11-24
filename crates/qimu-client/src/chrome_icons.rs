//! Prototype SVG glyphs shared by navigation, toolbars and presentation controls.
use super::*;
use craft_ui::icons::{SvgIconSet, SvgStyle};

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
    Home,
    Showroom,
    Index,
    Content,
    Trash,
    Settings,
    NewIndex,
    Import,
    Reading,
    Presenter,
    Play,
    Export,
    Download,
    Back,
    Refresh,
    Power,
    Folder,
    FolderOpen,
    FolderPlus,
    Split,
    Clipboard,
}

static SVG_ICONS: &[(&str, &[u8])] = &[
    (
        "idx-active",
        include_bytes!("../assets/icons/idx-active.svg"),
    ),
    (
        "idx-history",
        include_bytes!("../assets/icons/idx-history.svg"),
    ),
    ("idx-all", include_bytes!("../assets/icons/idx-all.svg")),
    (
        "idx-directory",
        include_bytes!("../assets/icons/idx-directory.svg"),
    ),
    (
        "idx-lineage",
        include_bytes!("../assets/icons/idx-lineage.svg"),
    ),
    ("idx-push", include_bytes!("../assets/icons/idx-push.svg")),
    (
        "idx-publish",
        include_bytes!("../assets/icons/idx-publish.svg"),
    ),
    ("idx-merge", include_bytes!("../assets/icons/idx-merge.svg")),
    ("idx-more", include_bytes!("../assets/icons/idx-more.svg")),
    ("bell", include_bytes!("../assets/icons/bell.svg")),
    ("theme", include_bytes!("../assets/icons/theme.svg")),
    ("nav-home", include_bytes!("../assets/icons/nav-home.svg")),
    ("nav-show", include_bytes!("../assets/icons/nav-show.svg")),
    ("nav-index", include_bytes!("../assets/icons/nav-index.svg")),
    (
        "nav-content",
        include_bytes!("../assets/icons/nav-content.svg"),
    ),
    ("nav-trash", include_bytes!("../assets/icons/nav-trash.svg")),
    (
        "all-application",
        include_bytes!("../assets/icons/all-application.svg"),
    ),
    ("doc-add", include_bytes!("../assets/icons/doc-add.svg")),
    ("upload", include_bytes!("../assets/icons/upload.svg")),
    (
        "deck-reading",
        include_bytes!("../assets/icons/deck-reading.svg"),
    ),
    (
        "deck-presenter",
        include_bytes!("../assets/icons/deck-presenter.svg"),
    ),
    ("play", include_bytes!("../assets/icons/play.svg")),
    ("export", include_bytes!("../assets/icons/export.svg")),
    ("download", include_bytes!("../assets/icons/download.svg")),
    ("back", include_bytes!("../assets/icons/back.svg")),
    ("refresh", include_bytes!("../assets/icons/refresh.svg")),
    ("power", include_bytes!("../assets/icons/power.svg")),
    (
        "folder-close",
        include_bytes!("../assets/icons/folder-close.svg"),
    ),
    (
        "folder-open",
        include_bytes!("../assets/icons/folder-open.svg"),
    ),
    (
        "folder-plus",
        include_bytes!("../assets/icons/folder-plus.svg"),
    ),
    ("split", include_bytes!("../assets/icons/split.svg")),
    ("clipboard", include_bytes!("../assets/icons/clipboard.svg")),
];
static ICONS: std::sync::LazyLock<SvgIconSet> = std::sync::LazyLock::new(|| {
    SvgIconSet::new(
        SVG_ICONS,
        SvgStyle {
            current_color: "#ffffff",
            stroke_width: None,
        },
        "bytes://qimu/chrome/",
        "nav-home",
    )
});

impl ChromeIcon {
    pub(super) fn image(self, size: f32, tint: egui::Color32) -> egui::Image<'static> {
        let name = match self {
            Self::Active => "idx-active",
            Self::History => "idx-history",
            Self::All => "idx-all",
            Self::Directory => "idx-directory",
            Self::Lineage => "idx-lineage",
            Self::Push => "idx-push",
            Self::Publish => "idx-publish",
            Self::Merge => "idx-merge",
            Self::More => "idx-more",
            Self::Bell => "bell",
            Self::Theme => "theme",
            Self::Home => "nav-home",
            Self::Showroom => "nav-show",
            Self::Index => "nav-index",
            Self::Content => "nav-content",
            Self::Trash => "nav-trash",
            Self::Settings => "all-application",
            Self::NewIndex => "doc-add",
            Self::Import => "upload",
            Self::Reading => "deck-reading",
            Self::Presenter => "deck-presenter",
            Self::Play => "play",
            Self::Export => "export",
            Self::Download => "download",
            Self::Back => "back",
            Self::Refresh => "refresh",
            Self::Power => "power",
            Self::Folder => "folder-close",
            Self::FolderOpen => "folder-open",
            Self::FolderPlus => "folder-plus",
            Self::Split => "split",
            Self::Clipboard => "clipboard",
        };
        egui::Image::new(ICONS.source(name))
            .fit_to_exact_size(egui::Vec2::splat(size))
            .tint(tint)
    }
}

#[derive(Clone, Copy)]
pub(super) struct ChromeIcons;

impl ChromeIcons {
    pub(super) fn paint(
        self,
        ui: &egui::Ui,
        icon: ChromeIcon,
        center: egui::Pos2,
        size: f32,
        tint: egui::Color32,
    ) {
        icon.image(size, tint).paint_at(
            ui,
            egui::Rect::from_center_size(center, egui::Vec2::splat(size)),
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

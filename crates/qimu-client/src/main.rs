//! qimu-client — the Rust desktop client (spec §4): four top-level pages
//! 演示厅 / 页面 / 索引页 / 设置. Working data lives in the client's
//! own in-process local library (qimu-core's Store, local-first
//! P0–P4); organization servers are an added layer, talked to over
//! HTTP only when the user connects one.
#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

mod account_menu;
mod chrome_icons;
mod content_detail;
mod index_controls;
mod index_history;
mod index_lineage;
mod page_editor;
mod prototype_dialogs;
mod server_menu;
mod shell;
mod workspace_controls;

use page_editor::*;
#[allow(dead_code)] // T2 spike: hosted-show shell, no call sites yet
mod hosted_show;
mod hosted_audio;
mod local_render;
mod local_store;
use prototype_dialogs::{PullDialogState, PushDialogState, ShareDialogState};

use chrome_icons::{ChromeIcon, ChromeIcons};
use content_detail::{
    binding_targets, BindPanelState, LightState, PageBindingPick, PageBindingRequest,
};
use index_controls::{IndexMoreAction, IndexOverviewControls, IndexScope};
use index_lineage::IndexGraphs;
use qimu_core::store::IndexGraph;
use workspace_controls::{
    palette_color, query_component, WorkspaceControls, ENGLISH_UI, LIGHT_THEME,
};

use base64::Engine;
use eframe::egui;
use serde_json::{json, Value};
use std::collections::BTreeMap;

fn install_system_fonts(ctx: &egui::Context) {
    use egui::epaint::text::{FallbackRequest, FontProvider};
    use egui::{FontDefinitions, FontFamily};
    use std::sync::Arc;

    let provider = Arc::new(egui_system_font_provider::SystemFontProvider::new());
    let mut fonts = FontDefinitions::default();
    for family in [FontFamily::Proportional, FontFamily::Monospace] {
        let face = if family == FontFamily::Proportional {
            system_ui_font()
        } else {
            None
        }
        .or_else(|| {
            let font = provider.font_for(&FallbackRequest {
                cluster: "A",
                family: &family,
            })?;
            Some((font.name, font.data))
        });
        let Some((name, data)) = face else {
            // Keep this family's framework defaults if the system has no usable font.
            continue;
        };
        fonts.font_data.insert(name.clone(), Arc::new(data));
        // Install the default face before discovering CJK and other fallback glyphs.
        // egui's private-use control symbols still need their icon face.
        fonts
            .families
            .insert(family, vec![name, "egui-icons".into()]);
    }
    fonts
        .font_data
        .retain(|name, _| fonts.families.values().any(|chain| chain.contains(name)));
    ctx.set_fonts(fonts);
    ctx.add_font_provider(provider);
}

fn system_ui_font() -> Option<(String, egui::FontData)> {
    use fontique::{
        Collection, CollectionOptions, GenericFamily, QueryFamily, QueryStatus, SourceCache,
    };

    let mut collection = Collection::new(CollectionOptions {
        shared: false,
        system_fonts: true,
    });
    let mut sources = SourceCache::default();
    let mut query = collection.query(&mut sources);
    query.set_families([QueryFamily::Generic(GenericFamily::SystemUi)]);
    let mut face = None;
    query.matches_with(|font| {
        if font.charmap().and_then(|map| map.map('A')).is_none() {
            return QueryStatus::Continue;
        }
        let (bytes, _) = font.blob.clone().into_raw_parts();
        face = Some((
            "system-ui".into(),
            egui::FontData::from_blob(bytes, font.index),
        ));
        QueryStatus::Stop
    });
    face
}

/// The shared palette uses the prototype's surfaces and accent. Text and
/// icon colors provide higher contrast in both themes.
pub const ACCENT: egui::Color32 = egui::Color32::from_rgb(0x2F, 0x6B, 0xF5);
/// Secondary blue — the prototype's --blue (info text, version meta).
pub const ACCENT_DEEP: egui::Color32 = egui::Color32::from_rgb(0x5B, 0x8C, 0xFF);
/// Canvas / panel fill — the prototype's --bg: side panels and page
/// canvases sit directly on it (the prototype's bars and aside are
/// transparent over --bg).
pub const CHROME: egui::Color32 = egui::Color32::from_rgb(0x0B, 0x0B, 0x0D);
pub const APP_BG: egui::Color32 = egui::Color32::from_rgb(0x0B, 0x0B, 0x0D);
pub const CARD_BG: egui::Color32 = egui::Color32::from_rgb(0x1B, 0x1B, 0x21);
pub const CARD_BORDER: egui::Color32 = egui::Color32::from_rgb(0x38, 0x38, 0x42);
/// 「我维护」 card fill (prototype --mine-bg): the panel bg mixed
/// 10% toward the accent — #181E2F on the dark theme, mapped to
/// #EAF0FE on the light theme by palette_color.
pub const MINE_BG: egui::Color32 = egui::Color32::from_rgb(0x18, 0x1E, 0x2F);
pub const SEPARATOR: egui::Color32 = egui::Color32::from_rgb(0x28, 0x28, 0x30);
pub const TEXT_MAIN: egui::Color32 = egui::Color32::WHITE;
pub const TEXT_WEAK: egui::Color32 = egui::Color32::from_rgb(0xC4, 0xC4, 0xCC);
/// Metadata, placeholders and lower-emphasis labels.
pub const TEXT_FAINT: egui::Color32 = egui::Color32::from_rgb(0xAA, 0xAA, 0xB4);
/// Row/ghost hover fill — the prototype's --panel2 (.tree button:hover).
pub const HOVER_BG: egui::Color32 = egui::Color32::from_rgb(0x1D, 0x1D, 0x23);
/// Button hover / pressed / open fill — the prototype's --panel3
/// (.btn:hover; segmented .on).
pub const PRESSED_BG: egui::Color32 = egui::Color32::from_rgb(0x26, 0x26, 0x2E);
/// Menu/tool row fill — the prototype's bars sit on --bg.
pub const HEADER_BG: egui::Color32 = egui::Color32::from_rgb(0x0B, 0x0B, 0x0D);
/// Dock panel fill — the prototype's --panel: tab strips and panel
/// bodies share it.
pub const PANEL_BG: egui::Color32 = egui::Color32::from_rgb(0x15, 0x15, 0x19);
/// Dock tab text: inactive --muted / active --ink (color-only
/// difference — no fill, no underline on panel tabs).
pub const TAB_TEXT: egui::Color32 = TEXT_WEAK;
pub const TAB_TEXT_ACTIVE: egui::Color32 = TEXT_MAIN;
/// Workspace-tab active blue — EffectCraft hot_text #3D8FF5; used
/// ONLY on the tool row's workspace tabs (text + 2px underline).
/// Aligned to the prototype accent #2f6bf5 in phase 1 (was
/// EffectCraft's #3D8FF5) so tabs and accents speak one blue.
pub const HOT: egui::Color32 = egui::Color32::from_rgb(0x2F, 0x6B, 0xF5);
/// Strong border — the prototype's --line2 (toast outline, hover
/// outlines).
pub const FIELD_BORDER: egui::Color32 = egui::Color32::from_rgb(0x3A, 0x3A, 0x44);
/// Tool-row vertical separator — the prototype's --line.
pub const LINE_34: egui::Color32 = egui::Color32::from_rgb(0x28, 0x28, 0x30);
/// Foreground for monochrome controls on neutral surfaces.
pub const ICON_TINT: egui::Color32 = TEXT_MAIN;
/// Icon-slot hover fill — the prototype's --panel3 (.icon-btn:hover).
pub const ICON_HOVER: egui::Color32 = egui::Color32::from_rgb(0x26, 0x26, 0x2E);
/// Text-field fill — the prototype's --panel2 (skin layer §5:
/// inputs sit on --panel2 with a 1px --line2 border).
pub const FIELD_BG: egui::Color32 = egui::Color32::from_rgb(0x1D, 0x1D, 0x23);
/// Secondary button fill — the prototype's --panel2 (skin layer
/// §5: .btn rests on --panel2 with a 1px --line2 border, hover
/// steps to --panel3).
pub const BTN_BG: egui::Color32 = egui::Color32::from_rgb(0x1D, 0x1D, 0x23);
/// Selected list-row fill — the prototype's --accent-soft
/// (rgba(47,107,245,.16); .tree button.active pairs it with accent
/// text).
pub const SEL_BG: egui::Color32 = egui::Color32::from_rgba_unmultiplied_const(0x2F, 0x6B, 0xF5, 41);
/// Preset folder colors (user ruling 2026-11-24): picked from the
/// client's context menu, stored on the folder as the hex string,
/// and used to dye the folder's icon (the name keeps its normal
/// color). The values sit with the accent and the standard
/// card's scope-edge colors (company blue / ancestor purple /
/// dept-sub green / dept orange). No entry = the default tint.
pub const FOLDER_COLORS: [(&str, &str); 7] = [
    ("红", "#D95F5F"),
    ("橙", "#D08A3E"),
    ("黄", "#D9B23A"),
    ("绿", "#4DA674"),
    ("青", "#3AA6A0"),
    ("蓝", "#3B82C4"),
    ("紫", "#8B7CC9"),
];

/// A folder's stored color, parsed. Anything unparseable (no
/// color set, a hand-edited database) falls back to the default
/// tint.
fn folder_color_of(f: &Value) -> Option<egui::Color32> {
    parse_folder_hex(&s(f, "color"))
}

/// Parse a "#RRGGBB" hex string (the FOLDER_COLORS storage form).
fn parse_folder_hex(hex: &str) -> Option<egui::Color32> {
    let hex = hex.strip_prefix('#')?;
    if hex.len() != 6 {
        return None;
    }
    let r = u8::from_str_radix(hex.get(0..2)?, 16).ok()?;
    let g = u8::from_str_radix(hex.get(2..4)?, 16).ok()?;
    let b = u8::from_str_radix(hex.get(4..6)?, 16).ok()?;
    Some(egui::Color32::from_rgb(r, g, b))
}
/// Layout tokens: page content inset inside a dock frame — the
/// prototype's main padding (20px top / 22px sides); the sidebar
/// column is 268px on every page (.appbody grid).
pub const PAGE_PAD_L: f32 = 22.0;

fn apply_modern_style(ctx: &egui::Context) {
    let light = LIGHT_THEME.get();
    ctx.set_theme(if light {
        egui::Theme::Light
    } else {
        egui::Theme::Dark
    });
    let mut style = (*ctx.global_style()).clone();
    let mut v = if light {
        egui::Visuals::light()
    } else {
        egui::Visuals::dark()
    };
    v.panel_fill = palette_color(CHROME);
    v.window_fill = palette_color(CARD_BG);
    // egui's default window stroke is gray-60 (#3C3C3C) — brighter
    // than anything the framework draws; windows, popups and menus
    // all take it. Press it to the separator tier (#26262B).
    v.window_stroke = egui::Stroke::new(1.0, palette_color(SEPARATOR));
    // extreme_bg serves as the text-field fill: inset #1A1A1A, darker
    // than panels and cards alike, no stroke (spec §1).
    v.extreme_bg_color = palette_color(FIELD_BG);
    v.faint_bg_color = palette_color(CHROME);
    // Text selection: the prototype accent's hue (the prototype sets
    // no ::selection rule, so the framework's alpha 90 is kept); the
    // selection stroke doubles as the focused text field's outline,
    // so it carries the accent.
    v.selection.bg_fill = egui::Color32::from_rgba_unmultiplied(0x2F, 0x6B, 0xF5, 90);
    v.selection.stroke = egui::Stroke::new(1.0, ACCENT);
    v.hyperlink_color = palette_color(ACCENT_DEEP);
    v.warn_fg_color = palette_color(CHIP_ORANGE_FG);
    v.error_fg_color = palette_color(ROSE);
    let foreground = palette_color(TEXT_MAIN);
    v.override_text_color = Some(foreground);
    v.weak_text_color = Some(palette_color(TEXT_FAINT));
    // Windows/dialogs: the prototype's dialog cascade resolves to
    // 10px (18px → 14px → 10px rules of equal specificity, last wins).
    v.window_corner_radius = egui::CornerRadius::same(10);
    // Menus/popovers take the prototype's menu tier: .ctx-menu /
    // .edit-menu are 8px.
    v.menu_corner_radius = egui::CornerRadius::same(8);
    // Shadows live only on windows/popups: the prototype's single
    // --shadow token (0 14px 34px rgba(0,0,0,.5)); cards and chrome
    // layer by brightness + border alone.
    let shadow = if light {
        egui::Color32::from_rgba_unmultiplied(20, 25, 40, 26)
    } else {
        egui::Color32::from_black_alpha(128)
    };
    v.window_shadow = egui::epaint::Shadow {
        offset: [0, 14],
        blur: 34,
        spread: 0,
        color: shadow,
    };
    v.popup_shadow = egui::epaint::Shadow {
        offset: [0, 14],
        blur: 34,
        spread: 0,
        color: shadow,
    };
    // Widget-level corner radius 6 across the board: the prototype's
    // final tightening layer puts every control (.btn, inputs, .tree
    // button, seg containers, chips) at 6px. egui has a single widget
    // radius, so the remaining tiers (dialog 10, toast 8, badge 4,
    // slide 4, pills 999) live at call sites.
    let r = egui::CornerRadius::same(6);
    for w in [
        &mut v.widgets.noninteractive,
        &mut v.widgets.inactive,
        &mut v.widgets.hovered,
        &mut v.widgets.active,
        &mut v.widgets.open,
    ] {
        w.corner_radius = r;
        w.fg_stroke.color = foreground;
    }
    v.widgets.noninteractive.bg_stroke = egui::Stroke::new(1.0, palette_color(SEPARATOR));
    // State ladder (prototype, skin layer §5): buttons/fields rest
    // on --panel2 with a 1px --line2 border; button hover steps to
    // --panel3 (.btn:hover), weak/ghost hover to --panel2 (.tree
    // button:hover); pressed/open sit on --panel3. State colors
    // apply instantly — no color tweening anywhere.
    v.widgets.inactive.weak_bg_fill = egui::Color32::TRANSPARENT;
    v.widgets.inactive.bg_fill = palette_color(BTN_BG);
    v.widgets.inactive.bg_stroke = egui::Stroke::new(1.0, palette_color(FIELD_BORDER));
    v.widgets.hovered.weak_bg_fill = palette_color(HOVER_BG);
    v.widgets.hovered.bg_fill = palette_color(PRESSED_BG);
    v.widgets.hovered.bg_stroke = egui::Stroke::new(1.0, palette_color(FIELD_BORDER));
    v.widgets.active.weak_bg_fill = palette_color(PRESSED_BG);
    v.widgets.active.bg_fill = palette_color(PRESSED_BG);
    v.widgets.active.bg_stroke = egui::Stroke::new(1.0, palette_color(FIELD_BORDER));
    v.widgets.open.weak_bg_fill = palette_color(HOVER_BG);
    v.widgets.open.bg_fill = palette_color(PRESSED_BG);
    v.widgets.open.bg_stroke = egui::Stroke::new(1.0, palette_color(FIELD_BORDER));
    style.visuals = v;
    use egui::{FontFamily, FontId, TextStyle};
    // Type ladder (prototype): body copy 13.5px (the skin layer's
    // body rule), controls 12.5 (.btn), small 11, page titles
    // 17–19 (crumb 18).
    style.text_styles.insert(
        TextStyle::Heading,
        FontId::new(18.0, FontFamily::Proportional),
    );
    style
        .text_styles
        .insert(TextStyle::Body, FontId::new(13.5, FontFamily::Proportional));
    style.text_styles.insert(
        TextStyle::Button,
        FontId::new(12.5, FontFamily::Proportional),
    );
    style.text_styles.insert(
        TextStyle::Small,
        FontId::new(11.0, FontFamily::Proportional),
    );
    style.text_styles.insert(
        TextStyle::Monospace,
        FontId::new(12.0, FontFamily::Monospace),
    );
    style.spacing.item_spacing = egui::vec2(8.0, 5.0);
    style.spacing.button_padding = egui::vec2(10.0, 4.0);
    // Product floor (§8): desktop hit targets stay ≈28–30 high.
    style.spacing.interact_size.y = 28.0;
    // Motion (spec §4): the family's whole character is 0.2s linear for
    // structural animation and instant state-color changes; set the
    // duration explicitly (the fork default is already 0.2/linear) and
    // take figcraft's 0.4s tooltip delay.
    style.animation_time = 0.2;
    style.interaction.tooltip_delay = 0.4;
    // Scroll bars float thin and quiet instead of egui's solid gray rail;
    // the boards show no visible bar at rest.
    style.spacing.scroll = egui::style::ScrollStyle {
        floating: true,
        bar_width: 10.0,
        // The prototype's scrollbar: 10px bar, --panel3 thumb with a
        // 3px transparent border — a ≈4px visible core, 99px radius.
        floating_width: 4.0,
        foreground_color: false,
        dormant_background_opacity: 0.0,
        dormant_handle_opacity: 0.5,
        active_handle_opacity: 0.8,
        ..Default::default()
    };
    style.spacing.indent = 10.0;
    // Dialog bodies take the prototype's .modal-body padding (20px).
    style.spacing.window_margin = egui::Margin::same(20);
    ctx.set_global_style(style);
}

/// Native-dialog frame (component C30): the prototype's <dialog>
/// surface — --panel fill, 1px --line stroke, 10px radius (the
/// 18 → 14 → 10 cascade), the shared --shadow token, and
/// .modal-body's 20px padding on all four sides. The global style
/// already resolves radius/shadow for the default window frame;
/// dialogs that draw their own title row (title_bar(false)) pass
/// this frame explicitly so the surface tier is --panel rather
/// than the generic window CARD_BG. Shadow values mirror
/// apply_modern_style's window_shadow per theme.
fn dlg_frame() -> egui::Frame {
    let shadow_color = if LIGHT_THEME.get() {
        egui::Color32::from_rgba_unmultiplied(20, 25, 40, 26)
    } else {
        egui::Color32::from_black_alpha(128)
    };
    egui::Frame::NONE
        .fill(palette_color(PANEL_BG))
        .stroke(egui::Stroke::new(1.0, palette_color(SEPARATOR)))
        .corner_radius(egui::CornerRadius::same(10))
        .inner_margin(egui::Margin::same(20))
        .shadow(egui::epaint::Shadow {
            offset: [0, 14],
            blur: 34,
            spread: 0,
            color: shadow_color,
        })
}

/// The ✕ for dialogs that draw their own title row: it keeps the
/// close path the egui title-bar X used to provide (dismiss, no
/// action taken). Wire it to the same flag the X fed — the
/// dialog's cancel/dismissed flag — never to a confirm action.
fn dlg_close_x(ui: &mut egui::Ui) -> bool {
    ui.add(
        egui::Button::new(
            egui::RichText::new("✕")
                .size(13.0)
                .color(palette_color(TEXT_WEAK)),
        )
        .frame(false)
        .min_size(egui::vec2(28.0, 28.0)),
    )
    .clicked()
}

/// The one standard card: a raised dark surface over the --bg
/// canvas, 1px --line hairline as the only finish, 6px radius
/// (the final tightening layer puts .std-card and every panel-class
/// surface at 6px), NO shadow (the final .std-card rule sets
/// box-shadow:none) — cover-first layout. Inner margin follows the
/// prototype's .show-body padding (11px top / 12px sides / 12px bottom).
fn card_frame() -> egui::Frame {
    egui::Frame::NONE
        .fill(palette_color(CARD_BG))
        .stroke(egui::Stroke::new(1.0, palette_color(CARD_BORDER)))
        .corner_radius(egui::CornerRadius::same(6))
        .inner_margin(egui::Margin {
            left: 12,
            right: 12,
            top: 11,
            bottom: 12,
        })
        .shadow(egui::epaint::Shadow {
            offset: [0, 0],
            blur: 0,
            spread: 0,
            color: egui::Color32::TRANSPARENT,
        })
}

/// Index entry (PPT) card variant: the same 6px card tier as every
/// other card under the prototype's tightening layer; kept as a
/// named variant for its call site.
fn card_frame_square() -> egui::Frame {
    card_frame().corner_radius(egui::CornerRadius::same(6))
}

/// Small tinted pill for statuses (version policy, excerpt, version no).
/// Painted manually with an exact-size allocation: a Frame here inherits
/// whatever layout surrounds it and stretches/wraps unpredictably.
fn chip(ui: &mut egui::Ui, text: &str, fg: egui::Color32, bg: egui::Color32) {
    // Badge geometry (prototype .badge, final cascade): 11px text,
    // 2px/7px padding, 4px radius (tightening layer).
    let font = egui::FontId::new(11.0, egui::FontFamily::Proportional);
    let galley = ui.painter().layout_no_wrap(text.to_string(), font, fg);
    let size = egui::vec2(galley.size().x + 14.0, 20.0);
    let (rect, _) = ui.allocate_exact_size(size, egui::Sense::hover());
    let painter = ui.painter();
    painter.rect_filled(rect, 4.0, palette_color(bg));
    painter.galley(
        egui::pos2(rect.min.x + 7.0, rect.center().y - galley.size().y / 2.0),
        galley,
        fg,
    );
}

/// Type tag from the unified content model (user ruling
/// 2026-11-25): a unit holding exactly one page IS a 单页; a
/// unit holding more is a 内容块 (pages composed in order).
/// Presentation only — storage and behavior treat both
/// identically; the tag just makes the atom visible in the UI.
fn unit_kind_tag(page_count: i64) -> Option<&'static str> {
    match page_count {
        1 => Some("单页"),
        n if n > 1 => Some("内容块"),
        _ => None,
    }
}

/// Status chips: the prototype's semantic pairs (fg over its *-soft
/// bg; gray = --muted on --panel2).
pub const CHIP_GREEN_FG: egui::Color32 = egui::Color32::from_rgb(0x3F, 0xAE, 0x74);
pub const CHIP_GREEN_BG: egui::Color32 =
    egui::Color32::from_rgba_unmultiplied_const(0x3F, 0xAE, 0x74, 38);
pub const CHIP_ORANGE_FG: egui::Color32 = egui::Color32::from_rgb(0xE0, 0x91, 0x3A);
pub const CHIP_ORANGE_BG: egui::Color32 =
    egui::Color32::from_rgba_unmultiplied_const(0xE0, 0x91, 0x3A, 36);
pub const CHIP_BLUE_BG: egui::Color32 =
    egui::Color32::from_rgba_unmultiplied_const(0x5B, 0x8C, 0xFF, 41);
pub const CHIP_GRAY_FG: egui::Color32 = TEXT_WEAK;
pub const CHIP_GRAY_BG: egui::Color32 = egui::Color32::from_rgb(0x1D, 0x1D, 0x23);
/// Danger pair — the prototype's --rose / --rose-soft.
pub const ROSE: egui::Color32 = egui::Color32::from_rgb(0xFF, 0x72, 0x76);
pub const ROSE_SOFT_BG: egui::Color32 =
    egui::Color32::from_rgba_unmultiplied_const(0xF2, 0x55, 0x5A, 36);
/// Accent-soft fill (prototype --accent-soft — the same value as
/// SEL_BG): the .show-seg selected state and accent-tinted fills.
pub const ACCENT_SOFT: egui::Color32 =
    egui::Color32::from_rgba_unmultiplied_const(0x2F, 0x6B, 0xF5, 41);
/// Badge Bad tone: the prototype's --rose over its soft pair
/// (identical values to ROSE / ROSE_SOFT_BG above).
pub const BADGE_BAD_FG: egui::Color32 = ROSE;
pub const BADGE_BAD_BG: egui::Color32 =
    egui::Color32::from_rgba_unmultiplied_const(0xF2, 0x55, 0x5A, 36);
/// Solid danger button fill with white text.
pub const DANGER_BG: egui::Color32 = egui::Color32::from_rgb(0xB9, 0x1C, 0x1C);

/// Ghost text action — the boards draw secondary actions as plain text
/// (no box): 开始演示 in accent, the rest in main/weak text color.
fn text_action(ui: &mut egui::Ui, label: &str, color: egui::Color32, size: f32) -> bool {
    ui.add(
        egui::Button::new(egui::RichText::new(label).size(size).color(color))
            .fill(egui::Color32::TRANSPARENT)
            .stroke(egui::Stroke::NONE)
            .corner_radius(egui::CornerRadius::same(6)),
    )
    .clicked()
}

fn text_action_enabled(
    ui: &mut egui::Ui,
    enabled: bool,
    label: &str,
    color: egui::Color32,
    size: f32,
) -> bool {
    ui.add_enabled(
        enabled,
        egui::Button::new(egui::RichText::new(label).size(size).color(color))
            .fill(egui::Color32::TRANSPARENT)
            .stroke(egui::Stroke::NONE)
            .corner_radius(egui::CornerRadius::same(6)),
    )
    .clicked()
}

/// Full-width panel row (folder tree / settings nav): the
/// prototype's .tree button — 28px min height (final compact
/// layer), 6px radius (tightening layer), ink text at rest, hover
/// --panel2, selected = --accent-soft fill with accent text. An
/// optional IconPark glyph (15px, the tree's mask-icon size) leads
/// the label. Returns the row's response (click + right-click
/// context menu both hang off it).
fn tree_row(
    ui: &mut egui::Ui,
    selected: bool,
    indent: f32,
    icon: Option<ChromeIcon>,
    label: &str,
    size: f32,
) -> egui::Response {
    tree_row_ex(
        ui,
        selected,
        indent,
        icon,
        None,
        label,
        size,
        egui::Sense::click(),
    )
}

/// tree_row with an explicit icon tint and sense. Selected rows
/// always dye BOTH the label and the icon with the accent — the
/// same color as the primary button's fill (user ruling
/// 2026-11-24; the prototype's .tree.active pairs the accent-soft
/// fill with accent text, and SEL_BG's own note says so). At rest
/// the icon takes `icon_color` when one is given (a folder's
/// custom color) and the default tint otherwise; the label keeps
/// its normal color either way.
#[allow(clippy::too_many_arguments)]
fn tree_row_ex(
    ui: &mut egui::Ui,
    selected: bool,
    indent: f32,
    icon: Option<ChromeIcon>,
    icon_color: Option<egui::Color32>,
    label: &str,
    size: f32,
    sense: egui::Sense,
) -> egui::Response {
    let width = ui.available_width();
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(width, 28.0), sense);
    let painter = ui.painter();
    if selected {
        painter.rect_filled(rect, 6.0, palette_color(SEL_BG));
    } else if resp.hovered() {
        painter.rect_filled(rect, 6.0, palette_color(HOVER_BG));
    }
    let mut tx = rect.min.x + 10.0 + indent;
    if let Some(icon) = icon {
        let ir = egui::Rect::from_min_size(
            egui::pos2(tx, rect.center().y - 7.5),
            egui::vec2(15.0, 15.0),
        );
        let tint = if selected {
            palette_color(ACCENT)
        } else {
            icon_color.unwrap_or_else(|| palette_color(ICON_TINT))
        };
        icon.image(15.0, tint).paint_at(ui, ir);
        tx += 15.0 + 6.0;
    }
    let font = egui::FontId::new(size, egui::FontFamily::Proportional);
    let color = if selected {
        palette_color(ACCENT)
    } else {
        palette_color(TEXT_MAIN)
    };
    let galley = painter.layout_no_wrap(label.to_string(), font, color);
    let ty = rect.center().y - galley.size().y / 2.0;
    painter.galley(egui::pos2(tx, ty), galley, color);
    resp
}

/// Page body padding inside the shared content area: the prototype's main
/// padding — 22px sides, 20px top — with 46px at the bottom, the
/// --bottombar-h the prototype's content column reserves. The fill
/// is the canvas fill used by the prototype's content column.
fn body_frame() -> egui::Frame {
    egui::Frame::NONE
        .fill(palette_color(APP_BG))
        .inner_margin(egui::Margin {
            left: PAGE_PAD_L as i8,
            right: 22,
            top: 20,
            bottom: 14,
        })
}

/// Local per-machine state lives under ~/.qimu: the saved server
/// connections (servers.json), small client settings (client-settings.json)
/// and — unless the user moves it — the offline cache.
fn qimu_home() -> std::path::PathBuf {
    let home = std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .unwrap_or_else(|_| ".".into());
    std::path::Path::new(&home).join(".qimu")
}

fn exe_dir() -> std::path::PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.to_path_buf()))
        .unwrap_or_else(|| std::path::PathBuf::from("."))
}

fn client_settings() -> Value {
    // Audit2 X10 (light): a corrupt settings file used to degrade
    // to {} silently — every preference "forgotten" with no trace.
    // Behavior is unchanged ({} fallback), but the loss is logged.
    let path = qimu_home().join("client-settings.json");
    match std::fs::read_to_string(&path) {
        Ok(t) => serde_json::from_str(&t).unwrap_or_else(|e| {
            startup_log(&format!(
                "client-settings.json unreadable ({e}); defaults in use"
            ));
            json!({})
        }),
        Err(_) => json!({}),
    }
}

/// Merge `patch` keys into client-settings.json and write it back.
fn save_client_settings(patch: Value) {
    let mut cur = client_settings();
    if let (Some(obj), Some(p)) = (cur.as_object_mut(), patch.as_object()) {
        for (k, v) in p {
            if v.is_null() {
                obj.remove(k);
            } else {
                obj.insert(k.clone(), v.clone());
            }
        }
    }
    let dir = qimu_home();
    let _ = std::fs::create_dir_all(&dir);
    let _ = craft_atomic::atomic_write(
        &dir.join("client-settings.json"),
        serde_json::to_string_pretty(&cur)
            .unwrap_or_default()
            .as_bytes(),
    );
}

/// Probe that `dir` can serve as a cache root: create it, then write
/// and remove a scratch file (named per process, so two clients
/// probing the same fresh directory do not trip over each other).
#[cfg(any(target_os = "windows", target_os = "macos"))]
fn dir_writable_probe(dir: &std::path::Path) -> bool {
    if std::fs::create_dir_all(dir).is_err() {
        return false;
    }
    let probe = dir.join(format!(".qimu-probe-{}", std::process::id()));
    match std::fs::write(&probe, b"qimu") {
        Ok(()) => {
            let _ = std::fs::remove_file(&probe);
            true
        }
        Err(_) => false,
    }
}

/// Platform-preferred default cache root (user ruling 2026-11-24):
/// keep cache bulk off the system disk — Windows prefers the D:
/// drive, macOS a writable non-system volume. `None` keeps the
/// historic beside-the-executable default. Probed once per process
/// (see `platform_cache_resolved`): a volume (un)mounted mid-run does
/// not move a running client's cache, same as the exe-side default.
#[cfg(target_os = "windows")]
fn platform_cache_default() -> Option<std::path::PathBuf> {
    let cand = std::path::PathBuf::from("D:\\Qimu\\cache");
    if std::path::Path::new("D:\\").is_dir() && dir_writable_probe(&cand) {
        Some(cand)
    } else {
        None
    }
}

#[cfg(target_os = "macos")]
fn platform_cache_default() -> Option<std::path::PathBuf> {
    let vols = std::fs::read_dir("/Volumes").ok()?;
    let mut best: Option<(u64, std::path::PathBuf)> = None;
    for entry in vols.flatten() {
        let vol = entry.path();
        if !vol.is_dir() {
            continue;
        }
        // The boot volume appears in /Volumes as a link to `/` (or a
        // folder canonicalizing to it) — never a candidate.
        if std::fs::canonicalize(&vol)
            .map(|c| c == std::path::Path::new("/"))
            .unwrap_or(false)
        {
            continue;
        }
        let cand = vol.join("Qimu").join("cache");
        if !dir_writable_probe(&cand) {
            continue;
        }
        // Among writable candidates the roomiest volume wins.
        let free = df_available_kb(&vol).unwrap_or(0);
        if best.as_ref().map(|(f, _)| free > *f).unwrap_or(true) {
            best = Some((free, cand));
        }
    }
    best.map(|(_, p)| p)
}

/// Available KiB on the volume holding `path`, via `df -k` (its
/// fourth column). `None` when df is missing or its output does not
/// parse — callers treat that as zero, never as an error.
#[cfg(target_os = "macos")]
fn df_available_kb(path: &std::path::Path) -> Option<u64> {
    let out = std::process::Command::new("df")
        .arg("-k")
        .arg(path)
        .output()
        .ok()?;
    let text = String::from_utf8_lossy(&out.stdout);
    let line = text.lines().nth(1)?;
    line.split_whitespace().nth(3)?.parse().ok()
}

#[cfg(not(any(target_os = "windows", target_os = "macos")))]
fn platform_cache_default() -> Option<std::path::PathBuf> {
    None
}

/// The platform default, resolved at most once per process.
fn platform_cache_resolved() -> Option<std::path::PathBuf> {
    static PLATFORM_DEFAULT: std::sync::OnceLock<Option<std::path::PathBuf>> =
        std::sync::OnceLock::new();
    PLATFORM_DEFAULT.get_or_init(platform_cache_default).clone()
}

/// Offline cache (§3.17): adopted snapshots live in the cache directory
/// with a small JSON index — usable with no server at all. The directory
/// is a local setting: a user-configured path always wins; the default
/// prefers a platform location off the system disk (Windows: `D:\Qimu`,
/// macOS: a writable non-system volume) and otherwise falls back to a
/// `cache` folder beside the client executable (the install dir); the
/// user can move it in 设置.
fn default_cache_dir() -> std::path::PathBuf {
    platform_cache_resolved().unwrap_or_else(|| exe_dir().join("cache"))
}

fn legacy_cache_dir() -> std::path::PathBuf {
    qimu_home().join("cache")
}

fn cache_dir() -> std::path::PathBuf {
    client_settings()
        .get("cache_dir")
        .and_then(|v| v.as_str())
        .filter(|s| !s.trim().is_empty())
        .map(std::path::PathBuf::from)
        .unwrap_or_else(default_cache_dir)
}

fn copy_dir_recursive(src: &std::path::Path, dst: &std::path::Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dst)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let to = dst.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_dir_recursive(&entry.path(), &to)?;
        } else {
            std::fs::copy(entry.path(), &to)?;
        }
    }
    Ok(())
}

/// Relative (path, size) listing used to verify a directory copy.
fn dir_manifest(root: &std::path::Path) -> Vec<(String, u64)> {
    fn walk(base: &std::path::Path, dir: &std::path::Path, out: &mut Vec<(String, u64)>) {
        if let Ok(rd) = std::fs::read_dir(dir) {
            for e in rd.flatten() {
                let p = e.path();
                if p.is_dir() {
                    walk(base, &p, out);
                } else if let Ok(m) = e.metadata() {
                    let rel = p
                        .strip_prefix(base)
                        .unwrap_or(&p)
                        .to_string_lossy()
                        .into_owned();
                    out.push((rel, m.len()));
                }
            }
        }
    }
    let mut out = Vec::new();
    walk(root, root, &mut out);
    out.sort();
    out
}

/// One-time move of the pre-config era cache (~/.qimu/cache) to the new
/// default location (beside the executable). Only when the user has not
/// configured a path themselves. If the copy cannot be verified, the old
/// directory is kept and pinned as the configured path — nothing is lost.
fn migrate_cache_dir_once() {
    if client_settings()
        .get("cache_dir")
        .and_then(|v| v.as_str())
        .is_some()
    {
        return;
    }
    let legacy = legacy_cache_dir();
    let target = default_cache_dir();
    if legacy == target || !legacy.is_dir() {
        return;
    }
    let has_content = std::fs::read_dir(&legacy)
        .map(|mut d| d.next().is_some())
        .unwrap_or(false);
    if !has_content || target.join("index.json").exists() {
        return;
    }
    let copied = copy_dir_recursive(&legacy, &target)
        .map(|_| dir_manifest(&legacy) == dir_manifest(&target))
        .unwrap_or(false);
    if copied {
        let _ = std::fs::remove_dir_all(&legacy);
    } else {
        save_client_settings(json!({ "cache_dir": legacy.to_string_lossy() }));
    }
}

fn load_cache_index() -> Vec<Value> {
    // Audit2 X10 (light): same treatment as client_settings — a
    // corrupt cache index still falls back to empty, but not
    // silently (the encrypted files it orphaned are the symptom
    // the user would otherwise report as "cache vanished").
    let path = cache_dir().join("index.json");
    match std::fs::read_to_string(&path) {
        Ok(t) => serde_json::from_str::<Value>(&t)
            .ok()
            .and_then(|v| v.as_array().cloned())
            .unwrap_or_else(|| {
                startup_log("cache index.json unreadable; starting with an empty cache index");
                Vec::new()
            }),
        Err(_) => Vec::new(),
    }
}

/// Fingerprint of a showroom item's pinned formal version: its
/// pinned commit plus the ordered version ids its entries
/// resolve to. A cached bundle records the fingerprint of the
/// formal version it captured; when a fresh live list shows a
/// different one, the bundle no longer matches the formal
/// version (pin audit N3). FNV-1a 64 — a change detector, not a
/// security digest: it only has to notice a re-pin, never to
/// resist forgery.
fn pin_fingerprint(commit_id: &str, version_ids: &[String]) -> String {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    let mut mix = |bytes: &[u8]| {
        for b in bytes {
            h ^= *b as u64;
            h = h.wrapping_mul(0x0000_0100_0000_01b3);
        }
    };
    mix(commit_id.as_bytes());
    mix(&[0xff]);
    for id in version_ids {
        mix(id.as_bytes());
        mix(&[0xfe]);
    }
    format!("{h:016x}")
}

/// The pin fingerprint of a showroom item payload, from the
/// fields the list payload carries (the item's pinned commit and
/// its pinned resolution rows). Empty when the payload carries
/// neither — nothing verifiable to compare against.
fn item_pin_fingerprint(item: &Value) -> String {
    let commit = s(item, "commit_id");
    let ids: Vec<String> = item
        .get("resolved_entries")
        .and_then(|r| r.as_array())
        .map(|rows| rows.iter().map(|row| s(&row["resolved"], "id")).collect())
        .unwrap_or_default();
    if commit.is_empty() && ids.iter().all(|i| i.is_empty()) {
        return String::new();
    }
    pin_fingerprint(&commit, &ids)
}

/// The in-app PPT clipboard (PPT 剪贴板): one fragment .pptx — the
/// same currency as a version fragment — plus a small JSON sidecar
/// (page count, source label, when it was taken). It lives in the
/// cache dir, so it survives restarts exactly like the offline
/// cache; PowerPoint interop is layered on top of it through the
/// bridge (see `App::clip_store`).
fn clip_fragment_path() -> std::path::PathBuf {
    cache_dir().join("ppt-clipboard.pptx")
}

fn clip_meta_path() -> std::path::PathBuf {
    cache_dir().join("ppt-clipboard.json")
}

/// Name for a block born from one clipboard page: the page's first
/// non-empty text run, trimmed — 未命名内容块 when the page carries
/// no text at all.
fn clip_page_name(single: &qimu_core::pptx::Package, idx: usize) -> String {
    if let Ok(runs) = qimu_core::pptx::slide_text_runs(single, 0) {
        for run in runs {
            let t = run.trim();
            if !t.is_empty() {
                return t.chars().take(24).collect();
            }
        }
    }
    let _ = idx;
    "未命名内容块".to_string()
}

fn load_clip_meta() -> Option<Value> {
    if !clip_fragment_path().exists() {
        return None;
    }
    let meta: Value = std::fs::read_to_string(clip_meta_path())
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())?;
    if i(&meta, "pages") > 0 {
        Some(meta)
    } else {
        None
    }
}

/// At-rest encryption for the offline cache (§3.17: 静态加密必需）:
/// AES-256-GCM with a per-machine key kept inside the cache directory
/// (cache.key), so the key travels with the cache when the user moves it.
fn read_key_file(path: &std::path::Path) -> Option<[u8; 32]> {
    let hex = std::fs::read_to_string(path).ok()?;
    let hex = hex.trim();
    if hex.len() != 64 {
        return None;
    }
    let mut key = [0u8; 32];
    for i in 0..32 {
        key[i] = u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16).ok()?;
    }
    Some(key)
}

fn key_hex(key: &[u8; 32]) -> String {
    key.iter().map(|b| format!("{b:02x}")).collect()
}

fn cache_key() -> Option<[u8; 32]> {
    use aes_gcm::aead::rand_core::RngCore;
    use aes_gcm::aead::OsRng;
    let dir = cache_dir();
    let path = dir.join("cache.key");
    if let Some(key) = read_key_file(&path) {
        return Some(key);
    }
    // Audit2 E-M3: a key file that EXISTS but does not parse is not
    // the same as no key file. Everything already cached was
    // encrypted under it; silently minting a fresh key (the old
    // behavior) orphaned the whole cache behind a misleading
    // "decryption failed". Preserve the corrupt file under a
    // timestamped name and log it, so the loss is diagnosable and
    // the file itself survives for any recovery attempt.
    if path.exists() {
        let stamp = qimu_core::auth::now_ms();
        let preserved = dir.join(format!("cache.key.corrupt-{stamp}"));
        if std::fs::rename(&path, &preserved).is_ok() {
            startup_log(&format!(
                "cache.key was unreadable; preserved as {} — previously cached content cannot be decrypted",
                preserved.display()
            ));
        }
    }
    // Pre-move key location (~/.qimu/cache.key): reuse it so snapshots
    // encrypted before the move still open; copy it into the cache dir.
    let legacy = qimu_home().join("cache.key");
    if legacy != path {
        if let Some(key) = read_key_file(&legacy) {
            let _ = std::fs::create_dir_all(&dir);
            let _ = craft_atomic::atomic_write(&path, key_hex(&key).as_bytes());
            return Some(key);
        }
    }
    let mut key = [0u8; 32];
    OsRng.fill_bytes(&mut key);
    let _ = std::fs::create_dir_all(&dir);
    if craft_atomic::atomic_write(&path, key_hex(&key).as_bytes()).is_ok() {
        Some(key)
    } else {
        None
    }
}

fn encrypt_bytes(key: &[u8; 32], plain: &[u8]) -> Option<Vec<u8>> {
    use aes_gcm::aead::rand_core::RngCore;
    use aes_gcm::aead::{Aead, OsRng};
    use aes_gcm::{Aes256Gcm, Key, KeyInit, Nonce};
    let cipher = Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(key));
    let mut nonce = [0u8; 12];
    OsRng.fill_bytes(&mut nonce);
    let ct = cipher.encrypt(Nonce::from_slice(&nonce), plain).ok()?;
    let mut out = nonce.to_vec();
    out.extend_from_slice(&ct);
    Some(out)
}

fn decrypt_bytes(key: &[u8; 32], data: &[u8]) -> Option<Vec<u8>> {
    use aes_gcm::aead::Aead;
    use aes_gcm::{Aes256Gcm, Key, KeyInit, Nonce};
    if data.len() < 12 {
        return None;
    }
    let cipher = Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(key));
    cipher
        .decrypt(Nonce::from_slice(&data[..12]), &data[12..])
        .ok()
}

fn spawn_open(path: &std::path::Path) -> std::io::Result<std::process::Child> {
    if cfg!(target_os = "windows") {
        std::process::Command::new("cmd")
            .args(["/C", "start", ""])
            .arg(path)
            .spawn()
    } else if cfg!(target_os = "macos") {
        std::process::Command::new("open").arg(path).spawn()
    } else {
        std::process::Command::new("xdg-open").arg(path).spawn()
    }
}

// ================= DeckCraft handoff (present migration) ===============
// Design: ~/workspace/qimu-present-handoff/DESIGN.md. The showroom
// drill-down's three consumption views (play, reading, presenter)
// run in the DeckCraft desktop app; qimu only exports the deck
// (with notes), jumps, and takes the return. The control channel is
// DeckCraft's documented JSON-lines protocol (docs/control-protocol.md):
// one request object per line, one reply per line, `ok` + `result`/`error`.

/// Which DeckCraft view a jump asks for.
#[derive(Clone, Copy, PartialEq, Eq)]
enum DcMode {
    /// Full-screen slide show (`show.start {reading: false}`).
    Play,
    /// Windowed reading view (`show.start {reading: true}`).
    Reading,
    /// Full-screen show + presenter window (`show.presenter {on: true}`).
    Presenter,
}

/// Events the DeckCraft watcher thread reports back to the UI thread.
enum DcEvent {
    /// The show was observed running and then ended. Carries the
    /// watcher generation that observed it (audit2 E-M9: an event
    /// already queued when a newer jump superseded its session
    /// used to be applied to the NEW session — the poll side now
    /// drops events whose generation is no longer current).
    ShowEnded(u64),
    /// The control channel is dead (instance closed or crashed).
    Gone(u64),
}

/// The managed DeckCraft instance (one per client, never the user's
/// own DeckCraft if they run one separately).
struct DcSession {
    child: std::process::Child,
    port: u16,
    /// DeckCraft's presenter_view flag before our jump, restored on
    /// the auto-quit return path (it is a persisted DeckCraft
    /// preference; our jump must not leak into the user's own use).
    presenter_before: Option<bool>,
    /// The disposable copy handed to DeckCraft for this jump.
    file: std::path::PathBuf,
    auto_quit: bool,
}

/// Locate the DeckCraft desktop app: $QIMU_DECKCRAFT_APP, then a
/// deckcraft executable next to this client, then PATH. This mirrors
/// the server's deckcraft-cli convention (deckcraft_cli_bin).
fn deckcraft_app_path() -> Option<std::path::PathBuf> {
    if let Ok(p) = std::env::var("QIMU_DECKCRAFT_APP") {
        // Audit2 E-L6: a stale env path used to be trusted blindly —
        // every door then believed DeckCraft was installed, skipped
        // the install prompt and failed opaquely in the jump. Only
        // an existing file counts.
        if !p.is_empty() {
            let path = std::path::PathBuf::from(p);
            if path.is_file() {
                return Some(path);
            }
        }
    }
    let name = format!("deckcraft{}", std::env::consts::EXE_SUFFIX);
    if let Ok(exe) = std::env::current_exe() {
        let p = exe.with_file_name(&name);
        if p.exists() {
            return Some(p);
        }
    }
    if let Ok(paths) = std::env::var("PATH") {
        for dir in std::env::split_paths(&paths) {
            let p = dir.join(&name);
            if p.exists() {
                return Some(p);
            }
        }
    }
    None
}

/// Free loopback port for a fresh DeckCraft control channel.
fn dc_free_port() -> Option<u16> {
    std::net::TcpListener::bind("127.0.0.1:0")
        .ok()
        .and_then(|l| l.local_addr().ok())
        .map(|a| a.port())
}

/// Content stamp for disposable present copies: FNV-1a of the deck
/// bytes, 8 hex chars. The export response carries no commit id, so
/// the stamp stands in for the design's "{commit 短戳}" — the name
/// changes exactly when the content does.
fn dc_content_stamp(bytes: &[u8]) -> String {
    let mut h: u64 = 0xcbf29ce484222325;
    for b in bytes {
        h ^= *b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    format!("{:08x}", (h ^ (h >> 32)) as u32)
}

/// Full-width (64-bit) content stamp for the unified opener's
/// disposable copies (audit2 E-N3): those names carry no item
/// scope, so the folded 32-bit stamp alone made collisions between
/// different decks a matter of volume; the full hash plus the byte
/// length is the name.
fn dc_content_stamp64(bytes: &[u8]) -> String {
    let mut h: u64 = 0xcbf29ce484222325;
    for b in bytes {
        h ^= *b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    format!("{h:016x}-{}", bytes.len())
}

/// Device-local preference. Preserve explicit choices from the old settings file.
fn load_dc_auto_quit() -> bool {
    client_settings()["deckcraft_auto_quit"]
        .as_bool()
        .or_else(|| {
            let text = std::fs::read_to_string(qimu_home().join("present.json")).ok()?;
            serde_json::from_str::<Value>(&text).ok()?["auto_quit"].as_bool()
        })
        .unwrap_or(false)
}

/// Which engine the unified view opener presents with
/// （设置 → 演示偏好「放映打开方式」, user ruling 2026-11-25).
/// Device-local preference in client-settings.json, persisted and
/// loaded exactly like `deckcraft_auto_quit` next door.
#[derive(Clone, Copy, PartialEq, Eq)]
enum PresentEngine {
    /// The in-process hosted show; the DeckCraft jump remains its
    /// fallback (the pre-setting behaviour, and the default).
    Builtin,
    /// The external DeckCraft app as the PRIMARY path (the jump,
    /// promoted from fallback). A missing app or a failed jump is
    /// reported — never a silent switch to another engine.
    DeckCraft,
    /// The deck is written into the cache directory and opened in
    /// PowerPoint through the bridge, slide show auto-started.
    PowerPoint,
}

impl PresentEngine {
    /// The client-settings.json spelling of this choice.
    fn key(self) -> &'static str {
        match self {
            PresentEngine::Builtin => "builtin",
            PresentEngine::DeckCraft => "deckcraft",
            PresentEngine::PowerPoint => "powerpoint",
        }
    }
}

fn load_present_engine() -> PresentEngine {
    match client_settings()["present_engine"].as_str() {
        Some("deckcraft") => PresentEngine::DeckCraft,
        Some("powerpoint") => PresentEngine::PowerPoint,
        _ => PresentEngine::Builtin,
    }
}

/// Plaza folder assignments (user ruling 2026-11-25, option (a)):
/// a showroom item carries its OWN folder home in the plaza
/// sidebar's "showroom"-scope folder tree — the wall filters by
/// the item's assignment, never by its source index's folder
/// (the two id spaces are disjoint since sidebar isolation, so
/// the old filter could only ever produce an empty wall).
///
/// The assignment is a client-side mapping, persisted in
/// client-settings.json under `plaza_item_folders` as
/// { item_id: folder_id }: the server's showroom record has no
/// folder field (see the report's server-gap note), and the
/// plaza folders themselves are this client's organizational
/// view, so the mapping lives and dies with the client. Keying
/// by item id keeps the assignment across live-list replacement
/// and lets offline cache-backfilled items carry one too.
fn load_plaza_item_folders() -> std::collections::HashMap<String, String> {
    let mut out = std::collections::HashMap::new();
    if let Some(obj) = client_settings()["plaza_item_folders"].as_object() {
        for (item_id, folder_id) in obj {
            if let Some(fid) = folder_id.as_str() {
                if !item_id.is_empty() && !fid.is_empty() {
                    out.insert(item_id.clone(), fid.to_string());
                }
            }
        }
    }
    out
}

/// One JSON-lines control connection to a DeckCraft instance.
struct DcControl {
    reader: std::io::BufReader<std::net::TcpStream>,
    writer: std::net::TcpStream,
    next_id: u64,
}

impl DcControl {
    fn connect(port: u16) -> std::io::Result<DcControl> {
        let addr = std::net::SocketAddr::from(([127, 0, 0, 1], port));
        let stream =
            std::net::TcpStream::connect_timeout(&addr, std::time::Duration::from_millis(600))?;
        stream.set_read_timeout(Some(std::time::Duration::from_secs(3)))?;
        stream.set_write_timeout(Some(std::time::Duration::from_secs(3)))?;
        let _ = stream.set_nodelay(true);
        let writer = stream.try_clone()?;
        Ok(DcControl {
            reader: std::io::BufReader::new(stream),
            writer,
            next_id: 0,
        })
    }

    /// Send one request and wait for the reply with the same id.
    /// Non-matching lines are skipped (the protocol keeps one reply
    /// per request; nothing unsolicited is expected).
    fn call(&mut self, method: &str, params: Value) -> Result<Value, String> {
        use std::io::{BufRead, Write};
        self.next_id += 1;
        let id = self.next_id;
        let req = json!({ "id": id, "method": method, "params": params });
        let mut line = serde_json::to_string(&req).map_err(|e| e.to_string())?;
        line.push('\n');
        self.writer
            .write_all(line.as_bytes())
            .map_err(|e| format!("控制通道写入失败：{e}"))?;
        let mut buf = String::new();
        loop {
            buf.clear();
            let n = self
                .reader
                .read_line(&mut buf)
                .map_err(|e| format!("控制通道读取失败：{e}"))?;
            if n == 0 {
                return Err("控制通道已断开".into());
            }
            let v: Value = match serde_json::from_str(&buf) {
                Ok(v) => v,
                Err(_) => continue,
            };
            if v.get("id").and_then(|x| x.as_i64()) != Some(id as i64) {
                continue;
            }
            return match v.get("ok").and_then(|x| x.as_bool()) {
                Some(true) => Ok(v.get("result").cloned().unwrap_or(Value::Null)),
                _ => Err(v
                    .get("error")
                    .and_then(|x| x.as_str())
                    .unwrap_or("控制命令被拒绝")
                    .to_string()),
            };
        }
    }
}

/// Wait for a freshly launched DeckCraft to answer `ui.inspect` on
/// its control port (bounded ~12s). Returns the connection plus the
/// first inspect payload.
fn dc_wait_ready(port: u16) -> Option<(DcControl, Value)> {
    for _ in 0..48 {
        if let Ok(mut ctl) = DcControl::connect(port) {
            if let Ok(v) = ctl.call("ui.inspect", json!({})) {
                return Some((ctl, v));
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(250));
    }
    None
}

/// Watcher thread (auto-quit sessions only): polls `ui.inspect`
/// every ~1.2s. Once a show has been seen running, its end is
/// reported exactly once; five consecutive channel failures mean
/// the instance is gone. The generation counter kills watchers from
/// superseded sessions.
fn dc_watch(
    port: u16,
    gen: std::sync::Arc<std::sync::atomic::AtomicU64>,
    my_gen: u64,
    tx: std::sync::mpsc::Sender<DcEvent>,
) {
    let mut ctl: Option<DcControl> = None;
    let mut seen_showing = false;
    let mut sent_end = false;
    let mut errors = 0usize;
    loop {
        if gen.load(std::sync::atomic::Ordering::SeqCst) != my_gen {
            return;
        }
        if ctl.is_none() {
            ctl = DcControl::connect(port).ok();
        }
        let res = match &mut ctl {
            Some(c) => c.call("ui.inspect", json!({})),
            None => Err("connect".into()),
        };
        match res {
            Ok(v) => {
                errors = 0;
                let showing = v.get("showing").map(|s| !s.is_null()).unwrap_or(false);
                if showing {
                    seen_showing = true;
                } else if seen_showing && !sent_end {
                    sent_end = true;
                    let _ = tx.send(DcEvent::ShowEnded(my_gen));
                }
            }
            Err(_) => {
                ctl = None;
                errors += 1;
                if errors >= 5 {
                    let _ = tx.send(DcEvent::Gone(my_gen));
                    return;
                }
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(1200));
    }
}

/// Persist the cache index. Returns false when the write failed
/// (audit2 E-M4: callers used to report "已缓存" unconditionally —
/// the entry then vanished on restart and its encrypted file
/// became an orphan).
fn save_cache_index(entries: &[Value]) -> bool {
    let dir = cache_dir();
    let _ = std::fs::create_dir_all(&dir);
    craft_atomic::atomic_write(
        &dir.join("index.json"),
        serde_json::to_string_pretty(&json!(entries))
            .unwrap_or_default()
            .as_bytes(),
    )
    .is_ok()
}

/// Short human label for one index entry's version policy (merge view).
fn semver_key(v: &str) -> (u64, u64, u64) {
    let mut it = v.split('.').map(|x| x.parse::<u64>().unwrap_or(0));
    (
        it.next().unwrap_or(0),
        it.next().unwrap_or(0),
        it.next().unwrap_or(0),
    )
}

fn entry_label(e: &Value) -> String {
    if e.is_null() {
        return "（无此条目）".into();
    }
    let pol = e.get("policy").cloned().unwrap_or(Value::Null);
    let mut base = if pol.get("locked").is_some() {
        format!(
            "锁定 v{}",
            e.get("locked_semver")
                .and_then(|x| x.as_str())
                .unwrap_or("?")
        )
    } else {
        "跟随最新".to_string()
    };
    if let Some(slots) = e.get("excerpt_slots").and_then(|x| x.as_array()) {
        base += &format!(" · 摘录 {} 页", slots.len());
    }
    base
}

/// Visible-scope label for a showroom item's scope value
/// (org-model DESIGN §2.1 vocabulary). Only `org_public` is
/// produced this phase; the reserved tiers are mapped too, so
/// a future scoped item reads truthfully instead of leaking
/// its storage string. An empty value is missing data ("—");
/// an unrecognized non-empty value is shown as-is.
fn scope_label(scope: &str) -> String {
    match scope {
        "" => "—".into(),
        "org_public" => "全组织公开".into(),
        "private" => "仅自己".into(),
        _ if scope.starts_with("users:") => "指定成员".into(),
        _ if scope.starts_with("scoped:") && scope.contains("subtree") => "锚点部门及下级".into(),
        _ if scope.starts_with("scoped:") && scope.contains("node") => "锚点部门".into(),
        _ if scope.starts_with("scoped:") => "部门范围".into(),
        _ => scope.to_string(),
    }
}

fn fmt_time(ms: i64) -> String {
    // unix ms -> "YYYY-MM-DD HH:MM" (UTC+8 display for the LAN product)
    let secs = ms / 1000 + 8 * 3600;
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    let (hh, mm) = (rem / 3600, (rem % 3600) / 60);
    let (y, mo, d) = civil_from_days(days);
    format!("{y:04}-{mo:02}-{d:02} {hh:02}:{mm:02}")
}

fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

fn b64e(bytes: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

fn b64d(s: &str) -> Result<Vec<u8>, String> {
    base64::engine::general_purpose::STANDARD
        .decode(s)
        .map_err(|e| e.to_string())
}

#[derive(Clone, Copy, PartialEq)]
enum Page {
    Home,
    Showroom,
    Content,
    Index,
    Trash,
    Settings,
    Messages,
}

/// Content page state. Only page-level fields live here: the
/// folder filter (driven by the nav frame, covers every frame)
/// and the import group (wall toolbar). The
/// open block's detail and forms — formerly Option-assembled on
/// this struct — live in the frames of `stack` now (drill-down
/// discipline: a frame owns its identity and its drafts, created
/// on push, destroyed on pop).
struct ContentState {
    folder_filter: Option<String>,
    import_group: String,
    /// Content page mode: the local-cache management view
    /// (prototype #6b) replaces the wall while this is set.
    cache_open: bool,
    stack: NavStack<ContentFrame>,
}

impl Default for ContentState {
    fn default() -> Self {
        Self {
            folder_filter: None,
            import_group: String::new(),
            cache_open: false,
            stack: NavStack::new(ContentFrame::Wall(WallState::default())),
        }
    }
}

/// Content frames: the maintenance wall (prototype wallView) and
/// the block drill-down (prototype drillView).
enum ContentFrame {
    Wall(WallState),
    Unit(UnitState),
}

/// Which field of a folder-flow block header is being edited in
/// place (content page folder flow, user ruling 2026-11-24).
#[derive(Clone, Copy, PartialEq, Eq)]
enum FlowField {
    Name,
    Note,
}

/// One in-place edit on a folder-flow block header: double-click
/// the folder name or its note to open it; Enter or focus loss
/// commits, Esc cancels.
#[derive(Clone)]
struct FlowEdit {
    folder_id: String,
    field: FlowField,
    text: String,
}

#[derive(Default)]
struct WallState {
    search_text: String,
    search_hits: Option<Value>,
    /// Single-click selection on the wall (the accent ring). The
    /// drill-down itself is the Unit frame, not this field.
    selected: Option<String>,
    /// Ctrl/⌘-click multi-selection (组合 / 剪贴板复制·剪切的前置):
    /// unit ids, in click order. `selected` stays the anchor card.
    multi: Vec<String>,
    /// Folder-flow header edit currently open on a folder block
    /// (name or note), if any.
    flow_edit: Option<FlowEdit>,
    /// Focus request for the flow header editor's first frame.
    flow_edit_focus: bool,
    /// The sidebar folder selection this wall was last scrolled
    /// to (outer None = never positioned). In the folder-flow
    /// view the selection is a scroll anchor, never a filter.
    flow_anchor: Option<Option<String>>,
    /// Measured layout heights for the folder-flow
    /// virtualization (see FlowMetrics).
    flow_metrics: FlowMetrics,
    /// The pixels-per-point `flow_metrics` was measured at; a
    /// zoom change rescales every font metric and invalidates
    /// the measurements.
    flow_metrics_ppp: f32,
}

/// Card count at which the content wall's folder flow switches
/// to virtualized rendering (see `App::content_wall`).
const FLOW_VIRTUALIZE_MIN: usize = 200;

/// Layout heights the folder-flow virtualization measured from
/// real widgets. Fonts come from the system provider, so a
/// header's height and a card's body height are runtime facts,
/// not constants the code can hard-code. A metric is only
/// trusted after a real widget produced it; until then the wall
/// builds the real widget (the pre-virtualization path), which
/// is also what keeps the first frames pixel-identical.
#[derive(Default)]
struct FlowMetrics {
    /// Total cursor advance of one folder header (name row +
    /// gap + rule, including the layout's trailing item
    /// spacing), by folder id.
    header_by_folder: std::collections::HashMap<String, f32>,
    /// The same advance per depth bucket (0 / 1 / 2+), for
    /// folders whose own header has not been measured yet.
    header_by_depth: [Option<f32>; 3],
    /// Effective card height minus the cover height, per
    /// [pick-insert mode][lead rendition has a latest version].
    /// A card's height over its cover does not depend on the
    /// card's width (the title truncates to one line and the
    /// meta row is fixed-height), so this one number sizes a
    /// card at any width: height = cover height + this (the
    /// stored value already includes the 89pt body floor of
    /// `card_h`, since it is measured off a real card).
    card_body: [[Option<f32>; 2]; 2],
}

impl FlowMetrics {
    fn header_adv_for(&self, folder_id: &str, depth: usize) -> Option<f32> {
        self.header_by_folder
            .get(folder_id)
            .copied()
            .or(self.header_by_depth[depth.min(2)])
    }

    fn record_header(&mut self, folder_id: &str, depth: usize, adv: f32) {
        self.header_by_folder.insert(folder_id.to_string(), adv);
        self.header_by_depth[depth.min(2)] = Some(adv);
    }

    fn card_h(&self, insert: bool, has_version: bool, cover_h: f32) -> Option<f32> {
        self.card_body[insert as usize][has_version as usize].map(|body| cover_h + body)
    }
}

/// Whether a wall item's lead rendition has a latest version —
/// the card-variant key of the flow metrics. Must agree with
/// the `latest` extraction in `App::flow_unit_card`.
fn flow_item_has_version(item: &Value) -> bool {
    item.get("renditions")
        .and_then(|r| r.as_array())
        .and_then(|a| a.first())
        .and_then(|r| r.get("latest"))
        .is_some_and(|l| l.is_object())
}

/// Which header meta field the inline editor is standing in
/// for (prototype data-dbl-edit on the detail header).
#[derive(Clone, Copy, PartialEq, Eq)]
enum MetaField {
    Name,
    Description,
}

/// One open content block: its detail payload plus every
/// detail-period form (publish, per-page language bindings,
/// scripts editor, AI translation target). Drafts die with the
/// frame — what used to leak from one block into the next through
/// ContentState now cannot.
/// The version policy of the index entry this block detail was
/// opened THROUGH (audit R1): the #101 header's third hint
/// segment is entry data （锁定版本 / 跟随最新）, and the same
/// block can sit in several indexes under different policies —
/// so the policy travels with the entry path that opened the
/// detail, never with the block itself. Set only by the index
/// entry path (`start_pick_pages`); every other entry point
/// leaves it None and the header renders the prototype's
/// default form (a block's policy defaults to follow — the
/// segment reads 跟随最新）.
#[derive(Clone)]
enum EntryPolicyView {
    FollowLatest,
    Locked { semver: String },
}

/// The header view of one index entry's stored policy
/// (`"follow_latest"` | `{"locked": {"version_id": …}}`). The
/// locked display number prefers the entry's captured
/// `locked_semver`; the resolved version (which for a locked
/// entry IS the locked version) backs it up.
fn entry_policy_view(entry: &Value, resolved: &Value) -> EntryPolicyView {
    if entry.get("policy").and_then(|p| p.get("locked")).is_some() {
        let semver = entry
            .get("locked_semver")
            .and_then(|x| x.as_str())
            .filter(|v| !v.is_empty())
            .map(|v| v.to_string())
            .unwrap_or_else(|| s(resolved, "semver"));
        EntryPolicyView::Locked { semver }
    } else {
        EntryPolicyView::FollowLatest
    }
}

struct UnitState {
    unit_id: String,
    detail: Value,
    /// The entry policy carried in by the opening path (see
    /// EntryPolicyView); survives detail reloads, which mutate
    /// this state in place.
    entry_policy: Option<EntryPolicyView>,
    publish_bump: usize, // 0 major 1 minor 2 patch
    publish_form: String,
    publish_lang: String,
    publish_note: String,
    publish_file: Option<std::path::PathBuf>,
    bindings: Vec<Value>,
    selected_rendition: Option<String>,
    /// The history version the user clicked in the version list
    /// (views unification, user ruling 2026-11-25): while set, the
    /// pages wall shows THIS version and the bottom-bar views open
    /// it — a frozen page sequence — instead of the rendition's
    /// latest. Cleared by the rendition chips; survives detail
    /// reloads (reload mutates the frame in place).
    selected_version: Option<String>,
    selected_page: Option<usize>,
    bind_pick: Option<PageBindingPick>,
    /// The #101 bind panel's open state (prototype bindPanel):
    /// which page of the shown version it serves, and the target
    /// language its candidates currently show.
    bind_panel: Option<BindPanelState>,
    /// Header meta inline edit: (field, draft text) while the
    /// title or the intro is being edited in place.
    meta_edit: Option<(MetaField, String)>,
    meta_edit_focus: bool,
    scripts_for: Option<String>, // version id whose 讲稿 is being edited
    scripts_edit: Vec<String>,
    translate_to: String,
    /// Set by the frame's own close actions （返回 / 删除内容块）;
    /// the dispatcher pops instead of putting the frame back.
    closed: bool,
}

impl UnitState {
    fn new(unit_id: &str, detail: Value) -> Self {
        Self {
            unit_id: unit_id.to_string(),
            detail,
            entry_policy: None,
            publish_bump: 1, // default preselect: minor （小版本）, as at App::new before
            publish_form: String::new(),
            publish_lang: String::new(),
            publish_note: String::new(),
            publish_file: None,
            bindings: Vec::new(),
            selected_rendition: None,
            selected_version: None,
            selected_page: None,
            bind_pick: None,
            bind_panel: None,
            meta_edit: None,
            meta_edit_focus: false,
            scripts_for: None,
            scripts_edit: Vec::new(),
            translate_to: String::new(),
            closed: false,
        }
    }
}

struct IndexState {
    /// 概览按文件夹筛选（页级：概览帧与导航树共用）。
    overview_folder: Option<String>,
    overview_controls: IndexOverviewControls,
    stack: NavStack<IndexFrame>,
}

impl Default for IndexState {
    fn default() -> Self {
        Self {
            overview_folder: None,
            overview_controls: IndexOverviewControls::default(),
            stack: NavStack::new(IndexFrame::Overview(OverviewState::default())),
        }
    }
}

/// One frame of the index page's drill stack (design §3).
enum IndexFrame {
    Overview(OverviewState),
    Detail(IndexDetailState),
    Create(CreateState),
}

enum IndexDetailAction {
    Reload,
    ChangeBranch(String),
    FocusCommit {
        branch: String,
        id: String,
    },
    OpenGraphTarget {
        index: String,
        branch: String,
        commit: Option<String>,
    },
    Export(&'static str),
    Commit(Vec<Value>, &'static str),
    Insert(usize),
    PickPages(usize, Value, Vec<bool>),
    SwitchVersion(usize, String, String, bool),
}

/// 概览帧状态：卡片墙的选中索引（单选高亮）。
#[derive(Default)]
struct OverviewState {
    selected: Option<String>,
    graphs: Option<Result<IndexGraphs, String>>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum IndexDetailView {
    Wall,
    Commits,
    Versions,
    Exports,
    Lineage,
}

/// 索引详情帧状态：一个打开的索引的全部工作状态，随帧销毁。
struct IndexDetailState {
    index_id: String,
    detail: Value,
    branch: String,
    commits: Option<Value>,
    exports: Option<Value>,
    lineage: Option<Result<IndexGraph, String>>,
    more_action: Option<IndexMoreAction>,
    view: IndexDetailView,
    selected_commit: Option<String>,
    /// Entry-wall remove confirmation (prototype .del-pop
    /// inrow, drill re-audit N2): the entry position whose
    /// inline 删除 confirmation is armed. The card's × only
    /// arms it; the removal commits from the pop's 确认删除.
    confirm_remove_entry: Option<usize>,
    /// rendition_id -> versions, semver desc（打开时预取）。
    unit_versions: std::collections::HashMap<String, Vec<Value>>,
    tag_name: String,
    focus_tag: bool,
    new_branch: String,
    merge_from: String,
    focus_merge: bool,
    merge_preview: Option<Value>,
    merge_resolutions: std::collections::HashMap<String, String>,
    include_notes: bool,
    /// Set by in-frame close actions; the dispatcher pops.
    closed: bool,
}

impl IndexDetailState {
    fn new(index_id: &str, branch: String) -> Self {
        Self {
            index_id: index_id.to_string(),
            detail: json!({}),
            branch,
            commits: None,
            exports: None,
            lineage: None,
            more_action: None,
            view: IndexDetailView::Wall,
            selected_commit: None,
            confirm_remove_entry: None,
            unit_versions: Default::default(),
            tag_name: String::new(),
            focus_tag: false,
            new_branch: String::new(),
            merge_from: String::new(),
            focus_merge: false,
            merge_preview: None,
            merge_resolutions: Default::default(),
            include_notes: true,
            closed: false,
        }
    }
}

/// 新建索引帧状态（整页单帧呈现）。表单随帧销毁（旧实现跨次
/// 保留 new_name/new_group 的泄漏在此收敛，见 REPORT §3.5）。
#[derive(Default)]
struct CreateState {
    new_name: String,
    new_group: String,
    /// Home folder captured when the create flow was entered:
    /// Some(id) when a real folder was selected in the index
    /// sidebar (an explicit choice, which wins); None = the
    /// default home, 未分类 (the create call sends folder_id
    /// None and the index lands folder-less).
    folder: Option<String>,
    /// unit_id, rendition_id, checked
    pick: Vec<(String, String, bool)>,
    /// Set by 取消/创建成功；the dispatcher pops.
    closed: bool,
    /// 创建成功后要打开的新索引 id（分发器在弹帧后执行）。
    open_after: Option<String>,
}

#[derive(Default)]
struct SettingsState {
    section: usize,
    ai_provider: String,
    ai_endpoint: String,
    ai_model: String,
    ai_key: String,
    ai_has_key: bool,
    ai_loaded: bool,
    glossary_text: String,
    profile: BTreeMap<String, String>,
    company: BTreeMap<String, String>,
    languages_text: String,
    dept_name: String,
    user_name: String,
    user_display: String,
    user_password: String,
    group_name: String,
    grant_group: String,
    grant_subject_type: String,
    grant_subject_id: String,
    grant_capability: String,
}

/// === Drill-down navigation stack (PageHost, prototype A9) =======
/// Architecture rule (user, 2026-11-23): a top-level page's
/// sub-pages are an explicit frame stack, never Option fields
/// toggled inside the main frame. Drill-in pushes a typed frame
/// carrying the identity of what it shows (list -> detail(id) ->
/// version(id) -> page(id)); back pops; the breadcrumb renders the
/// whole stack and any crumb can be clicked to jump back. A frame's
/// state is created on push and destroyed on pop (draft-retention
/// policy is noted per frame type where it matters). Modal tasks
/// live on a separate overlay stack above the frames: Esc closes
/// the top overlay first, then pops a frame. Switching top-level
/// pages never clears another page's stack. The legacy pages keep
/// their existing Option-based sub-pages until the retrofit package
/// moves them; everything written from this line on follows the
/// stack rule.
struct NavStack<F> {
    frames: Vec<F>,
}

impl<F> NavStack<F> {
    fn new(root: F) -> Self {
        Self { frames: vec![root] }
    }
    fn depth(&self) -> usize {
        self.frames.len()
    }
    fn top(&self) -> &F {
        self.frames
            .last()
            .expect("a nav stack always keeps its root frame")
    }
    fn top_mut(&mut self) -> &mut F {
        self.frames
            .last_mut()
            .expect("a nav stack always keeps its root frame")
    }
    fn push(&mut self, frame: F) {
        self.frames.push(frame);
    }
    /// Pop one frame; the root frame is never popped. Returns
    /// whether a frame was actually popped.
    fn pop(&mut self) -> bool {
        if self.frames.len() > 1 {
            self.frames.pop();
            true
        } else {
            false
        }
    }
    /// Breadcrumb jump: keep the frames up to and including `depth`.
    fn pop_to(&mut self, depth: usize) {
        if depth + 1 < self.frames.len() {
            self.frames.truncate(depth + 1);
        }
    }

    /// Swap the top frame out (frame-state take-out: a frame's form
    /// state is rendered alongside `&mut App`, which borrowing the
    /// stack would forbid; the caller renders the taken frame and
    /// swaps it back — or drops it and pops the placeholder to
    /// close). The placeholder is never rendered.
    fn replace_top(&mut self, frame: F) -> F {
        std::mem::replace(
            self.frames
                .last_mut()
                .expect("a nav stack always keeps its root frame"),
            frame,
        )
    }

    /// The root frame (page-level view state that other pages'
    /// actions occasionally reset, e.g. the wall selection).
    fn root_mut(&mut self) -> &mut F {
        self.frames
            .first_mut()
            .expect("a nav stack always keeps its root frame")
    }
}

/// Breadcrumb bar: renders the whole stack; returns the depth of a
/// clicked crumb (the top crumb is current and not clickable).
fn crumb_bar(ui: &mut egui::Ui, crumbs: &[&str]) -> Option<usize> {
    let mut clicked = None;
    ui.horizontal(|ui| {
        for (idx, label) in crumbs.iter().enumerate() {
            if idx > 0 {
                ui.label(egui::RichText::new("›").color(palette_color(TEXT_FAINT)));
            }
            if idx + 1 == crumbs.len() {
                ui.label(
                    egui::RichText::new(*label)
                        .size(12.5)
                        .color(palette_color(TEXT_MAIN))
                        .strong(),
                );
            } else if text_action(ui, label, palette_color(ACCENT_DEEP), 12.5) {
                clicked = Some(idx);
            }
        }
    });
    clicked
}

/// Home is a launcher: its stack holds only the root frame today
/// (actions navigate to other pages rather than drilling in), but
/// it follows the same stack discipline so recent-item detail can
/// drill in later without a second mechanism.
enum HomeFrame {
    Root,
}

/// Trash frames (prototype #trashView). The list frame owns the
/// filter and the multi-selection — both die with the frame, which
/// is the point: leaving the trash resets the batch selection.
/// The item data itself is a page-level cache on TrashState (it
/// survives frame pops; only view state is per-frame).
enum TrashFrame {
    List(TrashListState),
    Preview(TrashPreviewState),
}

#[derive(Default)]
struct TrashListState {
    filter: usize,                               // 0 all | 1 index | 2 unit | 3 showroom
    selected: std::collections::HashSet<String>, // trash row ids
}

struct TrashPreviewState {
    trash_id: String,
    kind: String,
    title: String,
    version_id: String, // cover version whose pages are previewed
    page_count: usize,
    page: usize,
}

struct TrashState {
    stack: NavStack<TrashFrame>,
    items: Vec<Value>,
    loaded: bool,
    load_error: String,
}

/// One bottom-bar delete target (user ruling 2026-11-25): the
/// trash kind ("showroom" | "index" | "unit" | "version"), the
/// object id, and the human label failure reports name. The
/// shell computes the current selection's targets
/// (deckbar_delete_targets); App::trash_add_many executes them.
#[derive(Clone)]
struct DelTarget {
    kind: &'static str,
    id: String,
    label: String,
}

/// Whether a trash row belongs to a positional trash filter
/// (0 all | 1 index | 2 页面 | 3 showroom). History versions
/// file under 页面 with the units — their owning blocks' home
/// category (user ruling 2026-11-25).
fn trash_kind_in_filter(kind: &str, filter: usize) -> bool {
    match filter {
        0 => true,
        1 => kind == "index",
        2 => kind == "unit" || kind == "version",
        3 => kind == "showroom",
        _ => true,
    }
}

impl TrashState {
    fn new() -> Self {
        Self {
            stack: NavStack::new(TrashFrame::List(TrashListState::default())),
            items: vec![],
            loaded: false,
            load_error: String::new(),
        }
    }
}

/// Approval-queue frames (prototype settings 审批 section). The
/// detail frame owns the rejection draft: the reason text lives and
/// dies with the frame — popping the frame abandons the draft,
/// which is deliberate (a half-written rejection must never leak
/// into another item's decision).
enum ReviewFrame {
    Queue(ReviewQueueState),
    Detail(ReviewDetailState),
    /// F1: a P3 submission package's content preview. The
    /// manifest and page renders come from the staged package
    /// itself (GET /v1/submissions/:id/manifest + /files/:hash),
    /// never from the submitter's source library — what the
    /// reviewer sees is exactly what an approval would land.
    Submission(SubmissionDetailState),
}

struct SubmissionDetailState {
    sub_id: String,
    /// The endpoint's `{submission, manifest}` payload.
    manifest: Option<Value>,
    loaded: bool,
    load_error: String,
    reject_open: bool,
    reject_text: String,
}

#[derive(Default)]
struct ReviewQueueState {
    tab: usize, // 0 pending | 1 processed records
}

struct ReviewDetailState {
    item_id: String,
    reject_open: bool,
    reject_text: String,
    deck: Option<Value>, // source index detail, for the page preview
    deck_loaded: bool,
}

struct ReviewState {
    stack: NavStack<ReviewFrame>,
    records: Vec<Value>,
    records_loaded: bool,
    /// Cross-page deep-jump source (nav ruler 3): the showroom
    /// review bar's reject action lands on the detail frame here;
    /// at the queue root a back row returns to the source page.
    jump_source: Option<Page>,
}

impl ReviewState {
    fn new() -> Self {
        Self {
            stack: NavStack::new(ReviewFrame::Queue(ReviewQueueState::default())),
            records: vec![],
            records_loaded: false,
            jump_source: None,
        }
    }
}

/// Built-in presenter state (prototype playDlg, A28). One deck,
/// flattened to (version_id, page) pairs like the showroom deck
/// view builds them; scripts align with the flattened pages.
struct PresentState {
    title: String,
    pages: Vec<(String, usize)>,
    scripts: Vec<String>,
    cur: usize,
    speaker: bool,
    blackout: bool,
    started: std::time::Instant,
}

/// Working copy of one text run in the v2 element editor (mirrors
/// core's ElRun; style fields are `None` when the run inherits).
#[derive(Clone, Default, PartialEq)]
struct RunWork {
    reset_style: bool,
    text: String,
    sz: Option<f32>,
    b: Option<bool>,
    color: Option<String>,
}

#[derive(Clone, PartialEq)]
struct ParaWork {
    runs: Vec<RunWork>,
}

/// Working copy of one slide element (mirrors core's SlideElement;
/// geometry in EMU). `new_image` is a pending picture replacement —
/// (extension, bytes) picked by the user, uploaded at save time and
/// stored through the server's content-addressed file layer.
#[derive(Clone, PartialEq)]
struct ElWork {
    path: Vec<usize>,
    preset: Option<String>,
    line_color: Option<String>,
    group_view: Option<[i64; 4]>,
    rot: i64,
    flip_h: bool,
    flip_v: bool,
    kind: String,
    name: String,
    x: i64,
    y: i64,
    cx: i64,
    cy: i64,
    has_xfrm: bool,
    media: Option<String>,
    paras: Vec<ParaWork>,
    new_image: Option<(String, Vec<u8>)>,
}

/// An in-progress stage drag (move or resize), anchored at the
/// pointer position and the element geometry when the drag started,
/// so per-frame updates never accumulate drift.
#[derive(Clone, Copy)]
struct ElDrag {
    el: usize,
    resize: bool,
    start_pos: egui::Pos2,
    transform: StageTransform,
    x: i64,
    y: i64,
    cx: i64,
    cy: i64,
}

fn el_kind_label(kind: &str) -> &str {
    match kind {
        "text" => tr("文本"),
        "shape" => tr("形状"),
        "picture" => tr("图片"),
        "line" => tr("线条"),
        "graphic" => tr("图表"),
        "group" => tr("组合"),
        _ => tr("元素"),
    }
}

/// One undoable editor state: the flat run lists (v1 path), the
/// element working copies (v2 path) and the speaker notes. Undo/redo
/// are session-level: they never touch the published version, only
/// this overlay's working state.
#[derive(Clone, PartialEq)]
struct EditSnapshot {
    page_count: usize,
    work: std::collections::HashMap<usize, Vec<String>>,
    elems: std::collections::HashMap<usize, Vec<ElWork>>,
    scripts: Vec<String>,
}

fn elwork_from_json(v: &Value) -> ElWork {
    let paras = v
        .get("paras")
        .and_then(|x| x.as_array())
        .map(|ps| {
            ps.iter()
                .map(|p| ParaWork {
                    runs: p
                        .get("runs")
                        .and_then(|x| x.as_array())
                        .map(|rs| {
                            rs.iter()
                                .map(|r| {
                                    let st = r.get("style").cloned().unwrap_or(json!({}));
                                    RunWork {
                                        reset_style: false,
                                        text: s(r, "text"),
                                        sz: st.get("sz").and_then(|x| x.as_f64()).map(|f| f as f32),
                                        b: st.get("b").and_then(|x| x.as_bool()),
                                        color: st
                                            .get("color")
                                            .and_then(|x| x.as_str())
                                            .map(str::to_string),
                                    }
                                })
                                .collect()
                        })
                        .unwrap_or_default(),
                })
                .collect()
        })
        .unwrap_or_default();
    let group_view = v["group_view"].as_array().and_then(|numbers| {
        let coords: Vec<_> = numbers.iter().filter_map(Value::as_i64).collect();
        coords.try_into().ok()
    });
    ElWork {
        path: Vec::new(),
        preset: v["geom"].as_str().map(str::to_owned),
        line_color: v["line_color"].as_str().map(str::to_owned),
        group_view,
        rot: i(v, "rot"),
        flip_h: v["flip_h"] == true,
        flip_v: v["flip_v"] == true,
        kind: s(v, "kind"),
        name: s(v, "name"),
        x: i(v, "x") as i64,
        y: i(v, "y") as i64,
        cx: i(v, "cx") as i64,
        cy: i(v, "cy") as i64,
        has_xfrm: v.get("has_xfrm").and_then(|x| x.as_bool()).unwrap_or(false),
        media: v.get("media").and_then(|x| x.as_str()).map(str::to_string),
        paras,
        new_image: None,
    }
}

fn emu_mm(v: i64) -> f32 {
    v as f32 / 36000.0
}

fn mm_emu(v: f32) -> i64 {
    (v * 36000.0).round() as i64
}

fn hex_color32(hex: &str) -> egui::Color32 {
    let p = |a: usize| {
        hex.get(a..a + 2)
            .and_then(|h| u8::from_str_radix(h, 16).ok())
            .unwrap_or(0)
    };
    egui::Color32::from_rgb(p(0), p(2), p(4))
}

fn color32_hex(c: egui::Color32) -> String {
    format!("{:02X}{:02X}{:02X}", c.r(), c.g(), c.b())
}

/// Send changed direct formatting. Reset removes the three editable overrides.
fn push_style_op(
    out: &mut Vec<Value>,
    page: usize,
    el: usize,
    para: usize,
    run: usize,
    wr: &RunWork,
    br: &RunWork,
) {
    if wr.reset_style && !br.reset_style {
        out.push(json!({"kind": "reset_style", "page": page, "el": el, "para": para, "run": run}));
        push_full_style_op(out, page, el, para, run, wr);
        return;
    }
    let mut op = json!({"kind": "set_style", "page": page, "el": el, "para": para, "run": run});
    let mut any = false;
    if wr.sz != br.sz {
        if let Some(v) = wr.sz {
            op["sz"] = json!(v);
            any = true;
        }
    }
    if wr.b != br.b {
        if let Some(v) = wr.b {
            op["b"] = json!(v);
            any = true;
        }
    }
    if wr.color != br.color {
        if let Some(v) = &wr.color {
            op["color"] = json!(v);
            any = true;
        }
    }
    if any {
        out.push(op);
    }
}

/// Full-style op for a run created during this session (no base run
/// to diff against): every set field is shipped.
fn push_full_style_op(
    out: &mut Vec<Value>,
    page: usize,
    el: usize,
    para: usize,
    run: usize,
    wr: &RunWork,
) {
    let mut op = json!({"kind": "set_style", "page": page, "el": el, "para": para, "run": run});
    let mut any = false;
    if let Some(v) = wr.sz {
        op["sz"] = json!(v);
        any = true;
    }
    if let Some(v) = wr.b {
        op["b"] = json!(v);
        any = true;
    }
    if let Some(v) = &wr.color {
        op["color"] = json!(v);
        any = true;
    }
    if any {
        out.push(op);
    }
}

/// Page-level visual editor state: v1 flat run lists (`base` /
/// `work`) plus the v2 element working copies (`elems`).
struct PageEditState {
    version_id: String,
    // The block the edited version belongs to (sdBlockPublish's
    // publish half searches indexes by it). Filled at both
    // construction points, which all have the unit in hand.
    unit_id: String,
    unit_name: String,
    semver: String,
    page_count: usize,
    original_page_count: usize,
    page: usize,
    base: std::collections::HashMap<usize, Vec<String>>,
    work: std::collections::HashMap<usize, Vec<String>>,
    scripts: Vec<String>,
    scripts_base: Vec<String>,
    bump: usize, // 0 major | 1 minor | 2 patch — the publish dialog's rule
    note: String,
    // v2 element editing (2026-11-23 completion): per-page element
    // working copies loaded from the page-text endpoint's element
    // model. Pages present in `elems` save through the op list;
    // pages absent from it keep the v1 flat-run path.
    elems: std::collections::HashMap<usize, Vec<ElWork>>,
    elems_base: std::collections::HashMap<usize, Vec<ElWork>>,
    slide_cx: i64,
    slide_cy: i64,
    sel: Option<usize>,
    selection: std::collections::BTreeSet<usize>,
    preview: EditStagePreview,
    drag: Option<ElDrag>,
    undo: Vec<EditSnapshot>,
    redo: Vec<EditSnapshot>,
}

impl PageEditState {
    fn dirty(&self) -> bool {
        if self.added_pages() > 0 || self.scripts != self.scripts_base {
            return true;
        }
        if self
            .elems
            .iter()
            .any(|(page, list)| self.elems_base.get(page) != Some(list))
        {
            return true;
        }
        self.work
            .iter()
            .any(|(p, runs)| self.base.get(p).map(|b| b != runs).unwrap_or(false))
    }
    /// The flat edit list for the save call: (page, run, new text)
    /// for every run whose working copy differs from the baseline.
    fn edits(&self) -> Vec<(usize, usize, String)> {
        let mut out = Vec::new();
        let mut pages: Vec<usize> = self.work.keys().cloned().collect();
        pages.sort();
        for p in pages {
            let (Some(w), Some(b)) = (self.work.get(&p), self.base.get(&p)) else {
                continue;
            };
            for (r, text) in w.iter().enumerate() {
                if b.get(r) != Some(text) {
                    out.push((p, r, text.clone()));
                }
            }
        }
        out
    }
    /// Legacy flat edits restricted to pages that have no element
    /// model (v2 pages save exclusively through `ops()`, so a text
    /// change is never applied twice).
    fn legacy_edits(&self) -> Vec<(usize, usize, String)> {
        self.edits()
            .into_iter()
            .filter(|(p, _, _)| !self.elems.contains_key(p))
            .collect()
    }
    fn snapshot(&self) -> EditSnapshot {
        EditSnapshot {
            page_count: self.page_count,
            work: self.work.clone(),
            elems: self.elems.clone(),
            scripts: self.scripts.clone(),
        }
    }
    /// Record the current state as an undo step. Called BEFORE a
    /// mutation lands (or, for focus-driven text fields, on the
    /// frame focus is gained — before any character changes).
    /// Consecutive identical snapshots coalesce.
    fn push_undo(&mut self) {
        let snap = self.snapshot();
        if self.undo.last() == Some(&snap) {
            return;
        }
        self.undo.push(snap);
        if self.undo.len() > 100 {
            self.undo.remove(0);
        }
        self.redo.clear();
    }
    fn do_undo(&mut self) {
        if let Some(snap) = self.undo.pop() {
            let cur = self.snapshot();
            self.redo.push(cur);
            self.page_count = snap.page_count;
            self.page = self.page.min(self.page_count.saturating_sub(1));
            self.work = snap.work;
            self.elems = snap.elems;
            self.scripts = snap.scripts;
            self.select_element(None, false);
            self.drag = None;
        }
    }
    fn do_redo(&mut self) {
        if let Some(snap) = self.redo.pop() {
            let cur = self.snapshot();
            self.undo.push(cur);
            self.page_count = snap.page_count;
            self.page = self.page.min(self.page_count.saturating_sub(1));
            self.work = snap.work;
            self.elems = snap.elems;
            self.scripts = snap.scripts;
            self.select_element(None, false);
            self.drag = None;
        }
    }
    /// The v2 save payload: element operations for every page with
    /// an element model, in the index-safe order the core applies
    /// (per element: replacement/geometry, run updates, run
    /// deletions descending, run insertions, paragraph deletions
    /// descending, paragraph insertions ascending).
    fn page_ops(&self, only_page: Option<usize>) -> Vec<Value> {
        let mut out: Vec<Value> = Vec::new();
        let mut pages: Vec<usize> = self
            .elems
            .keys()
            .copied()
            .filter(|&page| only_page.is_none_or(|only| only == page))
            .collect();
        pages.sort_unstable();
        for p in pages {
            let w = &self.elems[&p];
            let b = self.elems_base.get(&p).map(Vec::as_slice).unwrap_or(&[]);
            for (index, we) in w.iter().enumerate() {
                let Some(&ei) = we.path.last() else { continue };
                let seed;
                let be = if let Some(base) = b.get(index) {
                    base
                } else {
                    seed = we.seed_insert(p, &mut out);
                    &seed
                };
                let first = out.len();
                if let Some((ext, bytes)) = we.new_image.as_ref().filter(|_| index < b.len()) {
                    out.push(json!({
                        "kind": "replace_pic", "page": p, "el": ei,
                        "ext": ext, "data_base64": b64e(bytes),
                    }));
                }
                if (we.x, we.y) != (be.x, be.y) {
                    out.push(json!({
                        "kind": "move", "page": p, "el": ei,
                        "x": we.x, "y": we.y,
                    }));
                }
                if (we.cx, we.cy) != (be.cx, be.cy) {
                    out.push(json!({
                        "kind": "resize", "page": p, "el": ei,
                        "cx": we.cx, "cy": we.cy,
                    }));
                }
                if we.preset != be.preset {
                    out.push(json!({"kind":"preset", "page":p, "el":ei, "preset":we.preset}));
                }
                if we.line_color != be.line_color {
                    out.push(
                        json!({"kind":"line_color", "page":p, "el":ei, "color":we.line_color}),
                    );
                }
                let common = we.paras.len().min(be.paras.len());
                for pi in 0..common {
                    let (wp, bp) = (&we.paras[pi], &be.paras[pi]);
                    let rcommon = wp.runs.len().min(bp.runs.len());
                    for ri in 0..rcommon {
                        let (wr, br) = (&wp.runs[ri], &bp.runs[ri]);
                        if wr.text != br.text {
                            out.push(json!({
                                "kind": "set_text", "page": p, "el": ei,
                                "para": pi, "run": ri, "text": wr.text,
                            }));
                        }
                        push_style_op(&mut out, p, ei, pi, ri, wr, br);
                    }
                    for ri in (rcommon..bp.runs.len()).rev() {
                        out.push(json!({
                            "kind": "del_run", "page": p, "el": ei,
                            "para": pi, "run": ri,
                        }));
                    }
                    for ri in rcommon..wp.runs.len() {
                        let wr = &wp.runs[ri];
                        out.push(json!({
                            "kind": "add_run", "page": p, "el": ei,
                            "para": pi, "text": wr.text,
                        }));
                        push_full_style_op(&mut out, p, ei, pi, ri, wr);
                    }
                }
                for pi in (common..be.paras.len()).rev() {
                    out.push(json!({
                        "kind": "del_para", "page": p, "el": ei, "para": pi,
                    }));
                }
                for pi in common..we.paras.len() {
                    let wp = &we.paras[pi];
                    let first = wp.runs.first();
                    out.push(json!({
                        "kind": "add_para", "page": p, "el": ei,
                        "para": if pi == 0 { Value::Null } else { json!(pi - 1) },
                        "text": first.map(|r| r.text.clone()).unwrap_or_default(),
                    }));
                    if let Some(fr) = first {
                        push_full_style_op(&mut out, p, ei, pi, 0, fr);
                    }
                    for ri in 1..wp.runs.len() {
                        let wr = &wp.runs[ri];
                        out.push(json!({
                            "kind": "add_run", "page": p, "el": ei,
                            "para": pi, "text": wr.text,
                        }));
                        push_full_style_op(&mut out, p, ei, pi, ri, wr);
                    }
                }
                if !we.parents().is_empty() {
                    for op in &mut out[first..] {
                        op["parents"] = json!(we.parents());
                    }
                }
            }
        }
        out
    }
}

/// A millimetre field for an EMU value; returns the new EMU value on
/// change. Focus gain raises `want_snapshot` (see the inspector's
/// undo discipline).
fn mm_field(ui: &mut egui::Ui, label: &str, emu: i64, want_snapshot: &mut bool) -> Option<i64> {
    let mut v = emu_mm(emu);
    let mut out = None;
    ui.horizontal(|ui| {
        ui.label(
            egui::RichText::new(label)
                .size(11.5)
                .color(palette_color(TEXT_FAINT)),
        );
        let resp = ui.add(
            egui::DragValue::new(&mut v)
                .speed(0.1)
                .range(0.0..=1000.0)
                .suffix(" mm"),
        );
        if resp.gained_focus() {
            *want_snapshot = true;
        }
        if resp.changed() {
            out = Some(mm_emu(v));
        }
    });
    out
}

/// The v2 editor's side-panel inspector: the page's element list
/// plus the selected element's editors. It works on a CLONE of the
/// selected element and returns it through `commit`; the caller
/// snapshots the pre-edit state (when `want_snapshot` was raised)
/// before landing the commit, which keeps every mutation made here
/// undoable at session level.
fn page_edit_inspector(
    ui: &mut egui::Ui,
    pe: &PageEditState,
    want_snapshot: &mut bool,
    commit: &mut Option<(usize, ElWork)>,
    sel_req: &mut Option<Option<usize>>,
) {
    let Some(list) = pe.elems.get(&pe.page) else {
        return;
    };
    ui.label(
        egui::RichText::new(format!("{}（{}）", tr("元素"), list.len()))
            .size(12.5)
            .color(palette_color(TEXT_MAIN)),
    );
    ui.add_space(4.0);
    if list.is_empty() {
        ui.label(
            egui::RichText::new(tr("本页没有元素。"))
                .size(12.0)
                .color(palette_color(TEXT_WEAK)),
        );
        return;
    }
    for (idx, el) in list.iter().enumerate() {
        let label = format!(
            "{} · {}",
            el_kind_label(&el.kind),
            if el.name.is_empty() {
                format!("{} {}", tr("元素"), idx + 1)
            } else {
                el.name.clone()
            }
        );
        let indent = "　".repeat(el.parents().len());
        if ui
            .selectable_label(pe.selection.contains(&idx), format!("{indent}{label}"))
            .clicked()
        {
            *sel_req = Some(Some(idx));
        }
    }
    let Some(sel) = pe.sel.filter(|s| *s < list.len()) else {
        ui.add_space(6.0);
        ui.label(
            egui::RichText::new(tr("点选画布或列表中的元素进行编辑。"))
                .size(12.0)
                .color(palette_color(TEXT_WEAK)),
        );
        return;
    };
    let orig = list[sel].clone();
    let mut elw = orig.clone();
    ui.add_space(8.0);
    ui.separator();
    ui.label(
        egui::RichText::new(format!(
            "{} · {}",
            el_kind_label(&elw.kind),
            if elw.name.is_empty() {
                tr("未命名元素")
            } else {
                elw.name.as_str()
            }
        ))
        .size(12.5)
        .color(palette_color(TEXT_MAIN)),
    );
    ui.add_space(4.0);
    if !elw.parents().is_empty() {
        ui.label(egui::RichText::new(tr("组内坐标")).weak());
    }
    // Geometry (EMU on the wire, millimetres in the panel).
    if elw.has_xfrm {
        if let Some(v) = mm_field(ui, tr("位置 X"), elw.x, want_snapshot) {
            elw.x = v;
        }
        if let Some(v) = mm_field(ui, tr("位置 Y"), elw.y, want_snapshot) {
            elw.y = v;
        }
        if let Some(v) = mm_field(ui, tr("宽度"), elw.cx, want_snapshot) {
            elw.cx = v.max(20000);
        }
        if let Some(v) = mm_field(ui, tr("高度"), elw.cy, want_snapshot) {
            elw.cy = v.max(20000);
        }
    } else {
        ui.label(
            egui::RichText::new(tr("位置由版式决定，不可拖动（可改文本与样式）。"))
                .size(11.5)
                .color(palette_color(TEXT_FAINT)),
        );
    }
    ui.add_space(6.0);
    match elw.kind.as_str() {
        "picture" => {
            if let Some(m) = &elw.media {
                ui.label(
                    egui::RichText::new(format!("{}：{}", tr("当前图片"), m))
                        .size(11.5)
                        .color(palette_color(TEXT_WEAK)),
                );
            }
            if elw.new_image.is_some() {
                ui.label(
                    egui::RichText::new(tr("已选择新图片，保存后替换。"))
                        .size(11.5)
                        .color(palette_color(ACCENT_DEEP)),
                );
            }
            if ui.button(tr("替换图片…")).clicked() {
                if let Some(path) = rfd::FileDialog::new()
                    .add_filter("图片", &["png", "jpg", "jpeg", "gif", "bmp"])
                    .pick_file()
                {
                    if let Ok(bytes) = std::fs::read(&path) {
                        let ext = path
                            .extension()
                            .and_then(|e| e.to_str())
                            .unwrap_or("png")
                            .to_lowercase();
                        elw.new_image = Some((ext, bytes));
                        *want_snapshot = true;
                    }
                }
            }
        }
        "text" | "shape" => {
            if let Some(preset) = &mut elw.preset {
                let label = SHAPE_PRESETS
                    .iter()
                    .find(|(key, _)| key == preset)
                    .map(|(_, label)| tr(label))
                    .unwrap_or(preset);
                egui::ComboBox::from_id_salt(("shape-preset", sel))
                    .selected_text(label.to_owned())
                    .show_ui(ui, |ui| {
                        for (key, label) in SHAPE_PRESETS {
                            if ui
                                .selectable_value(preset, (*key).into(), tr(label))
                                .changed()
                            {
                                *want_snapshot = true;
                            }
                        }
                    });
            }
            let para_count = elw.paras.len();
            let mut del_para_req: Option<usize> = None;
            for (pi, para) in elw.paras.iter_mut().enumerate() {
                ui.horizontal(|ui| {
                    ui.label(
                        egui::RichText::new(format!("{} {}", tr("段"), pi + 1))
                            .size(11.5)
                            .color(palette_color(TEXT_FAINT)),
                    );
                    if ui.small_button(tr("＋文本")).clicked() {
                        para.runs.push(RunWork {
                            reset_style: false,
                            text: tr("新文本").to_string(),
                            sz: None,
                            b: None,
                            color: None,
                        });
                        *want_snapshot = true;
                    }
                    if para_count > 1 && ui.small_button(tr("删段")).clicked() {
                        del_para_req = Some(pi);
                        *want_snapshot = true;
                    }
                });
                let mut del_run_req: Option<usize> = None;
                for (ri, run) in para.runs.iter_mut().enumerate() {
                    ui.horizontal(|ui| {
                        ui.label(
                            egui::RichText::new(format!("{}", ri + 1))
                                .size(11.5)
                                .color(palette_color(TEXT_FAINT)),
                        );
                        let resp = ui.add(
                            egui::TextEdit::singleline(&mut run.text)
                                .desired_width(168.0)
                                .margin(egui::vec2(6.0, 4.0)),
                        );
                        if resp.gained_focus() {
                            *want_snapshot = true;
                        }
                        if ui.small_button(tr("删")).clicked() {
                            del_run_req = Some(ri);
                            *want_snapshot = true;
                        }
                    });
                    ui.horizontal(|ui| {
                        ui.add_space(14.0);
                        ui.label(
                            egui::RichText::new(tr("字号"))
                                .size(11.0)
                                .color(palette_color(TEXT_FAINT)),
                        );
                        let mut szv = run.sz.unwrap_or(18.0);
                        let resp = ui.add(
                            egui::DragValue::new(&mut szv)
                                .speed(0.25)
                                .range(6.0..=96.0)
                                .suffix(" pt"),
                        );
                        if resp.gained_focus() {
                            *want_snapshot = true;
                        }
                        if resp.changed() {
                            run.sz = Some(szv);
                        }
                        if run.sz.is_none() {
                            ui.label(
                                egui::RichText::new(tr("继承"))
                                    .size(11.0)
                                    .color(palette_color(TEXT_FAINT)),
                            );
                        }
                        let mut bv = run.b.unwrap_or(false);
                        if ui.checkbox(&mut bv, tr("加粗")).changed() {
                            run.b = Some(bv);
                            *want_snapshot = true;
                        }
                        let mut cc = run
                            .color
                            .as_deref()
                            .map(hex_color32)
                            .unwrap_or(egui::Color32::WHITE);
                        if ui.color_edit_button_srgba(&mut cc).changed() {
                            run.color = Some(color32_hex(cc));
                            *want_snapshot = true;
                        }
                        if ui.small_button(tr("重置样式")).clicked() {
                            run.sz = None;
                            run.b = None;
                            run.color = None;
                            run.reset_style = true;
                            *want_snapshot = true;
                        }
                    });
                }
                if let Some(ri) = del_run_req {
                    para.runs.remove(ri);
                }
            }
            if let Some(pi) = del_para_req {
                elw.paras.remove(pi);
            }
            if ui.button(tr("＋ 加一段")).clicked() {
                elw.paras.push(ParaWork {
                    runs: vec![RunWork {
                        reset_style: false,
                        text: tr("新文本").to_string(),
                        sz: None,
                        b: None,
                        color: None,
                    }],
                });
                *want_snapshot = true;
            }
        }
        "line" => {
            ui.horizontal(|ui| {
                ui.label(tr("线条颜色"));
                let mut color = elw
                    .line_color
                    .as_deref()
                    .map(hex_color32)
                    .unwrap_or(egui::Color32::DARK_GRAY);
                if ui.color_edit_button_srgba(&mut color).changed() {
                    elw.line_color = Some(color32_hex(color));
                    *want_snapshot = true;
                }
            });
        }
        "graphic" => {
            ui.label(
                egui::RichText::new(tr("图表：仅位置与大小可调，数据请回源文件编辑。"))
                    .size(11.5)
                    .color(palette_color(TEXT_FAINT)),
            );
        }
        "group" => {
            ui.label(egui::RichText::new(tr("在上方展开的层级列表中选择组内元素。")).weak());
        }
        _ => {}
    }
    if elw != orig {
        *commit = Some((sel, elw));
    }
}

/// 「从模板新建」 picker state (home launcher): choose an existing
/// index as the template; its resolved entries seed the new index's
/// first commit.
struct PickTemplateState {
    new_name: String,
    template_id: String,
}

/// The overlay stack's payloads. Overlays render above the page
/// frames in stack order; Confirm is the top-most kind and is the
/// only one allowed to stack over another overlay (over PageEdit).
enum Overlay {
    /// `index_id` is captured when the window opens (audit M2):
    /// the window renders and acts on that index's detail frame
    /// only, and closes if a different frame is current — it
    /// must never silently rebind to whatever detail is open.
    AllVersions { entry_pos: usize, index_id: String },
    Present(PresentState),
    PageEdit(PageEditState),
    Light(LightState),
    PickTemplate(PickTemplateState),
    Confirm(ConfirmState),
    DeckInfo(DeckInfoState),
}

/// sdInfo (bottom bar, T5 ruling 2026-11-24): a read-only
/// metadata snapshot of one showroom item, computed at click
/// time and rendered as an overlay. Display strings are
/// prebuilt; a field the item does not carry arrives as
/// "—", never invented.
struct DeckInfoState {
    title: String,
    pages: String,
    source_index: String,
    version: String,
    submitter: String,
    scope: String,
}

struct ConfirmState {
    title: String,
    body: String,
    confirm_label: String,
    danger: bool,
    action: ConfirmAction,
}

enum ConfirmAction {
    PurgeTrash(String), // trash row id
    PurgeTrashMany(Vec<String>),
    EmptyTrash,
    DiscardPageEdit,
    MovePageBinding(PageBindingRequest),
    /// Transplant T1: the in-process PDF export hit TextLoss and
    /// the user accepted the raster-only retry (no text layer).
    /// Carries the deck bytes as assembled when the warning was
    /// raised (audit2 E-L5): the retry exports exactly the deck
    /// the user was warned about, not whatever the library has
    /// since become.
    PdfRasterFallback {
        deck: Vec<u8>,
        form: String,
        page: Option<usize>,
        stem: String,
    },
    /// Library discovery (2026-11-25): the post-relocation prompt's
    /// affirmative answer — delete the source library a verified
    /// relocation into the standard directory left behind.
    DeleteOldLibrary(std::path::PathBuf),
    /// Settings → 服务器卡片「关闭服务器」 (user ruling
    /// 2026-11-25): carries the saved-connection index whose
    /// server receives POST /v1/admin/shutdown.
    ShutdownServer(usize),
}

/// Showroom page state (drill retrofit): zone + folder filter
/// are page-level; the deck is a frame. The live/pending lists
/// stay App-level data caches, like units/indexes.
struct ShowroomState {
    zone: String, // plaza | library | inbox | groups
    folder: Option<String>,
    library_view: usize,
    share_source: usize,
    group_id: Option<String>,
    selected_card: Option<String>,
    stack: NavStack<SrFrame>,
}

impl Default for ShowroomState {
    fn default() -> Self {
        Self {
            zone: std::env::var("QIMU_ZONE").unwrap_or_else(|_| "plaza".into()),
            folder: None,
            library_view: 0,
            share_source: 0,
            group_id: None,
            selected_card: None,
            stack: NavStack::new(SrFrame::Wall),
        }
    }
}

/// One frame of the showroom drill stack (design §4).
enum SrFrame {
    Wall,
    Deck(DeckState),
    Present(SrPresentState),
}

/// 讲稿视图帧（原型 presentDlg，整页 sheetpage）：压在 deck 详情帧
/// 之上的一整页页面流——每页一行，左稿面（真实渲染图）、右讲稿
/// 栏。关闭即弹栈回详情帧，详情帧状态原样恢复；本帧保存过的
/// 讲稿在弹栈时回写详情帧的页列表（见 sync_present_notes）。
struct SrPresentState {
    item: Value,
    pages: Vec<ShowroomPage>,
    /// Per-page slide titles (captions under the rendered slide;
    /// empty when the resolving payload carried none).
    titles: Vec<String>,
    /// Full scripts arrays per resolved version id — the save
    /// payload base: PUT /v1/versions/:id/scripts takes the whole
    /// array, so a save must start from the version's complete
    /// scripts, never from this view's excerpted pages alone. A
    /// page whose version is absent here renders its script
    /// read-only: reconstructing a partial array could blank
    /// scripts at slots this view never loaded.
    scripts: std::collections::HashMap<String, Vec<String>>,
    /// Per-page edit buffers, seeded from the pages' scripts;
    /// a buffer commits on focus loss （就地保存， prototype
    /// presentCommit) through the same scripts path the unit
    /// detail uses — saving never bumps a version.
    edits: Vec<String>,
    /// 讲稿栏开关 (prototype 讲稿：开/关）: off hides the note
    /// column and the slides take the full row width.
    script_on: bool,
    /// The prototype toasts 已就地保存这一页。 exactly once per
    /// opening (presentState.edited); later saves stay silent.
    saved_once: bool,
    /// Set by the breadcrumb / 关闭预览； the dispatcher pops.
    closed: bool,
}

/// 演示 deck 帧状态：条目本体 + 已解析的来源索引（可空：个人
/// 组装或加载失败时逐页列表不可用，与旧 sr_deck=None 同义）。
struct DeckState {
    item: Value,
    pages: Vec<ShowroomPage>,
    selected_page: Option<usize>,
    view: DeckView,
    preview_open: bool,
    /// Set by the detail breadcrumb; the dispatcher pops the frame.
    closed: bool,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum DeckView {
    Thumbnails,
    Notes,
}

#[derive(Clone)]
struct ShowroomPage {
    version_id: String,
    slot: usize,
    note: String,
}

/// Resolve excerpt tokens to page positions in a version's JSON:
/// page entity ids match the version's page references, legacy
/// numeric tokens match positionally — the client's mirror of
/// qimu_core::store::excerpt_positions (the one compat read).
fn excerpt_positions_json(version: &Value, tokens: &[Value]) -> Vec<usize> {
    let total = i(version, "page_count").max(0) as usize;
    let pages = version.get("pages").and_then(|p| p.as_array());
    let mut out: Vec<usize> = Vec::new();
    for slot in tokens {
        let Some(tok) = slot.as_str() else { continue };
        if tok.starts_with("pg_") {
            if let Some(pages) = pages {
                for (pos, p) in pages.iter().enumerate() {
                    if p.get("id").and_then(|x| x.as_str()) == Some(tok)
                        && pos < total
                        && !out.contains(&pos)
                    {
                        out.push(pos);
                    }
                }
            }
        } else if let Ok(n) = tok.parse::<usize>() {
            if n < total && !out.contains(&n) {
                out.push(n);
            }
        }
    }
    out
}

fn showroom_pages(deck: &Value) -> Vec<ShowroomPage> {
    let mut pages = Vec::new();
    for entry in deck["resolved_entries"].as_array().into_iter().flatten() {
        let version = &entry["resolved"];
        let version_id = s(version, "id");
        if version_id.is_empty() {
            continue;
        }
        let total = i(version, "page_count").max(0) as usize;
        let slots = match entry["entry"]["excerpt_slots"].as_array() {
            Some(slots) => excerpt_positions_json(version, slots),
            None => (0..total).collect(),
        };
        pages.extend(slots.into_iter().map(|slot| {
            ShowroomPage {
                version_id: version_id.clone(),
                slot,
                note: version["scripts"]
                    .get(slot)
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned(),
            }
        }));
    }
    pages
}

/// Page titles in the same flattened order showroom_pages
/// produces (the 讲稿视图's per-row captions): each resolved
/// version's own page titles at the excerpted slots.
fn showroom_page_titles(deck: &Value) -> Vec<String> {
    let mut titles = Vec::new();
    for entry in deck["resolved_entries"].as_array().into_iter().flatten() {
        let version = &entry["resolved"];
        let version_id = s(version, "id");
        if version_id.is_empty() {
            continue;
        }
        let total = i(version, "page_count").max(0) as usize;
        let slots = match entry["entry"]["excerpt_slots"].as_array() {
            Some(slots) => excerpt_positions_json(version, slots),
            None => (0..total).collect(),
        };
        titles.extend(slots.into_iter().map(|slot| {
            version["pages"]
                .get(slot)
                .map(|p| s(p, "title"))
                .unwrap_or_default()
        }));
    }
    titles
}

/// Full scripts arrays per resolved version id, padded to the
/// version's page count — the base a 讲稿视图 save writes back
/// (see SrPresentState.scripts).
fn version_scripts_map(deck: &Value) -> std::collections::HashMap<String, Vec<String>> {
    let mut map = std::collections::HashMap::new();
    for entry in deck["resolved_entries"].as_array().into_iter().flatten() {
        let version = &entry["resolved"];
        let version_id = s(version, "id");
        if version_id.is_empty() || map.contains_key(&version_id) {
            continue;
        }
        let total = i(version, "page_count").max(0) as usize;
        let mut scripts: Vec<String> = version["scripts"]
            .as_array()
            .map(|a| {
                a.iter()
                    .map(|v| v.as_str().unwrap_or_default().to_owned())
                    .collect()
            })
            .unwrap_or_default();
        scripts.resize(total, String::new());
        map.insert(version_id, scripts);
    }
    map
}

/// Events from the AI-translation worker thread: progress ticks
/// while the server job runs, then exactly one terminal result (the
/// same payload the synchronous translate endpoint returns).
enum TranslateEvent {
    Progress(u64, usize, usize),
    Done(u64, Result<Value, String>),
}

/// Events from the import worker thread. `Opened` is the staged
/// import's (line ⑤) intermediate beat: stage 1 has landed every
/// page as a one-page unit, so the UI refreshes and shows the
/// deck while grouping still runs on the worker — the import is
/// NOT finished, `importing` stays set, and exactly one terminal
/// `Done` still follows per import (Ok is the success wording
/// the caller's flow computed from the import payload, Err the
/// failure wording).
enum ImportEvent {
    Opened(u64, String, Vec<String>),
    Done(u64, Result<String, String>),
    /// Lazy import (2026-11-25): a deck finished opening — its
    /// pages are on the wall as wrappers and the durable split
    /// state exists under this pending id. Carries no generation:
    /// the lazy pipeline only exists over the local library,
    /// which account/mode switches do not invalidate (G4 N4:
    /// local-mode imports are account-independent and keep
    /// running across them).
    LazyOpened(String),
    /// Lazy import: one page's split step landed (see
    /// LazyProgressInfo). Sent by the split worker after EVERY
    /// page, always paired with a repaint request on the wake
    /// context — a background event that does not wake the frame
    /// loop is the freeze line's F1 bug and must not recur here.
    LazyProgress(LazyProgressInfo),
}

/// One lazy split step's report to the UI.
struct LazyProgressInfo {
    pending_id: String,
    index: usize,
    split_count: usize,
    slide_count: usize,
    /// This step folded at least one block (the wall's unit set
    /// changed: wrappers were absorbed into a block unit).
    grouped: bool,
    done: bool,
    failed: bool,
    error: String,
}

/// One still-splitting deck as the UI mirrors it (the durable
/// truth lives in the core's pending-import records; this is
/// the display + queue-bookkeeping copy).
#[derive(Clone)]
struct LazyPendingUi {
    stem: String,
    slide_count: usize,
    split_count: usize,
    failed: bool,
    error: String,
}

/// Commands to the lazy split worker. Priority, highest first:
/// Boost (a page card the user clicked) > Visible (the wall's
/// current viewport) > the worker's own background order (oldest
/// deck first, page order within it).
enum LazyCmd {
    Boost(Vec<(String, usize)>),
    Visible(Vec<(String, usize)>),
    Kick,
}

/// Phase-1 change events (design §2.2): a mutation point marks
/// the view slices it actually touched; `App::apply_dirty`
/// re-reads exactly those slices once, between the frame's
/// channel drains and its drawing. Daily mutation points may
/// only fire the smallest slice events — the composites
/// (LocalWork / OrgAll / All) exist for the login / connection
/// switch class of full-context changes and the migration
/// terminal points the design names.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Change {
    Folders,
    Groups,
    Units,
    Indexes,
    IndexMeta,
    OrgTree,
    ShowroomLive,
    ShowroomPending,
    ReviewQueue,
    MySubmissions,
    Assemblies,
    Inbox,
    GroupSpaces,
    MyTeams,
    Library,
    SessionProfile,
    CacheIndex,
    Trash,
    LocalWork,
    OrgAll,
    All,
}

// DirtySet slice bits.
const D_FOLDERS: u32 = 1 << 0;
const D_GROUPS: u32 = 1 << 1;
const D_UNITS: u32 = 1 << 2;
const D_INDEXES: u32 = 1 << 3;
const D_INDEX_META: u32 = 1 << 4;
const D_ORG_TREE: u32 = 1 << 5;
const D_SHOWROOM_LIVE: u32 = 1 << 6;
const D_SHOWROOM_PENDING: u32 = 1 << 7;
const D_REVIEW_QUEUE: u32 = 1 << 8;
const D_MY_SUBMISSIONS: u32 = 1 << 9;
const D_ASSEMBLIES: u32 = 1 << 10;
const D_INBOX: u32 = 1 << 11;
const D_GROUP_SPACES: u32 = 1 << 12;
const D_MY_TEAMS: u32 = 1 << 13;
const D_LIBRARY: u32 = 1 << 14;
const D_SESSION_PROFILE: u32 = 1 << 15;
const D_CACHE_INDEX: u32 = 1 << 16;
const D_TRASH: u32 = 1 << 17;
const D_LOCAL_BITS: u32 = D_FOLDERS | D_GROUPS | D_UNITS | D_INDEXES;
const D_ORG_BITS: u32 = D_ORG_TREE
    | D_SHOWROOM_LIVE
    | D_SHOWROOM_PENDING
    | D_REVIEW_QUEUE
    | D_MY_SUBMISSIONS
    | D_ASSEMBLIES
    | D_INBOX
    | D_GROUP_SPACES
    | D_MY_TEAMS
    | D_LIBRARY
    | D_SESSION_PROFILE
    | D_CACHE_INDEX;

/// The dirty set (design §2.1): a UI-thread slice bitset plus
/// the fine-grained L5 index-id set. Marking is a bit set —
/// synchronous, no queue, no lock; consumption happens at the
/// single frame point in `apply_dirty`. Cross-thread producers
/// never touch this: they keep posting through their existing
/// channels, and the UI-thread drain marks on their behalf.
#[derive(Default)]
struct DirtySet {
    bits: u32,
    index_ids: std::collections::HashSet<String>,
    /// A consistent four-table snapshot was explicitly asked
    /// for (LocalWork / All), not merely implied by ≥2 L bits.
    local_consistent: bool,
}

impl DirtySet {
    fn mark(&mut self, c: Change) {
        match c {
            Change::Folders => self.bits |= D_FOLDERS,
            Change::Groups => self.bits |= D_GROUPS,
            Change::Units => self.bits |= D_UNITS,
            Change::Indexes => self.bits |= D_INDEXES,
            Change::IndexMeta => self.bits |= D_INDEX_META,
            Change::OrgTree => self.bits |= D_ORG_TREE,
            Change::ShowroomLive => self.bits |= D_SHOWROOM_LIVE,
            Change::ShowroomPending => self.bits |= D_SHOWROOM_PENDING,
            Change::ReviewQueue => self.bits |= D_REVIEW_QUEUE,
            Change::MySubmissions => self.bits |= D_MY_SUBMISSIONS,
            Change::Assemblies => self.bits |= D_ASSEMBLIES,
            Change::Inbox => self.bits |= D_INBOX,
            Change::GroupSpaces => self.bits |= D_GROUP_SPACES,
            Change::MyTeams => self.bits |= D_MY_TEAMS,
            Change::Library => self.bits |= D_LIBRARY,
            Change::SessionProfile => self.bits |= D_SESSION_PROFILE,
            Change::CacheIndex => self.bits |= D_CACHE_INDEX,
            Change::Trash => self.bits |= D_TRASH,
            Change::LocalWork => {
                self.bits |= D_LOCAL_BITS;
                self.local_consistent = true;
            }
            Change::OrgAll => self.bits |= D_ORG_BITS,
            Change::All => {
                self.bits |= D_LOCAL_BITS | D_ORG_BITS | D_INDEX_META;
                self.local_consistent = true;
            }
        }
    }

    fn mark_index(&mut self, id: &str) {
        self.bits |= D_INDEX_META;
        self.index_ids.insert(id.to_string());
    }

    fn has(&self, bit: u32) -> bool {
        self.bits & bit != 0
    }

    fn is_empty(&self) -> bool {
        self.bits == 0 && self.index_ids.is_empty()
    }

    fn clear(&mut self) {
        self.bits = 0;
        self.index_ids.clear();
        self.local_consistent = false;
    }
}

struct App {
    server: String,
    current_server_name: String,
    username: String,
    password: String,
    token: Option<String>,
    user: Option<Value>,
    conns: Vec<Conn>,
    active: Option<usize>,
    /// Reachability latch (audit issue 4): set by the REST call
    /// layer when a request to the active server fails at
    /// transport level, cleared by the next call the server
    /// answers (any HTTP response — success or business error)
    /// and whenever the connection context is re-mirrored. The
    /// session status in `conns` is deliberately NOT rewritten:
    /// a transient outage must not discard a still-valid
    /// session, and nothing revalidates one periodically.
    org_unreachable: std::cell::Cell<bool>,
    add_url: String,
    add_name: String,
    add_user: String,
    add_pass: String,
    discovered: Vec<Value>,
    disc_rx: Option<std::sync::mpsc::Receiver<Vec<Value>>>,
    disc_scanning: bool,
    cache_dir_input: String,
    /// Settings 「数据目录」 input (library-discovery line): the
    /// staged text of the data-directory field; saving runs a
    /// verified relocation and rewrites the bootstrap config.
    data_dir_input: String,
    pick: Option<PickState>,
    /// Cross-page deep-jump source (nav ruler 3): set when Home
    /// opens a detail on another page; that page's root then shows
    /// a back row returning here. Manual tab/menu switches clear it.
    deep_source: Option<Page>,
    folder_edit: Option<FolderEdit>,
    folder_edit_focus: bool,
    folder_delete_confirm: Option<(String, String)>,
    /// Folder delete in flight on its worker thread: (folder id,
    /// mode). The delete never runs on the UI thread (see
    /// folder_delete_window); this doubles as the single-flight
    /// guard for the worker channel below.
    folder_deleting: Option<(String, String)>,
    folder_del_tx: std::sync::mpsc::Sender<FolderDeleteEvent>,
    folder_del_rx: std::sync::mpsc::Receiver<FolderDeleteEvent>,
    /// Generation of the current folder-delete context, same
    /// discipline as translate_gen: bumped by every account
    /// reset; a worker result from an older generation is
    /// dropped, never applied.
    folder_del_gen: u64,
    /// The folder row currently being pressed (long-press drag
    /// gate); see FolderPress.
    folder_press: Option<FolderPress>,
    page: Page,
    /// Five-features surfaces (2026-11-23): the trash and the
    /// approval queue run on drill-down frame stacks; the home
    /// launcher holds a (root-only) stack by the same discipline;
    /// the presenter, the page editor, the template picker and the
    /// confirmations run on the overlay stack above the frames.
    trash: TrashState,
    review: ReviewState,
    nav_home: NavStack<HomeFrame>,
    home_recent_query: String,
    overlays: Vec<Overlay>,
    msg: String,
    folders: Vec<Value>,
    groups: Vec<Value>,
    units: Vec<Value>,
    indexes: Vec<Value>,
    org: Value,
    grants: Vec<Value>,
    showroom_live: Vec<Value>,
    /// Plaza item → folder assignments (client-side mapping;
    /// see load_plaza_item_folders). An item with no entry is
    /// unassigned and appears only under the plaza 全部 node.
    plaza_item_folders: std::collections::HashMap<String, String>,
    showroom_pending: Vec<Value>,
    /// This local library's OWN showroom rows (bundled-server era
    /// rows in the library's showroom table), surfaced in the
    /// showroom 我的 zone as 本地演示. Fed by refresh_local only —
    /// never mixed into showroom_live, which stays the
    /// organization wall's single data face.
    local_showroom: Vec<Value>,
    /// P3 submission chain (design §6.3): pending real-byte
    /// submissions feeding the super approval queue, and the
    /// signed-in user's own submissions for 「我的提交」.
    submissions_pending: Vec<Value>,
    my_submissions: Vec<Value>,
    /// Inline reject-reason draft on a submission card in the
    /// approval queue: (submission id, reason text).
    sub_reject: Option<(String, String)>,
    showroom: ShowroomState,
    my_assemblies: Vec<Value>,
    inbox: Vec<Value>,
    group_spaces: Vec<Value>,
    my_teams: Vec<Value>,
    library_refs: Vec<Value>,
    my_id: String,
    new_team_name: String,
    team_create_open: bool,
    team_member_pick: String,
    cache_entries: Vec<Value>,
    /// PPT clipboard sidecar meta (see `load_clip_meta`); None while
    /// the clipboard is empty. The fragment bytes stay on disk.
    ppt_clip: Option<Value>,
    cache_policy: Value,
    index_covers: std::collections::HashMap<String, String>,
    index_tags: std::collections::HashMap<String, String>,
    index_search_tags: BTreeMap<String, Vec<String>>,
    content: ContentState,
    rendering_unit: Option<String>,
    pending_unit_reload: Option<String>,
    index: IndexState,
    settings: SettingsState,
    frames: u64,
    screenshot_path: Option<String>,
    screenshot_done: bool,
    thumbs: std::collections::HashMap<String, ThumbState>,
    icon_tex: std::collections::HashMap<&'static str, egui::TextureHandle>,
    thumb_tx: std::sync::mpsc::Sender<(String, Option<Vec<u8>>)>,
    thumb_rx: std::sync::mpsc::Receiver<(String, Option<Vec<u8>>)>,
    att_cache: std::collections::HashMap<String, Vec<Value>>,
    pdf_view: Option<PdfView>,
    pdf_tx: std::sync::mpsc::Sender<(
        String,
        u64,
        usize,
        Option<std::sync::Arc<Vec<u8>>>,
        Result<PdfImg, String>,
    )>,
    pdf_rx: std::sync::mpsc::Receiver<(
        String,
        u64,
        usize,
        Option<std::sync::Arc<Vec<u8>>>,
        Result<PdfImg, String>,
    )>,
    /// LAN update-mirror check (craft-update protocol layer, prompt only):
    /// the note shown when the mirror offers a newer release, plus the
    /// worker channel — payload is (was_manual, result).
    upd_note: Option<String>,
    upd_tx: std::sync::mpsc::Sender<(bool, Result<Option<String>, String>)>,
    upd_rx: std::sync::mpsc::Receiver<(bool, Result<Option<String>, String>)>,
    /// AI translation: worker channel plus, while a job is in flight,
    /// (unit id, runs done, runs total) for the progress readout next
    /// to the translate button.
    translate_tx: std::sync::mpsc::Sender<TranslateEvent>,
    translate_rx: std::sync::mpsc::Receiver<TranslateEvent>,
    translating: Option<(String, usize, usize)>,
    /// Generation of the current translation context (audit H1):
    /// bumped by every account-context reset, captured by the
    /// worker at spawn; events from an older generation belong to
    /// the previous account and are dropped, never applied.
    translate_gen: u64,
    /// PPT import: worker channel plus, while an import runs on
    /// its worker thread, the deck name for the in-progress
    /// readout. The import itself (base64, parse, split, verify,
    /// assemble, store) never runs on the UI thread.
    import_tx: std::sync::mpsc::Sender<ImportEvent>,
    import_rx: std::sync::mpsc::Receiver<ImportEvent>,
    importing: Option<String>,
    /// Lazy import (2026-11-25): decks still splitting, by
    /// pending id; wrapper unit id -> (pending id, page index)
    /// for every page not yet split (the map IS the "unusable"
    /// set: cards in it render but cannot open, select or
    /// insert, and clicking one boosts its split); the split
    /// worker's command channel (None until the first pending
    /// deck appears); the turbo flag shared with the worker;
    /// the unsplit unit ids the wall rendered last frame
    /// (viewport priority feed) and the set last sent.
    lazy_pendings: std::collections::HashMap<String, LazyPendingUi>,
    lazy_unit_page: std::collections::HashMap<String, (String, usize)>,
    lazy_cmd_tx: Option<std::sync::mpsc::Sender<LazyCmd>>,
    lazy_turbo: std::sync::Arc<std::sync::atomic::AtomicBool>,
    lazy_visible_boost: std::collections::HashSet<String>,
    lazy_visible_sent: std::collections::HashSet<String>,
    /// Frame-wake context the lazy threads clone: every lazy
    /// background event is paired with a repaint request on it.
    /// (Kept apart from `egui_ctx` below — the freeze line's
    /// per-frame handle — so each pipeline wakes through its
    /// own field.)
    lazy_wake_ctx: Option<egui::Context>,
    /// egui context handle, refreshed every frame in `ui`, so a
    /// worker thread can wake the frame loop when it posts an
    /// event: the reactive loop otherwise sleeps until the next
    /// input event arrives, and events queued meanwhile (an
    /// import's `Opened`, then its `Done`) drain in one late
    /// batch — the window looks frozen and every page appears
    /// at once.
    egui_ctx: Option<egui::Context>,
    /// An import `Opened` beat whose wall refresh is still
    /// waiting for the local store's lock (the grouping stage
    /// holds it): retried on the following frames instead of
    /// blocking the frame that collected the beat.
    open_refresh_pending: bool,
    /// Phase-1 dirty set (design §2.1): slices marked dirty by
    /// mutation points through `changed()`, consumed once per
    /// frame by `apply_dirty` between the channel drains and
    /// drawing. Cleared by `clear_account_state` alongside the
    /// generation bumps, so a previous context's marks can never
    /// be consumed against a new context's data face.
    dirty: DirtySet,
    /// When `apply_dirty` last actually re-read a local slice —
    /// the lazy-split throttle (design §3.1 point 1) spaces
    /// grouped-beat re-reads from here.
    last_local_reread: Option<std::time::Instant>,
    /// Generation of the current import context (audit N4): same
    /// discipline as `translate_gen` for the REST-fallback import
    /// worker, which captures the connection at spawn.
    import_gen: u64,
    /// Wrapper unit ids of the in-flight staged import, between
    /// its Opened beat and its Done (audit N1): grouping is about
    /// to absorb those units, so opening / editing / trashing
    /// them is refused until the import settles.
    import_open_units: Vec<String>,
    /// PDF preview request generation (audit M3): bumped by every
    /// open / page flip; render results carrying an older
    /// generation are stale and dropped.
    pdf_seq: u64,
    /// Forced password change (G2 contract): the login surface
    /// reported `must_change_password` for the active account, so
    /// a non-dismissable change dialog gates the main UI until
    /// the change succeeds. The draft fields live and die here.
    must_change_pw: bool,
    pw_change_old: String,
    pw_change_new: String,
    pw_change_new2: String,
    pw_change_error: String,
    /// Toast re-stamp nonce (audit L4): bumped by `say`, so a
    /// repeated action whose outcome wording is identical to the
    /// previous message still re-shows the toast and re-records
    /// in the message center.
    toast_nonce: u64,
    toast_seen_nonce: u64,
    /// Last /v1/ai/status payload (provider / configured / model) —
    /// drives the "stub placeholder" hint by the translate button.
    ai_status: Option<Value>,
    /// (code, label) target-language options from the server's
    /// languages setting; codes are what renditions store.
    lang_options: Vec<(String, String)>,
    /// DeckCraft handoff (present migration): the managed DeckCraft
    /// desktop instance, its watcher channel and generation counter,
    /// and the auto-quit preference (persisted in present.json).
    dc: Option<DcSession>,
    dc_tx: std::sync::mpsc::Sender<DcEvent>,
    dc_rx: std::sync::mpsc::Receiver<DcEvent>,
    dc_gen: std::sync::Arc<std::sync::atomic::AtomicU64>,
    dc_auto_quit: bool,
    /// 设置 → 演示偏好「放映打开方式」: which engine the unified
    /// view opener routes to (ruling 2026-11-25).
    present_engine: PresentEngine,
    /// T2 (DeckCraft transplant, strategy (b)): the in-process
    /// hosted show, while one is running. While Some, the eframe
    /// App forwards logic / raw_input_hook / ui to the guest
    /// SlideApp and qimu's own UI is not drawn; the guest is
    /// dropped on the first frame its show has ended, and qimu's
    /// fonts/visuals are restored (hosted_show::restore_host_ui).
    hosted: Option<hosted_show::HostedShow>,
    /// Toast state (the framework has no status bar): the message
    /// currently floating and when it appeared, in egui time.
    workspace_controls: WorkspaceControls,
    toast_msg: String,
    toast_since: f64,
    /// Login popup visibility (login is a closable dialog over the
    /// main window, never an entry gate — see login_window).
    login_open: bool,
    /// About dialog visibility.
    about_open: bool,
    /// The in-process local library (local-first P0–P4), opened in
    /// App::new before any connection restore. None only when
    /// opening failed — the reason is kept in `local_open_error`
    /// and shown on the settings page.
    local: Option<std::sync::Arc<local_store::LocalStore>>,
    /// Why the local library failed to open, if it did — shown in
    /// the settings page's local-library block (a lock-held failure
    /// used to surface only as "see the console log"). None when
    /// the library opened fine.
    local_open_error: Option<String>,
    share_dialog: Option<ShareDialogState>,
    pull_dialog: Option<PullDialogState>,
    push_dialog: Option<PushDialogState>,
}

enum ThumbState {
    Loading,
    Ready(egui::TextureHandle),
    /// Carries when the fetch failed (audit2 E-M2): a failure is
    /// retried after a bounded delay instead of pinning the card
    /// to its placeholder for the rest of the session.
    Failed(std::time::Instant),
}

/// One rendered PDF page (PdfCraft / hayro raster), ready to upload as a
/// texture. `rgba` is unpremultiplied RGBA8 (full pages clear to white, so
/// hayro's premultiplied output is identical in practice).
struct PdfImg {
    w: usize,
    h: usize,
    rgba: Vec<u8>,
    pages: usize,
}

/// In-client PDF attachment preview state. The downloaded bytes are kept so
/// page flips re-render locally without another download.
struct PdfView {
    att_id: String,
    filename: String,
    bytes: Option<std::sync::Arc<Vec<u8>>>,
    page: usize,
    pages: usize,
    tex: Option<egui::TextureHandle>,
    note: String,
    /// Generation of the latest render request (audit M3): a
    /// result arriving with an older generation is a page the
    /// user has already flipped away from.
    seq: u64,
}

/// Render one page of a PDF with PdfCraft's rasterizer (off the UI thread).
fn render_pdf_page(bytes: &std::sync::Arc<Vec<u8>>, page: usize) -> Result<PdfImg, String> {
    let mut renderer = pdfcraft_render::PageRenderer::new(
        bytes.clone(),
        pdfcraft_render::RenderConfig::default().on_screen(),
    );
    let pages = renderer.page_count();
    if pages == 0 {
        return Err("PDF 没有可渲染的页面".into());
    }
    let out = renderer.render(pdfcraft_render::RenderRequest {
        page: page.min(pages - 1),
        kind: pdfcraft_render::RequestKind::Pixels,
        tile: None,
        scale: 1.6,
        tag: 0,
        preview: false,
    });
    if let Some(e) = out.error {
        return Err(e);
    }
    let words = out.rgba.into_words();
    let mut rgba = Vec::with_capacity(words.len() * 4);
    for w in &words {
        rgba.extend_from_slice(&w.to_le_bytes());
    }
    Ok(PdfImg {
        w: out.width as usize,
        h: out.height as usize,
        rgba,
        pages,
    })
}

/// One LAN update-mirror probe against a qimu-server's /update/ endpoints,
/// using craft-update's protocol layer (storytold/craft-libs, MIT OR
/// Apache-2.0): the mirror serves a GitHub-shaped release list; we parse
/// it, compare versions, and map the first asset back under the mirror —
/// prompt only, nothing is downloaded or installed here.
/// Ok(Some(note)) = a newer release is offered; Ok(None) = up to date (or
/// the mirror has no releases); Err = the probe itself failed.
fn lan_update_check(server: &str) -> Result<Option<String>, String> {
    let base = format!("{}/update/", server.trim_end_matches('/'));
    let text = ureq::get(&format!("{base}releases.json"))
        .timeout(std::time::Duration::from_secs(5))
        .call()
        .map_err(|e| format!("读取镜像发布列表失败：{e}"))?
        .into_string()
        .map_err(|e| format!("读取镜像发布列表失败：{e}"))?;
    // Audit2 X7: the slug was mistyped "lanline136" — craft-update
    // filters assets by exact release URL, so every mirrored asset
    // was filtered out and the prompt never carried a download.
    let latest = craft_update::parse_github_release_list(&text, "lanline123/qimu", false)
        .map_err(|e| format!("解析镜像发布列表失败：{e}"))?;
    let Some(rel) = latest else { return Ok(None) };
    let (Some(rel_v), Some(cur_v)) = (
        rel.version.as_ref(),
        craft_update::parse_version(env!("CARGO_PKG_VERSION")),
    ) else {
        return Ok(None);
    };
    if !craft_update::version::offers_update(rel_v, &cur_v) {
        return Ok(None);
    }
    let mut note = format!(
        "发现新版本 {}（当前 v{}）：{}",
        rel.version_label(),
        env!("CARGO_PKG_VERSION"),
        rel.name
    );
    if let Some(asset) = rel.assets.first() {
        let url = format!("{}{}/{}", base, rel.tag, asset.name);
        if craft_update::release::download_url_under_mirror(&url, &base) {
            note += &format!("；安装包 {} 可从本服务器下载：{}", asset.name, url);
        }
    }
    note += "（仅提示，不会自动下载安装）";
    Ok(Some(note))
}

/// IconPark action icons (James 2026-11-15: icon-only actions on the
/// showroom deck header). Official SVGs from github.com/bytedance/IconPark
/// (Apache-2.0), re-themed to IconPark's outline style and pre-rendered
/// to PNG — provenance and icon names in assets/icons/README.md.
/// craft-i18n (storytold/craft-libs, MIT OR Apache-2.0): source-keyed
/// localization. The client's source language is Chinese; lookups fall
/// back to the source string, so unrouted or untranslated text renders
/// exactly as before. The app chrome (top bar, login, settings
/// navigation) is routed through `tr()`; QIMU_LOCALE selects the locale
/// ("en" → en-US catalog, anything else → zh-Hans source). Page bodies
/// migrate incrementally — the source-keyed design makes that safe.
static I18N_EMBEDDED: [(&str, &str, &str); 2] = [
    ("zh-hans", "", include_str!("../locales/zh-hans.tsv")),
    ("en-us", "", include_str!("../locales/en-us.tsv")),
];
static I18N: std::sync::OnceLock<craft_i18n::I18n> = std::sync::OnceLock::new();
fn init_i18n() {
    I18N.get_or_init(|| {
        let (i18n, problems) = craft_i18n::I18n::new(&I18N_EMBEDDED, &["zh-hans", "en-us"]);
        for problem in problems {
            eprintln!("i18n: {problem}");
        }
        i18n
    });
    let settings = client_settings();
    // Audit2 X14: the in-app language choice wins; QIMU_LOCALE is
    // only the default for a profile that has never chosen (an env
    // var set machine-wide used to override the user's own pick on
    // every single launch).
    let chosen = s(&settings, "locale");
    let locale = if !chosen.is_empty() {
        chosen
    } else {
        std::env::var("QIMU_LOCALE").unwrap_or_default()
    };
    ENGLISH_UI.set(locale.to_ascii_lowercase().starts_with("en"));
    LIGHT_THEME.set(settings["theme"] == "light");
}

/// Translate a UI string through the embedded catalogs.
fn tr(source: &str) -> &str {
    let Some(i18n) = I18N.get() else {
        return source;
    };
    let code = if ENGLISH_UI.get() { "en-us" } else { "zh-hans" };
    i18n.from_code(code)
        .map_or(source, |lang| i18n.t(lang, source))
}

fn icon_png(b64: &str) -> &'static [u8] {
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(b64.trim())
        .expect("embedded icon base64 must decode");
    Box::leak(bytes.into_boxed_slice())
}

/// The brand logo (blue Q ribbon; user-supplied 2026-11-24,
/// full-res master archived as assets/logo.png), stored as
/// base64 text (assets/logo.png.b64). Drives the shell / login /
/// about image, the runtime window icon (app_icon) and — via
/// build.rs — the executable's embedded icon.
static LOGO_PNG: std::sync::LazyLock<&'static [u8]> =
    std::sync::LazyLock::new(|| icon_png(include_str!("../assets/logo.png.b64")));

/// Temporary index-initiated modes hosted on the content page (§3.10):
/// picking an entry's pages, or inserting a new entry, both start from
/// the index detail and return there — confirm writes one index commit,
/// cancel writes nothing.
#[derive(Clone)]
enum PickState {
    Pages {
        index_name: String,
        entry_pos: usize,
        unit_id: String,
        unit_name: String,
        version_id: String,
        sel: Vec<bool>,
    },
    Insert {
        index_name: String,
        at: usize,
    },
}

/// Inline folder-tree editing (right-click menu): rename an existing
/// folder, or create one under `parent` (None = root level).
#[derive(Clone)]
struct FolderEdit {
    target: Option<String>,
    parent: Option<String>,
    text: String,
    /// Sidebar scope of the surface that opened the editor
    /// (sidebar isolation, user ruling 2026-11-24): a create
    /// lands in that surface's tree and no other.
    scope: String,
}

/// Drag-and-drop payload for the sidebar folder tree: the id of
/// the folder being dragged (see `App::folder_tree`).
#[derive(Clone)]
struct FolderDrag(String);

/// A finished folder delete, posted by its worker thread:
/// (generation, folder id, mode, result). See
/// `App::folder_delete_window` for why the delete runs off the
/// UI thread at all.
type FolderDeleteEvent = (u64, String, String, Result<Value, String>);

/// How long the pointer must rest on a folder row before the row
/// becomes draggable (user ruling 2026-11-24: a plain press must
/// never start a drag — "一点就拖" was the old behaviour's bug).
/// 400ms sits in the usual long-press band (350–450ms): long
/// enough that clicks and scroll starts never arm a drag, short
/// enough to feel responsive.
const FOLDER_LONG_PRESS_SECS: f64 = 0.4;

/// How far the pointer may wander from the press point while the
/// long-press is still pending. Beyond this the gesture is a
/// scroll (or a click-drag elsewhere) and the pending press is
/// cancelled, so the sidebar keeps scrolling exactly as before.
const FOLDER_PRESS_MOVE_TOLERANCE: f32 = 10.0;

/// One in-progress press on a folder row, tracked by
/// `App::folder_tree`. Created on the primary-press frame;
/// `armed` flips when the long-press threshold is reached — only
/// then is the DragAndDrop payload set and the floating row
/// painted. Cleared on release, on an early move (scroll), or
/// when the drag is aborted (Escape clears the payload).
struct FolderPress {
    id: String,
    name: String,
    depth: usize,
    since: f64,
    origin: egui::Pos2,
    armed: bool,
}

// ---------------- server connections (multi-server) -----------------

#[derive(Clone, PartialEq)]
enum ConnStatus {
    Connected,
    Failed,
    LoggedOut,
    /// Anonymous (guest) session: connected for reading the
    /// server's public content, no identity attached.
    Anonymous,
}

/// One saved server connection, persisted in ~/.qimu/servers.json as
/// {name, url, username, token, anon}. The client holds several at
/// once; one is "active" and feeds every API call (pages show the
/// active server). An anonymous connection (anon = true) is a guest
/// session on its server and lives next to, never instead of, a
/// named connection for the same address. Since org-model G1 the
/// anonymous connection carries a server-issued *guest* token in
/// its token field; the anon flag — never token presence — is
/// what marks the connection as identity-less.
#[derive(Clone)]
struct Conn {
    name: String,
    url: String,
    username: String,
    token: Option<String>,
    status: ConnStatus,
    anon: bool,
}

fn normalize_url(u: &str) -> String {
    u.trim().trim_end_matches('/').to_string()
}

fn conn_display_name(url: &str) -> String {
    let host = url
        .trim_start_matches("http://")
        .trim_start_matches("https://")
        .split(['/', ':'])
        .next()
        .unwrap_or(url);
    if host.is_empty() {
        url.to_string()
    } else {
        host.to_string()
    }
}

fn fetch_server_name(url: &str) -> Result<String, String> {
    let response = raw_call(url, None, "GET", "/health", None, true)?;
    if response.get("status").and_then(Value::as_str) != Some("ok") {
        return Err("服务器健康检查未通过".into());
    }
    response
        .get("name")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .map(str::to_string)
        .ok_or_else(|| "服务器未返回名称，请尝试旧版本或手动填写".into())
}

fn load_conns() -> Vec<Conn> {
    let arr = std::fs::read_to_string(qimu_home().join("servers.json"))
        .ok()
        .and_then(|t| serde_json::from_str::<Value>(&t).ok())
        .and_then(|v| v.as_array().cloned())
        .unwrap_or_default();
    arr.iter()
        .filter_map(|v| {
            let url = normalize_url(v.get("url")?.as_str()?);
            if url.is_empty() {
                return None;
            }
            let token = v
                .get("token")
                .and_then(|t| t.as_str())
                .filter(|t| !t.is_empty())
                .map(|t| t.to_string());
            let anon = v.get("anon").and_then(|a| a.as_bool()).unwrap_or(false);
            Some(Conn {
                name: v
                    .get("name")
                    .and_then(|n| n.as_str())
                    .filter(|n| !n.is_empty())
                    .map(|n| n.to_string())
                    .unwrap_or_else(|| conn_display_name(&url)),
                url,
                username: v
                    .get("username")
                    .and_then(|u| u.as_str())
                    .unwrap_or("")
                    .to_string(),
                status: if anon {
                    ConnStatus::Anonymous // probed at startup like the rest
                } else if token.is_some() {
                    ConnStatus::Failed // until validated at startup
                } else {
                    ConnStatus::LoggedOut
                },
                token,
                anon,
            })
        })
        .collect()
}

fn save_conns(conns: &[Conn]) {
    let arr: Vec<Value> = conns
        .iter()
        .map(|c| {
            json!({
                "name": c.name,
                "url": c.url,
                "username": c.username,
                "token": c.token,
                "anon": c.anon,
            })
        })
        .collect();
    let dir = qimu_home();
    let _ = std::fs::create_dir_all(&dir);
    // Audit2 X6: servers.json holds session tokens — it gets the
    // same atomic write as client-settings.json next door (a bare
    // fs::write could leave it truncated mid-crash), and a failed
    // save is logged instead of silently swallowed.
    if let Err(e) = craft_atomic::atomic_write(
        &dir.join("servers.json"),
        serde_json::to_string_pretty(&Value::Array(arr))
            .unwrap_or_default()
            .as_bytes(),
    ) {
        startup_log(&format!("save_conns failed: {e}"));
    }
}

/// Ask a server for an anonymous guest session (org-model G1):
/// POST /v1/auth/guest with nothing but the connection itself.
/// Returns None on a pre-G1 server (no such endpoint) — callers
/// fall back to the tokenless guest probe there.
fn issue_guest_token(url: &str) -> Option<String> {
    raw_call(url, None, "POST", "/v1/auth/guest", None, true)
        .ok()?
        .get("token")
        .and_then(|t| t.as_str())
        .filter(|t| !t.is_empty())
        .map(|t| t.to_string())
}

/// Why a REST call failed, in the one distinction the client
/// acts on (audit issue 4): `Http` — the server answered with a
/// status, so the box is reachable and the error is a business
/// verdict; `Transport` — the request never landed (connect /
/// DNS / timeout), so the box is unreachable right now. The
/// carried string is byte-identical to the error `raw_call`
/// has always returned.
enum CallFail {
    Http(String),
    Transport(String),
}

impl CallFail {
    fn into_string(self) -> String {
        match self {
            CallFail::Http(s) | CallFail::Transport(s) => s,
        }
    }
}

/// The shared REST agent for ordinary calls (audit H2): bounded
/// connect/read timeouts, so a server that vanishes without a
/// RST can no longer freeze the UI thread (or a worker thread)
/// forever. The values are LAN-generous — a 5 s connect bound and
/// a 120 s read-inactivity window leave slow servers and large
/// import payloads room, while guaranteeing every call returns.
fn rest_agent() -> ureq::Agent {
    static AGENT: std::sync::OnceLock<ureq::Agent> = std::sync::OnceLock::new();
    AGENT
        .get_or_init(|| {
            ureq::AgentBuilder::new()
                .timeout_connect(std::time::Duration::from_secs(5))
                .timeout_read(std::time::Duration::from_secs(120))
                .build()
        })
        .clone()
}

/// The thumbnail fetch agent (audit H2): thumbnails are small and
/// plentiful, so their read bound is tighter than the general
/// REST agent's — a stalled fetch resolves as a (retryable)
/// failure instead of pinning the card in Loading forever.
fn thumb_agent() -> ureq::Agent {
    static AGENT: std::sync::OnceLock<ureq::Agent> = std::sync::OnceLock::new();
    AGENT
        .get_or_init(|| {
            ureq::AgentBuilder::new()
                .timeout_connect(std::time::Duration::from_secs(5))
                .timeout_read(std::time::Duration::from_secs(30))
                .build()
        })
        .clone()
}

/// One HTTP call against an explicit server/token (App::call delegates
/// here with the active connection). `short_timeout` is for session
/// validation probes, where a dead server must not stall startup.
fn raw_call_k(
    url: &str,
    token: Option<&str>,
    method: &str,
    path: &str,
    body: Option<Value>,
    short_timeout: bool,
) -> Result<Value, CallFail> {
    // Phase-1 measurement M3 (design §4.1): every REST call is
    // timed and counted (thread-local counters feed the refresh
    // summaries); with the perf switch off this is one flag read.
    if !perf_enabled() {
        return raw_call_k_impl(url, token, method, path, body, short_timeout);
    }
    let t = std::time::Instant::now();
    let r = raw_call_k_impl(url, token, method, path, body, short_timeout);
    let ms = t.elapsed().as_millis();
    perf_rest_note(ms);
    perf_log(&format!("rest {method} {path} {ms}ms"));
    r
}

fn raw_call_k_impl(
    url: &str,
    token: Option<&str>,
    method: &str,
    path: &str,
    body: Option<Value>,
    short_timeout: bool,
) -> Result<Value, CallFail> {
    let full = format!("{}{}", normalize_url(url), path);
    let mut req = if short_timeout {
        let agent = ureq::AgentBuilder::new()
            .timeout_connect(std::time::Duration::from_millis(1200))
            .timeout_read(std::time::Duration::from_secs(3))
            .build();
        match method {
            "GET" => agent.get(&full),
            "PUT" => agent.put(&full),
            "PATCH" => agent.patch(&full),
            "DELETE" => agent.delete(&full),
            _ => agent.post(&full),
        }
    } else {
        let agent = rest_agent();
        match method {
            "GET" => agent.get(&full),
            "PUT" => agent.put(&full),
            "PATCH" => agent.patch(&full),
            "DELETE" => agent.delete(&full),
            _ => agent.post(&full),
        }
    };
    if let Some(t) = token {
        req = req.set("Authorization", &format!("Bearer {t}"));
    }
    let resp = match body {
        Some(b) => req.send_json(b),
        None => req.call(),
    };
    match resp {
        Ok(r) => r.into_json::<Value>().map_err(|e| CallFail::Http(e.to_string())),
        Err(ureq::Error::Status(_, r)) => {
            let text = r.into_string().unwrap_or_default();
            Err(CallFail::Http(serde_json::from_str::<Value>(&text)
                .ok()
                .and_then(|v| {
                    v.get("error")
                        .and_then(|e| e.as_str())
                        .map(|s| s.to_string())
                })
                .unwrap_or(text)))
        }
        Err(e) => Err(CallFail::Transport(format!("请求失败: {e}"))),
    }
}

/// The string-error shape every other caller of the REST layer
/// (validation probes, guest issuance, server-name fetch) was
/// written against; `raw_call_k` is the kind-carrying form and
/// `App::call` uses it to feed the reachability latch.
fn raw_call(
    url: &str,
    token: Option<&str>,
    method: &str,
    path: &str,
    body: Option<Value>,
    short_timeout: bool,
) -> Result<Value, String> {
    raw_call_k(url, token, method, path, body, short_timeout).map_err(CallFail::into_string)
}

/// The worker-thread twin of `App::call_work` (the thumbnail
/// worker's precedent): local dispatch first, REST fallback via
/// `raw_call`. The one deliberate difference is the reachability
/// latch, which is UI-thread state — the import completion path
/// refreshes on the UI thread and re-feeds the latch there.
fn work_call_thread(
    local: &Option<std::sync::Arc<local_store::LocalStore>>,
    server: &str,
    token: Option<&str>,
    method: &str,
    path: &str,
    body: Option<Value>,
) -> Result<Value, String> {
    if let Some(local) = local {
        if let Some(r) = local.dispatch(method, path, body.as_ref()) {
            return r;
        }
    }
    raw_call(server, token, method, path, body, false)
}

/// The staged import sequence (line ⑤) shared by the import
/// actions, run on the caller's worker thread: stage 1 "open"
/// lands every page as a one-page unit — an `Opened` beat tells
/// the UI to refresh onto the new cards — then stage 2 groups
/// the pages into section / supplied blocks while a warm-up
/// thread renders the imported pages' thumbnails in page order
/// (page-entity keyed: a page renders at most once per library,
/// cached pages return from disk). With no local library open,
/// the one-shot /v1/import runs the same two core stages
/// composed server-side. Returns the group stage's payload
/// (`slide_count`, `units`, …).
#[allow(clippy::too_many_arguments)]
fn staged_import_thread(
    local: &Option<std::sync::Arc<local_store::LocalStore>>,
    server: &str,
    token: Option<&str>,
    tx: &std::sync::mpsc::Sender<ImportEvent>,
    gen: u64,
    name: &str,
    bytes: &[u8],
    folder_id: &Option<String>,
    group_id: &str,
    blocks: Option<Value>,
    wake: Option<&egui::Context>,
) -> Result<Value, String> {
    let data = b64e(bytes);
    if local.is_none() {
        let mut body = json!({
            "filename": name,
            "data_base64": data,
            "folder_id": folder_id,
            "group_id": group_id,
        });
        if let Some(b) = blocks {
            body["blocks"] = b;
        }
        return work_call_thread(local, server, token, "POST", "/v1/import", Some(body));
    }
    let mut open_body = json!({
        "filename": name,
        "data_base64": data,
        "folder_id": folder_id,
        "group_id": group_id,
    });
    if let Some(b) = &blocks {
        open_body["blocks"] = b.clone();
    }
    let opened = work_call_thread(
        local,
        server,
        token,
        "POST",
        "/v1/import/open",
        Some(open_body),
    )?;
    let slide_count = opened
        .get("slide_count")
        .and_then(|x| x.as_i64())
        .unwrap_or(0);
    // The wrapper unit ids ride along so the UI can lock those
    // cards until grouping settles (audit N1): a wrapper opened,
    // edited or trashed mid-grouping would be silently discarded
    // — or fail the whole group stage after half of it landed.
    let wrapper_ids: Vec<String> = opened
        .get("pages")
        .and_then(|p| p.as_array())
        .map(|arr| arr.iter().map(|p| s(p, "unit_id")).filter(|u| !u.is_empty()).collect())
        .unwrap_or_default();
    let _ = tx.send(ImportEvent::Opened(
        gen,
        format!("已打开「{name}」：{slide_count} 页，正在成块…"),
        wrapper_ids,
    ));
    // Wake the frame loop so the beat is collected (and the
    // freshly landed cards painted) now, not whenever the next
    // input event happens to schedule a frame.
    if let Some(ctx) = wake {
        ctx.request_repaint();
    }
    if let (Some(local_arc), Some(pages)) = (
        local.clone(),
        opened.get("pages").and_then(|p| p.as_array()),
    ) {
        let page_ids: Vec<String> = pages
            .iter()
            .map(|p| s(p, "page_id"))
            .filter(|id| !id.is_empty())
            .collect();
        std::thread::spawn(move || {
            for pid in &page_ids {
                let _ = local_render::page_thumb(&local_arc, pid);
            }
        });
    }
    let mut group_body = json!({
        "opened": opened,
        "folder_id": folder_id,
        "group_id": group_id,
    });
    if let Some(b) = blocks {
        group_body["blocks"] = b;
    }
    work_call_thread(
        local,
        server,
        token,
        "POST",
        "/v1/import/group",
        Some(group_body),
    )
}

/// The lazy import split worker (user ruling 2026-11-25): a
/// long-lived background thread that drains every pending
/// import page by page through the local face's lazy-split
/// route — one page per call, so the store lock is held for a
/// single page's split and the UI thread never parks behind it
/// (the freeze line's F3 discipline). Page choice, highest
/// priority first: pages the user clicked (Boost) > pages in
/// the wall's current viewport (Visible) > background order
/// (oldest pending deck first, page order inside it — the
/// worker re-reads the durable states for this tier, so a
/// restart or a forgotten deck can never desync it). After
/// every page it reports progress on the import channel AND
/// requests a repaint on the wake context: an event that does
/// not wake the frame loop is the freeze line's F1 bug.
///
/// Pacing: in the default posture the worker yields 30 ms
/// between pages so interactive thumbnail renders and other
/// local calls interleave — the split is still the background's
/// main job and runs continuously. Turbo （极速分割） removes
/// the yield entirely. The store is a single serial connection,
/// so no mode parallelizes splits against each other; turbo is
/// honestly "no throttling", never fake concurrency.
fn lazy_worker_thread(
    local: std::sync::Arc<local_store::LocalStore>,
    cmd_rx: std::sync::mpsc::Receiver<LazyCmd>,
    tx: std::sync::mpsc::Sender<ImportEvent>,
    wake: Option<egui::Context>,
    turbo: std::sync::Arc<std::sync::atomic::AtomicBool>,
) {
    use std::collections::VecDeque;
    let mut boost: VecDeque<(String, usize)> = VecDeque::new();
    let mut visible: VecDeque<(String, usize)> = VecDeque::new();
    // Whether any deck might still have unsplit pages. The
    // worker only spawns when a pending deck exists, and every
    // new deck arrives with a command, so once a background pick
    // finds nothing, the worker blocks on the channel instead
    // of polling the state list forever.
    let mut dirty = true;
    loop {
        // Collect commands; block while there is nothing queued
        // (a Kick or a fresh Boost/Visible is what wakes us).
        if boost.is_empty() && visible.is_empty() {
            let got = if dirty {
                cmd_rx.recv_timeout(std::time::Duration::from_millis(300))
            } else {
                cmd_rx
                    .recv()
                    .map_err(|_| std::sync::mpsc::RecvTimeoutError::Disconnected)
            };
            match got {
                Ok(cmd) => {
                    dirty = true;
                    apply_lazy_cmd(cmd, &mut boost, &mut visible);
                }
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => return,
            }
        }
        while let Ok(cmd) = cmd_rx.try_recv() {
            dirty = true;
            apply_lazy_cmd(cmd, &mut boost, &mut visible);
        }
        let next = boost.pop_front().or_else(|| visible.pop_front());
        let next = match next {
            Some(p) => Some(p),
            None if dirty => {
                let pick = lazy_background_pick(&local);
                if pick.is_none() {
                    dirty = false;
                }
                pick
            }
            None => None,
        };
        let Some((pending_id, index)) = next else {
            continue;
        };
        let body = json!({ "pending_id": pending_id, "index": index });
        let r = local
            .dispatch("POST", "/v1/import/lazy-split", Some(&body))
            .unwrap_or_else(|| Err("本地库未接管分割请求".to_string()));
        match r {
            Ok(v) => {
                let info = LazyProgressInfo {
                    pending_id: pending_id.clone(),
                    index,
                    split_count: i(&v, "split_count") as usize,
                    slide_count: i(&v, "slide_count") as usize,
                    grouped: v
                        .get("grouped_units")
                        .and_then(|g| g.as_array())
                        .map(|a| !a.is_empty())
                        .unwrap_or(false),
                    done: v.get("done").and_then(|x| x.as_bool()).unwrap_or(false),
                    failed: v.get("failed").and_then(|x| x.as_bool()).unwrap_or(false),
                    error: s(&v, "error"),
                };
                if info.failed {
                    boost.retain(|(p, _)| p != &pending_id);
                    visible.retain(|(p, _)| p != &pending_id);
                }
                let _ = tx.send(ImportEvent::LazyProgress(info));
                if let Some(ctx) = &wake {
                    ctx.request_repaint();
                }
            }
            Err(_) => {
                // A failed step (transient store error, a deck
                // forgotten mid-flight): back off briefly instead
                // of spinning on it; the page stays pending and a
                // later Kick or click retries it.
                std::thread::sleep(std::time::Duration::from_millis(400));
            }
        }
        if !turbo.load(std::sync::atomic::Ordering::Relaxed) {
            std::thread::sleep(std::time::Duration::from_millis(30));
        }
    }
}

fn apply_lazy_cmd(
    cmd: LazyCmd,
    boost: &mut std::collections::VecDeque<(String, usize)>,
    visible: &mut std::collections::VecDeque<(String, usize)>,
) {
    match cmd {
        // Front of the queue, argument order preserved: the
        // first page named is the next page split.
        LazyCmd::Boost(pages) => {
            for p in pages.into_iter().rev() {
                boost.push_front(p);
            }
        }
        LazyCmd::Visible(pages) => {
            for p in pages.into_iter().rev() {
                if !visible.contains(&p) {
                    visible.push_front(p);
                }
            }
        }
        LazyCmd::Kick => {}
    }
}

/// The worker's background pick: the oldest pending deck's
/// first unsplit page, straight from the durable states. A deck
/// whose pages all read split but whose record still exists
/// (grouping / the deck-level gate not yet run — e.g. right
/// after a restart) is picked once at its first page, which
/// runs exactly that advance inside the core.
fn lazy_background_pick(
    local: &local_store::LocalStore,
) -> Option<(String, usize)> {
    let states = local
        .dispatch("POST", "/v1/import/lazy-list", None)
        .and_then(|r| r.ok())?;
    for st in states.as_array()? {
        if s(st, "state") == "failed" {
            continue;
        }
        let pid = s(st, "pending_id");
        let pages = st.get("pages").and_then(|p| p.as_array())?;
        if let Some(p) = pages
            .iter()
            .find(|p| p.get("split").and_then(|x| x.as_bool()) != Some(true))
        {
            return Some((pid, i(p, "index") as usize));
        }
        if !pages.is_empty() {
            return Some((pid, i(&pages[0], "index") as usize));
        }
    }
    None
}

/// LAN discovery: broadcast QIMU_DISCOVER_V1 on UDP 8731 and collect the
/// servers' JSON hellos for ~1.5s (deduped by ip+port).
fn discover_servers() -> Vec<Value> {
    use std::net::UdpSocket;
    let mut found: Vec<Value> = Vec::new();
    let Ok(sock) = UdpSocket::bind("0.0.0.0:0") else {
        return found;
    };
    let _ = sock.set_broadcast(true);
    for target in ["255.255.255.255:8731", "127.0.0.1:8731"] {
        let _ = sock.send_to(b"QIMU_DISCOVER_V1", target);
    }
    let _ = sock.set_read_timeout(Some(std::time::Duration::from_millis(200)));
    let deadline = std::time::Instant::now() + std::time::Duration::from_millis(1500);
    let mut buf = [0u8; 2048];
    let mut seen = std::collections::HashSet::new();
    while std::time::Instant::now() < deadline {
        if let Ok((n, src)) = sock.recv_from(&mut buf) {
            if let Ok(v) = serde_json::from_slice::<Value>(&buf[..n]) {
                if v.get("app").and_then(|a| a.as_str()) == Some("qimu") {
                    let port = v.get("port").and_then(|p| p.as_i64()).unwrap_or(8730);
                    let ip = src.ip().to_string();
                    if seen.insert((ip.clone(), port)) {
                        found.push(json!({
                            "name": v.get("name").cloned().unwrap_or_else(|| json!("启幕服务器")),
                            "ip": ip,
                            "port": port,
                            "url": format!("http://{ip}:{port}"),
                            "version": v.get("version").cloned().unwrap_or_else(|| json!("")),
                        }));
                    }
                }
            }
        }
    }
    found
}


/// Login-field seeds from the process environment — the automation
/// harness's way to start the client already pointed at a test
/// server with credentials (audit2 F-N1). Debug builds only: the
/// shipped packages are compiled with the staging or release
/// profile, both of which inherit release defaults
/// (debug_assertions off), so in a shipped client the environment
/// can neither pre-fill a password nor steer the first login at an
/// attacker-chosen server — the fields start with the plain
/// defaults, exactly as if the variables were unset.
#[cfg(debug_assertions)]
fn env_login_seed() -> (String, String, String) {
    (
        std::env::var("QIMU_SERVER").unwrap_or_else(|_| "http://127.0.0.1:8730".into()),
        std::env::var("QIMU_USERNAME").unwrap_or_else(|_| "admin".into()),
        std::env::var("QIMU_PASSWORD").unwrap_or_default(),
    )
}

/// Release-class counterpart of the seed above: constant defaults,
/// the environment is never consulted for credentials.
#[cfg(not(debug_assertions))]
fn env_login_seed() -> (String, String, String) {
    (
        "http://127.0.0.1:8730".into(),
        "admin".into(),
        String::new(),
    )
}

/// Whether the automation harness asked for an immediate login
/// (QIMU_AUTOLOGIN). Debug builds only, like the seed: in shipped
/// packages this is a compile-time false and the branch it guards
/// (including its QIMU_OPEN sub-actions) is eliminated.
#[cfg(debug_assertions)]
fn env_autologin_requested() -> bool {
    std::env::var("QIMU_AUTOLOGIN").is_ok()
}

/// Release-class counterpart: never.
#[cfg(not(debug_assertions))]
fn env_autologin_requested() -> bool {
    false
}

impl App {
    fn new(cc: &eframe::CreationContext<'_>) -> Self {
        egui_extras::install_image_loaders(&cc.egui_ctx);
        install_system_fonts(&cc.egui_ctx);
        init_i18n();
        apply_modern_style(&cc.egui_ctx);
        migrate_cache_dir_once();
        // Local-first: open the in-process local library before
        // any connection is restored — its availability must not
        // depend on the connection state machine. The library is
        // the client's own foundation (P4), opened unconditionally;
        // a failure leaves `local` None with the reason kept for
        // the settings page, and organization connections still
        // work.
        let (local, local_open_error) = local_store::LocalStore::open_library();
        let local = local.map(std::sync::Arc::new);
        let (thumb_tx, thumb_rx) = std::sync::mpsc::channel();
        let (pdf_tx, pdf_rx) = std::sync::mpsc::channel();
        let (upd_tx, upd_rx) = std::sync::mpsc::channel();
        let (translate_tx, translate_rx) = std::sync::mpsc::channel();
        let (import_tx, import_rx) = std::sync::mpsc::channel();
        let (dc_tx, dc_rx) = std::sync::mpsc::channel();
        let (folder_del_tx, folder_del_rx) = std::sync::mpsc::channel();
        let mut app = App {
            server: env_login_seed().0,
            current_server_name: String::new(),
            username: env_login_seed().1,
            password: env_login_seed().2,
            token: None,
            user: None,
            conns: vec![],
            active: None,
            org_unreachable: std::cell::Cell::new(false),
            add_url: String::new(),
            add_name: String::new(),
            add_user: String::new(),
            add_pass: String::new(),
            discovered: vec![],
            disc_rx: None,
            disc_scanning: false,
            cache_dir_input: cache_dir().to_string_lossy().into_owned(),
            data_dir_input: local_store::library_dir().to_string_lossy().into_owned(),
            pick: None,
            deep_source: None,
            folder_edit: None,
            folder_edit_focus: false,
            folder_delete_confirm: None,
            folder_deleting: None,
            folder_del_tx,
            folder_del_rx,
            folder_del_gen: 0,
            folder_press: None,
            page: match std::env::var("QIMU_PAGE").as_deref() {
                Ok("showroom") => Page::Showroom,
                Ok("content") => Page::Content,
                Ok("index") => Page::Index,
                Ok("trash") => Page::Trash,
                Ok("settings") => Page::Settings,
                // Default entry is the home page (user ruling
                // 2026-11-24); QIMU_PAGE remains a test hook only.
                _ => Page::Home,
            },
            trash: TrashState::new(),
            review: ReviewState::new(),
            nav_home: NavStack::new(HomeFrame::Root),
            home_recent_query: String::new(),
            overlays: vec![],
            msg: String::new(),
            folders: vec![],
            groups: vec![],
            units: vec![],
            indexes: vec![],
            org: json!({}),
            grants: vec![],
            showroom_live: vec![],
            local_showroom: vec![],
            showroom_pending: vec![],
            submissions_pending: vec![],
            my_submissions: vec![],
            sub_reject: None,
            showroom: ShowroomState::default(),
            my_assemblies: vec![],
            inbox: vec![],
            group_spaces: vec![],
            my_teams: vec![],
            library_refs: vec![],
            my_id: String::new(),
            new_team_name: String::new(),
            team_create_open: false,
            team_member_pick: String::new(),
            cache_entries: load_cache_index(),
            ppt_clip: load_clip_meta(),
            cache_policy: json!({ "allowed": true, "max_mb": 500 }),
            index_covers: std::collections::HashMap::new(),
            index_tags: std::collections::HashMap::new(),
            index_search_tags: BTreeMap::new(),
            content: ContentState::default(),
            rendering_unit: None,
            pending_unit_reload: None,
            index: IndexState::default(),
            settings: SettingsState {
                grant_subject_type: "dept".into(),
                grant_capability: "view".into(),
                ..Default::default()
            },
            frames: 0,
            screenshot_path: std::env::var("QIMU_SCREENSHOT").ok(),
            screenshot_done: false,
            thumbs: std::collections::HashMap::new(),
            icon_tex: std::collections::HashMap::new(),
            thumb_tx,
            thumb_rx,
            att_cache: std::collections::HashMap::new(),
            pdf_view: None,
            pdf_tx,
            pdf_rx,
            upd_note: None,
            upd_tx,
            upd_rx,
            translate_tx,
            translate_rx,
            translating: None,
            translate_gen: 0,
            import_tx,
            import_rx,
            importing: None,
            lazy_pendings: std::collections::HashMap::new(),
            lazy_unit_page: std::collections::HashMap::new(),
            lazy_cmd_tx: None,
            lazy_turbo: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
            lazy_visible_boost: std::collections::HashSet::new(),
            lazy_visible_sent: std::collections::HashSet::new(),
            lazy_wake_ctx: Some(cc.egui_ctx.clone()),
            egui_ctx: None,
            open_refresh_pending: false,
            dirty: DirtySet::default(),
            last_local_reread: None,
            import_gen: 0,
            import_open_units: vec![],
            pdf_seq: 0,
            must_change_pw: false,
            pw_change_old: String::new(),
            pw_change_new: String::new(),
            pw_change_new2: String::new(),
            pw_change_error: String::new(),
            toast_nonce: 0,
            toast_seen_nonce: 0,
            ai_status: None,
            lang_options: Vec::new(),
            workspace_controls: WorkspaceControls::default(),
            toast_msg: String::new(),
            toast_since: 0.0,
            dc: None,
            dc_tx,
            dc_rx,
            dc_gen: std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0)),
            dc_auto_quit: load_dc_auto_quit(),
            present_engine: load_present_engine(),
            plaza_item_folders: load_plaza_item_folders(),
            hosted: None,
            login_open: false,
            about_open: false,
            local,
            local_open_error,
            share_dialog: None,
            pull_dialog: None,
            push_dialog: None,
        };
        // Library discovery (2026-11-25): when this launch relocated
        // an old library into the standard directory and the new
        // library opened, apply the stored disposition policy to
        // the source. The new library being open IS the
        // re-confirmation the policy requires before any deletion;
        // when the open failed, nothing is deleted and nothing is
        // asked. On the relocation-failure fallback there is no
        // `migrated_from` at all — the source is the live library.
        if app.local.is_some() {
            let resolution = local_store::library_resolution();
            if let Some(src) = &resolution.migrated_from {
                match local_store::read_bootstrap().policy() {
                    local_store::OldLibraryPolicy::Delete => {
                        match std::fs::remove_dir_all(src) {
                            Ok(()) => eprintln!(
                                "[qimu] 搬迁后的旧库已按设置（删除）移除：{}",
                                src.display()
                            ),
                            Err(e) => {
                                eprintln!(
                                    "[qimu] 搬迁后的旧库删除失败（{e}）：{}",
                                    src.display()
                                );
                                // Not stderr-only (audit P5): the
                                // old libraries usually sit under
                                // Program Files, where this delete
                                // routinely lacks permission — the
                                // user who chose "delete" must hear
                                // that it did not happen.
                                app.msg = format!(
                                    "旧库自动删除失败（{e}）：{} 仍保留在原处，可手动删除，或在设置里调整旧库处置",
                                    src.display()
                                );
                            }
                        }
                    }
                    local_store::OldLibraryPolicy::Ask => {
                        app.overlays.push(Overlay::Confirm(ConfirmState {
                            title: "旧库搬迁完成".to_string(),
                            body: format!(
                                "已把旧版本的数据目录从 {} 搬到新目录 {}，内容已核对完整，新库已启用。旧目录里的那份库还留着，不影响使用，只是多占一份空间。要现在删除旧库吗？",
                                src.display(),
                                resolution.dir.display()
                            ),
                            confirm_label: "删除旧库".to_string(),
                            danger: true,
                            action: ConfirmAction::DeleteOldLibrary(src.clone()),
                        }));
                    }
                    local_store::OldLibraryPolicy::Keep => {}
                }
            }
        }
        // Lazy import resume (2026-11-25): decks that were still
        // splitting when the client last closed pick up where
        // they stopped — their pending records are durable in
        // the local library, so this launch re-adopts them and
        // restarts the split worker.
        app.lazy_resume();
        // Plaza wall backfill (2026-11-25): seed the showroom
        // wall from the local cache index before any refresh
        // runs, so a cold start with no reachable server still
        // shows the decks this machine holds cached; the first
        // successful live fetch replaces the list wholesale.
        app.backfill_showroom_from_cache();
        if env_autologin_requested() && !app.password.is_empty() {
            app.do_login();
            // Screenshot helper: QIMU_OPEN=index|unit opens the first item's detail.
            match std::env::var("QIMU_OPEN").as_deref() {
                Ok("index") => {
                    if let Some(first) = app.indexes.first() {
                        let id = s(first, "id");
                        app.open_index_detail(&id);
                    }
                }
                Ok("unit") => {
                    if let Some(first) = app.units.first() {
                        if let Some(u) = first.get("unit") {
                            let id = s(u, "id");
                            app.open_unit_detail(&id);
                        }
                    }
                }
                _ => {}
            }
        } else {
            // Normal start: restore saved connections (validating
            // each token); with none saved, the client simply runs
            // on its local library — no server is started for it.
            app.restore_connections();
            if std::env::var("QIMU_DISCOVER").is_ok() {
                // Screenshot/test hook: run one LAN scan up front.
                app.discovered = discover_servers();
            }
        }
        if app.active.is_none() {
            app.current_server_name =
                fetch_server_name(&app.server).unwrap_or_else(|_| conn_display_name(&app.server));
        }
        // A startup relocation (a settings-registered data-dir
        // move, or a cancelled/failed one) leaves a one-shot
        // notice in the bootstrap config; surface it once here —
        // set after the connection restore so nothing overwrites
        // it before the first frame.
        if let Some(notice) = local_store::take_relocate_notice() {
            app.msg = notice;
        }
        // Screenshot/test hooks that need a logged-in app, on either
        // start path: QIMU_OPEN=showroom:<item-id> opens the deck view;
        // QIMU_PICK=pages:<index-id>:<pos> / insert:<index-id>:<at>
        // enters the content-page pick modes.
        if app.user.is_some() {
            if let Ok(sec) = std::env::var("QIMU_SECTION") {
                if let Ok(n) = sec.parse::<usize>() {
                    app.settings.section = n;
                }
            }
            if let Ok(key) = std::env::var("QIMU_OPEN_CACHE") {
                // Test hook: open a cached deck (decrypts it into
                // cache/open/) without a click.
                app.open_cache(&key);
            }
            if std::env::var("QIMU_PASTE_TEST").is_ok() {
                // Test hook: run the add-card paste flow once (the
                // fragment comes from QIMU_PASTE_FRAGMENT).
                app.create_unit_from_paste();
            }
            if let Ok(item_id) = std::env::var("QIMU_CACHE_DECK") {
                // Test hook: run the plaza "cache locally" flow for one
                // showroom item without a click.
                if let Some(item) = app
                    .showroom_live
                    .iter()
                    .find(|x| s(x, "id") == item_id)
                    .cloned()
                {
                    let src = s(&item, "source_index_id");
                    let title = s(&item, "title");
                    if let Ok(v) = app.call(
                        "POST",
                        &format!("/v1/indexes/{src}/export"),
                        Some(json!({ "branch": "main", "kind": "personal" })),
                    ) {
                        if let Some(data) = v.get("data_base64").and_then(|x| x.as_str()) {
                            if let Ok(bytes) = b64d(data) {
                                let pages = i(&v, "page_count");
                                app.cache_deck(&format!("sr_{item_id}"), &title, &bytes, pages);
                            }
                        }
                    }
                }
            }
            if let Ok(open) = std::env::var("QIMU_OPEN") {
                if let Some(uid) = open.strip_prefix("unit:") {
                    app.open_unit_detail(uid);
                } else if let Some(iid) = open.strip_prefix("index:") {
                    app.open_index_detail(iid);
                } else if let Some(item_id) = open.strip_prefix("showroom:") {
                    if let Some(item) = app
                        .showroom_live
                        .iter()
                        .find(|x| s(x, "id") == item_id)
                        .cloned()
                    {
                        app.open_showroom_deck(&item);
                    }
                }
            }
            if let Ok(dialog) = std::env::var("QIMU_DIALOG") {
                match dialog.as_str() {
                    "share" => {
                        if let Some(item) = app.showroom_live.first().cloned() {
                            app.open_share_dialog(&item);
                        }
                    }
                    "pull" => {
                        if let Some(item) = app.showroom_live.first().cloned() {
                            app.open_pull_dialog(&item, None);
                        }
                    }
                    "push" => {
                        if let Some(index) = app.indexes.first() {
                            app.open_push_dialog(&s(index, "id"), &s(index, "name"));
                        }
                    }
                    _ => {}
                }
            }
            if let Ok(att_id) = std::env::var("QIMU_PDF_PREVIEW") {
                // Test hook: open the in-client PDF preview for one
                // attachment without a click (parent detail first, so the
                // preview renders inside its attachments section).
                if let Some(first) = app.units.first() {
                    if let Some(u) = first.get("unit") {
                        let id = s(u, "id");
                        app.open_unit_detail(&id);
                    }
                }
                app.pdf_open(&json!({ "id": att_id, "filename": "preview.pdf" }));
            }
            if let Ok(pick) = std::env::var("QIMU_PICK") {
                let parts: Vec<&str> = pick.split(':').collect();
                if parts.len() == 3 {
                    let (mode, idx_id, n) = (parts[0], parts[1], parts[2]);
                    app.open_index_detail(idx_id);
                    if mode == "pages" {
                        if let Ok(pos) = n.parse::<usize>() {
                            let resolved = match app.index.stack.top() {
                                IndexFrame::Detail(d) => d
                                    .detail
                                    .get("resolved_entries")
                                    .and_then(|r| r.as_array())
                                    .cloned()
                                    .unwrap_or_default(),
                                _ => Vec::new(),
                            };
                            if let Some(re) = resolved.get(pos).cloned() {
                                let total = re
                                    .get("resolved")
                                    .map(|v| i(v, "page_count"))
                                    .unwrap_or(0)
                                    .max(0) as usize;
                                app.start_pick_pages(pos, &re, vec![true; total]);
                            }
                        }
                    } else if mode == "insert" {
                        if let Ok(at) = n.parse::<usize>() {
                            app.pick = Some(PickState::Insert {
                                index_name: app.current_index_name(),
                                at,
                            });
                            app.content.stack.pop_to(0);
                            app.page = Page::Content;
                        }
                    }
                }
            }
        }
        app
    }

    fn call(&self, method: &str, path: &str, body: Option<Value>) -> Result<Value, String> {
        // Reachability latch (audit issue 4): every call against
        // the active server feeds `org_unreachable` — a transport
        // failure marks the server unreachable, so `org_online`
        // turns false at once instead of trusting a stale
        // session status; any answered call (success or business
        // error) proves reachability and clears the mark. The
        // error strings callers see are unchanged.
        let r = raw_call_k(
            &self.server,
            self.token.as_deref(),
            method,
            path,
            body,
            false,
        );
        match &r {
            Ok(_) => self.org_unreachable.set(false),
            Err(CallFail::Http(_)) => self.org_unreachable.set(false),
            Err(CallFail::Transport(_)) => self.org_unreachable.set(true),
        }
        r.map_err(CallFail::into_string)
    }

    /// Working-data call (local-first P1/P4): the working-data
    /// paths are served by the in-process local library; anything
    /// the local dispatcher does not claim — organization surfaces,
    /// objects that live on the server — falls through to REST
    /// unchanged. With no local library (its open failed) this is
    /// exactly `call`.
    fn call_work(&self, method: &str, path: &str, body: Option<Value>) -> Result<Value, String> {
        if let Some(local) = &self.local {
            if let Some(r) = local.dispatch(method, path, body.as_ref()) {
                return r;
            }
        }
        self.call(method, path, body)
    }

    /// Identity gate for LOCAL actions (local-first P1, design
    /// §6.2): an action whose data lands in the local library —
    /// import, create, paste, folder edits, index work — never
    /// needs a remote identity, so with the local library open it
    /// proceeds ungated. Organization actions (submit, review,
    /// org settings) keep using `need_identity`.
    fn need_identity_local(&mut self, action: &str) -> bool {
        if self.local.is_some() {
            return true;
        }
        self.need_identity(action)
    }

    fn do_login(&mut self) {
        self.server = normalize_url(&self.server);
        let r = self.call(
            "POST",
            "/v1/auth/login",
            Some(json!({ "username": self.username, "password": self.password })),
        );
        match r {
            Ok(v) => {
                self.token = v
                    .get("token")
                    .and_then(|t| t.as_str())
                    .map(|s| s.to_string());
                self.user = v.get("user").cloned();
                self.note_must_change(&v);
                self.msg = "登录成功".into();
                self.login_open = false;
                // Persist this server as a saved connection so the next
                // start goes straight in.
                let url = self.server.clone();
                let name = fetch_server_name(&url).unwrap_or_else(|_| conn_display_name(&url));
                let idx = match self
                    .conns
                    .iter()
                    .position(|c| c.url == url && c.username == self.username)
                {
                    Some(i) => {
                        self.conns[i].name = name;
                        self.conns[i].token = self.token.clone();
                        self.conns[i].status = ConnStatus::Connected;
                        i
                    }
                    None => {
                        self.conns.push(Conn {
                            name,
                            url,
                            username: self.username.clone(),
                            token: self.token.clone(),
                            status: ConnStatus::Connected,
                            anon: false,
                        });
                        self.conns.len() - 1
                    }
                };
                self.active = Some(idx);
                self.persist_conns();
                // A fresh identity was just established: reset the
                // account context before loading this account's
                // surfaces, so nothing of a previous identity
                // (another user, another server, a guest session)
                // survives the login.
                self.clear_account_state();
                self.refresh_all();
                self.check_lan_update(false);
            }
            Err(e) => self.msg = format!("登录失败：{e}"),
        }
    }

    /// LAN update-mirror check (craft-update protocol layer; prompt only —
    /// never downloads or installs). Runs on a worker thread against the
    /// active server's /update/ mirror; the result lands via upd_rx.
    fn check_lan_update(&mut self, manual: bool) {
        let server = self.server.clone();
        let tx = self.upd_tx.clone();
        std::thread::spawn(move || {
            let _ = tx.send((manual, lan_update_check(&server)));
        });
    }

    // ---- connection management ----

    fn persist_conns(&self) {
        save_conns(&self.conns);
        let active_url = self
            .active
            .and_then(|i| self.conns.get(i))
            .map(|c| c.url.clone());
        save_client_settings(json!({ "active_server": active_url }));
    }

    /// Mirror the active connection into the working fields every API
    /// call reads (self.server / self.token / self.username).
    fn sync_from_active(&mut self) {
        // Re-mirroring follows a fresh validation (activate /
        // restore / server switch): the reachability latch
        // starts clean for the new context (audit issue 4).
        self.org_unreachable.set(false);
        if let Some(i) = self.active {
            if let Some(c) = self.conns.get(i) {
                self.server = c.url.clone();
                self.username = c.username.clone();
                // A named session mirrors its user token; an
                // anonymous connection mirrors its guest token
                // (org-model G1), so guest reads carry the
                // session. Identity decisions never key on this
                // field — the connection's anon flag is the key.
                self.token =
                    if c.status == ConnStatus::Connected || c.status == ConnStatus::Anonymous {
                        c.token.clone()
                    } else {
                        None
                    };
                return;
            }
        }
        self.token = None;
    }

    /// Validate conn i's saved token against /v1/me; returns the user
    /// object when the session is still alive.
    fn validate_conn(&mut self, i: usize) -> Option<Value> {
        let (url, token) = {
            let c = self.conns.get(i)?;
            (c.url.clone(), c.token.clone())
        };
        if self.conns[i].anon {
            // Anonymous connection (org-model G1): the connection
            // holds a server-issued guest token; validation probes
            // the guest surface *with* it. An expired guest session
            // (24h by design) is silently re-issued — guest expiry
            // must be invisible. A pre-G1 server never issued a
            // token and refuses bearer probes; there the tokenless
            // probe remains the check, and a stale token is dropped.
            let probe = |t: Option<&str>| {
                raw_call(&url, t, "GET", "/v1/showroom?state=live", None, true).is_ok()
            };
            let mut ok = probe(token.as_deref());
            if !ok {
                if let Some(t) = issue_guest_token(&url) {
                    self.conns[i].token = Some(t.clone());
                    ok = probe(Some(&t));
                }
            }
            if !ok && self.conns[i].token.is_some() && probe(None) {
                self.conns[i].token = None;
                ok = true;
            }
            self.conns[i].status = if ok {
                ConnStatus::Anonymous
            } else {
                ConnStatus::Failed
            };
            return None;
        }
        match token {
            None => {
                self.conns[i].status = ConnStatus::LoggedOut;
                None
            }
            Some(t) => match raw_call(&url, Some(&t), "GET", "/v1/me", None, true) {
                Ok(v) => {
                    self.conns[i].status = ConnStatus::Connected;
                    v.get("user").cloned()
                }
                Err(_) => {
                    self.conns[i].status = ConnStatus::Failed;
                    None
                }
            },
        }
    }

    /// G2 contract: a login / session payload flagging that the
    /// account still uses a must-change password (e.g. the seeded
    /// default). The flag may ride at the payload's top level or
    /// inside its `user` object; either raises the forced change
    /// dialog (`force_change_pw_ui`).
    fn note_must_change(&mut self, v: &Value) {
        let flag = |x: &Value| {
            x.get("must_change_password")
                .and_then(|b| b.as_bool())
                .unwrap_or(false)
        };
        if flag(v) || v.get("user").map(|u| flag(u)).unwrap_or(false) {
            self.must_change_pw = true;
        }
    }

    /// Make conn i the active server: validate, mirror fields, reload.
    fn activate(&mut self, i: usize) {
        if i >= self.conns.len() {
            return;
        }
        if self.active != Some(i) {
            self.clear_account_state();
        }
        self.active = Some(i);
        // The validated identity replaces whatever was there —
        // unconditionally: a failed validation (or an anonymous
        // connection, which validates to None) must never leave
        // the previous account's user object on screen.
        self.user = self.validate_conn(i);
        self.sync_from_active();
        self.persist_conns();
        if self.conns[i].status == ConnStatus::Connected {
            self.msg = format!("已切换到服务器 {}", self.conns[i].name);
            self.refresh_all();
        } else if self.conns[i].status == ConnStatus::Anonymous {
            self.msg = format!(
                "已匿名连接 {}：可浏览公开内容，发布与审批需实名登录",
                self.conns[i].name
            );
            self.refresh_guest();
        } else if self.conns[i].status == ConnStatus::Failed {
            // A failed revalidation must not be silent (audit
            // L2): the previous identity's object is already
            // cleared above; say plainly that this connection did
            // not come back.
            self.say(format!(
                "连接 {} 失败或会话已过期，未能切换；请重新登录",
                self.conns[i].name
            ));
        } else if self.conns[i].status == ConnStatus::LoggedOut {
            self.say(format!("服务器 {} 尚未登录", self.conns[i].name));
        }
        // G2 contract: a session that comes back flagged
        // must-change raises the forced change dialog.
        if let Some(u) = self.user.clone() {
            self.note_must_change(&u);
        }
    }

    /// Startup path when no env-driven login was requested: load saved
    /// connections, validate them, enter through the remembered active
    /// one (or the first that still answers). With nothing saved, make
    /// sure a local server is up so the login screen is ready to use.
    fn restore_connections(&mut self) {
        self.conns = load_conns();
        if self.conns.is_empty() {
            // No organization server saved: the client runs purely
            // on its in-process local library (P4 — standalone is a
            // real mode, not a bundled server in disguise; nothing
            // is spawned, nothing is adopted). Organization surfaces
            // stay empty until the user adds a server.
            self.refresh_all();
            return;
        }
        for i in 0..self.conns.len() {
            self.validate_conn(i);
        }
        // Validation may have re-issued guest sessions (G1); keep
        // the fresh tokens across restarts.
        self.persist_conns();
        let saved = client_settings()
            .get("active_server")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
        let pick = saved
            .as_deref()
            .and_then(|u| {
                self.conns.iter().position(|c| {
                    c.url == u
                        && (c.status == ConnStatus::Connected || c.status == ConnStatus::Anonymous)
                })
            })
            .or_else(|| {
                self.conns
                    .iter()
                    .position(|c| c.status == ConnStatus::Connected)
            })
            .or(Some(0));
        if let Some(i) = pick {
            self.active = Some(i);
            if self.conns[i].status == ConnStatus::Connected {
                let (url, token) = (self.conns[i].url.clone(), self.conns[i].token.clone());
                if let Ok(v) = raw_call(&url, token.as_deref(), "GET", "/v1/me", None, true) {
                    self.user = v.get("user").cloned();
                    self.note_must_change(&v);
                }
            }
            self.sync_from_active();
            if self.conns[i].status == ConnStatus::Connected {
                self.refresh_all();
            }
        }
        if self.token.is_none() || self.anon_active() {
            self.refresh_guest();
        }
        // Startup local load (2026-11-25 rescan fix): the
        // working lists (folders / groups / units / indexes)
        // are this machine's own data and must be loaded at
        // startup whatever the connection restore decided.
        // Until now they loaded only as a side effect of
        // refresh_all, which this function calls solely when
        // the picked connection validated as Connected; on the
        // anonymous / failed / logged-out outcomes only
        // refresh_guest ran (showroom + cache index — never the
        // local lists), so a first launch after a reinstall
        // showed empty walls over a full local library until
        // the first write op's trailing refresh_all happened
        // to load them. refresh_local is idempotent (four
        // local reads plus the per-index detail pass), so on
        // the Connected path this is one extra cheap pass; on
        // every other path it is the load itself.
        if self.local.is_some() {
            self.refresh_local();
        }
    }

    fn fetch_add_server_name(&mut self) {
        let url = normalize_url(&self.add_url);
        if url.is_empty() {
            self.msg = "请先填写服务器地址".into();
            return;
        }
        match fetch_server_name(&url) {
            Ok(name) => {
                self.add_name = name.clone();
                self.msg = format!("已获取服务器名称：{name}");
            }
            Err(error) => self.msg = format!("获取服务器名称失败：{error}"),
        }
    }

    fn server_name_for_add(&mut self, url: &str) -> String {
        let custom = self.add_name.trim();
        if !custom.is_empty() {
            return custom.to_string();
        }
        match fetch_server_name(url) {
            Ok(name) => name,
            Err(_) => conn_display_name(url),
        }
    }

    /// Settings → 添加服务器： log in against the typed address and, on
    /// success, save + activate the new connection.
    fn add_server_login(&mut self) {
        let url = normalize_url(&self.add_url);
        if url.is_empty() || self.add_user.trim().is_empty() {
            self.msg = "请填写服务器地址和账号".into();
            return;
        }
        let username = self.add_user.trim().to_string();
        match raw_call(
            &url,
            None,
            "POST",
            "/v1/auth/login",
            Some(json!({ "username": username, "password": self.add_pass })),
            false,
        ) {
            Ok(v) => {
                self.note_must_change(&v);
                let token = v
                    .get("token")
                    .and_then(|t| t.as_str())
                    .map(|s| s.to_string());
                let name = self.server_name_for_add(&url);
                let idx = match self
                    .conns
                    .iter()
                    .position(|c| c.url == url && c.username == username)
                {
                    Some(i) => {
                        self.conns[i].name = name;
                        self.conns[i].token = token;
                        self.conns[i].status = ConnStatus::Connected;
                        i
                    }
                    None => {
                        self.conns.push(Conn {
                            name,
                            url,
                            username,
                            token,
                            status: ConnStatus::Connected,
                            anon: false,
                        });
                        self.conns.len() - 1
                    }
                };
                self.add_pass.clear();
                self.activate(idx);
                self.msg = "服务器已添加并连接".into();
            }
            Err(e) => self.msg = format!("添加服务器失败：{e}"),
        }
    }

    /// Enter a server anonymously (user ruling 2026-11-23): the
    /// address alone makes the connection — a guest session that
    /// can browse the server's public content and nothing more.
    /// The anonymous connection is a separate saved entry from any
    /// named one for the same address, so entering anonymously
    /// never touches a saved named session. A guest session is
    /// issued first (org-model G1); an unreachable (or
    /// pre-guest-surface) server is reported, not silently
    /// "connected".
    fn activate_anonymous(&mut self, url: &str) -> bool {
        self.activate_anonymous_named(url, None)
    }

    fn activate_anonymous_named(&mut self, url: &str, name: Option<String>) -> bool {
        let url = normalize_url(url);
        if url.is_empty() {
            self.msg = "请先填写服务器地址".into();
            return false;
        }
        let name = name
            .unwrap_or_else(|| fetch_server_name(&url).unwrap_or_else(|_| conn_display_name(&url)));
        // Org-model G1: entering anonymously takes a real guest
        // session from the server (POST /v1/auth/guest) — the
        // token is what the reads ride on, and what makes the
        // anonymous surface revocable and auditable server-side.
        // A pre-G1 server issues nothing; there the historical
        // tokenless probe decides, unchanged.
        let issued = issue_guest_token(&url);
        if issued.is_none()
            && raw_call(&url, None, "GET", "/v1/showroom?state=live", None, true).is_err()
        {
            self.msg = format!("无法匿名连接 {url}：服务器不可达，或版本过旧不支持匿名浏览");
            return false;
        }
        let idx = match self.conns.iter().position(|c| c.url == url && c.anon) {
            Some(i) => {
                self.conns[i].name = name;
                self.conns[i].status = ConnStatus::Anonymous;
                if issued.is_some() {
                    self.conns[i].token = issued.clone();
                }
                i
            }
            None => {
                self.conns.push(Conn {
                    name,
                    url: url.clone(),
                    username: String::new(),
                    token: issued.clone(),
                    status: ConnStatus::Anonymous,
                    anon: true,
                });
                self.conns.len() - 1
            }
        };
        self.active = Some(idx);
        self.server = url;
        self.token = issued;
        self.user = None;
        // This entry mirrors the connection by hand instead of
        // routing through sync_from_active, so it clears the
        // reachability latch itself (audit issues 3+4): the
        // probes above just proved this server answers, and the
        // new guest context must not inherit the previous
        // connection's unreachable mark.
        self.org_unreachable.set(false);
        self.persist_conns();
        // A different identity context than whatever was on screen:
        // drop the previous context's server data, then load the
        // guest one. Local data (offline cache) is untouched.
        self.clear_account_state();
        self.refresh_guest();
        self.msg = format!(
            "已匿名连接 {}：可浏览公开内容，发布与审批需实名登录",
            self.conns[idx].name
        );
        true
    }

    /// The login popup's 「匿名进入」: connect to the typed server
    /// address as a guest. Success closes the popup (as a named
    /// login does); failure keeps it open with the reason inside.
    fn enter_anonymous(&mut self) {
        let url = self.server.clone();
        if self.activate_anonymous(&url) {
            self.login_open = false;
        }
    }

    /// Settings → 添加服务器, the anonymous counterpart of
    /// add_server_login: save + activate a guest connection for
    /// the typed address, no account involved.
    fn add_server_anon(&mut self) {
        let url = normalize_url(&self.add_url);
        if url.is_empty() {
            self.msg = "请填写服务器地址".into();
            return;
        }
        let name = self.server_name_for_add(&url);
        self.activate_anonymous_named(&url, Some(name));
    }

    fn start_discovery(&mut self) {
        if self.disc_scanning {
            return;
        }
        self.disc_scanning = true;
        let (tx, rx) = std::sync::mpsc::channel();
        self.disc_rx = Some(rx);
        std::thread::spawn(move || {
            let _ = tx.send(discover_servers());
        });
    }

    fn disconnect_conn(&mut self, i: usize) {
        let Some(conn) = self.conns.get(i) else {
            return;
        };
        if let Some(token) = conn.token.as_deref() {
            let _ = raw_call(
                &conn.url,
                Some(token),
                "POST",
                "/v1/auth/logout",
                None,
                true,
            );
        }
        let name = conn.name.clone();
        if let Some(conn) = self.conns.get_mut(i) {
            conn.token = None;
            conn.status = ConnStatus::LoggedOut;
        }
        if self.active == Some(i) {
            self.token = None;
            self.user = None;
            self.clear_account_state();
        }
        self.persist_conns();
        self.msg = format!("已断开服务器 {name}（保存的地址保留）");
    }

    /// 设置 → 服务器卡片「关闭服务器」 (user ruling 2026-11-25):
    /// permission prompts come first — per the button-state
    /// standard the button is never greyed for permission, it
    /// stays clickable and explains — then the danger
    /// confirmation. The server re-checks the same gate, so a
    /// saved connection whose role is not known locally (not the
    /// active one) goes to the confirmation and lets the server
    /// decide.
    fn ask_shutdown_server(&mut self, i: usize) {
        let Some(c) = self.conns.get(i) else {
            return;
        };
        if c.anon {
            self.msg = "关闭服务器需要以管理员身份登录该服务器（当前是匿名连接）".into();
            return;
        }
        if self.active == Some(i) {
            let role = self
                .user
                .as_ref()
                .map(|u| s(u, "role"))
                .unwrap_or_default();
            if role != "super" && role != "dept_admin" {
                self.msg = "关闭服务器需要管理员权限（当前账号不是管理员）".into();
                return;
            }
        }
        let name = c.name.clone();
        self.overlays.push(Overlay::Confirm(ConfirmState {
            title: "关闭服务器".to_string(),
            body: format!(
                "将关闭服务器「{name}」。关闭后，局域网内的客户端都将断开连接，组织内容暂时无法访问；恢复需要在服务器所在的电脑上重新启动启幕服务器，或等下次登录时自启。"
            ),
            confirm_label: "关闭服务器".to_string(),
            danger: true,
            action: ConfirmAction::ShutdownServer(i),
        }));
    }

    /// Execute the confirmed server shutdown: POST the admin
    /// endpoint with that connection's own token (never the
    /// active connection's), then let the local state fall to
    /// disconnected quietly — no logout call (the server is
    /// going away), no reconnect attempt, one message only.
    fn do_shutdown_server(&mut self, i: usize) {
        let Some(c) = self.conns.get(i).cloned() else {
            return;
        };
        let Some(token) = c.token.as_deref() else {
            self.msg = "关闭服务器需要先以管理员身份登录该服务器".into();
            return;
        };
        match raw_call(
            &c.url,
            Some(token),
            "POST",
            "/v1/admin/shutdown",
            Some(json!({})),
            false,
        ) {
            Ok(_) => {
                let name = c.name.clone();
                if let Some(conn) = self.conns.get_mut(i) {
                    conn.token = None;
                    conn.status = ConnStatus::LoggedOut;
                }
                if self.active == Some(i) {
                    self.token = None;
                    self.user = None;
                    self.clear_account_state();
                }
                self.persist_conns();
                self.msg = format!(
                    "服务器「{name}」已关闭（局域网客户端已断开，恢复需在服务器电脑上重新启动）"
                );
            }
            Err(e) => {
                self.msg = format!("关闭服务器失败：{e}");
            }
        }
    }

    fn active_server_label(&self) -> String {
        if let Some(i) = self.active {
            if let Some(c) = self.conns.get(i) {
                return c.name.clone();
            }
        }
        if !self.current_server_name.is_empty() {
            return self.current_server_name.clone();
        }
        conn_display_name(&self.server)
    }

    /// Log out of the active connection (the old top bar's 退出 action,
    /// now on the tool row's exit slot and the 文件 menu).
    fn logout(&mut self) {
        let _ = self.call("POST", "/v1/auth/logout", None);
        self.token = None;
        self.user = None;
        if let Some(i) = self.active {
            let (anon, url) = self
                .conns
                .get(i)
                .map(|c| (c.anon, c.url.clone()))
                .unwrap_or((false, String::new()));
            if let Some(c) = self.conns.get_mut(i) {
                c.token = None;
                c.status = ConnStatus::LoggedOut;
            }
            if anon {
                // Logging out of an anonymous connection: fall
                // back to a FRESH guest session, so status, token
                // and org_online() agree again (audit L3 — the old
                // code kept status=Anonymous with the token
                // cleared, claiming "online" while every guest
                // read failed). If the server cannot issue one,
                // the connection honestly reads as logged out.
                if let Some(t) = issue_guest_token(&url) {
                    if let Some(c) = self.conns.get_mut(i) {
                        c.token = Some(t.clone());
                        c.status = ConnStatus::Anonymous;
                    }
                    self.token = Some(t);
                }
            }
            self.persist_conns();
        }
        // The shell stays usable logged out: drop the previous
        // identity's server data (it must not linger on screen) and
        // fall back to the guest surface (published showroom + the
        // local offline cache).
        self.clear_account_state();
        self.refresh_guest();
    }

    /// Identity gate — anonymous connections only (user rulings
    /// 2026-11-23). Standalone/local use never trips it: with a
    /// named session, or with no anonymous connection active (the
    /// local standalone form, where the bundled server is the
    /// user's own data plane), every action proceeds ungated —
    /// local functions need no account, full stop. An anonymous
    /// (guest) session on an organization server is read-only by
    /// design, so write-class actions there — import, create,
    /// publish, review — explain and open the login popup for a
    /// named login instead of dying at the server with a bare 401.
    /// Closing the popup leaves page and context untouched;
    /// nothing is retried automatically after login.
    fn need_identity(&mut self, action: &str) -> bool {
        if !self.anon_active() {
            return true;
        }
        self.msg = format!("「{action}」需要实名登录：匿名连接只能浏览公开内容，请先登录");
        self.login_open = true;
        false
    }

    /// True when the active connection is an anonymous (guest)
    /// session: connected for reading, carrying no named identity.
    /// Keyed on the connection's anon flag rather than on token
    /// presence, so the mode survived the move to server-issued
    /// guest tokens (org-model G1) unchanged — the guest token now
    /// rides in the same token fields a named session uses.
    fn anon_active(&self) -> bool {
        self.active
            .and_then(|i| self.conns.get(i))
            .map(|c| c.anon && c.status == ConnStatus::Anonymous)
            .unwrap_or(false)
    }

    /// What a client without a named session may load: the
    /// showroom's public items (the server serves live items
    /// anonymously — this is both the standalone local server's
    /// own plaza and an organization server's public surface)
    /// plus the local offline cache index. Named-session data
    /// loads through refresh_all instead.
    fn refresh_guest(&mut self) {
        let live_fetched = if let Ok(v) = self.call("GET", "/v1/showroom?state=live", None) {
            self.showroom_live = v
                .get("items")
                .and_then(|f| f.as_array())
                .cloned()
                .unwrap_or_default();
            self.reconcile_cache_staleness();
            // Same failed-cover recovery as the named path
            // (refresh_org): drop empty placeholders so a cover
            // that failed once is retried on the next refresh
            // instead of staying blank for the whole guest
            // session (audit L5).
            self.index_covers.retain(|_, v| !v.is_empty());
            self.resolve_showroom_covers();
            true
        } else {
            false
        };
        self.cache_entries = load_cache_index();
        if !live_fetched {
            // No live list this pass: if none has landed yet
            // this session either, floor the wall with the
            // local cache index instead of leaving it empty.
            self.backfill_showroom_from_cache();
        }
    }

    /// Plaza card covers: resolve each live item's source index to
    /// its first resolved version id (the thumb source). Shared by
    /// the named and guest refresh paths — what a guest actually
    /// receives is bounded by the server's guest read surface, so
    /// on a server that does not (yet) serve index reads
    /// anonymously the lookups fail soft and covers stay blank
    /// until it does; no client change is needed when it expands.
    fn resolve_showroom_covers(&mut self) {
        for item in self.showroom_live.clone() {
            let src = s(&item, "source_index_id");
            if src.is_empty() || self.index_covers.contains_key(&src) {
                continue;
            }
            self.index_covers.insert(src.clone(), String::new());
            if let Ok(d) = self.call("GET", &format!("/v1/indexes/{src}"), None) {
                if let Some(vid) = d
                    .get("resolved_entries")
                    .and_then(|r| r.as_array())
                    .and_then(|a| a.first())
                    .and_then(|e| e.get("resolved"))
                    .and_then(|r| r.get("id"))
                    .and_then(|x| x.as_str())
                {
                    self.index_covers.insert(src, vid.to_string());
                }
            }
        }
    }

    /// Account-context reset — THE single point every identity
    /// change routes through (activate, do_login, logout,
    /// disconnect_conn, anonymous entry, active-server deletion).
    /// Boundary (local-first): everything the *organization*
    /// surface fed is dropped — the org tree and org settings,
    /// showroom and review data, team / inbox / library lists,
    /// the share / pull / push dialogs, server-derived caches.
    /// Everything the *local library* owns is NOT touched: the
    /// working lists while it is open, the offline cache, the
    /// PPT clipboard, the trash — local content belongs to this
    /// machine, not to whichever account happens to be active.
    /// Only in the REST-fallback mode (the local library failed
    /// to open, so the working lists are server-fed) do the
    /// working lists clear here too — and in that mode the trash
    /// is server-fed as well, so it resets here too (audit H3).
    /// Async workers and armed confirmations belong to the
    /// context that spawned them and are closed out here as well
    /// (audits H1 / M2 / N4).
    fn clear_account_state(&mut self) {
        // Credential drafts typed for the previous context.
        self.password.clear();
        self.add_pass.clear();
        // Settings: org-fed sections and org admin form drafts
        // (the section navigation position itself is kept).
        self.settings.profile.clear();
        self.settings.company.clear();
        self.settings.languages_text.clear();
        self.settings.glossary_text.clear();
        self.settings.ai_provider.clear();
        self.settings.ai_endpoint.clear();
        self.settings.ai_model.clear();
        self.settings.ai_key.clear();
        self.settings.ai_has_key = false;
        self.settings.ai_loaded = false;
        self.settings.dept_name.clear();
        self.settings.user_name.clear();
        self.settings.user_display.clear();
        self.settings.user_password.clear();
        self.settings.group_name.clear();
        self.settings.grant_group.clear();
        self.settings.grant_subject_type.clear();
        self.settings.grant_subject_id.clear();
        self.settings.grant_capability.clear();
        // Org data proper.
        self.org = json!({});
        self.grants.clear();
        self.cache_policy = json!({ "allowed": true, "max_mb": 500 });
        self.ai_status = None;
        self.lang_options.clear();
        self.showroom_live.clear();
        self.showroom_pending.clear();
        self.submissions_pending.clear();
        self.my_submissions.clear();
        self.sub_reject = None;
        self.my_assemblies.clear();
        self.inbox.clear();
        self.group_spaces.clear();
        self.my_teams.clear();
        self.library_refs.clear();
        self.new_team_name.clear();
        self.team_member_pick.clear();
        self.my_id.clear();
        // Review face: processed records and any open detail
        // frame (its deck is an org submission's source index).
        self.review = ReviewState::new();
        // Dialogs and previews carrying org content.
        self.share_dialog = None;
        self.pull_dialog = None;
        self.push_dialog = None;
        self.pdf_view = None;
        // Overlays: the DeckInfo overlay snapshots an org showroom
        // item; an armed Confirm would execute against the NEW
        // context if left open (purge-all above all), and the
        // all-versions window reads its target from whatever
        // detail frame is current when it renders — all three go
        // (audit M2). The folder-delete confirm is an App field
        // with the same exposure and goes too. Page-editor
        // overlays are local work and stay.
        self.overlays.retain(|o| {
            !matches!(
                o,
                Overlay::DeckInfo(_) | Overlay::Confirm(_) | Overlay::AllVersions { .. }
            )
        });
        self.folder_delete_confirm = None;
        // In-flight translation belongs to the previous account:
        // its worker polls the old server with the old token.
        // Clear the busy state and bump the generation so the
        // worker's eventual result is dropped, never applied to
        // this context (audit H1).
        self.translating = None;
        self.translate_gen += 1;
        // Phase-1 (design §5.1): the dirty set dies with the
        // context that marked it — same point as the generation
        // bumps — or the next frame would consume the previous
        // account's marks against the new account's data face.
        self.dirty.clear();
        self.last_local_reread = None;
        // The snapshot-retry latch belongs to the old context's
        // in-flight import too; with the dirt gone nothing would
        // ever clear it and the 50ms retry tick would spin on.
        self.open_refresh_pending = false;
        // Same for an in-flight folder delete: in REST-fallback
        // mode it runs against the previous context's server, so
        // its result must not land here either.
        self.folder_deleting = None;
        self.folder_del_gen += 1;
        // The forced-change gate belongs to the identity that
        // raised it.
        self.must_change_pw = false;
        self.pw_change_old.clear();
        self.pw_change_new.clear();
        self.pw_change_new2.clear();
        self.pw_change_error.clear();
        // Trash (audit H3): with the local library open, the
        // trash is this machine's own data and stays. In the
        // REST fallback it is *server*-fed — the previous
        // server's list and preview frame must not survive the
        // switch, or a purge would land on the new server's
        // trash the user never saw.
        if self.local.is_none() {
            self.trash = TrashState::new();
            // An in-flight fallback import likewise captured the
            // previous connection: it would land its deck on the
            // previous server while reporting success here
            // (audit N4). In local mode the import targets the
            // local library regardless of account and is left
            // running.
            self.importing = None;
            self.import_open_units.clear();
            self.import_gen += 1;
        }
        // Showroom navigation: back to the wall, no stale
        // selection of the previous account's items.
        self.showroom.stack.pop_to(0);
        self.showroom.selected_card = None;
        self.showroom.group_id = None;
        self.showroom.folder = None;
        // Derived caches (refetched / rebuilt on next refresh).
        self.index_covers.clear();
        self.index_tags.clear();
        self.index_search_tags.clear();
        self.att_cache.clear();
        // Navigation hygiene for the index and workspace faces
        // (view state only — the data itself is local, below).
        self.index.stack = NavStack::new(IndexFrame::Overview(OverviewState::default()));
        self.index.overview_folder = None;
        self.workspace_controls = WorkspaceControls::default();
        // Working lists: local-library property while it is open
        // (refresh_local owns them); server-fed in the fallback.
        if self.local.is_none() {
            self.folders.clear();
            self.groups.clear();
            self.units.clear();
            self.indexes.clear();
        }
    }

    /// After an import settles, drop wall selection ids whose
    /// units no longer exist (grouping absorbed the one-page
    /// wrappers): the bottom bar must not count or act on dead
    /// ids (audit N1).
    fn prune_wall_selection(&mut self) {
        let alive: Vec<String> = self
            .units
            .iter()
            .filter_map(|it| it.get("unit"))
            .map(|u| s(u, "id"))
            .collect();
        if let ContentFrame::Wall(w) = self.content.stack.root_mut() {
            w.multi.retain(|id| alive.contains(id));
            if let Some(sel) = &w.selected {
                if !alive.contains(sel) {
                    w.selected = None;
                }
            }
        }
    }

    /// Refresh dispatcher (local-first P0/P4): working data
    /// (folders/groups/units/indexes) is fed by the in-process
    /// local library and only the organization surfaces go over
    /// REST. The all-REST refresh (refresh_all_rest below) remains
    /// verbatim as the fallback for a local library that failed
    /// to open.
    /// The import `Opened` beat's wall refresh: stage 1 has
    /// landed every page as a one-page unit and those cards
    /// should appear NOW, while the grouping stage may still be
    /// running on the worker thread. Unlike `refresh_all` this
    /// never blocks the frame: it takes the four working-data
    /// lists through the store's try-lock snapshot (skipping
    /// the per-index detail pass and the org sweep — the
    /// completion refresh covers both) and returns false when
    /// the lock is held, leaving `open_refresh_pending` set so
    /// the drain retries on the following frames. (The full
    /// `refresh_all` here parked the UI thread on the very lock
    /// the grouping stage holds, so the window froze until the
    /// whole import finished and only then showed every page
    /// at once.)
    fn refresh_opened(&mut self) -> bool {
        let Some(local) = self.local.clone() else {
            return true;
        };
        match local.try_snapshot() {
            Some((folders, groups, units, indexes)) => {
                self.folders = folders;
                self.groups = groups;
                self.units = units;
                if let IndexFrame::Overview(overview) = self.index.stack.root_mut() {
                    overview.graphs = None;
                }
                self.indexes = indexes;
                self.open_refresh_pending = false;
                true
            }
            None => {
                self.open_refresh_pending = true;
                // Phase-1 (design §2.5): the retry latch is now
                // shared with the dirty set — a failed snapshot
                // also marks LocalWork so the frame-point
                // apply_dirty owns the retry, not just this
                // function's callers.
                self.dirty.mark(Change::LocalWork);
                false
            }
        }
    }

    fn refresh_all(&mut self) {
        // M3 summary line (kind=all); a no-op unless perf is on.
        let perf_t = std::time::Instant::now();
        let perf_before = perf_rest_snapshot();
        if self.local.is_some() {
            self.refresh_local();
            self.refresh_org();
        } else {
            self.refresh_all_rest();
        }
        perf_refresh_summary("all", perf_t, perf_before, &[]);
    }

    /// Re-read a chosen subset of the working-data lists
    /// (folders / units / indexes) from whichever face serves
    /// them — the local library, or REST in fallback mode —
    /// carrying the same per-list side effects refresh_local /
    /// refresh_all_rest attach: overview-graph invalidation and
    /// the per-index cover/tag pass on an indexes re-read (the
    /// pass only touches indexes whose tags are not remembered
    /// yet, so its cost is bounded by genuinely new indexes, not
    /// by the library size), wall-selection pruning on a units
    /// re-read. Groups are never part of the subset: no delete
    /// path touches them. The post-delete refresh paths use this
    /// instead of refresh_all, whose org sweep (14 sequential
    /// REST GETs in local mode; 18+N round trips in fallback
    /// mode) cannot be affected by a local delete.
    fn refresh_work_lists(&mut self, folders: bool, units: bool, indexes: bool) {
        if let Some(local) = self.local.clone() {
            if folders {
                self.folders = local.list_folders();
            }
            if units {
                self.units = local.list_units();
            }
            if indexes {
                if let IndexFrame::Overview(overview) = self.index.stack.root_mut() {
                    overview.graphs = None;
                }
                self.indexes = local.list_indexes();
                for x in self.indexes.clone() {
                    let xid = s(&x, "id");
                    if xid.is_empty() || self.index_search_tags.contains_key(&xid) {
                        continue;
                    }
                    if let Some(d) = local.index_detail(&xid, "main") {
                        if self
                            .index_covers
                            .get(&xid)
                            .map(|c| c.is_empty())
                            .unwrap_or(true)
                        {
                            if let Some(vid) = d
                                .get("resolved_entries")
                                .and_then(|r| r.as_array())
                                .and_then(|a| a.first())
                                .and_then(|e| e.get("resolved"))
                                .and_then(|r| r.get("id"))
                                .and_then(|v| v.as_str())
                            {
                                self.index_covers.insert(xid.clone(), vid.to_string());
                            }
                        }
                        self.remember_index_tags(&xid, &d);
                    }
                }
            }
        } else {
            if folders {
                if let Ok(v) = self.call("GET", "/v1/folders", None) {
                    self.folders = v
                        .get("folders")
                        .and_then(|f| f.as_array())
                        .cloned()
                        .unwrap_or_default();
                }
            }
            if units {
                if let Ok(v) = self.call("GET", "/v1/units", None) {
                    self.units = v
                        .get("units")
                        .and_then(|f| f.as_array())
                        .cloned()
                        .unwrap_or_default();
                }
            }
            if indexes {
                if let Ok(v) = self.call("GET", "/v1/indexes", None) {
                    if let IndexFrame::Overview(overview) = self.index.stack.root_mut() {
                        overview.graphs = None;
                    }
                    self.indexes = v
                        .get("indexes")
                        .and_then(|f| f.as_array())
                        .cloned()
                        .unwrap_or_default();
                }
                for x in self.indexes.clone() {
                    let xid = s(&x, "id");
                    if xid.is_empty() || self.index_search_tags.contains_key(&xid) {
                        continue;
                    }
                    if let Ok(d) = self.call("GET", &format!("/v1/indexes/{xid}"), None) {
                        if self
                            .index_covers
                            .get(&xid)
                            .map(|c| c.is_empty())
                            .unwrap_or(true)
                        {
                            if let Some(vid) = d
                                .get("resolved_entries")
                                .and_then(|r| r.as_array())
                                .and_then(|a| a.first())
                                .and_then(|e| e.get("resolved"))
                                .and_then(|r| r.get("id"))
                                .and_then(|v| v.as_str())
                            {
                                self.index_covers.insert(xid.clone(), vid.to_string());
                            }
                        }
                        self.remember_index_tags(&xid, &d);
                    }
                }
            }
        }
        if units {
            self.prune_wall_selection();
        }
    }

    /// Post-folder-delete refresh (delete-freeze round 2,
    /// 2026-11-25). The delete itself already runs on the
    /// folder_del worker thread, but the drain used to answer it
    /// with refresh_all: in local mode that is refresh_local's
    /// four-list sweep — list_units alone expands U×(1+2R) SQL
    /// statements for the WHOLE library — plus refresh_org's 14
    /// sequential REST GETs, all on the UI thread in one frame;
    /// in REST-fallback mode it is 18+N sequential round trips.
    /// That drain stall, growing with total library size rather
    /// than with the deleted subtree, is what users still felt
    /// as "delete freezes" on the build that already carried the
    /// worker-thread fix. A folder delete can only change the
    /// folder tree itself and the units / indexes homed in it
    /// (lifted one level up, or gone to the trash), plus the
    /// trash page's rows — so re-read exactly those lists, and
    /// mark the trash stale (its page and sidebar counts reload
    /// on entry; the trash page is not visible while a sidebar
    /// folder delete lands). Org surfaces cannot move.
    fn refresh_after_folder_delete(&mut self, mode: &str) {
        self.refresh_work_lists(true, true, true);
        if mode == "trash" {
            self.trash.loaded = false;
        }
    }

    /// Working data from the local library: folders/groups/
    /// units/indexes reads (P0) plus the per-index detail pass
    /// (covers/tags) assembled locally (P1 — write paths go through
    /// the dispatch in `call_work`, not through here).
    fn refresh_local(&mut self) {
        let Some(local) = self.local.clone() else {
            return;
        };
        self.folders = local.list_folders();
        self.groups = local.list_groups();
        self.units = local.list_units();
        if let IndexFrame::Overview(overview) = self.index.stack.root_mut() {
            overview.graphs = None;
        }
        self.indexes = local.list_indexes();
        self.local_showroom = local.list_local_showroom();
        // Per-index detail pass (P1): covers and search tags for
        // local indexes are assembled from the local store,
        // mirroring the REST loop in refresh_all_rest (which skips
        // indexes whose tags are already remembered).
        self.reread_index_meta_local(None);
    }

    /// The L5 slice (design §1.1): per-index cover + search-tag
    /// fill from the local store. `only` restricts the pass to
    /// the given index ids (the fine-grained dirty set); `None`
    /// is the full form — every not-yet-remembered index. The
    /// remembered-tags memo skips apply in both forms, so an
    /// already-filled id costs nothing.
    fn reread_index_meta_local(
        &mut self,
        only: Option<&std::collections::HashSet<String>>,
    ) {
        let Some(local) = self.local.clone() else {
            return;
        };
        for x in self.indexes.clone() {
            let xid = s(&x, "id");
            if xid.is_empty() || self.index_search_tags.contains_key(&xid) {
                continue;
            }
            if let Some(ids) = only {
                if !ids.contains(&xid) {
                    continue;
                }
            }
            if let Some(d) = local.index_detail(&xid, "main") {
                if self
                    .index_covers
                    .get(&xid)
                    .map(|c| c.is_empty())
                    .unwrap_or(true)
                {
                    if let Some(vid) = d
                        .get("resolved_entries")
                        .and_then(|r| r.as_array())
                        .and_then(|a| a.first())
                        .and_then(|e| e.get("resolved"))
                        .and_then(|r| r.get("id"))
                        .and_then(|v| v.as_str())
                    {
                        self.index_covers.insert(xid.clone(), vid.to_string());
                    }
                }
                self.remember_index_tags(&xid, &d);
            }
        }
    }

    /// The showroom live-wall slice of refresh_org, factored out
    /// verbatim so a single-item change to the wall (a showroom
    /// item trashed or restored) can re-read just this slice —
    /// items, cache-staleness reconciliation, cover re-resolution,
    /// and the offline cache backfill when the fetch fails —
    /// instead of the whole org sweep.
    fn refresh_showroom_live(&mut self) {
        // Phase-1 merge note: the verbatim block now lives in
        // the O2 slice (`reread_showroom_live`, byte-identical);
        // this name remains as the direct-call entry the
        // post-delete refresh paths use.
        self.reread_showroom_live();
    }

    /// Organization surfaces over REST: the org-facing half of the
    /// legacy refresh (same calls, same order) — org tree, showroom,
    /// assemblies, cache policy, inbox, group spaces, teams, library,
    /// profile/company/languages/AI status.
    ///
    /// Phase-1 (design §1.1/§2.3): the body is decomposed into
    /// one method per slice (`reread_*` below, verbatim blocks in
    /// the original order); this composite remains the full sweep
    /// for refresh_all, while `apply_dirty` calls only the slices
    /// a mutation actually dirtied.
    fn refresh_org(&mut self) {
        self.reread_org_tree();
        self.reread_showroom_live();
        self.reread_showroom_pending();
        self.reread_review_queue();
        self.reread_my_submissions();
        self.reread_assemblies();
        self.reread_cache_policy();
        self.reread_inbox();
        self.reread_group_spaces();
        self.reread_my_teams();
        self.reread_library();
        self.reread_cache_index();
        self.reread_profile_tail();
        self.default_import_group();
    }

    /// The O11 slice (design §1.1): cache policy + profile /
    /// company / languages / AI status — five GETs whose content
    /// barely ever changes inside a session. Physically split
    /// into the cache-policy block and the profile tail (their
    /// original positions in the sweep); together they are the
    /// slice `apply_dirty` re-reads.
    fn reread_session_profile(&mut self) {
        self.reread_cache_policy();
        self.reread_profile_tail();
    }

    /// The O1 slice: the organization tree.
    fn reread_org_tree(&mut self) {
        if let Ok(v) = self.call("GET", "/v1/org", None) {
            self.org = v;
        }
    }

    /// The O2 slice: the public showroom wall, with its three
    /// §1.2 side effects (cache staleness reconciliation, empty
    /// cover eviction + cover resolution) and the offline cache
    /// backfill when the fetch itself fails.
    fn reread_showroom_live(&mut self) {
        let live_fetched = if let Ok(v) = self.call("GET", "/v1/showroom?state=live", None) {
            self.showroom_live = v
                .get("items")
                .and_then(|f| f.as_array())
                .cloned()
                .unwrap_or_default();
            self.reconcile_cache_staleness();
            self.index_covers.retain(|_, v| !v.is_empty());
            self.resolve_showroom_covers();
            true
        } else {
            false
        };
        if !live_fetched {
            self.backfill_showroom_from_cache();
        }
    }

    /// The O3 slice: the pending-review showroom wall.
    fn reread_showroom_pending(&mut self) {
        if let Ok(v) = self.call("GET", "/v1/showroom?state=pending", None) {
            self.showroom_pending = v
                .get("items")
                .and_then(|f| f.as_array())
                .cloned()
                .unwrap_or_default();
        }
    }

    /// The O4 slice: the submission approval queue (super only).
    fn reread_review_queue(&mut self) {
        // P3 submission chain (design §6.3): the approval queue
        // also counts real-byte submissions, and the signed-in
        // user's own submissions feed the 「我的提交」 face.
        if self.role() == "super" {
            if let Ok(v) = self.call("GET", "/v1/submissions?state=pending", None) {
                self.submissions_pending = v.get("items").and_then(|f| f.as_array()).cloned().unwrap_or_default();
            }
        }
    }

    /// The O5 slice: the signed-in user's own submissions.
    fn reread_my_submissions(&mut self) {
        if self.user.is_some() {
            if let Ok(v) = self.call("GET", "/v1/submissions?mine=1", None) {
                self.my_submissions = v.get("items").and_then(|f| f.as_array()).cloned().unwrap_or_default();
            }
        }
    }

    /// The O6 slice: my assemblies.
    fn reread_assemblies(&mut self) {
        if let Ok(v) = self.call("GET", "/v1/assemblies", None) {
            self.my_assemblies = v
                .get("assemblies")
                .and_then(|f| f.as_array())
                .cloned()
                .unwrap_or_default();
        }
    }

    /// O11 part 1: the cache policy.
    fn reread_cache_policy(&mut self) {
        if let Ok(v) = self.call("GET", "/v1/settings/cache-policy", None) {
            self.cache_policy = v;
        }
    }

    /// The O7 slice: the share inbox.
    fn reread_inbox(&mut self) {
        if let Ok(v) = self.call("GET", "/v1/shares/inbox", None) {
            self.inbox = v
                .get("shares")
                .and_then(|f| f.as_array())
                .cloned()
                .unwrap_or_default();
        }
    }

    /// The O8 slice: group spaces.
    fn reread_group_spaces(&mut self) {
        if let Ok(v) = self.call("GET", "/v1/group-space", None) {
            self.group_spaces = v
                .get("spaces")
                .and_then(|f| f.as_array())
                .cloned()
                .unwrap_or_default();
        }
    }

    /// The O9 slice: my custom groups.
    fn reread_my_teams(&mut self) {
        if let Ok(v) = self.call("GET", "/v1/custom-groups", None) {
            self.my_teams = v
                .get("groups")
                .and_then(|f| f.as_array())
                .cloned()
                .unwrap_or_default();
        }
    }

    /// The O10 slice: my library references.
    fn reread_library(&mut self) {
        if let Ok(v) = self.call("GET", "/v1/library", None) {
            self.library_refs = v
                .get("entries")
                .and_then(|f| f.as_array())
                .cloned()
                .unwrap_or_default();
        }
    }

    /// The C1 slice: the on-disk cache-bundle index. (The
    /// staleness reconciliation against the live wall rides the
    /// O2 re-read, per design §1.2; this slice is the disk read
    /// itself.)
    fn reread_cache_index(&mut self) {
        self.cache_entries = load_cache_index();
    }

    /// O11 part 2: /v1/me + company profile + languages + AI
    /// status (their original position at the sweep's tail).
    fn reread_profile_tail(&mut self) {
        if let Ok(v) = self.call("GET", "/v1/me", None) {
            if let Some(u) = v.get("user") {
                self.my_id = s(u, "id");
            }
            if let Some(p) = v.get("profile").and_then(|p| p.as_object()) {
                self.settings.profile = p
                    .iter()
                    .map(|(k, val)| (k.clone(), val.as_str().unwrap_or("").to_string()))
                    .collect();
            }
        }
        if let Ok(v) = self.call("GET", "/v1/company-profile", None) {
            if let Some(p) = v.get("fields").and_then(|p| p.as_object()) {
                self.settings.company = p
                    .iter()
                    .map(|(k, val)| (k.clone(), val.as_str().unwrap_or("").to_string()))
                    .collect();
            }
        }
        if let Ok(v) = self.call("GET", "/v1/settings/languages", None) {
            if let Some(l) = v.get("languages").and_then(|p| p.as_array()) {
                self.settings.languages_text = l
                    .iter()
                    .filter_map(|x| x.as_str())
                    .collect::<Vec<_>>()
                    .join(", ");
                let codes: Vec<String> = v
                    .get("codes")
                    .and_then(|p| p.as_array())
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|x| x.as_str().map(|s| s.to_string()))
                            .collect()
                    })
                    .unwrap_or_default();
                if !codes.is_empty() {
                    self.lang_options = codes
                        .iter()
                        .enumerate()
                        .map(|(idx, code)| {
                            let label = l
                                .get(idx)
                                .and_then(|x| x.as_str())
                                .unwrap_or(code.as_str())
                                .to_string();
                            (code.clone(), label)
                        })
                        .collect();
                }
            }
        }
        if let Ok(v) = self.call("GET", "/v1/ai/status", None) {
            self.ai_status = Some(v);
        }
    }

    /// The org sweep's tail side effect: a default import group
    /// from the first local group, when none was ever chosen.
    /// Depends on the (local) groups list, not on any org slice.
    fn default_import_group(&mut self) {
        if self.content.import_group.is_empty() {
            if let Some(g) = self.groups.first() {
                self.content.import_group = g
                    .get("id")
                    .and_then(|x| x.as_str())
                    .unwrap_or("")
                    .to_string();
            }
        }
    }

    fn refresh_all_rest(&mut self) {
        self.refresh_tables_rest();
        self.refresh_org_rest_tail();
    }

    /// The REST-fallback four-table segment (design §5.1): the
    /// working lists fed over REST when no local library is
    /// open. `apply_dirty` upgrades L-slice dirt to exactly this
    /// segment — never the org sweep below.
    fn refresh_tables_rest(&mut self) {
        if let Ok(v) = self.call("GET", "/v1/folders", None) {
            self.folders = v
                .get("folders")
                .and_then(|f| f.as_array())
                .cloned()
                .unwrap_or_default();
        }
        if let Ok(v) = self.call("GET", "/v1/groups", None) {
            self.groups = v
                .get("groups")
                .and_then(|f| f.as_array())
                .cloned()
                .unwrap_or_default();
        }
        if let Ok(v) = self.call("GET", "/v1/units", None) {
            self.units = v
                .get("units")
                .and_then(|f| f.as_array())
                .cloned()
                .unwrap_or_default();
        }
        if let Ok(v) = self.call("GET", "/v1/indexes", None) {
            if let IndexFrame::Overview(overview) = self.index.stack.root_mut() {
                overview.graphs = None;
            }
            self.indexes = v
                .get("indexes")
                .and_then(|f| f.as_array())
                .cloned()
                .unwrap_or_default();
        }
    }

    /// Everything in the all-REST refresh past the four tables:
    /// the org sweep in its REST form (verbatim blocks).
    fn refresh_org_rest_tail(&mut self) {
        if let Ok(v) = self.call("GET", "/v1/org", None) {
            self.org = v;
        }
        let live_fetched = if let Ok(v) = self.call("GET", "/v1/showroom?state=live", None) {
            self.showroom_live = v
                .get("items")
                .and_then(|f| f.as_array())
                .cloned()
                .unwrap_or_default();
            self.reconcile_cache_staleness();
            // A guest pass earlier in this session may have parked
            // empty placeholders for covers it could not resolve;
            // a named session re-resolves instead of inheriting
            // the blanks.
            self.index_covers.retain(|_, v| !v.is_empty());
            self.resolve_showroom_covers();
            true
        } else {
            false
        };
        if !live_fetched {
            self.backfill_showroom_from_cache();
        }
        self.reread_index_meta_rest();
        if let Ok(v) = self.call("GET", "/v1/showroom?state=pending", None) {
            self.showroom_pending = v
                .get("items")
                .and_then(|f| f.as_array())
                .cloned()
                .unwrap_or_default();
        }
        if self.role() == "super" {
            if let Ok(v) = self.call("GET", "/v1/submissions?state=pending", None) {
                self.submissions_pending = v.get("items").and_then(|f| f.as_array()).cloned().unwrap_or_default();
            }
        }
        if self.user.is_some() {
            if let Ok(v) = self.call("GET", "/v1/submissions?mine=1", None) {
                self.my_submissions = v.get("items").and_then(|f| f.as_array()).cloned().unwrap_or_default();
            }
        }
        if let Ok(v) = self.call("GET", "/v1/assemblies", None) {
            self.my_assemblies = v
                .get("assemblies")
                .and_then(|f| f.as_array())
                .cloned()
                .unwrap_or_default();
        }
        if let Ok(v) = self.call("GET", "/v1/settings/cache-policy", None) {
            self.cache_policy = v;
        }
        if let Ok(v) = self.call("GET", "/v1/shares/inbox", None) {
            self.inbox = v
                .get("shares")
                .and_then(|f| f.as_array())
                .cloned()
                .unwrap_or_default();
        }
        if let Ok(v) = self.call("GET", "/v1/group-space", None) {
            self.group_spaces = v
                .get("spaces")
                .and_then(|f| f.as_array())
                .cloned()
                .unwrap_or_default();
        }
        if let Ok(v) = self.call("GET", "/v1/custom-groups", None) {
            self.my_teams = v
                .get("groups")
                .and_then(|f| f.as_array())
                .cloned()
                .unwrap_or_default();
        }
        if let Ok(v) = self.call("GET", "/v1/library", None) {
            self.library_refs = v
                .get("entries")
                .and_then(|f| f.as_array())
                .cloned()
                .unwrap_or_default();
        }
        self.cache_entries = load_cache_index();
        if let Ok(v) = self.call("GET", "/v1/me", None) {
            if let Some(u) = v.get("user") {
                self.my_id = s(u, "id");
            }
            if let Some(p) = v.get("profile").and_then(|p| p.as_object()) {
                self.settings.profile = p
                    .iter()
                    .map(|(k, val)| (k.clone(), val.as_str().unwrap_or("").to_string()))
                    .collect();
            }
        }
        if let Ok(v) = self.call("GET", "/v1/company-profile", None) {
            if let Some(p) = v.get("fields").and_then(|p| p.as_object()) {
                self.settings.company = p
                    .iter()
                    .map(|(k, val)| (k.clone(), val.as_str().unwrap_or("").to_string()))
                    .collect();
            }
        }
        if let Ok(v) = self.call("GET", "/v1/settings/languages", None) {
            if let Some(l) = v.get("languages").and_then(|p| p.as_array()) {
                self.settings.languages_text = l
                    .iter()
                    .filter_map(|x| x.as_str())
                    .collect::<Vec<_>>()
                    .join(", ");
                let codes: Vec<String> = v
                    .get("codes")
                    .and_then(|p| p.as_array())
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|x| x.as_str().map(|s| s.to_string()))
                            .collect()
                    })
                    .unwrap_or_default();
                if !codes.is_empty() {
                    self.lang_options = codes
                        .iter()
                        .enumerate()
                        .map(|(idx, code)| {
                            let label = l
                                .get(idx)
                                .and_then(|x| x.as_str())
                                .unwrap_or(code.as_str())
                                .to_string();
                            (code.clone(), label)
                        })
                        .collect();
                }
            }
        }
        if let Ok(v) = self.call("GET", "/v1/ai/status", None) {
            self.ai_status = Some(v);
        }
        if self.content.import_group.is_empty() {
            if let Some(g) = self.groups.first() {
                self.content.import_group = g
                    .get("id")
                    .and_then(|x| x.as_str())
                    .unwrap_or("")
                    .to_string();
            }
        }
    }

    /// The L5 slice in REST-fallback mode: per-index cover + tag
    /// fill over REST, skipping remembered indexes (the REST
    /// twin of `reread_index_meta_local`, full form only — the
    /// fallback's per-index GET loop is the unit the legacy code
    /// already had).
    fn reread_index_meta_rest(&mut self) {
        for x in self.indexes.clone() {
            let xid = s(&x, "id");
            if xid.is_empty() || self.index_search_tags.contains_key(&xid) {
                continue;
            }
            if let Ok(d) = self.call("GET", &format!("/v1/indexes/{xid}"), None) {
                if self
                    .index_covers
                    .get(&xid)
                    .map(|c| c.is_empty())
                    .unwrap_or(true)
                {
                    if let Some(vid) = d
                        .get("resolved_entries")
                        .and_then(|r| r.as_array())
                        .and_then(|a| a.first())
                        .and_then(|e| e.get("resolved"))
                        .and_then(|r| r.get("id"))
                        .and_then(|v| v.as_str())
                    {
                        self.index_covers.insert(xid.clone(), vid.to_string());
                    }
                }
                self.remember_index_tags(&xid, &d);
            }
        }
    }

    // ---------- Phase 1: change events + sliced re-reads ----------
    // (design §2 — the refresh_all replacement machinery. The
    // gray switch QIMU_REFRESH_V2 lives at this one entry family:
    // with it off, every entry below falls back to the legacy
    // refresh_all() and the migration call sites stay single-form.)

    /// Mark slices dirty; consumed at the frame point
    /// (`apply_dirty` in `ui`, after the channel drains).
    fn changed(&mut self, c: Change) {
        if refresh_v2_enabled() {
            self.dirty.mark(c);
        } else {
            self.refresh_all();
        }
    }

    /// Mark one index's L5 slice dirty (the fine-grained form).
    fn changed_index_meta(&mut self, id: &str) {
        if refresh_v2_enabled() {
            self.dirty.mark_index(id);
        } else {
            self.refresh_all();
        }
    }

    /// Mark + consume immediately — for call points that read
    /// the refreshed fields later in the same function (design
    /// §2.3's read-after-write rule).
    fn changed_now(&mut self, c: Change) {
        if refresh_v2_enabled() {
            self.dirty.mark(c);
            self.apply_dirty_inner(true);
        } else {
            self.refresh_all();
        }
    }

    /// Mark only (composition helper: multi-slice points mark
    /// each slice, then flush once through `flush_dirty_now`).
    fn mark(&mut self, c: Change) {
        if refresh_v2_enabled() {
            self.dirty.mark(c);
        }
    }

    fn mark_index_meta(&mut self, id: &str) {
        if refresh_v2_enabled() {
            self.dirty.mark_index(id);
        }
    }

    /// Consume everything marked so far, immediately (or, with
    /// the gray switch off, the legacy full refresh).
    fn flush_dirty_now(&mut self) {
        if refresh_v2_enabled() {
            self.apply_dirty_inner(true);
        } else {
            self.refresh_all();
        }
    }

    /// The frame-point consumer (design §2.3): called once per
    /// frame from `ui`, after all channel drains, before drawing.
    fn apply_dirty(&mut self) {
        self.apply_dirty_inner(false);
    }

    fn apply_dirty_inner(&mut self, forced: bool) {
        if self.dirty.is_empty() {
            return;
        }
        let perf_t = std::time::Instant::now();
        let perf_before = perf_rest_snapshot();
        let mut slices: Vec<String> = Vec::new();
        let mut consumed_any = false;

        // ---- local group (or its REST-fallback upgrade) ----
        let local_bits = self.dirty.bits & D_LOCAL_BITS;
        let meta_dirty = self.dirty.has(D_INDEX_META);
        if self.local.is_none() {
            // REST fallback (design §5.1): the L slices are
            // REST-fed in this mode, so their dirt upgrades to
            // the four-table REST segment — never the org sweep.
            if local_bits != 0 || meta_dirty {
                let t = std::time::Instant::now();
                self.refresh_tables_rest();
                if meta_dirty {
                    self.reread_index_meta_rest();
                }
                self.dirty.bits &= !(D_LOCAL_BITS | D_INDEX_META);
                self.dirty.index_ids.clear();
                self.dirty.local_consistent = false;
                self.last_local_reread = Some(std::time::Instant::now());
                slices.push(format!("LRest:{}ms", t.elapsed().as_millis()));
                consumed_any = true;
            }
        } else {
            if local_bits != 0 {
                // Lazy-split throttle (design §3.1 point 1):
                // while a split is running, grouped beats arrive
                // every frame; without this the local group would
                // be re-read per frame instead of at most once
                // per 500ms. Forced consumption (completion
                // beats, read-after-write points) never throttles.
                let throttled = !forced
                    && !self.lazy_pendings.is_empty()
                    && self
                        .last_local_reread
                        .map(|t| t.elapsed() < std::time::Duration::from_millis(500))
                        .unwrap_or(false);
                if !throttled {
                    let t = std::time::Instant::now();
                    let touched_wall = local_bits & (D_UNITS | D_INDEXES) != 0;
                    let consumed = if local_bits.count_ones() >= 2
                        || self.dirty.local_consistent
                    {
                        // Consistent snapshot — refresh_opened's
                        // body (design §2.5). On lock contention
                        // it returns false and the bits (plus the
                        // open_refresh_pending retry latch) stay
                        // set: no lost update.
                        self.refresh_opened()
                    } else {
                        if local_bits & D_FOLDERS != 0 {
                            self.reread_folders();
                        }
                        if local_bits & D_GROUPS != 0 {
                            self.reread_groups();
                        }
                        if local_bits & D_UNITS != 0 {
                            self.reread_units();
                        }
                        if local_bits & D_INDEXES != 0 {
                            self.reread_indexes();
                        }
                        true
                    };
                    if consumed {
                        self.dirty.bits &= !D_LOCAL_BITS;
                        self.dirty.local_consistent = false;
                        self.last_local_reread = Some(std::time::Instant::now());
                        if touched_wall {
                            // Design §3.3 point 4: a replaced
                            // units/indexes list must not leave
                            // the wall selection pointing at
                            // vanished ids.
                            self.prune_wall_selection();
                        }
                        slices.push(format!("L:{}ms", t.elapsed().as_millis()));
                        consumed_any = true;
                    }
                }
            }
            if self.dirty.has(D_INDEX_META) {
                let t = std::time::Instant::now();
                let ids = std::mem::take(&mut self.dirty.index_ids);
                if ids.is_empty() {
                    self.reread_index_meta_local(None);
                } else {
                    self.reread_index_meta_local(Some(&ids));
                }
                self.dirty.bits &= !D_INDEX_META;
                slices.push(format!("L5:{}ms", t.elapsed().as_millis()));
                consumed_any = true;
            }
        }

        // ---- org group: O2 first (the wall is the user's
        // focus), then enum order (design §2.3 step 4). A failed
        // GET keeps the old value and still clears the bit — the
        // legacy sweep's semantics; the next related event
        // re-reads.
        let org_order: [(u32, &str, fn(&mut App)); 12] = [
            (D_SHOWROOM_LIVE, "O2", App::reread_showroom_live),
            (D_ORG_TREE, "O1", App::reread_org_tree),
            (D_SHOWROOM_PENDING, "O3", App::reread_showroom_pending),
            (D_REVIEW_QUEUE, "O4", App::reread_review_queue),
            (D_MY_SUBMISSIONS, "O5", App::reread_my_submissions),
            (D_ASSEMBLIES, "O6", App::reread_assemblies),
            (D_INBOX, "O7", App::reread_inbox),
            (D_GROUP_SPACES, "O8", App::reread_group_spaces),
            (D_MY_TEAMS, "O9", App::reread_my_teams),
            (D_LIBRARY, "O10", App::reread_library),
            (D_SESSION_PROFILE, "O11", App::reread_session_profile),
            (D_CACHE_INDEX, "C1", App::reread_cache_index),
        ];
        for (bit, label, reread) in org_order {
            if self.dirty.has(bit) {
                let t = std::time::Instant::now();
                reread(self);
                self.dirty.bits &= !bit;
                slices.push(format!("{label}:{}ms", t.elapsed().as_millis()));
                consumed_any = true;
            }
        }

        // ---- trash (T1 keeps its on-demand shape, design §1.3):
        // reload at once when the trash page is up, else just
        // invalidate the loaded flag for the next entry.
        if self.dirty.has(D_TRASH) {
            self.dirty.bits &= !D_TRASH;
            if self.page == Page::Trash {
                self.trash_load();
            } else {
                self.trash.loaded = false;
            }
            slices.push("T1".to_string());
            consumed_any = true;
        }

        if consumed_any {
            self.default_import_group();
        }
        perf_refresh_summary("dirty", perf_t, perf_before, &slices);
    }

    /// Single-slice local re-reads (design §2.3 step 2): exactly
    /// one list call each — the saving over the four-table sweep
    /// is the point (a Units-only change pays the unit_json
    /// expansion and nothing else).
    fn reread_folders(&mut self) {
        if let Some(local) = self.local.clone() {
            self.folders = local.list_folders();
        }
    }

    fn reread_groups(&mut self) {
        if let Some(local) = self.local.clone() {
            self.groups = local.list_groups();
        }
    }

    fn reread_units(&mut self) {
        if let Some(local) = self.local.clone() {
            self.units = local.list_units();
        }
    }

    fn reread_indexes(&mut self) {
        if let Some(local) = self.local.clone() {
            if let IndexFrame::Overview(overview) = self.index.stack.root_mut() {
                overview.graphs = None;
            }
            self.indexes = local.list_indexes();
        }
    }

    fn group_name(&self, id: &str) -> String {
        self.groups
            .iter()
            .find(|g| g.get("id").and_then(|x| x.as_str()) == Some(id))
            .and_then(|g| g.get("name").and_then(|x| x.as_str()))
            .unwrap_or(id)
            .to_string()
    }

    fn folder_name(&self, id: &str) -> String {
        self.folders
            .iter()
            .find(|f| f.get("id").and_then(|x| x.as_str()) == Some(id))
            .and_then(|f| f.get("name").and_then(|x| x.as_str()))
            .unwrap_or("")
            .to_string()
    }

    fn role(&self) -> String {
        self.user
            .as_ref()
            .and_then(|u| u.get("role"))
            .and_then(|r| r.as_str())
            .unwrap_or("member")
            .to_string()
    }

    fn open_unit_detail(&mut self, unit_id: &str) {
        // Lazy import: an unsplit page is unusable — every entry
        // that would open it (wall, home, search, history…) lands
        // here, so the guard lives here: the click just boosts
        // the page's split instead.
        if let Some((pid, idx)) = self.lazy_unit_page.get(unit_id).cloned() {
            self.lazy_boost(&pid, idx);
            return;
        }
        // A staged import's one-page wrappers are about to be
        // absorbed by grouping (audit N1): opening one now would
        // leave the detail frame pointing at a permanently
        // deleted id, and any edit made in it would be silently
        // discarded. Refuse until the import settles.
        if self.importing.is_some() && self.import_open_units.iter().any(|u| u == unit_id) {
            self.say("该页正在导入成块，完成后再打开");
            return;
        }
        // The wall ring follows the open block, as selected_unit did.
        if let ContentFrame::Wall(w) = self.content.stack.root_mut() {
            w.selected = Some(unit_id.to_string());
        }
        match self.call_work("GET", &format!("/v1/units/{unit_id}"), None) {
            Ok(v) => {
                let mut st = UnitState::new(unit_id, v.clone());
                // default the publish target to the unit's first rendition
                if let Some(first) = v
                    .get("renditions")
                    .and_then(|r| r.as_array())
                    .and_then(|a| a.first())
                    .and_then(|r| r.get("rendition"))
                {
                    st.publish_form = s(first, "form");
                    st.publish_lang = s(first, "lang");
                }
                if let Some(u) = v.get("unit") {
                    let rev = i(u, "current_revision");
                    let ver = if rev > 0 {
                        format!("r{rev}")
                    } else {
                        String::new()
                    };
                    self.note_recent("unit", unit_id, &s(u, "name"), &ver);
                }
                self.load_attachments(unit_id);
                st.bindings = self
                    .call_work("GET", &format!("/v1/units/{unit_id}/page-bindings"), None)
                    .ok()
                    .and_then(|b| b.get("bindings").and_then(|x| x.as_array()).cloned())
                    .unwrap_or_default();
                // Opening replaces the drill-down, as the single
                // detail slot did: reset to the wall, then push.
                self.content.stack.pop_to(0);
                self.content.stack.push(ContentFrame::Unit(st));
            }
            Err(e) => self.msg = format!("打开失败：{e}"),
        }
    }

    fn reload_unit_detail(&mut self, unit_id: &str) {
        if self.rendering_unit.as_deref() == Some(unit_id) {
            self.pending_unit_reload = Some(unit_id.to_string());
            return;
        }
        let same_unit = matches!(self.content.stack.top(), ContentFrame::Unit(st) if st.unit_id == unit_id);
        if !same_unit {
            self.open_unit_detail(unit_id);
            return;
        }
        let detail = match self.call_work("GET", &format!("/v1/units/{unit_id}"), None) {
            Ok(detail) => detail,
            Err(error) => {
                self.msg = format!("刷新失败：{error}");
                return;
            }
        };
        let bindings = match self.call_work(
            "GET",
            &format!("/v1/units/{unit_id}/page-bindings"),
            None,
        ) {
            Ok(value) => value
                .get("bindings")
                .and_then(|x| x.as_array())
                .cloned()
                .unwrap_or_default(),
            Err(error) => {
                self.msg = format!("刷新绑定失败：{error}");
                return;
            }
        };
        if let ContentFrame::Unit(st) = self.content.stack.top_mut() {
            st.detail = detail.clone();
            st.bindings = bindings.clone();
            st.bind_pick = None;
        }
        for overlay in &mut self.overlays {
            if let Overlay::Light(light) = overlay {
                if light.unit_id == unit_id {
                    light.bindings = bindings.clone();
                    light.targets = binding_targets(&detail, &s(&light.version, "id"));
                }
            }
        }
    }

    /// Close the block drill-down (the former detail = None):
    /// pop the content stack back to the wall and clear the wall
    /// selection, exactly what the old field pair did.
    fn close_unit_detail(&mut self) {
        self.content.stack.pop_to(0);
        if let ContentFrame::Wall(w) = self.content.stack.root_mut() {
            w.selected = None;
        }
    }

    fn load_attachments(&mut self, parent_id: &str) {
        if let Ok(v) = self.call_work(
            "GET",
            &format!("/v1/attachments?parent_id={parent_id}"),
            None,
        ) {
            let arr = v
                .get("attachments")
                .and_then(|a| a.as_array())
                .cloned()
                .unwrap_or_default();
            self.att_cache.insert(parent_id.to_string(), arr);
        }
    }

    /// Open the in-client PDF preview for an attachment: download it on a
    /// worker thread, render page 0 with PdfCraft, keep the bytes for flips.
    fn pdf_open(&mut self, att: &Value) {
        let att_id = s(att, "id");
        let filename = s(att, "filename");
        self.pdf_seq += 1;
        let seq = self.pdf_seq;
        self.pdf_view = Some(PdfView {
            att_id: att_id.clone(),
            filename,
            bytes: None,
            page: 0,
            pages: 0,
            tex: None,
            note: "正在加载…".into(),
            seq,
        });
        let (server, token, tx) = (self.server.clone(), self.token.clone(), self.pdf_tx.clone());
        // Local-first P4: an attachment held by the local library
        // is read from its CAS first (the T0 thumbnail pattern);
        // anything else keeps the server download below, unchanged.
        let local = self.local.clone();
        std::thread::spawn(move || {
            let out = match local
                .as_deref()
                .and_then(|store| store.attachment_bytes(&att_id))
            {
                Some(bytes) => {
                    let bytes = std::sync::Arc::new(bytes);
                    render_pdf_page(&bytes, 0).map(|img| (bytes, img))
                }
                None => raw_call(
                    &server,
                    token.as_deref(),
                    "GET",
                    &format!("/v1/attachments/{att_id}/download"),
                    None,
                    false,
                )
                .and_then(|v| {
                    v.get("data_base64")
                        .and_then(|x| x.as_str())
                        .map(str::to_string)
                        .ok_or_else(|| "附件下载响应异常".to_string())
                })
                .and_then(|b| b64d(&b))
                .map(std::sync::Arc::new)
                .and_then(|bytes| render_pdf_page(&bytes, 0).map(|img| (bytes, img))),
            };
            match out {
                Ok((bytes, img)) => {
                    let _ = tx.send((att_id, seq, 0, Some(bytes), Ok(img)));
                }
                Err(e) => {
                    let _ = tx.send((att_id, seq, 0, None, Err(e)));
                }
            }
        });
    }

    fn pdf_goto(&mut self, page: usize) {
        let Some(v) = &self.pdf_view else { return };
        let Some(bytes) = v.bytes.clone() else { return };
        let att_id = v.att_id.clone();
        // Every flip bumps the request generation (audit M3):
        // renders complete out of order, and only the latest
        // request's result may land on the preview.
        self.pdf_seq += 1;
        let seq = self.pdf_seq;
        if let Some(vv) = &mut self.pdf_view {
            vv.note = "正在加载…".into();
            vv.seq = seq;
        }
        let tx = self.pdf_tx.clone();
        std::thread::spawn(move || {
            let res = render_pdf_page(&bytes, page);
            let _ = tx.send((att_id, seq, page, None, res));
        });
    }

    fn pdf_preview_ui(&mut self, ui: &mut egui::Ui) {
        let Some(v) = &self.pdf_view else { return };
        let (note, tex, page, pages) = (v.note.clone(), v.tex.clone(), v.page, v.pages);
        if !note.is_empty() {
            ui.label(egui::RichText::new(&note).color(palette_color(TEXT_WEAK)));
        }
        if let Some(tex) = &tex {
            let size = tex.size_vec2();
            let w = 620.0f32.min(size.x);
            let h = if size.x > 0.0 {
                size.y * (w / size.x)
            } else {
                0.0
            };
            ui.image((tex.id(), egui::vec2(w, h)));
        }
        let mut delta: Option<isize> = None;
        let mut close = false;
        ui.horizontal(|ui| {
            if ui
                .add_enabled(page > 0, egui::Button::new("‹ 上一页").small())
                .clicked()
            {
                delta = Some(-1);
            }
            ui.label(
                egui::RichText::new(format!("第 {} / {} 页", page + 1, pages.max(1)))
                    .color(palette_color(TEXT_WEAK)),
            );
            if ui
                .add_enabled(page + 1 < pages, egui::Button::new("下一页 ›").small())
                .clicked()
            {
                delta = Some(1);
            }
            if ui.small_button("关闭预览").clicked() {
                close = true;
            }
        });
        if close {
            self.pdf_view = None;
            return;
        }
        if let Some(d) = delta {
            let target = (page as isize + d).max(0) as usize;
            self.pdf_goto(target);
        }
    }

    /// Attachment section shared by the index detail and the unit detail:
    /// list + add + download for any file; PDF files additionally get the
    /// in-client PdfCraft preview.
    fn attachments_ui(&mut self, ui: &mut egui::Ui, parent_type: &str, parent_id: &str) {
        ui.add_space(10.0);
        ui.horizontal(|ui| {
            ui.label(
                egui::RichText::new("附件")
                    .strong()
                    .color(palette_color(TEXT_MAIN)),
            );
            if ui.small_button("＋ 添加附件").clicked() {
                if let Some(path) = rfd::FileDialog::new().pick_file() {
                    let filename = path
                        .file_name()
                        .map(|n| n.to_string_lossy().to_string())
                        .unwrap_or_default();
                    match std::fs::read(&path) {
                        Ok(bytes) => {
                            let body = json!({
                                "parent_type": parent_type,
                                "parent_id": parent_id,
                                "filename": filename,
                                "data_base64": b64e(&bytes),
                            });
                            match self.call_work("POST", "/v1/attachments", Some(body)) {
                                Ok(_) => {
                                    self.msg = "附件已添加".into();
                                    self.load_attachments(parent_id);
                                }
                                Err(e) => self.msg = format!("添加附件失败：{e}"),
                            }
                        }
                        Err(e) => self.msg = format!("读取文件失败：{e}"),
                    }
                }
            }
        });
        let atts = self.att_cache.get(parent_id).cloned().unwrap_or_default();
        for att in &atts {
            let filename = s(att, "filename");
            let is_pdf = filename.to_lowercase().ends_with(".pdf");
            let size = att.get("size").and_then(|x| x.as_i64()).unwrap_or(0);
            let size_txt = if size >= 1024 * 1024 {
                format!("{:.1} MB", size as f64 / 1048576.0)
            } else {
                format!("{:.0} KB", size as f64 / 1024.0)
            };
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new(&filename).color(palette_color(TEXT_MAIN)));
                ui.label(
                    egui::RichText::new(format!("{size_txt} · {}", s(att, "uploader")))
                        .color(palette_color(TEXT_FAINT)),
                );
                if is_pdf && ui.small_button("预览").clicked() {
                    self.pdf_open(att);
                }
                if ui.small_button("下载").clicked() {
                    let att_id = s(att, "id");
                    match self.call_work(
                        "GET",
                        &format!("/v1/attachments/{att_id}/download"),
                        None,
                    ) {
                        Ok(v) => {
                            let b64 = v
                                .get("data_base64")
                                .and_then(|x| x.as_str())
                                .unwrap_or_default()
                                .to_string();
                            match b64d(&b64) {
                                Ok(bytes) => {
                                    if let Some(path) =
                                        rfd::FileDialog::new().set_file_name(&filename).save_file()
                                    {
                                        match craft_atomic::atomic_write(&path, &bytes) {
                                            Ok(()) => self.msg = "附件已保存".into(),
                                            Err(e) => self.msg = format!("保存失败：{e}"),
                                        }
                                    }
                                }
                                Err(e) => self.msg = format!("下载失败：{e}"),
                            }
                        }
                        Err(e) => self.msg = format!("下载失败：{e}"),
                    }
                }
            });
            if self.pdf_view.as_ref().map(|v| v.att_id.as_str()) == Some(s(att, "id").as_str()) {
                self.pdf_preview_ui(ui);
            }
        }
    }

    fn open_index_detail(&mut self, index_id: &str) {
        // The branch carries over from the previously open index,
        // as it did when branch was a page-level field.
        let branch = match self.index.stack.top() {
            IndexFrame::Detail(d) => d.branch.clone(),
            _ => String::new(),
        };
        self.open_index_detail_as(index_id, branch);
    }

    /// Open from the overview wall goes through here with branch
    /// "main": the old overview double-click reset the branch
    /// selector before opening.
    fn open_index_detail_as(&mut self, index_id: &str, branch: String) {
        // Load first; only drill in on success (the old behavior on
        // a failed load was to stay on the overview).
        let mut st = IndexDetailState::new(index_id, branch);
        let b = if st.branch.is_empty() {
            "main".to_string()
        } else {
            st.branch.clone()
        };
        match self.call_work(
            "GET",
            &format!("/v1/indexes/{index_id}?branch={}", query_component(&b)),
            None,
        ) {
            Ok(v) => st.detail = v,
            Err(e) => {
                self.msg = format!("打开索引失败：{e}");
                return;
            }
        }
        let idx = st.detail.get("index").cloned().unwrap_or(json!({}));
        self.remember_index_tags(index_id, &st.detail);
        self.index.stack.pop_to(0);
        self.index.stack.push(IndexFrame::Detail(st));
        self.load_index_related(index_id);
        self.note_recent("index", index_id, &s(&idx, "name"), "");
    }

    fn remember_index_tags(&mut self, id: &str, detail: &Value) {
        let tags = detail["tags"].as_array().map(Vec::as_slice).unwrap_or(&[]);
        self.index_search_tags
            .insert(id.into(), tags.iter().map(|tag| s(tag, "name")).collect());
        let latest = tags
            .iter()
            .filter(|tag| tag["branch"] == "main")
            .filter_map(|tag| tag["name"].as_str())
            .max_by_key(|name| semver_key(name));
        if let Some(name) = latest {
            self.index_tags.insert(id.into(), name.into());
        } else {
            self.index_tags.remove(id);
        }
    }

    fn load_index_detail(&mut self) {
        let (id, branch) = match self.index.stack.top() {
            IndexFrame::Detail(d) => (
                d.index_id.clone(),
                if d.branch.is_empty() {
                    "main".to_string()
                } else {
                    d.branch.clone()
                },
            ),
            _ => return,
        };
        match self.call_work(
            "GET",
            &format!("/v1/indexes/{id}?branch={}", query_component(&branch)),
            None,
        ) {
            Ok(v) => {
                if let IndexFrame::Overview(overview) = self.index.stack.root_mut() {
                    overview.graphs = None;
                }
                self.remember_index_tags(&id, &v);
                if let IndexFrame::Detail(d) = self.index.stack.top_mut() {
                    d.detail = v;
                }
            }
            Err(e) => self.msg = format!("打开索引失败：{e}"),
        }
        self.load_index_related(&id);
    }

    fn change_index_branch(&mut self, branch: String) {
        let IndexFrame::Detail(st) = self.index.stack.top() else {
            return;
        };
        let id = st.index_id.clone();
        match self.call_work(
            "GET",
            &format!("/v1/indexes/{id}?branch={}", query_component(&branch)),
            None,
        ) {
            Ok(detail) => {
                if let IndexFrame::Detail(st) = self.index.stack.top_mut() {
                    st.branch = branch;
                    st.detail = detail;
                    st.selected_commit = None;
                    st.merge_preview = None;
                    st.merge_resolutions.clear();
                }
                self.load_index_related(&id);
            }
            Err(e) => self.msg = format!("切换分支失败：{e}"),
        }
    }

    /// Attachments, per-rendition version prefetch, commits and
    /// exports for the open index (split out of load_index_detail
    /// so open_index_detail can load the detail before drilling in).
    fn load_index_related(&mut self, id: &str) {
        let branch = match self.index.stack.top() {
            IndexFrame::Detail(d) if d.branch.is_empty() => "main".to_string(),
            IndexFrame::Detail(d) => d.branch.clone(),
            _ => "main".to_string(),
        };
        self.load_attachments(id);
        // prefetch version history per rendition for the quick-switch strips
        let detail = match self.index.stack.top() {
            IndexFrame::Detail(d) => d.detail.clone(),
            _ => Value::Null,
        };
        let mut seen_units: Vec<String> = Vec::new();
        if let Some(arr) = detail.get("resolved_entries").and_then(|r| r.as_array()) {
            for re in arr {
                if let Some(e) = re.get("entry") {
                    let uid = s(e, "unit_id");
                    if !uid.is_empty() && !seen_units.contains(&uid) {
                        seen_units.push(uid);
                    }
                }
            }
        }
        let mut unit_versions: std::collections::HashMap<String, Vec<Value>> = Default::default();
        for uid in seen_units {
            if let Ok(u) = self.call_work("GET", &format!("/v1/units/{uid}"), None) {
                if let Some(rends) = u.get("renditions").and_then(|r| r.as_array()) {
                    for r in rends {
                        let rid = r.get("rendition").map(|x| s(x, "id")).unwrap_or_default();
                        let mut vs: Vec<Value> = r
                            .get("versions")
                            .and_then(|v| v.as_array())
                            .cloned()
                            .unwrap_or_default()
                            .into_iter()
                            .filter(|v| {
                                let st = s(v, "status");
                                st.is_empty() || st == "normal"
                            })
                            .collect();
                        vs.sort_by(|a, b| {
                            semver_key(&s(b, "semver")).cmp(&semver_key(&s(a, "semver")))
                        });
                        unit_versions.insert(rid, vs);
                    }
                }
            }
        }
        if let IndexFrame::Detail(d) = self.index.stack.top_mut() {
            d.unit_versions = unit_versions;
        }
        let commits = self
            .call_work(
                "GET",
                &format!(
                    "/v1/indexes/{id}/commits?branch={}",
                    query_component(&branch)
                ),
                None,
            )
            .ok();
        let exports = self
            .call_work("GET", &format!("/v1/exports?index_id={id}"), None)
            .ok();
        if let IndexFrame::Detail(d) = self.index.stack.top_mut() {
            d.commits = commits;
            d.exports = exports;
            d.lineage = None;
        }
    }

    fn commit_entries(&mut self, entries: Vec<Value>, message: &str) {
        let (id, branch) = match self.index.stack.top() {
            IndexFrame::Detail(d) => (
                d.index_id.clone(),
                if d.branch.is_empty() {
                    "main".to_string()
                } else {
                    d.branch.clone()
                },
            ),
            _ => return,
        };
        let r = self.call_work(
            "POST",
            &format!("/v1/indexes/{id}/commits"),
            Some(json!({ "branch": branch, "message": message, "entries": entries })),
        );
        match r {
            Ok(_) => {
                self.msg = "已提交".into();
                self.load_index_detail();
            }
            Err(e) => self.msg = format!("提交失败：{e}"),
        }
    }

    fn current_entries(&self) -> Vec<Value> {
        let detail = match self.index.stack.top() {
            IndexFrame::Detail(d) => &d.detail,
            _ => return Vec::new(),
        };
        detail
            .get("resolved_entries")
            .and_then(|r| r.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|re| re.get("entry").cloned())
                    .collect()
            })
            .unwrap_or_default()
    }

    fn current_index_name(&self) -> String {
        let id = match self.index.stack.top() {
            IndexFrame::Detail(d) => d.index_id.clone(),
            _ => String::new(),
        };
        self.indexes
            .iter()
            .find(|x| s(x, "id") == id)
            .map(|x| s(x, "name"))
            .unwrap_or_default()
    }

    /// 选页 on an index entry: jump to the content page in pick mode,
    /// with the entry's block opened (§3.10 scenario 44). The
    /// entry's version policy travels into the opened detail
    /// (audit R1): the #101 header's policy segment describes
    /// THIS entry's hold on the block, so entering through a
    /// different entry of the same block re-reads that entry's
    /// policy — the states never bleed into each other.
    fn start_pick_pages(&mut self, pos: usize, re: &Value, sel: Vec<bool>) {
        let entry = re.get("entry").cloned().unwrap_or(json!({}));
        let resolved = re.get("resolved").cloned().unwrap_or(Value::Null);
        let policy = entry_policy_view(&entry, &resolved);
        let unit_id = s(&entry, "unit_id");
        self.pick = Some(PickState::Pages {
            index_name: self.current_index_name(),
            entry_pos: pos,
            unit_id: unit_id.clone(),
            unit_name: re
                .get("unit_name")
                .and_then(|x| x.as_str())
                .unwrap_or("")
                .to_string(),
            version_id: s(&resolved, "id"),
            sel,
        });
        if let Some(item) = self
            .units
            .iter()
            .find(|u| s(&u.get("unit").cloned().unwrap_or(json!({})), "id") == unit_id)
        {
            let unit = item.get("unit").cloned().unwrap_or(json!({}));
            let fid = s(&unit, "folder_id");
            self.content.folder_filter = if fid.is_empty() { None } else { Some(fid) };
        }
        self.page = Page::Content;
        self.open_unit_detail(&unit_id);
        // Stamp the entry policy onto the freshly opened frame
        // (open_unit_detail may have refused — a lazy wrapper or
        // a mid-import page — in which case there is no frame to
        // stamp and nothing to show).
        if let ContentFrame::Unit(st) = self.content.stack.top_mut() {
            if st.unit_id == unit_id {
                st.entry_policy = Some(policy);
            }
        }
    }

    /// The context bar shown on the content page while a pick mode
    /// (pages / insert) is active.
    fn pick_bar(&mut self, ui: &mut egui::Ui) {
        let Some(pick) = self.pick.clone() else {
            return;
        };
        let mut confirm = false;
        let mut cancel = false;
        egui::Frame::NONE
            .fill(palette_color(CHIP_BLUE_BG))
            .corner_radius(8)
            .inner_margin(egui::Margin::symmetric(14, 8))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    match &pick {
                        PickState::Pages { index_name, unit_name, .. } => {
                            ui.label(
                                egui::RichText::new(format!(
                                    "为索引「{index_name}」选页 · {unit_name}"
                                ))
                                .strong()
                                .size(13.0),
                            );
                        }
                        PickState::Insert { index_name, at } => {
                            ui.label(
                                egui::RichText::new(format!(
                                    "为索引「{index_name}」插入条目 · 插入到第 {at} 条之后，点内容块的「插入」即生效"
                                ))
                                .strong()
                                .size(13.0),
                            );
                        }
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.button("取消").clicked() {
                            cancel = true;
                        }
                        if matches!(pick, PickState::Pages { .. })
                            && ui
                                .add_enabled(
                                    matches!(
                                        &pick,
                                        PickState::Pages { sel, .. }
                                            if sel.iter().any(|b| *b)
                                    ),
                                    egui::Button::new(
                                        egui::RichText::new("确认选页")
                                            .color(egui::Color32::WHITE),
                                    )
                                    .fill(ACCENT)
                                    .corner_radius(6),
                                )
                                .clicked()
                        {
                            confirm = true;
                        }
                    });
                });
            });
        ui.add_space(10.0);
        if cancel {
            self.cancel_pick();
        }
        if confirm {
            self.confirm_pick_pages();
        }
    }

    /// Page entity ids of a version, in reference order, from the
    /// data the client already holds: the open unit detail's
    /// version lists first, then the index detail's. Empty when
    /// the version is not loaded anywhere.
    fn version_page_ids(&self, version_id: &str) -> Vec<String> {
        fn ids_of(versions: &[Value], version_id: &str) -> Option<Vec<String>> {
            let v = versions.iter().find(|v| s(v, "id") == version_id)?;
            let pages = v.get("pages")?.as_array()?;
            Some(
                pages
                    .iter()
                    .map(|p| p.get("id").and_then(|x| x.as_str()).unwrap_or("").to_string())
                    .collect(),
            )
        }
        if let ContentFrame::Unit(st) = self.content.stack.top() {
            if let Some(rends) = st.detail.get("renditions").and_then(|r| r.as_array()) {
                for r in rends {
                    if let Some(vs) = r.get("versions").and_then(|v| v.as_array()) {
                        if let Some(ids) = ids_of(vs, version_id) {
                            return ids;
                        }
                    }
                }
            }
        }
        if let IndexFrame::Detail(d) = self.index.stack.top() {
            for vs in d.unit_versions.values() {
                if let Some(ids) = ids_of(vs, version_id) {
                    return ids;
                }
            }
        }
        Vec::new()
    }

    fn cancel_pick(&mut self) {
        self.pick = None;
        self.close_unit_detail();
        self.page = Page::Index;
    }

    fn confirm_pick_pages(&mut self) {
        let Some(PickState::Pages {
            entry_pos,
            sel,
            version_id,
            ..
        }) = self.pick.clone()
        else {
            return;
        };
        let chosen: Vec<usize> = sel
            .iter()
            .enumerate()
            .filter(|(_, b)| **b)
            .map(|(n, _)| n)
            .collect();
        if chosen.is_empty() {
            self.msg = "至少选一页".into();
            return;
        }
        let mut es = self.current_entries();
        if entry_pos >= es.len() {
            self.msg = "条目已变化，请重新选页".into();
            self.pick = None;
            return;
        }
        if chosen.len() == sel.len() {
            es[entry_pos]["excerpt_slots"] = Value::Null;
        } else {
            // Excerpts address pages by entity id (the version's
            // reference sequence), so a later insert/delete in the
            // block can no longer shift what the excerpt means.
            let page_ids = self.version_page_ids(&version_id);
            if page_ids.len() == sel.len() && page_ids.iter().all(|id| !id.is_empty()) {
                es[entry_pos]["excerpt_slots"] = json!(chosen
                    .iter()
                    .map(|n| page_ids[*n].clone())
                    .collect::<Vec<_>>());
            } else {
                es[entry_pos]["excerpt_slots"] =
                    json!(chosen.iter().map(|n| n.to_string()).collect::<Vec<_>>());
            }
        }
        self.pick = None;
        self.close_unit_detail();
        self.page = Page::Index;
        self.commit_entries(es, "摘录选页");
    }

    /// Insert-mode pick: the clicked block joins the index at the
    /// recorded position with its recommended rendition (first one with
    /// a latest version), following latest.
    fn insert_pick_unit(&mut self, item: &Value) {
        let Some(PickState::Insert { at, .. }) = self.pick.clone() else {
            return;
        };
        let unit = item.get("unit").cloned().unwrap_or(json!({}));
        let uid = s(&unit, "id");
        // Lazy import: an unsplit page can never enter an index
        // (the card hides its insert button too; this is the
        // belt-and-braces guard for the same rule).
        if let Some((pid, idx)) = self.lazy_unit_page.get(&uid).cloned() {
            self.lazy_boost(&pid, idx);
            return;
        }
        let rid = item
            .get("renditions")
            .and_then(|r| r.as_array())
            .and_then(|arr| {
                arr.iter()
                    .find(|r| r.get("latest").map(|l| l.is_object()).unwrap_or(false))
                    .map(|r| s(&r.get("rendition").cloned().unwrap_or(json!({})), "id"))
            })
            .unwrap_or_default();
        if rid.is_empty() {
            self.msg = "该内容块暂无可用版本".into();
            return;
        }
        let mut es = self.current_entries();
        let at = at.min(es.len());
        es.insert(
            at,
            json!({
                "unit_id": uid,
                "rendition_id": rid,
                "policy": "follow_latest",
                "locked_semver": Value::Null,
                "excerpt_slots": Value::Null,
            }),
        );
        self.pick = None;
        self.page = Page::Index;
        self.commit_entries(es, "插入条目");
    }

    /// Grid-end add card on the content page: blank block, or a block
    /// whose first version is pasted in from PowerPoint.
    fn content_add_card(&mut self, ui: &mut egui::Ui, card_w: f32, card_h: f32) {
        let (rect, resp) = ui.allocate_exact_size(egui::vec2(card_w, card_h), egui::Sense::click());
        ui.painter().rect_stroke(
            rect,
            6.0,
            egui::Stroke::new(1.0, palette_color(CARD_BORDER)),
            egui::StrokeKind::Middle,
        );
        let mut blank = resp.clicked();
        let mut paste = false;
        ui.scope_builder(egui::UiBuilder::new().max_rect(rect), |ui| {
            ui.vertical_centered(|ui| {
                ui.add_space(64.0);
                ui.label(
                    egui::RichText::new("+")
                        .size(34.0)
                        .color(palette_color(TEXT_FAINT)),
                );
                ui.label(
                    egui::RichText::new("新建内容块")
                        .size(13.0)
                        .color(palette_color(TEXT_WEAK)),
                );
                ui.add_space(14.0);
                ui.horizontal(|ui| {
                    let w = ui.available_width();
                    ui.add_space((w - 196.0).max(0.0) / 2.0);
                    if ui
                        .add_sized(
                            [92.0, 28.0],
                            egui::Button::new(egui::RichText::new("新建空白块").size(11.5))
                                .fill(palette_color(BTN_BG))
                                .corner_radius(6),
                        )
                        .clicked()
                    {
                        blank = true;
                    }
                    if ui
                        .add_sized(
                            [104.0, 28.0],
                            egui::Button::new(egui::RichText::new("粘贴 PPT 页新建").size(11.5))
                                .fill(palette_color(BTN_BG))
                                .corner_radius(6),
                        )
                        .clicked()
                    {
                        paste = true;
                    }
                });
            });
        });
        if paste {
            self.create_unit_from_paste();
        } else if blank {
            self.create_blank_unit();
        }
    }

    fn create_blank_unit(&mut self) {
        if !self.need_identity_local("新建内容块") {
            return;
        }
        let r = self.call_work(
            "POST",
            "/v1/units",
            Some(json!({
                "name": "未命名内容块",
                "group_id": self.content.import_group,
                "folder_id": self.content.folder_filter,
            })),
        );
        match r {
            Ok(v) => {
                let id = if !s(&v, "id").is_empty() {
                    s(&v, "id")
                } else {
                    s(&v.get("unit").cloned().unwrap_or(json!({})), "id")
                };
                // Phase-1 (design §3.1 point 7): read-after-
                // write — the detail open below navigates from
                // the refreshed list, so consume at once.
                self.changed_now(Change::Units);
                if !id.is_empty() {
                    self.open_unit_detail(&id);
                }
            }
            Err(e) => self.msg = format!("新建失败：{e}"),
        }
    }

    /// Paste path: capture the slides copied in PowerPoint through the
    /// bridge, create a block and publish the fragment as its first
    /// version. QIMU_PASTE_FRAGMENT points at a prepared fragment file
    /// and stands in for the bridge capture in headless verification.
    fn create_unit_from_paste(&mut self) {
        if !self.need_identity_local("粘贴新建") {
            return;
        }
        let mut from_bridge = false;
        let frag: Option<std::path::PathBuf> = std::env::var("QIMU_PASTE_FRAGMENT")
            .ok()
            .map(std::path::PathBuf::from)
            .filter(|p| p.exists())
            .or_else(
                || match qimu_bridge::platform_bridge().capture_copied_slides() {
                    Ok(p) => {
                        from_bridge = p.is_some();
                        p
                    }
                    Err(e) => {
                        self.msg = format!("无法读取 PowerPoint 剪贴板：{e}");
                        None
                    }
                },
            );
        let Some(path) = frag else {
            self.msg = "先在 PowerPoint 里复制页面，再点「粘贴 PPT 页新建」".into();
            return;
        };
        let read = std::fs::read(&path);
        if from_bridge {
            // Audit2 E-L2: bridge captures are one-shot temps (the
            // QIMU_PASTE_FRAGMENT fixture file is the caller's own
            // and is never deleted).
            let _ = std::fs::remove_file(&path);
        }
        let Ok(bytes) = read else {
            self.msg = "读取粘贴片段失败".into();
            return;
        };
        if let Some(uid) = self.publish_bytes_as_unit(&bytes, "未命名内容块", "剪贴板粘贴新建")
        {
            self.msg = "已粘贴新建内容块".into();
            // Phase-1 (design §3.1 point 8): same read-after-
            // write shape as point 7.
            self.changed_now(Change::Units);
            self.open_unit_detail(&uid);
        }
    }

    /// Create a content block from raw PPTX bytes and publish them
    /// as its first version. Shared by 粘贴新建 and every clipboard
    /// paste path. Returns the new unit id; failures land in
    /// `self.msg`.
    fn publish_bytes_as_unit(&mut self, bytes: &[u8], name: &str, note: &str) -> Option<String> {
        let folder = self.content.folder_filter.clone();
        self.publish_bytes_as_unit_to(bytes, name, note, folder)
    }

    /// publish_bytes_as_unit with an explicit home folder
    /// (None = the library root): 解除组合 lands the split
    /// single-page blocks in the original block's own folder,
    /// not in whatever folder the wall happens to target.
    fn publish_bytes_as_unit_to(
        &mut self,
        bytes: &[u8],
        name: &str,
        note: &str,
        folder_id: Option<String>,
    ) -> Option<String> {
        let created = self.call_work(
            "POST",
            "/v1/units",
            Some(json!({
                "name": name,
                "group_id": self.content.import_group,
                "folder_id": folder_id,
            })),
        );
        let uid = match created {
            Ok(v) => {
                if !s(&v, "id").is_empty() {
                    s(&v, "id")
                } else {
                    s(&v.get("unit").cloned().unwrap_or(json!({})), "id")
                }
            }
            Err(e) => {
                self.msg = format!("新建失败：{e}");
                return None;
            }
        };
        if uid.is_empty() {
            self.msg = "新建失败：服务器未返回内容块".into();
            return None;
        }
        match self.call_work(
            "POST",
            &format!("/v1/units/{uid}/publish"),
            Some(json!({
                "data_base64": b64e(bytes),
                "form": "标准",
                "lang": "zh",
                "bump": "minor",
                "note": note,
            })),
        ) {
            Ok(_) => Some(uid),
            Err(e) => {
                self.msg = format!("发布失败：{e}");
                None
            }
        }
    }

    // ---------------- PPT clipboard (PPT 剪贴板) ----------------
    //
    // The clipboard holds ONE fragment .pptx on disk plus a JSON
    // sidecar. PowerPoint interop goes through the bridge both ways:
    // capture (already shipped) reads slides PowerPoint put on the
    // system clipboard; publish (qimu-bridge) lets PowerPoint itself
    // render the fragment onto the system clipboard, because its
    // slide format is a private serialization. Copy/cut always fill
    // the in-app clipboard; the PowerPoint leg is best-effort and
    // the status message says honestly whether it happened.

    /// The bottom bar's PPT clipboard chip text; None = chip hidden.
    fn clip_badge(&self) -> Option<String> {
        self.ppt_clip
            .as_ref()
            .map(|m| format!("PPT 剪贴板 · {} 页", i(m, "pages")))
    }

    /// Store fragment bytes as the clipboard content. When
    /// `offer_to_powerpoint` is set, also publish them to the system
    /// clipboard through the bridge and report the outcome honestly.
    /// Store the in-app clipboard fragment + its sidecar.
    /// Returns true only when BOTH files actually landed —
    /// audit2 E-H1: cut operations used to proceed (trashing /
    /// re-versioning the source) without checking this, judging
    /// success by the stale `ppt_clip` of a previous copy; a failed
    /// store then destroyed the source while the clipboard held
    /// old content. Both writes are atomic (E-L1: the fragment
    /// used to be overwritten in place, the sidecar's result
    /// swallowed).
    fn clip_store(&mut self, bytes: &[u8], source: &str, offer_to_powerpoint: bool) -> bool {
        let pkg = match qimu_core::pptx::Package::load(bytes) {
            Ok(p) => p,
            Err(e) => {
                self.say(format!("复制失败：不是有效的 PPTX 片段（{e}）"));
                return false;
            }
        };
        let pages = qimu_core::pptx::slides(&pkg).map(|v| v.len()).unwrap_or(0);
        if pages == 0 {
            self.say("复制失败：片段里没有页面");
            return false;
        }
        if craft_atomic::atomic_write(&clip_fragment_path(), bytes).is_err() {
            self.say("复制失败：本地剪贴板写入失败");
            return false;
        }
        let meta = json!({
            "pages": pages,
            "source": source,
            "created_ms": qimu_core::auth::now_ms(),
        });
        if craft_atomic::atomic_write(
            &clip_meta_path(),
            serde_json::to_string(&meta).unwrap_or_default().as_bytes(),
        )
        .is_err()
        {
            // Fragment and meta must stay a pair (audit L1):
            // drop the fragment just written and clear the badge
            // state rather than leave a clipboard whose badge
            // (meta) and content (fragment) disagree.
            let _ = std::fs::remove_file(clip_fragment_path());
            self.ppt_clip = None;
            self.say("复制失败：本地剪贴板元信息写入失败");
            return false;
        }
        self.ppt_clip = Some(meta);
        if !offer_to_powerpoint {
            self.say(format!("已复制 {pages} 页到 PPT 剪贴板"));
            return true;
        }
        match qimu_bridge::platform_bridge().publish_slides_to_clipboard(&clip_fragment_path()) {
            Ok(true) => {
                self.say(format!("已复制 {pages} 页到 PPT 剪贴板（可直接粘贴到 PowerPoint）"))
            }
            Ok(false) => {
                self.say(format!(
                    "已复制 {pages} 页到 PPT 剪贴板（PowerPoint 未运行，未同步到系统剪贴板）"
                ))
            }
            Err(e) => {
                self.say(format!("已复制 {pages} 页到 PPT 剪贴板（同步到 PowerPoint 失败：{e}）"))
            }
        }
        true
    }

    /// The clipboard bytes, adopting whatever PowerPoint last put on
    /// the system clipboard when the in-app clipboard is empty (the
    /// same capture path 粘贴新建 uses).
    fn clip_source_bytes(&mut self) -> Option<Vec<u8>> {
        if self.ppt_clip.is_some() {
            // The badge says the clipboard holds pages; if the
            // fragment behind it cannot be read, say exactly that
            // (audit L1) instead of falling through to the
            // PowerPoint capture and ending at "剪贴板是空的" —
            // a message that contradicts the badge.
            match std::fs::read(clip_fragment_path()) {
                Ok(bytes) => return Some(bytes),
                Err(e) => {
                    self.say(format!("PPT 剪贴板内容读取失败（{e}），请重新复制"));
                    return None;
                }
            }
        }
        match qimu_bridge::platform_bridge().capture_copied_slides() {
            Ok(Some(path)) => {
                let read = std::fs::read(&path);
                // Audit2 E-L2: the capture file is a one-shot temp
                // in the OS temp dir; it used to stay there forever.
                let _ = std::fs::remove_file(&path);
                match read {
                    Ok(bytes) => {
                        self.clip_store(&bytes, "PowerPoint", false);
                        Some(bytes)
                    }
                    Err(e) => {
                        self.msg = format!("读取 PowerPoint 剪贴板失败：{e}");
                        None
                    }
                }
            }
            Ok(None) => {
                self.msg = "PPT 剪贴板是空的".into();
                None
            }
            Err(e) => {
                self.msg = format!("读取 PowerPoint 剪贴板失败：{e}");
                None
            }
        }
    }

    fn clip_clear(&mut self) {
        let _ = std::fs::remove_file(clip_fragment_path());
        let _ = std::fs::remove_file(clip_meta_path());
        self.ppt_clip = None;
        self.msg = "已清空 PPT 剪贴板".into();
    }

    /// 粘贴：逐页成块 (each clipboard page becomes its own block) or
    /// 整组成块 (all pages become one multi-page block).
    fn clip_paste(&mut self, split_into_blocks: bool) {
        if !self.need_identity_local("粘贴") {
            return;
        }
        let Some(bytes) = self.clip_source_bytes() else {
            return;
        };
        let source = self
            .ppt_clip
            .as_ref()
            .map(|m| s(m, "source"))
            .unwrap_or_default();
        if split_into_blocks {
            let pkg = match qimu_core::pptx::Package::load(&bytes) {
                Ok(p) => p,
                Err(e) => {
                    self.msg = format!("粘贴失败：{e}");
                    return;
                }
            };
            let singles = match qimu_core::pptx::split(&pkg) {
                Ok(v) => v,
                Err(e) => {
                    self.msg = format!("粘贴失败：{e}");
                    return;
                }
            };
            // Report the same {made}/{total} accounting as
            // clip_ungroup_unit (audit M4): a page that fails to
            // serialize or publish is counted and named in the
            // summary, never silently skipped.
            let total = singles.len();
            let mut made = 0usize;
            let mut failed = 0usize;
            for (idx, one) in singles.iter().enumerate() {
                let name = clip_page_name(one, idx);
                let Ok(b) = one.to_bytes() else {
                    failed += 1;
                    continue;
                };
                if self
                    .publish_bytes_as_unit(&b, &name, "剪贴板逐页成块")
                    .is_some()
                {
                    made += 1;
                } else {
                    failed += 1;
                }
            }
            if failed == 0 {
                self.say(format!("已逐页成块：新建 {made} 个内容块"));
            } else {
                self.say(format!(
                    "逐页成块未全部完成：新建 {made}/{total} 个内容块，{failed} 页失败"
                ));
            }
        } else {
            let name = if source.is_empty() {
                "剪贴板内容块".to_string()
            } else {
                source.clone()
            };
            if self
                .publish_bytes_as_unit(&bytes, &name, "剪贴板整组成块")
                .is_some()
            {
                self.msg = "已整组成块：新建 1 个内容块".into();
            }
        }
        // Phase-1 (design §3.1 point 9): both paste shapes only
        // ever add units.
        self.changed(Change::Units);
    }

    /// 解除组合：split one multi-page block back into one
    /// single-page block per page — the inverse of 组合为内容块.
    /// The split/publish machinery is clip_paste's （逐页成块）:
    /// pptx::split into single-page packages, each published as
    /// its own block, named like a per-page paste, but homed in
    /// the ORIGINAL block's folder. The original goes to the
    /// recycle bin (trash_add, the same path cut uses) only
    /// after every page landed as a new block — a partial
    /// failure keeps the original untouched.
    /// Returns true only when the split fully landed AND the
    /// original reached the recycle bin (audit N3): the deck-bar
    /// handler clears the selection on success only.
    fn clip_ungroup_unit(&mut self, uid: &str) -> bool {
        if !self.need_identity_local("解除组合") {
            return false;
        }
        let name = self.unit_name(uid);
        let Some(vid) = self.unit_latest_version_id(uid) else {
            self.say(format!("解除组合失败：「{name}」没有可用的版本"));
            return false;
        };
        let frag = match self.version_fragment_bytes(&vid) {
            Ok(b) => b,
            Err(e) => {
                self.say(format!("解除组合失败：{e}"));
                return false;
            }
        };
        let pkg = match qimu_core::pptx::Package::load(&frag) {
            Ok(p) => p,
            Err(e) => {
                self.say(format!("解除组合失败：{e}"));
                return false;
            }
        };
        let singles = match qimu_core::pptx::split(&pkg) {
            Ok(v) => v,
            Err(e) => {
                self.say(format!("解除组合失败：{e}"));
                return false;
            }
        };
        if singles.len() < 2 {
            self.say(format!("「{name}」只有一页，无需解除组合"));
            return false;
        }
        // The original block's home folder (empty = root).
        let folder: Option<String> = self
            .units
            .iter()
            .find_map(|item| {
                let unit = item.get("unit")?;
                if s(unit, "id") == uid {
                    Some(s(unit, "folder_id"))
                } else {
                    None
                }
            })
            .filter(|f| !f.is_empty());
        let total = singles.len();
        let mut made = 0usize;
        for (idx, one) in singles.iter().enumerate() {
            let pname = clip_page_name(one, idx);
            let Ok(b) = one.to_bytes() else { continue };
            if self
                .publish_bytes_as_unit_to(&b, &pname, "解除组合", folder.clone())
                .is_some()
            {
                made += 1;
            }
        }
        // The recycle-bin outcome is part of the report (audit
        // N2): a failed trash_add used to be overwritten by the
        // success wording, telling the user the original was in
        // the bin while it still sat on the wall.
        let mut fully_done = false;
        if made == total {
            if self.trash_add("unit", uid) {
                fully_done = true;
                self.say(format!(
                    "已解除组合「{name}」：拆为 {made} 个单页内容块，原内容块已移入回收站"
                ));
            } else {
                self.say(format!(
                    "已解除组合「{name}」：拆为 {made} 个单页内容块，但原内容块移入回收站失败，仍保留在页面墙上"
                ));
            }
        } else if made > 0 {
            self.say(format!(
                "解除组合未完成：已新建 {made}/{total} 个单页内容块，原内容块「{name}」保留未动"
            ));
        } else {
            self.say(format!(
                "解除组合失败：未能新建任何单页内容块，原内容块「{name}」保留未动"
            ));
        }
        // Phase-1 (design §3.1 point 10): the split blocks are
        // new units; the original's move to the bin already
        // emitted its own events through trash_add.
        self.changed(Change::Units);
        fully_done
    }

    // ---------------- copy / cut sources ----------------

    fn unit_name(&self, uid: &str) -> String {
        for item in &self.units {
            if let Some(unit) = item.get("unit") {
                if s(unit, "id") == uid {
                    return s(unit, "name");
                }
            }
        }
        uid.to_string()
    }

    fn unit_latest_version_id(&self, uid: &str) -> Option<String> {
        for item in &self.units {
            let Some(unit) = item.get("unit") else {
                continue;
            };
            if s(unit, "id") != uid {
                continue;
            }
            let rends = item.get("renditions")?.as_array()?;
            let id = s(rends.first()?.get("latest")?, "id");
            if id.is_empty() {
                return None;
            }
            return Some(id);
        }
        None
    }

    /// Page count of a block's lead rendition's latest version,
    /// from the wall list payload (the same `latest.page_count`
    /// the wall cards print). None when the block or its version
    /// is unknown.
    fn unit_latest_page_count(&self, uid: &str) -> Option<i64> {
        for item in &self.units {
            let Some(unit) = item.get("unit") else {
                continue;
            };
            if s(unit, "id") != uid {
                continue;
            }
            let rends = item.get("renditions")?.as_array()?;
            let latest = rends.first()?.get("latest")?;
            if !latest.is_object() {
                return None;
            }
            return Some(i(latest, "page_count"));
        }
        None
    }

    fn unit_lead_rendition(&self, uid: &str) -> Option<String> {
        for item in &self.units {
            let Some(unit) = item.get("unit") else {
                continue;
            };
            if s(unit, "id") != uid {
                continue;
            }
            let rends = item.get("renditions")?.as_array()?;
            let id = s(rends.first()?.get("rendition")?, "id");
            if id.is_empty() {
                return None;
            }
            return Some(id);
        }
        None
    }

    fn version_fragment_bytes(&mut self, vid: &str) -> Result<Vec<u8>, String> {
        let resp = self
            .call_work("GET", &format!("/v1/versions/{vid}/fragment"), None)
            .map_err(|e| format!("{e}"))?;
        let data = resp
            .get("data_base64")
            .and_then(|x| x.as_str())
            .ok_or("服务器未返回片段数据")?;
        b64d(data).map_err(|_| "片段数据损坏".to_string())
    }

    /// The selected pages of a fragment as one merged fragment:
    /// split to single-page fragments, then cross-source assemble.
    fn clip_pick_pages(&self, frag: &[u8], pages: &[usize]) -> Result<Vec<u8>, String> {
        let pkg = qimu_core::pptx::Package::load(frag)?;
        let singles = qimu_core::pptx::split(&pkg)?;
        let mut picked = Vec::new();
        for &p in pages {
            match singles.get(p) {
                Some(one) => picked.push(one.clone()),
                None => return Err(format!("第 {} 页不存在", p + 1)),
            }
        }
        if picked.is_empty() {
            return Err("没有选中页面".into());
        }
        qimu_core::assemble::assemble(&picked)?.to_bytes()
    }

    /// All pages of the given blocks, in the given order, as one
    /// fragment (each block contributes its latest version).
    fn clip_bytes_from_units(&mut self, unit_ids: &[String]) -> Result<Vec<u8>, String> {
        let mut singles: Vec<qimu_core::pptx::Package> = Vec::new();
        for uid in unit_ids {
            let vid = self
                .unit_latest_version_id(uid)
                .ok_or_else(|| format!("「{}」没有可用的版本", self.unit_name(uid)))?;
            let frag = self.version_fragment_bytes(&vid)?;
            let pkg = qimu_core::pptx::Package::load(&frag)?;
            singles.extend(qimu_core::pptx::split(&pkg)?);
        }
        if singles.is_empty() {
            return Err("没有可复制的页面".into());
        }
        qimu_core::assemble::assemble(&singles)?.to_bytes()
    }

    /// 复制到 PPT 剪贴板 / 剪切：whole blocks from the content wall.
    /// Cut sends each source block to the recycle bin (回收站) —
    /// only after the clipboard holds its pages.
    /// Returns whether the operation fully succeeded (audit
    /// N3): for a copy, the clipboard holds the new fragment; for
    /// a cut, additionally every original reached the recycle
    /// bin. The deck-bar handler clears the selection on success
    /// only.
    fn clip_copy_units(&mut self, unit_ids: &[String], cut: bool) -> bool {
        let what = if cut {
            "剪切内容块"
        } else {
            "复制内容块"
        };
        if !self.need_identity_local(what) {
            return false;
        }
        if unit_ids.is_empty() {
            self.say("先选中内容块");
            return false;
        }
        let label = if unit_ids.len() == 1 {
            self.unit_name(&unit_ids[0])
        } else {
            format!("{} 个内容块", unit_ids.len())
        };
        match self.clip_bytes_from_units(unit_ids) {
            Ok(bytes) => {
                if !self.clip_store(&bytes, &label, true) {
                    return false;
                }
                if !cut {
                    return true;
                }
                // Per-unit trash outcomes are counted, not
                // overwritten by the success wording (audit N2).
                let mut trashed = 0usize;
                for uid in unit_ids {
                    if self.trash_add("unit", uid) {
                        trashed += 1;
                    }
                }
                let all = trashed == unit_ids.len();
                if all {
                    self.say(format!("已剪切 {label}（原内容块已移入回收站）"));
                } else {
                    self.say(format!(
                        "已剪切 {label}，但 {} 个原内容块移入回收站失败，仍保留在页面墙上",
                        unit_ids.len() - trashed
                    ));
                }
                // Phase-1 (design §3.1 point 11, as re-checked
                // against this baseline): the pure-copy path
                // returns above and never refreshed — this point
                // sits in the CUT branch, whose library writes
                // ride through trash_add (which emits Units +
                // Trash itself). Per the design's fallback rule
                // for points that do write the library, Units is
                // marked here as a backstop.
                self.changed(Change::Units);
                all
            }
            Err(e) => {
                self.say(format!("{}失败：{e}", if cut { "剪切" } else { "复制" }));
                false
            }
        }
    }

    /// Copy a showroom deck (演示厅成品) into the clipboard via its
    /// personal export — the deck-bar 复制到PPT剪贴板 wiring point.
    fn clip_copy_deck(&mut self, item: &Value) {
        let resp = self.call(
            "POST",
            &format!("/v1/showroom/{}/export", s(item, "id")),
            Some(json!({ "kind": "personal", "include_notes": true })),
        );
        match resp {
            Ok(v) => match v.get("data_base64").and_then(|x| x.as_str()) {
                Some(data) => match b64d(data) {
                    Ok(bytes) => {
                        self.clip_store(&bytes, &s(item, "title"), true);
                    }
                    Err(_) => self.msg = "复制失败：演示稿数据损坏".into(),
                },
                None => self.msg = "复制失败：服务器未返回演示稿".into(),
            },
            Err(e) => self.msg = format!("复制失败：{e}"),
        }
    }

    /// Copy selected pages of one version (unit-detail page picking).
    fn clip_copy_pages(&mut self, version_id: &str, pages: &[usize], source: &str) {
        match self.version_fragment_bytes(version_id) {
            Ok(frag) => match self.clip_pick_pages(&frag, pages) {
                Ok(picked) => {
                    self.clip_store(&picked, source, true);
                }
                Err(e) => self.msg = format!("复制失败：{e}"),
            },
            Err(e) => self.msg = format!("复制失败：{e}"),
        }
    }

    /// Cut selected pages of a block: the pages land in the
    /// clipboard first; the remainder is published back to the same
    /// block as a new version — or, when nothing remains, the block
    /// goes to the recycle bin instead.
    fn clip_cut_pages(
        &mut self,
        unit_id: &str,
        version_id: &str,
        pages: &[usize],
        form: &str,
        lang: &str,
    ) {
        if !self.need_identity_local("剪切页面") {
            return;
        }
        let frag = match self.version_fragment_bytes(version_id) {
            Ok(b) => b,
            Err(e) => {
                self.msg = format!("剪切失败：{e}");
                return;
            }
        };
        let total = qimu_core::pptx::Package::load(&frag)
            .ok()
            .and_then(|p| qimu_core::pptx::slides(&p).ok())
            .map(|v| v.len())
            .unwrap_or(0);
        let stored = match self.clip_pick_pages(&frag, pages) {
            Ok(picked) => self.clip_store(&picked, &self.unit_name(unit_id), true),
            Err(e) => {
                self.msg = format!("剪切失败：{e}");
                return;
            }
        };
        if !stored {
            // Audit2 E-H1: the clipboard does not hold the cut
            // pages — the source block must stay exactly as it is.
            return;
        }
        if total == 0 || pages.len() >= total {
            if self.trash_add("unit", unit_id) {
                self.say("已剪切全部页面（原内容块已移入回收站）");
            } else {
                self.say("已剪切全部页面，但原内容块移入回收站失败，仍保留在页面墙上");
            }
            // Phase-1 (design §3.1 point 12).
            self.changed(Change::Units);
            return;
        }
        let rest: Vec<usize> = (0..total).filter(|p| !pages.contains(p)).collect();
        match self.clip_pick_pages(&frag, &rest) {
            Ok(rest_bytes) => match self.call_work(
                "POST",
                &format!("/v1/units/{unit_id}/publish"),
                Some(json!({
                    "data_base64": b64e(&rest_bytes),
                    "form": form,
                    "lang": lang,
                    "bump": "minor",
                    "note": "剪切页面",
                })),
            ) {
                Ok(_) => {
                    self.msg = format!("已剪切 {} 页（原内容块已发布新版本）", pages.len());
                    // Phase-1 (design §3.1 point 13).
                    self.changed(Change::Units);
                }
                Err(e) => self.msg = format!("页面已复制到剪贴板，但源块发布新版本失败：{e}"),
            },
            Err(e) => self.msg = format!("剪切失败：{e}"),
        }
    }

    // ---------------- 组合 (combine) ----------------

    /// 组合为新内容块：the selected blocks' pages, in selection
    /// order, assembled into one new block.
    fn clip_combine_units(&mut self, unit_ids: &[String]) -> bool {
        if !self.need_identity_local("组合") {
            return false;
        }
        if unit_ids.len() < 2 {
            self.say("组合需要至少选中 2 个内容块");
            return false;
        }
        match self.clip_bytes_from_units(unit_ids) {
            Ok(bytes) => {
                let name = format!(
                    "{} 等 {} 个内容块组合",
                    self.unit_name(&unit_ids[0]),
                    unit_ids.len()
                );
                if self
                    .publish_bytes_as_unit(&bytes, &name, "组合新建")
                    .is_some()
                {
                    self.say(format!("已组合为新内容块「{name}」"));
                    // Phase-1 (design §3.1 point 14).
                    self.changed(Change::Units);
                    true
                } else {
                    false
                }
            }
            Err(e) => {
                self.say(format!("组合失败：{e}"));
                false
            }
        }
    }

    /// 组合为新索引：one index whose entries are the selected
    /// blocks' lead renditions, in selection order.
    fn clip_combine_index(&mut self, unit_ids: &[String]) -> bool {
        if !self.need_identity_local("组合为新索引") {
            return false;
        }
        if unit_ids.len() < 2 {
            self.say("组合为新索引需要至少选中 2 个内容块");
            return false;
        }
        let mut entries = Vec::new();
        for uid in unit_ids {
            match self.unit_lead_rendition(uid) {
                Some(rid) => entries.push(json!({ "unit_id": uid, "rendition_id": rid })),
                None => {
                    self.say(format!("「{}」没有可用的版本形态", self.unit_name(uid)));
                    return false;
                }
            }
        }
        let group_id = self.groups.first().map(|g| s(g, "id")).unwrap_or_default();
        let name = format!(
            "{} 等 {} 个内容块索引",
            self.unit_name(&unit_ids[0]),
            unit_ids.len()
        );
        match self.call_work(
            "POST",
            "/v1/indexes",
            Some(json!({
                "name": name,
                "group_id": group_id,
                "scope": "company",
                "entries": entries,
            })),
        ) {
            Ok(v) => {
                self.say(format!("已组合为新索引「{name}」"));
                // Phase-1 (design §3.1 point 15): a new index
                // (meta for the new id) over a units-backed
                // entry set; consumed at once — the caller acts
                // on the refreshed lists.
                let new_id = s(&v, "id");
                self.mark(Change::Units);
                self.mark(Change::Indexes);
                if new_id.is_empty() {
                    self.mark(Change::IndexMeta);
                } else {
                    self.mark_index_meta(&new_id);
                }
                self.flush_dirty_now();
                true
            }
            Err(e) => {
                self.say(format!("组合为新索引失败：{e}"));
                false
            }
        }
    }

    // The content wall's own clipboard/combine strip
    // (content_clip_bar) was removed by the 2026-11-25 deck-bar
    // consolidation ruling: the selection count, copy / cut,
    // combine, clear-selection and the two pastes now live only
    // in the bottom bar's action group (shell.rs
    // deckbar_actions), which calls the same clip_* methods.
    // clip_clear keeps its implementation — its only entry went
    // away with the strip, an accepted consequence of that
    // ruling (a fresh copy/cut overwrites the clipboard anyway).
}

// ---------------- UI ----------------

fn s(v: &Value, key: &str) -> String {
    v.get(key)
        .and_then(|x| x.as_str())
        .unwrap_or("")
        .to_string()
}

fn i(v: &Value, key: &str) -> i64 {
    v.get(key).and_then(|x| x.as_i64()).unwrap_or(0)
}

/// The tool row's vertical separator: 20 high, #343434, with the
/// framework's 6px breathing room on each side.
/// Toolbar separator: the prototype's .tool-sep — 1×18px --line
/// with 5px side margins.
fn tool_vsep(ui: &mut egui::Ui) {
    ui.add_space(5.0);
    let (rect, _) = ui.allocate_exact_size(egui::vec2(1.0, 18.0), egui::Sense::hover());
    ui.painter().vline(
        rect.center().x,
        rect.y_range(),
        egui::Stroke::new(1.0, palette_color(LINE_34)),
    );
    ui.add_space(5.0);
}

fn home_recent_row(ui: &mut egui::Ui, columns: [&str; 4], interactive: bool) -> egui::Response {
    let sense = if interactive {
        egui::Sense::click()
    } else {
        egui::Sense::hover()
    };
    let (rect, response) = ui.allocate_exact_size(egui::vec2(ui.available_width(), 32.0), sense);
    let painter = ui.painter();
    if interactive && response.hovered() {
        painter.rect_filled(rect, 6.0, palette_color(HOVER_BG));
    }
    let mut x = rect.min.x + 2.0;
    for (index, (text, fraction)) in columns.into_iter().zip([2.2, 1.1, 1.0, 1.0]).enumerate() {
        let width = rect.width() * fraction / 5.3;
        let color = if interactive && index == 0 {
            palette_color(TEXT_MAIN)
        } else {
            palette_color(TEXT_WEAK)
        };
        let column = egui::Rect::from_min_size(
            egui::pos2(x, rect.min.y),
            egui::vec2((width - 10.0).max(0.0), rect.height()),
        );
        painter
            .with_clip_rect(painter.clip_rect().intersect(column))
            .text(
                column.left_center(),
                egui::Align2::LEFT_CENTER,
                text,
                egui::FontId::proportional(12.0),
                color,
            );
        x += width;
    }
    if !interactive {
        painter.hline(
            rect.x_range(),
            rect.max.y - 0.5,
            egui::Stroke::new(1.0, palette_color(SEPARATOR)),
        );
    }
    response
}

fn showroom_columns(ui: &egui::Ui, minimum_width: f32) -> (usize, f32) {
    let available = ui.available_width();
    let columns = ((available + 12.0) / (minimum_width + 12.0))
        .floor()
        .max(1.0) as usize;
    let width = ((available + 12.0) / columns as f32 - 12.0).floor();
    (columns, width.max(1.0))
}

fn client_now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// The device-local recent-use record (home launcher, A27): newest
/// first, entries are {kind, id, title, ver, ms}.
fn recent_items() -> Vec<Value> {
    client_settings()
        .get("recent_items")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default()
}

/// Prototype primary, default and danger controls.
fn btn_primary(ui: &mut egui::Ui, label: &str) -> egui::Response {
    q_primary_button(ui, label)
}

fn btn_default(ui: &mut egui::Ui, label: &str) -> egui::Response {
    btn_default_enabled(ui, true, label)
}

fn btn_default_enabled(ui: &mut egui::Ui, enabled: bool, label: &str) -> egui::Response {
    ui.add_enabled(
        enabled,
        egui::Button::new(
            egui::RichText::new(label)
                .size(12.5)
                .color(palette_color(TEXT_MAIN)),
        )
        .fill(palette_color(BTN_BG))
        .corner_radius(6)
        .min_size(egui::vec2(0.0, 28.0)),
    )
}

fn btn_danger(ui: &mut egui::Ui, label: &str) -> egui::Response {
    ui.add(
        egui::Button::new(
            egui::RichText::new(label)
                .size(12.5)
                .color(egui::Color32::WHITE),
        )
        .fill(DANGER_BG)
        .corner_radius(6)
        .min_size(egui::vec2(0.0, 28.0)),
    )
}

/// The weak form, for dialog cancels: no fill, no border — muted
/// text only (the prototype's transparent ghost tier, .sp-ghost;
/// the .btn family itself has no ghost form). Min height matches
/// the .btn row (28) so mixed button rows stay aligned.
fn btn_weak(ui: &mut egui::Ui, label: &str) -> egui::Response {
    ui.add(
        egui::Button::new(
            egui::RichText::new(label)
                .size(12.5)
                .color(palette_color(TEXT_WEAK)),
        )
        .frame(false)
        .min_size(egui::vec2(0.0, 28.0)),
    )
}

/// Segmented selected-state styles (A6), following the
/// prototype's own division of labour: Soft is the segctl form
/// （视图/页签切换 — a --panel2 container, neutral --panel3
/// selected fill); Solid is the accent-filled pill （筛选激活，
/// the .idx-toolbar filter form) through q_seg_pills. The third
/// paradigm (accent-soft) belongs to .show-seg, which the
/// showroom sidebar draws directly.
#[derive(Clone, Copy, PartialEq)]
enum SegStyle {
    Solid,
    Soft,
}

fn segmented(
    ui: &mut egui::Ui,
    labels: &[&str],
    selected: usize,
    style: SegStyle,
) -> Option<usize> {
    match style {
        SegStyle::Solid => q_seg_pills(ui, labels, selected),
        SegStyle::Soft => {
            // The prototype's segctl: a --panel2 container with a
            // 1px --line border and a 2px inset; buttons at 6px
            // radius / 26px min height, selected = --panel3 fill
            // with ink text, resting = transparent with muted text.
            let mut changed = None;
            egui::Frame::NONE
                .fill(palette_color(HOVER_BG))
                .stroke(egui::Stroke::new(1.0, palette_color(SEPARATOR)))
                .corner_radius(egui::CornerRadius::same(6))
                .inner_margin(egui::Margin::same(2))
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 0.0;
                        for (i, label) in labels.iter().enumerate() {
                            let active = i == selected;
                            let btn = egui::Button::new(
                                egui::RichText::new(tr(*label)).size(12.5).color(if active {
                                    palette_color(TEXT_MAIN)
                                } else {
                                    palette_color(TEXT_WEAK)
                                }),
                            )
                            .fill(if active {
                                palette_color(PRESSED_BG)
                            } else {
                                egui::Color32::TRANSPARENT
                            })
                            .stroke(egui::Stroke::NONE)
                            .corner_radius(egui::CornerRadius::same(6))
                            .min_size(egui::vec2(0.0, 26.0));
                            if ui.add(btn).clicked() && !active {
                                changed = Some(i);
                            }
                        }
                    });
                });
            changed
        }
    }
}

/// Badge (A7): the semantic tone table — one place decides what
/// ok / warn / bad / info / pin look like (chip geometry underneath).
#[derive(Clone, Copy, PartialEq)]
enum BadgeTone {
    Neutral,
    Ok,
    Warn,
    Bad,
    Info,
    Pin,
}

fn badge(ui: &mut egui::Ui, text: &str, tone: BadgeTone) {
    let (fg, bg) = match tone {
        BadgeTone::Neutral => (palette_color(CHIP_GRAY_FG), palette_color(CHIP_GRAY_BG)),
        BadgeTone::Ok => (palette_color(CHIP_GREEN_FG), CHIP_GREEN_BG),
        BadgeTone::Warn | BadgeTone::Pin => (palette_color(CHIP_ORANGE_FG), CHIP_ORANGE_BG),
        BadgeTone::Bad => (palette_color(BADGE_BAD_FG), BADGE_BAD_BG),
        BadgeTone::Info => (palette_color(ACCENT_DEEP), CHIP_BLUE_BG),
    };
    chip(ui, text, fg, bg);
}

/// Dashed separator rule (B4): the card family's row divider —
/// 1px dashed line in a 9px strip.
fn dashed_rule(ui: &mut egui::Ui) {
    let (rect, _) =
        ui.allocate_exact_size(egui::vec2(ui.available_width(), 9.0), egui::Sense::hover());
    let painter = ui.painter();
    let y = rect.center().y;
    let stroke = egui::Stroke::new(1.0, palette_color(SEPARATOR));
    let mut x = rect.min.x;
    while x < rect.max.x {
        let x2 = (x + 4.0).min(rect.max.x);
        painter.line_segment([egui::pos2(x, y), egui::pos2(x2, y)], stroke);
        x += 7.0;
    }
}

/// StdCard (A1): the one card skeleton — cover slot, title, badge
/// row, meta row (below the dashed rule), sub row, action row. The
/// caller fills the slots in order inside the closure. `selected`
/// draws the accent outline (the prototype's selected state).
fn standard_card<R>(
    ui: &mut egui::Ui,
    selected: bool,
    scope: Option<egui::Color32>,
    flush: bool,
    add: impl FnOnce(&mut egui::Ui) -> R,
) -> egui::InnerResponse<R> {
    standard_card_v(ui, selected, scope, flush, CardVariant::Normal, add)
}

/// Card variants (the prototype's .std-card modifiers): `Mine`
/// is the 「我维护」 fill (--mine-bg); `Rejected` is the rose 2px
/// frame a rejected card wears until it is opened. Stroke
/// priority: selected > rejected > scope colour.
#[derive(Clone, Copy, PartialEq, Eq)]
enum CardVariant {
    Normal,
    Mine,
    Rejected,
}

fn standard_card_v<R>(
    ui: &mut egui::Ui,
    selected: bool,
    scope: Option<egui::Color32>,
    flush: bool,
    variant: CardVariant,
    add: impl FnOnce(&mut egui::Ui) -> R,
) -> egui::InnerResponse<R> {
    let mut frame = card_frame();
    if variant == CardVariant::Mine {
        frame = frame.fill(palette_color(MINE_BG));
    }
    if flush {
        frame = frame.inner_margin(0);
    }
    if selected {
        frame = frame.stroke(egui::Stroke::new(1.5, ACCENT));
    } else if variant == CardVariant::Rejected {
        frame = frame.stroke(egui::Stroke::new(2.0, palette_color(ROSE)));
    } else if let Some(color) = scope {
        frame = frame.stroke(egui::Stroke::new(1.0, color));
    }
    frame.show(ui, |ui| {
        ui.with_layout(egui::Layout::top_down(egui::Align::LEFT), add)
            .inner
    })
}

fn std_card(ui: &mut egui::Ui, selected: bool, add: impl FnOnce(&mut egui::Ui)) {
    standard_card(ui, selected, None, false, add);
}

struct WallCard<'a> {
    width: f32,
    min_height: f32,
    title: &'a str,
    cover: &'a str,
    selected: bool,
    scope: Option<egui::Color32>,
    variant: CardVariant,
}

/// The meta slot of a StdCard: content below the dashed rule.
fn std_card_meta(ui: &mut egui::Ui, add: impl FnOnce(&mut egui::Ui)) {
    dashed_rule(ui);
    add(ui);
}

/// SlideCover (A2): a slide's face — the real page thumbnail when
/// one is available, otherwise the prototype's drawn slide (colour
/// block + eyebrow + title + foot row), at card scale.
fn slide_cover(
    ui: &mut egui::Ui,
    tex: Option<egui::TextureHandle>,
    eyebrow: &str,
    title: &str,
    foot: &str,
    width: f32,
) {
    let h = width * 9.0 / 16.0;
    let (rect, _) = ui.allocate_exact_size(egui::vec2(width, h), egui::Sense::hover());
    let painter = ui.painter();
    if let Some(tex) = tex {
        painter.image(
            tex.id(),
            rect,
            egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
            egui::Color32::WHITE,
        );
        return;
    }
    painter.rect_filled(rect, 4.0, palette_color(HOVER_BG));
    let inner = rect.shrink(8.0);
    let eyebrow_font = egui::FontId::new(9.0, egui::FontFamily::Proportional);
    let g = painter.layout_no_wrap(eyebrow.to_string(), eyebrow_font, palette_color(TEXT_FAINT));
    painter.galley(
        egui::pos2(inner.min.x, inner.min.y),
        g,
        palette_color(TEXT_FAINT),
    );
    let title_font = egui::FontId::new(12.0, egui::FontFamily::Proportional);
    let g = painter.layout(
        title.to_string(),
        title_font,
        palette_color(TEXT_MAIN),
        inner.width(),
    );
    painter.galley(
        egui::pos2(inner.min.x, inner.min.y + 14.0),
        g,
        palette_color(TEXT_MAIN),
    );
    if !foot.is_empty() {
        let foot_font = egui::FontId::new(9.0, egui::FontFamily::Proportional);
        let g = painter.layout_no_wrap(foot.to_string(), foot_font, palette_color(TEXT_FAINT));
        painter.galley(
            egui::pos2(inner.min.x, inner.max.y - g.size().y),
            g,
            palette_color(TEXT_FAINT),
        );
    }
}

/// === Phase-1 unified components (2026-11-23) ===
/// The HTML prototype grew five button flavours and four segmented
/// controls; the client standardises on ONE primary button, ONE
/// ghost button and ONE segmented-pills control. New surfaces (Home,
/// Trash) are built only from these plus `card_frame()`; the four
/// legacy pages migrate onto them page by page (see phase-1 report).
fn q_primary_button(ui: &mut egui::Ui, label: &str) -> egui::Response {
    ui.add(
        egui::Button::new(
            egui::RichText::new(label)
                .size(12.5)
                .color(egui::Color32::WHITE),
        )
        .fill(ACCENT)
        .corner_radius(6)
        .min_size(egui::vec2(0.0, 28.0)),
    )
}

/// Segmented pills: returns the newly selected index when changed.
fn q_seg_pills(ui: &mut egui::Ui, labels: &[&str], selected: usize) -> Option<usize> {
    let mut changed = None;
    ui.horizontal(|ui| {
        for (i, label) in labels.iter().enumerate() {
            let active = i == selected;
            let btn =
                egui::Button::new(egui::RichText::new(tr(*label)).size(12.0).color(if active {
                    egui::Color32::WHITE
                } else {
                    palette_color(TEXT_WEAK)
                }))
                .fill(if active {
                    ACCENT
                } else {
                    palette_color(BTN_BG)
                })
                .corner_radius(255)
                .min_size(egui::vec2(0.0, 26.0));
            if ui.add(btn).clicked() && !active {
                changed = Some(i);
            }
        }
    });
    changed
}

impl App {
    fn home_nav_body(&mut self, ui: &mut egui::Ui) {
        for page in [Page::Showroom, Page::Index, Page::Content] {
            let image = page.icon().image(16.0, palette_color(ICON_TINT));
            let button = egui::Button::image_and_text(image, tr(page.label()))
                .fill(palette_color(HOVER_BG))
                .stroke(egui::Stroke::new(1.0, palette_color(FIELD_BORDER)));
            if ui.add_sized([ui.available_width(), 34.0], button).clicked() {
                self.navigate_to(page);
            }
        }
    }

    /// Sidebar zone header (user ruling 2026-11-24): the zone's
    /// label on the left, its create button at the row's right
    /// end. Returns true when the button was clicked.
    fn sr_header_row(
        &mut self,
        ui: &mut egui::Ui,
        label: &str,
        icon: ChromeIcon,
        tip: &str,
    ) -> bool {
        let mut clicked = false;
        ui.horizontal(|ui| {
            ui.label(
                egui::RichText::new(label)
                    .size(12.0)
                    .color(palette_color(TEXT_WEAK)),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if self.small_icon_button(ui, icon, tip) {
                    clicked = true;
                }
            });
        });
        clicked
    }

    /// Open the shared inline editor for a new root folder (the
    /// editor itself renders under the tree's 全部 row, or under
    /// the zone header where the zone has no folder tree).
    fn sr_new_folder(&mut self, scope: &str) {
        self.folder_edit = Some(FolderEdit {
            target: None,
            parent: None,
            text: "新文件夹".into(),
            scope: scope.into(),
        });
        self.folder_edit_focus = true;
    }

    fn sr_nav_body(&mut self, ui: &mut egui::Ui) {
        let zones = ["plaza", "library", "inbox", "groups"];
        let active = zones
            .iter()
            .position(|id| *id == self.showroom.zone)
            .unwrap_or(0);
        let labels = ["公共", "我的", "分享", "群组"];
        ui.horizontal(|ui| {
            if let Some(next) = segmented(ui, &labels, active, SegStyle::Soft) {
                self.showroom.zone = zones[next].to_string();
                self.showroom.selected_card = None;
                self.showroom.stack.pop_to(0);
                // Entering a zone lands on its 全部 selection
                // (user ruling 2026-11-24); the groups zone has
                // no 全部 row and keeps its current group.
                match zones[next] {
                    "plaza" => self.showroom.folder = None,
                    "library" => self.showroom.library_view = 0,
                    "inbox" => self.showroom.share_source = 0,
                    _ => {}
                }
            }
        });
        ui.add_space(8.0);
        match self.showroom.zone.as_str() {
            "library" => {
                if self.sr_header_row(ui, "目录", ChromeIcon::FolderPlus, tr("新建文件夹")) {
                    self.sr_new_folder("library");
                }
                if matches!(&self.folder_edit, Some(e) if e.target.is_none() && e.parent.is_none())
                {
                    self.folder_edit_row(
                        ui,
                        0.0,
                        ChromeIcon::Folder,
                        palette_color(ICON_TINT),
                    );
                    ui.add_space(2.0);
                }
                // Sidebar tree rows lead with an icon (prototype
                // .tree blanket rule); the 全部 row reuses the
                // all-applications glyph folder_tree uses for it.
                for (idx, (label, icon)) in [
                    ("全部", ChromeIcon::Settings),
                    ("收藏", ChromeIcon::Folder),
                    ("本地缓存", ChromeIcon::Cache),
                    ("我的组装", ChromeIcon::Folder),
                    ("本地演示", ChromeIcon::Showroom),
                ]
                .into_iter()
                .enumerate()
                {
                    if tree_row(
                        ui,
                        self.showroom.library_view == idx,
                        0.0,
                        Some(icon),
                        label,
                        12.5,
                    )
                    .clicked()
                    {
                        self.showroom.library_view = idx;
                        self.showroom.stack.pop_to(0);
                    }
                }
            }
            "inbox" => {
                if self.sr_header_row(ui, "分享来源", ChromeIcon::FolderPlus, tr("新建文件夹"))
                {
                    self.sr_new_folder("inbox");
                }
                for (idx, (label, icon)) in [
                    ("全部", ChromeIcon::Settings),
                    ("个人分享", ChromeIcon::Share),
                    ("部门分享", ChromeIcon::Directory),
                ]
                .into_iter()
                .enumerate()
                {
                    let count = self
                        .inbox
                        .iter()
                        .filter(|item| Self::share_matches_source(item, idx))
                        .count();
                    // The new-folder button lives on the 分享来源
                    // header row above (user ruling 2026-11-24);
                    // the rows here are plain source filters.
                    let clicked = tree_row(
                        ui,
                        self.showroom.share_source == idx,
                        0.0,
                        Some(icon),
                        &format!("{label} · {count}"),
                        12.5,
                    )
                    .clicked();
                    if clicked {
                        self.showroom.share_source = idx;
                        self.showroom.stack.pop_to(0);
                    }
                    if idx == 0
                        && matches!(&self.folder_edit, Some(e) if e.target.is_none() && e.parent.is_none())
                    {
                        self.folder_edit_row(
                            ui,
                            0.0,
                            ChromeIcon::Folder,
                            palette_color(ICON_TINT),
                        );
                        ui.add_space(2.0);
                    }
                }
            }
            "groups" => {
                // The header row's button creates a GROUP (user
                // ruling 2026-11-24), not a folder; it toggles an
                // inline name editor under the header that submits
                // through the same path as the groups view's form.
                if self.sr_header_row(ui, "群组", ChromeIcon::Directory, tr("新建群组")) {
                    self.team_create_open = !self.team_create_open;
                }
                if self.team_create_open {
                    ui.horizontal(|ui| {
                        ui.text_edit_singleline(&mut self.new_team_name);
                        if ui
                            .add_enabled(
                                !self.new_team_name.trim().is_empty(),
                                egui::Button::new("建组"),
                            )
                            .clicked()
                            && self.require_org_online("新建群组")
                        {
                            let name = self.new_team_name.clone();
                            match self.call(
                                "POST",
                                "/v1/custom-groups",
                                Some(json!({ "name": name })),
                            ) {
                                Ok(_) => {
                                    self.new_team_name.clear();
                                    self.team_create_open = false;
                                    self.msg = "群组已创建".into();
                                    self.refresh_all();
                                }
                                Err(e) => self.msg = format!("建组失败：{e}"),
                            }
                        }
                        if ui.button("取消").clicked() {
                            self.team_create_open = false;
                            self.new_team_name.clear();
                        }
                    });
                    ui.add_space(2.0);
                }
                let active = self
                    .active_showroom_group()
                    .map(|space| s(&space["group"], "id"));
                for space in &self.group_spaces {
                    let group = &space["group"];
                    let id = s(group, "id");
                    let label = format!(
                        "{} · {} 人",
                        s(group, "name"),
                        group["members"].as_array().map_or(0, Vec::len),
                    );
                    if tree_row(
                        ui,
                        active.as_ref() == Some(&id),
                        0.0,
                        Some(ChromeIcon::Folder),
                        &label,
                        12.5,
                    )
                    .clicked()
                    {
                        self.showroom.group_id = Some(id);
                        self.showroom.stack.pop_to(0);
                    }
                }
            }
            _ => {
                if self.sr_header_row(ui, "目录", ChromeIcon::FolderPlus, tr("新建文件夹")) {
                    self.sr_new_folder("showroom");
                }
                let mut selected = self.showroom.folder.clone();
                self.folder_tree(ui, &mut selected, false, "showroom");
                if selected != self.showroom.folder {
                    self.showroom.stack.pop_to(0);
                    self.showroom.selected_card = None;
                    self.showroom.folder = selected;
                }
            }
        }
    }

    fn active_showroom_group(&self) -> Option<&Value> {
        self.group_spaces
            .iter()
            .find(|space| space["group"]["id"].as_str() == self.showroom.group_id.as_deref())
            .or_else(|| self.group_spaces.first())
    }

    fn share_matches_source(item: &Value, source: usize) -> bool {
        match source {
            1 => item["target_kind"] == "user",
            2 => item["target_kind"] == "dept",
            _ => true,
        }
    }

    fn ct_nav_body(&mut self, ui: &mut egui::Ui) {
        // 目录 header row with the new-folder button at its right
        // end (user ruling 2026-11-24, correcting the same-day
        // removal): the button moved here from the tree's 全部 row.
        if self.sr_header_row(ui, "目录", ChromeIcon::FolderPlus, tr("新建文件夹")) {
            self.sr_new_folder("content");
        }
        let mut selected = self.content.folder_filter.clone();
        self.folder_tree(ui, &mut selected, false, "content");
        if self.content.folder_filter != selected
            && self.pick.is_none()
            && self.content.stack.depth() > 1
        {
            self.close_unit_detail();
        }
        self.content.folder_filter = selected;
    }

    fn ix_nav_body(&mut self, ui: &mut egui::Ui) {
        // 目录 header row, same pattern as the other folder-tree
        // sidebars (user ruling 2026-11-24).
        if self.sr_header_row(ui, "目录", ChromeIcon::FolderPlus, tr("新建文件夹")) {
            self.sr_new_folder("index");
        }
        let (in_detail, mut selected) = match self.index.stack.top() {
            IndexFrame::Detail(detail) => {
                let folder = self.index_folder_of(&detail.index_id);
                // A folder-less index's home is 未分类: keep the
                // Some("") sentinel so the tree highlights the
                // 未分类 node rather than 全部.
                (true, Some(folder))
            }
            _ => (false, self.index.overview_folder.clone()),
        };
        let previous = selected.clone();
        self.folder_tree(ui, &mut selected, true, "index");
        if in_detail && selected != previous {
            self.index.stack.pop_to(0);
        }
        if !in_detail || selected != previous {
            self.index.overview_folder = selected;
        }
    }

    fn st_nav_body(&mut self, ui: &mut egui::Ui) {
        let sections = self.settings_sections();
        if self.settings.section >= sections.len() {
            self.settings.section = 0;
        }
        let current = sections[self.settings.section];
        // Sidebar morphology (prototype settings sidebar): a
        // 「设置分区」 heading over the sections grouped by
        // ownership — 本机 vs 服务器与组织 (local-first P4; the
        // prototype's categories predate the local-first split) —
        // every row carrying its icon, and the approval row showing
        // the live pending count. The prototype renders a second
        // 「设置目录」 TOC with the same anchors; in this
        // section-switch layout the nav itself is the directory,
        // so the two lists are rendered as one.
        ui.label(
            egui::RichText::new(tr("设置分区"))
                .size(13.0)
                .strong()
                .color(palette_color(TEXT_MAIN)),
        );
        ui.add_space(4.0);
        let groups: &[(&str, &[(&str, ChromeIcon)])] = &[
            (
                "本机",
                &[
                    ("离线缓存", ChromeIcon::Download),
                    ("演示偏好", ChromeIcon::Presenter),
                ],
            ),
            (
                "服务器与组织",
                &[
                    ("服务器", ChromeIcon::Globe),
                    ("个人资料", ChromeIcon::Clipboard),
                    ("我的提交", ChromeIcon::Clipboard),
                    ("组织管理", ChromeIcon::Lineage),
                    ("内容组与授权", ChromeIcon::Directory),
                    ("审批队列", ChromeIcon::Active),
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
                    // Pending count (prototype .set-toc .cnt, L969):
                    // plain text, muted at rest, amber only when warn
                    // (prototype renderSetToc, L5537: warn = qn > 0;
                    // .cnt.warn{color:var(--amber)}, L970). No filled
                    // pill. Always rendered (muted "0" at rest, the
                    // merged nav follows the TOC form).
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
        // Org fold (prototype settings sidebar): the company org
        // tree under a collapsible header with the member count.
        // Its rows and add buttons land on the 组织管理 section,
        // where the real department/account forms live — the fold
        // is a shortcut into that section, not a second form.
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
            let mut jump_org = false;
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
                    if tree_row(
                        ui,
                        false,
                        0.0,
                        Some(ChromeIcon::Directory),
                        &s(d, "name"),
                        12.5,
                    )
                    .clicked()
                    {
                        jump_org = true;
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
                    jump_org = true;
                }
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
                    jump_org = true;
                }
            });
            if jump_org {
                if let Some(pos) = sections.iter().position(|x| *x == "组织管理") {
                    self.settings.section = pos;
                }
            }
        }
    }

    fn trash_nav_body(&mut self, ui: &mut egui::Ui) {
        // Sidebar (user rulings 2026-11-24): a 目录 header at the
        // top, then the three sections as folder rows — 演示 /
        // 索引 / 页面, each with its count. The trash is
        // local-first, so the sidebar renders with or without a
        // server connection. Clicking a section filters; clicking
        // the active section again clears back to all (the
        // prototype's toggle). Filter values are trash_list_ui's
        // positional filters.
        ui.label(
            egui::RichText::new("目录")
                .size(12.0)
                .color(palette_color(TEXT_WEAK)),
        );
        ui.add_space(2.0);
        let active = match self.trash.stack.top() {
            TrashFrame::List(state) => state.filter,
            TrashFrame::Preview(_) => usize::MAX,
        };
        for (filter, label) in [(3, "演示"), (1, "索引"), (2, "页面")] {
            let count = self
                .trash
                .items
                .iter()
                .filter(|item| trash_kind_in_filter(&s(item, "kind"), filter))
                .count();
            let text = format!("{} {}", tr(label), count);
            let icon = if active == filter {
                ChromeIcon::FolderOpen
            } else {
                ChromeIcon::Folder
            };
            if tree_row(ui, active == filter, 0.0, Some(icon), &text, 12.5).clicked() {
                self.trash.stack.pop_to(0);
                if let TrashFrame::List(state) = self.trash.stack.top_mut() {
                    state.filter = if active == filter { 0 } else { filter };
                }
            }
        }
    }
    fn confirm_empty_trash(&mut self) {
        self.overlays.push(Overlay::Confirm(ConfirmState {
            title: tr("清空回收站").to_string(),
            body: format!(
                "{} {} {}",
                tr("回收站中的"),
                self.trash.items.len(),
                tr("项将被永久删除，不可恢复。")
            ),
            confirm_label: tr("清空回收站").to_string(),
            danger: true,
            action: ConfirmAction::EmptyTrash,
        }));
    }

    fn page_home(&mut self, ui: &mut egui::Ui) {
        let viewport_width = ui.ctx().viewport_rect().width();
        egui::Frame::NONE
            .inner_margin(egui::Margin {
                left: 40,
                right: 40,
                top: 52,
                bottom: 18,
            })
            .show(ui, |ui| {
                egui::ScrollArea::vertical().show(ui, |ui| {
                    ui.label(egui::RichText::new(tr("欢迎使用启幕")).size(30.0).strong());
                    ui.add_space(5.0);
                    ui.label(
                        egui::RichText::new(format!(
                            "{} · 版本 {}",
                            tr("PPT 内容管理与索引系统"),
                            env!("CARGO_PKG_VERSION")
                        ))
                        .size(13.0)
                        .color(palette_color(TEXT_WEAK)),
                    );
                    ui.add_space(28.0);
                    let mut action = None;
                    let columns = if viewport_width <= 1000.0 { 2 } else { 4 };
                    let width = if viewport_width > 1200.0 {
                        ui.available_width().min(980.0)
                    } else {
                        ui.available_width()
                    };
                    let card_width = ((width + 12.0) / columns as f32 - 12.0).floor();
                    let actions = [
                        ("new", "新建索引", ChromeIcon::NewIndex),
                        ("sample", "打开示例项目", ChromeIcon::Showroom),
                        ("import", "导入素材…", ChromeIcon::Import),
                        ("template", "从模板新建", ChromeIcon::Settings),
                    ];
                    ui.spacing_mut().item_spacing = egui::vec2(12.0, 12.0);
                    for row in actions.chunks(columns) {
                        ui.horizontal(|ui| {
                            for &(key, label, icon) in row {
                                if self.home_action_card(ui, card_width, icon, tr(label)) {
                                    action = Some(key);
                                }
                            }
                        });
                    }
                    self.run_home_action(action);
                    ui.add_space(40.0);
                    self.home_recent_ui(ui);
                });
            });
    }

    fn run_home_action(&mut self, action: Option<&str>) {
        match action {
            Some("new") => self.act_new_index(),
            Some("sample") => self.open_sample_project(),
            Some("import") => {
                self.page = Page::Content;
                self.act_import_ppt();
            }
            Some("template") => {
                let first = self.indexes.first().map(|x| s(x, "id")).unwrap_or_default();
                self.overlays.push(Overlay::PickTemplate(PickTemplateState {
                    new_name: String::new(),
                    template_id: first,
                }));
            }
            _ => {}
        }
    }

    fn home_recent_ui(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new(tr("最近使用")).size(13.0).strong());
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.add(
                    egui::TextEdit::singleline(&mut self.home_recent_query)
                        .hint_text(tr("搜索最近文件"))
                        .desired_width(170.0)
                        .margin(egui::vec2(9.0, 6.0)),
                );
            });
        });
        ui.spacing_mut().item_spacing.y = 0.0;
        home_recent_row(
            ui,
            [tr("名称"), tr("打开时间"), tr("大小"), tr("类型")],
            false,
        );

        let query = self.home_recent_query.trim().to_lowercase();
        let mut open = None;
        let mut found = false;
        for r in recent_items() {
            let kind = match s(&r, "kind").as_str() {
                "index" => tr("索引"),
                "unit" => tr("内容块"),
                "showroom" => tr("演示稿"),
                _ => "",
            };
            let title = s(&r, "title");
            let version = s(&r, "ver");
            if ![title.as_str(), kind, version.as_str()]
                .iter()
                .any(|value| value.to_lowercase().contains(&query))
            {
                continue;
            }
            found = true;
            let time = fmt_time(i(&r, "ms"));
            let response = home_recent_row(ui, [&title, &time, "—", kind], true);
            if !version.is_empty() {
                response.clone().on_hover_text(version);
            }
            if response.clicked() {
                open = Some(r);
            }
        }
        if !found {
            ui.add_space(56.0);
            ui.vertical_centered(|ui| {
                if query.is_empty() {
                    ui.label(tr("你最近使用的文件会显示在这里"));
                    ui.add_space(7.0);
                    ui.label(
                        egui::RichText::new(tr("新建一个索引，或打开示例项目先熟悉一下界面。"))
                            .size(12.0)
                            .color(palette_color(TEXT_WEAK)),
                    );
                } else {
                    ui.label(tr("没有匹配的最近文件"));
                }
            });
        }
        if let Some(r) = open {
            self.deep_source = Some(Page::Home);
            match s(&r, "kind").as_str() {
                "index" => {
                    self.page = Page::Index;
                    self.open_index_detail(&s(&r, "id"));
                }
                "unit" => {
                    self.page = Page::Content;
                    self.open_unit_detail(&s(&r, "id"));
                }
                "showroom" => {
                    self.page = Page::Showroom;
                    self.open_deck_for_ref("showroom", &s(&r, "id"), &s(&r, "title"));
                }
                _ => {}
            }
        }
    }

    /// One launcher action card (A27): 22px icon + label on a
    /// 66px-tall card; the whole card is the button.
    fn home_action_card(
        &mut self,
        ui: &mut egui::Ui,
        width: f32,
        icon: ChromeIcon,
        label: &str,
    ) -> bool {
        let (rect, resp) = ui.allocate_exact_size(egui::vec2(width, 66.0), egui::Sense::click());
        let painter = ui.painter();
        let fill = if resp.hovered() {
            palette_color(HOVER_BG)
        } else {
            palette_color(CARD_BG)
        };
        painter.rect_filled(rect, 6.0, fill);
        painter.rect_stroke(
            rect,
            6.0,
            egui::Stroke::new(1.0, palette_color(CARD_BORDER)),
            egui::StrokeKind::Inside,
        );
        let ir = egui::Rect::from_min_size(
            egui::pos2(rect.min.x + 16.0, rect.center().y - 11.0),
            egui::vec2(22.0, 22.0),
        );
        icon.image(22.0, palette_color(TEXT_MAIN)).paint_at(ui, ir);
        let font = egui::FontId::new(13.5, egui::FontFamily::Proportional);
        let g = painter.layout_no_wrap(label.to_string(), font, palette_color(TEXT_MAIN));
        painter.galley(
            egui::pos2(rect.min.x + 50.0, rect.center().y - g.size().y / 2.0),
            g,
            palette_color(TEXT_MAIN),
        );
        resp.clicked()
    }

    /// 「打开示例项目」: the server seeds groups and folders but no
    /// sample deck, so the sample is whatever the deployment
    /// prepared — an index or showroom item named 示例 first, else
    /// the first live showroom item; the empty case says so plainly.
    fn open_sample_project(&mut self) {
        if let Some(idx) = self
            .indexes
            .iter()
            .find(|x| s(x, "name").contains("示例"))
            .cloned()
        {
            self.deep_source = Some(Page::Home);
            self.page = Page::Index;
            self.open_index_detail(&s(&idx, "id"));
            return;
        }
        let item = self
            .showroom_live
            .iter()
            .find(|x| s(x, "title").contains("示例"))
            .cloned()
            .or_else(|| self.showroom_live.first().cloned());
        match item {
            Some(it) => {
                self.deep_source = Some(Page::Home);
                self.page = Page::Showroom;
                self.open_showroom_deck(&it);
            }
            None => {
                self.msg = "示例项目尚未准备：当前服务器没有示例内容".into();
            }
        }
    }

    /// Record a recent-use entry (home launcher): newest first,
    /// de-duplicated by (kind, id), capped at 10, persisted in the
    /// local client settings — recent use is a property of this
    /// device, not of the account.
    fn note_recent(&mut self, kind: &str, id: &str, title: &str, ver: &str) {
        if id.is_empty() || title.is_empty() {
            return;
        }
        let mut items = recent_items();
        items.retain(|x| !(s(x, "kind") == kind && s(x, "id") == id));
        items.insert(
            0,
            json!({
                "kind": kind, "id": id, "title": title, "ver": ver,
                "ms": client_now_ms(),
            }),
        );
        items.truncate(10);
        save_client_settings(json!({ "recent_items": items }));
    }

    /// Trash (prototype #trashView): a standalone page on a
    /// drill-down frame stack — the list frame (kind filter + batch
    /// selection) and a preview frame for items whose cover version
    /// is known. Data comes from GET /v1/trash (the store's
    /// soft-delete overlay): restore drops the overlay row and is
    /// lossless; purge is the only destructive step and always goes
    /// through the Confirm overlay.
    fn page_trash(&mut self, ui: &mut egui::Ui) {
        body_frame().show(ui, |ui| {
        // The trash is the local library's first (user ruling
        // 2026-11-24): locally deleted units and indexes land in
        // the in-process store's trash and are managed here with
        // no server connection at all; organization showroom rows
        // join through the same call_work path when connected.
        // This page has no not-connected state.
        if !self.trash.loaded {
            self.trash_load();
        }
        let in_preview = matches!(self.trash.stack.top(), TrashFrame::Preview(_));
        if in_preview {
            self.trash_preview_ui(ui);
        } else {
            self.trash_list_ui(ui);
        }
        });
    }

    fn trash_load(&mut self) {
        match self.call_work("GET", "/v1/trash", None) {
            Ok(v) => {
                let mut items = v
                    .get("items")
                    .and_then(|x| x.as_array())
                    .cloned()
                    .unwrap_or_default();
                // F6: with a local library open, the local face
                // answers GET /v1/trash by itself, so organization
                // trash rows (showroom items trashed on the
                // server) never reached this page — even though
                // their restore/purge calls already route to the
                // server by row ownership. Merge the server's
                // rows in while connected (the server filters
                // them by the caller's rights), marked "org" so
                // purge-all can route them back individually.
                // The page stays local-first: offline, or with no
                // local library (where call_work already reads
                // the server), nothing changes.
                if self.local.is_some() && self.org_online() {
                    if let Ok(sv) = self.call("GET", "/v1/trash", None) {
                        if let Some(rows) = sv.get("items").and_then(|x| x.as_array()) {
                            for row in rows {
                                if !items.iter().any(|x| s(x, "id") == s(row, "id")) {
                                    let mut row = row.clone();
                                    row["org"] = json!(true);
                                    items.push(row);
                                }
                            }
                        }
                    }
                }
                self.trash.items = items;
                self.trash.loaded = true;
                self.trash.load_error.clear();
            }
            Err(e) => {
                // Stay loaded (no per-frame retry spin); the list
                // frame shows the error with an explicit retry.
                self.trash.loaded = true;
                self.trash.load_error = e;
            }
        }
    }

    /// Move an object to the trash (soft delete) — the producers:
    /// unit detail, index detail, showroom cards.
    fn trash_add(&mut self, kind: &str, object_id: &str) -> bool {
        // Trashing a staged import's wrapper mid-grouping fails
        // the whole group stage after half of it already landed
        // (audit N1); refuse until the import settles.
        if kind == "unit"
            && self.importing.is_some()
            && self.import_open_units.iter().any(|u| u == object_id)
        {
            self.say("正在导入成块，暂不能移入回收站");
            return false;
        }
        match self.call_work(
            "POST",
            "/v1/trash",
            Some(json!({ "kind": kind, "object_id": object_id })),
        ) {
            Ok(_) => {
                self.say("已移入回收站，可随时恢复");
                self.trash.loaded = false;
                // Phase-1 (design §3.1 point 18): the source
                // slice follows the trashed kind (this funnel
                // serves the unit / index / showroom walls);
                // Trash itself is invalidated above and marked
                // here for the slice machinery's bookkeeping.
                // (The per-kind set is delete-freeze round 2's
                // narrowing, in event form.)
                match kind {
                    "unit" => self.changed(Change::Units),
                    "index" => self.changed(Change::Indexes),
                    "showroom" => self.changed(Change::ShowroomLive),
                    _ => {
                        self.changed(Change::Units);
                        self.changed(Change::Indexes);
                    }
                }
                self.changed(Change::Trash);
                true
            }
            Err(e) => {
                self.say(format!("删除失败：{e}"));
                false
            }
        }
    }

    /// The batch face of trash_add for the bottom bar's delete
    /// (user ruling 2026-11-25: every selected object, of every
    /// kind, deletes in one click; failures report per item and
    /// never block the rest). Each target goes through the same
    /// POST /v1/trash the single-item path uses — the importing
    /// guard is trash_add's own, repeated here because trash_add
    /// folds its verdict into the message bar, while the batch
    /// needs the reason per item. Returns the failures as
    /// (target, reason); successes only mark the trash stale —
    /// the caller refreshes once, scoped to the kinds that
    /// actually went away (refresh_after_trash_delete).
    fn trash_add_many(&mut self, targets: &[DelTarget]) -> Vec<(DelTarget, String)> {
        let mut failures = Vec::new();
        let mut any_ok = false;
        for t in targets {
            if t.kind == "unit"
                && self.importing.is_some()
                && self.import_open_units.iter().any(|u| u == &t.id)
            {
                failures.push((
                    t.clone(),
                    "正在导入成块，暂不能移入回收站".to_string(),
                ));
                continue;
            }
            match self.call_work(
                "POST",
                "/v1/trash",
                Some(json!({ "kind": t.kind, "object_id": t.id })),
            ) {
                Ok(_) => any_ok = true,
                Err(e) => failures.push((t.clone(), e)),
            }
        }
        if any_ok {
            self.trash.loaded = false;
        }
        failures
    }

    /// Post-delete refresh for the bottom-bar batch, scoped to
    /// the kinds that were deleted (the delete-freeze line's
    /// rule: a delete re-reads only the slices it can have
    /// changed — never a fresh refresh_all). Units and versions
    /// share the units list (its rows carry the version strips);
    /// indexes reload their list and drop the overview graphs;
    /// showroom entries re-run refresh_org's live arm verbatim
    /// (fetch → cache staleness reconciliation → cover
    /// resolution, cache backfill when the fetch fails).
    fn refresh_after_trash_delete(&mut self, kinds: &[&str]) {
        if kinds.iter().any(|k| *k == "unit" || *k == "version") {
            if let Some(local) = self.local.clone() {
                self.units = local.list_units();
            } else if let Ok(v) = self.call("GET", "/v1/units", None) {
                self.units = v
                    .get("units")
                    .and_then(|f| f.as_array())
                    .cloned()
                    .unwrap_or_default();
            }
            self.prune_wall_selection();
        }
        if kinds.iter().any(|k| *k == "index") {
            if let Some(local) = self.local.clone() {
                if let IndexFrame::Overview(overview) = self.index.stack.root_mut() {
                    overview.graphs = None;
                }
                self.indexes = local.list_indexes();
            } else if let Ok(v) = self.call("GET", "/v1/indexes", None) {
                if let IndexFrame::Overview(overview) = self.index.stack.root_mut() {
                    overview.graphs = None;
                }
                self.indexes = v
                    .get("indexes")
                    .and_then(|f| f.as_array())
                    .cloned()
                    .unwrap_or_default();
            }
        }
        if kinds.iter().any(|k| *k == "showroom") {
            if let Ok(v) = self.call("GET", "/v1/showroom?state=live", None) {
                self.showroom_live = v
                    .get("items")
                    .and_then(|f| f.as_array())
                    .cloned()
                    .unwrap_or_default();
                self.reconcile_cache_staleness();
                self.index_covers.retain(|_, v| !v.is_empty());
                self.resolve_showroom_covers();
            } else {
                self.backfill_showroom_from_cache();
            }
        }
    }

    fn trash_restore(&mut self, trash_id: &str) {
        match self.call_work(
            "POST",
            &format!("/v1/trash/{trash_id}/restore"),
            Some(json!({})),
        ) {
            Ok(_) => {
                self.msg = "已恢复".into();
                self.trash.loaded = false;
                // The restored object re-enters exactly one
                // list (delete-freeze round 2's kind precision,
                // phase-1 §3.1 point 19's event form): the row's
                // kind is known from the loaded trash items —
                // unknown kinds conservatively mark both working
                // lists.
                let kind = self
                    .trash
                    .items
                    .iter()
                    .find(|t| s(t, "id") == trash_id)
                    .map(|t| s(t, "kind"))
                    .unwrap_or_default();
                match kind.as_str() {
                    "unit" => self.changed(Change::Units),
                    "index" => self.changed(Change::Indexes),
                    "showroom" => self.changed(Change::ShowroomLive),
                    _ => {
                        self.changed(Change::Units);
                        self.changed(Change::Indexes);
                    }
                }
                self.changed(Change::Trash);
            }
            Err(e) => self.msg = format!("恢复失败：{e}"),
        }
    }

    fn trash_purge(&mut self, trash_id: &str) {
        match self.call_work(
            "POST",
            &format!("/v1/trash/{trash_id}/purge"),
            Some(json!({})),
        ) {
            Ok(_) => {
                self.msg = "已彻底删除".into();
                self.trash.loaded = false;
                // If the preview frame shows the purged item, leave it.
                let showing = match self.trash.stack.top() {
                    TrashFrame::Preview(p) => p.trash_id == trash_id,
                    _ => false,
                };
                if showing {
                    self.trash.stack.pop();
                }
                // Phase-1 (design §3.1 point 20): a purge only
                // ever touches the bin itself — no wall refresh
                // (the purged object was already absent from
                // every list; delete-freeze round 2 deleted the
                // old refresh_all here as pure cost). The Trash
                // mark is the bin's bookkeeping; the page itself
                // also re-reads via trash.loaded above.
                self.changed(Change::Trash);
            }
            Err(e) => self.msg = format!("彻底删除失败：{e}"),
        }
    }

    fn trash_list_ui(&mut self, ui: &mut egui::Ui) {
        // Frame state snapshot (the frame itself is mutated after
        // rendering, from the collected intents).
        let (filter, selected) = match self.trash.stack.top() {
            TrashFrame::List(ls) => (ls.filter, ls.selected.clone()),
            _ => (0, std::collections::HashSet::new()),
        };
        let items = self.trash.items.clone();
        let shown: Vec<Value> = items
            .iter()
            .filter(|t| trash_kind_in_filter(&s(t, "kind"), filter))
            .cloned()
            .collect();

        let mut toggle: Option<String> = None;
        let mut restore: Option<String> = None;
        let mut purge: Option<(String, String)> = None;
        let mut preview: Option<Value> = None;
        let mut batch: Option<&'static str> = None;
        let mut retry = false;

        egui::ScrollArea::vertical().show(ui, |ui| {
            ui.horizontal(|ui| {
                // No page title here (user ruling 2026-11-24): the
                // content area starts with the count hint directly.
                ui.label(
                    egui::RichText::new(format!(
                        "{} {} · {}",
                        tr("共"),
                        items.len(),
                        tr("项 · 原版本与语言绑定原样保留")
                    ))
                    .size(11.5)
                    .color(palette_color(TEXT_FAINT)),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if btn_default(ui, tr("刷新")).clicked() {
                        retry = true;
                    }
                });
            });
            ui.add_space(10.0);
            if !selected.is_empty() {
                ui.horizontal(|ui| {
                    ui.label(
                        egui::RichText::new(format!(
                            "{} {} {}",
                            tr("已选"),
                            selected.len(),
                            tr("项")
                        ))
                        .size(12.0)
                        .color(palette_color(TEXT_MAIN)),
                    );
                    if btn_default(ui, tr("批量恢复")).clicked() {
                        batch = Some("restore");
                    }
                    if btn_danger(ui, tr("批量彻底删除")).clicked() {
                        batch = Some("purge");
                    }
                    if btn_default(ui, tr("取消选择")).clicked() {
                        batch = Some("clear");
                    }
                });
                ui.add_space(10.0);
            }
            if !self.trash.load_error.is_empty() {
                card_frame().show(ui, |ui| {
                    ui.label(
                        egui::RichText::new(format!(
                            "{}：{}",
                            tr("回收站加载失败"),
                            self.trash.load_error
                        ))
                        .size(12.5)
                        .color(palette_color(TEXT_MAIN)),
                    );
                    if btn_primary(ui, tr("重试")).clicked() {
                        retry = true;
                    }
                });
                ui.add_space(10.0);
            }
            for t in &shown {
                let tid = s(t, "id");
                let kind = s(t, "kind");
                let checked = selected.contains(&tid);
                let cover_vid = s(t, "cover_version_id");
                let tex = if cover_vid.is_empty() {
                    None
                } else {
                    self.thumb(&cover_vid, 0)
                };
                let (kind_label, tone) = match kind.as_str() {
                    "index" => (tr("索引"), BadgeTone::Info),
                    "unit" => (tr("内容块"), BadgeTone::Info),
                    "version" => (tr("历史版本"), BadgeTone::Info),
                    _ => (tr("演示稿"), BadgeTone::Neutral),
                };
                let title = s(t, "title");
                let origin = s(t, "origin");
                let by = s(t, "deleted_by");
                let at = fmt_time(i(t, "deleted_ms"));
                let mut check_change = false;
                std_card(ui, checked, |ui| {
                    ui.horizontal(|ui| {
                        let mut c = checked;
                        if ui.checkbox(&mut c, "").changed() {
                            check_change = true;
                        }
                        slide_cover(ui, tex, kind_label, &title, "", 84.0);
                        ui.vertical(|ui| {
                            ui.label(
                                egui::RichText::new(&title)
                                    .size(13.0)
                                    .color(palette_color(TEXT_MAIN)),
                            );
                            ui.horizontal(|ui| {
                                badge(ui, kind_label, tone);
                            });
                        });
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::TOP), |ui| {
                            ui.vertical(|ui| {
                                if btn_danger(ui, tr("彻底删除")).clicked() {
                                    purge = Some((tid.clone(), title.clone()));
                                }
                                if btn_default(ui, tr("恢复")).clicked() {
                                    restore = Some(tid.clone());
                                }
                                if !cover_vid.is_empty() && btn_default(ui, tr("预览")).clicked()
                                {
                                    preview = Some(t.clone());
                                }
                            });
                        });
                    });
                    std_card_meta(ui, |ui| {
                        ui.label(
                            egui::RichText::new(format!(
                                "{}「{}」 · {} {} · {}",
                                tr("原位置"),
                                origin,
                                by,
                                tr("删除"),
                                at
                            ))
                            .size(11.0)
                            .color(palette_color(TEXT_FAINT)),
                        );
                    });
                });
                if check_change {
                    toggle = Some(tid.clone());
                }
                ui.add_space(8.0);
            }
            ui.add_space(14.0);
        });

        // Apply intents.
        if retry {
            self.trash.loaded = false;
        }
        if let Some(tid) = toggle {
            if let TrashFrame::List(ls) = self.trash.stack.top_mut() {
                if !ls.selected.remove(&tid) {
                    ls.selected.insert(tid);
                }
            }
        }
        if let Some(tid) = restore {
            self.trash_restore(&tid);
        }
        if let Some((tid, title)) = purge {
            self.overlays.push(Overlay::Confirm(ConfirmState {
                title: tr("彻底删除").to_string(),
                body: format!(
                    "「{}」{}",
                    title,
                    tr("将被永久删除（全部版本与绑定），不可恢复。")
                ),
                confirm_label: tr("彻底删除").to_string(),
                danger: true,
                action: ConfirmAction::PurgeTrash(tid),
            }));
        }
        if let Some(t) = preview {
            self.trash
                .stack
                .push(TrashFrame::Preview(TrashPreviewState {
                    trash_id: s(&t, "id"),
                    kind: s(&t, "kind"),
                    title: s(&t, "title"),
                    version_id: s(&t, "cover_version_id"),
                    page_count: i(&t, "page_count").max(0) as usize,
                    page: 0,
                }));
        }
        match batch {
            Some("clear") => {
                if let TrashFrame::List(ls) = self.trash.stack.top_mut() {
                    ls.selected.clear();
                }
            }
            Some("restore") => {
                let ids: Vec<String> = selected.iter().cloned().collect();
                let mut ok = 0usize;
                for id in &ids {
                    if self
                        .call_work("POST", &format!("/v1/trash/{id}/restore"), Some(json!({})))
                        .is_ok()
                    {
                        ok += 1;
                    }
                }
                self.msg = format!("{} {} {}", tr("已恢复"), ok, tr("项"));
                if let TrashFrame::List(ls) = self.trash.stack.top_mut() {
                    ls.selected.clear();
                }
                self.trash.loaded = false;
                // Phase-1 (design §3.1 point 21): batch restore,
                // same event set as the single restore; the
                // showroom-rows check is delete-freeze round 2's
                // conditional live-wall cover, in mark form.
                self.changed(Change::Trash);
                self.changed(Change::Units);
                self.changed(Change::Indexes);
                if ids.iter().any(|id| {
                    self.trash
                        .items
                        .iter()
                        .any(|t| s(t, "id") == *id && s(t, "kind") == "showroom")
                }) {
                    self.changed(Change::ShowroomLive);
                }
            }
            Some("purge") => {
                let ids: Vec<String> = selected.iter().cloned().collect();
                self.overlays.push(Overlay::Confirm(ConfirmState {
                    title: tr("批量彻底删除").to_string(),
                    body: format!(
                        "{} {} {}",
                        tr("选中的"),
                        ids.len(),
                        tr("项将被永久删除，不可恢复。")
                    ),
                    confirm_label: tr("彻底删除").to_string(),
                    danger: true,
                    action: ConfirmAction::PurgeTrashMany(ids),
                }));
            }
            _ => {}
        }
    }

    fn trash_preview_ui(&mut self, ui: &mut egui::Ui) {
        let (trash_id, kind, title, version_id, page_count, page) = match self.trash.stack.top() {
            TrashFrame::Preview(p) => (
                p.trash_id.clone(),
                p.kind.clone(),
                p.title.clone(),
                p.version_id.clone(),
                p.page_count,
                p.page,
            ),
            _ => return,
        };
        let row = self
            .trash
            .items
            .iter()
            .find(|t| s(t, "id") == trash_id)
            .cloned();
        let mut go_back = false;
        let mut new_page: Option<usize> = None;
        let mut do_restore = false;
        let mut do_purge = false;
        egui::ScrollArea::vertical().show(ui, |ui| {
            ui.add_space(10.0);
            if let Some(d) = crumb_bar(ui, &[&title]) {
                if d == 0 {
                    go_back = true;
                }
            }
            ui.add_space(10.0);
            ui.vertical_centered(|ui| {
                self.thumb_ui(ui, &version_id, page, 820.0);
            });
            ui.add_space(10.0);
            ui.horizontal(|ui| {
                if ui
                    .add_enabled(page > 0, egui::Button::new(tr("上一页")))
                    .clicked()
                {
                    new_page = Some(page - 1);
                }
                ui.label(
                    egui::RichText::new(format!(
                        "{} {} / {} {}",
                        tr("第"),
                        page + 1,
                        page_count.max(1),
                        tr("页")
                    ))
                    .size(12.0)
                    .color(palette_color(TEXT_WEAK)),
                );
                if ui
                    .add_enabled(page + 1 < page_count, egui::Button::new(tr("下一页")))
                    .clicked()
                {
                    new_page = Some(page + 1);
                }
                ui.add_space(24.0);
                if btn_primary(ui, tr("恢复")).clicked() {
                    do_restore = true;
                }
                if btn_danger(ui, tr("彻底删除")).clicked() {
                    do_purge = true;
                }
            });
            if let Some(t) = &row {
                ui.add_space(8.0);
                let kind_label = match kind.as_str() {
                    "index" => tr("索引"),
                    "unit" => tr("内容块"),
                    _ => tr("演示稿"),
                };
                ui.label(
                    egui::RichText::new(format!(
                        "{} · {}「{}」 · {} {} · {}",
                        kind_label,
                        tr("原位置"),
                        s(t, "origin"),
                        s(t, "deleted_by"),
                        tr("删除"),
                        fmt_time(i(t, "deleted_ms"))
                    ))
                    .size(11.5)
                    .color(palette_color(TEXT_FAINT)),
                );
            }
            ui.add_space(14.0);
        });
        if let Some(p) = new_page {
            if let TrashFrame::Preview(pv) = self.trash.stack.top_mut() {
                pv.page = p;
            }
        }
        if do_restore {
            self.trash_restore(&trash_id);
            self.trash.stack.pop();
        }
        if do_purge {
            self.overlays.push(Overlay::Confirm(ConfirmState {
                title: tr("彻底删除").to_string(),
                body: format!(
                    "「{}」{}",
                    title,
                    tr("将被永久删除（全部版本与绑定），不可恢复。")
                ),
                confirm_label: tr("彻底删除").to_string(),
                danger: true,
                action: ConfirmAction::PurgeTrash(trash_id.clone()),
            }));
        }
        if go_back {
            self.trash.stack.pop();
        }
    }
}

impl App {
    /// Settings sections visible to the current role — sections a
    /// role may not see are not rendered at all (§7.9).
    fn settings_sections(&self) -> Vec<&'static str> {
        if self.user.is_none() {
            // No named identity (logged out, anonymous guest, or
            // standalone — the local library needs no identity):
            // only the local sections plus the server connection
            // entry. Identity-bound sections appear after login,
            // per role, as before.
            return vec!["服务器", "离线缓存", "演示偏好"];
        }
        let role = self.role();
        let mut sections: Vec<&str> = vec!["服务器", "个人资料", "我的提交"];
        if role != "member" {
            sections.push("组织管理");
        }
        if role == "super" {
            sections.extend([
                "审批队列",
                "内容组与授权",
                "公司公共资料",
                "内容语言",
                "AI 翻译",
                "离线缓存策略",
            ]);
        } else {
            // The cache path is a local per-user setting; the admin
            // policy section above stays super-only.
            sections.push("离线缓存");
        }
        sections.push("演示偏好");
        sections
    }

    /// Set the status message AND re-stamp the toast, even when
    /// the text is identical to the previous message (audit L4):
    /// repeated actions with the same outcome wording (save,
    /// restore, rename, delete…) still deserve their feedback.
    /// Plain `self.msg = …` assignments keep the old dedupe
    /// behaviour; action handlers route through here.
    fn say(&mut self, text: impl Into<String>) {
        self.msg = text.into();
        self.toast_nonce += 1;
    }

    /// Forced password change (G2 contract): while the active
    /// account is flagged `must_change_password`, this modal
    /// covers the app and cannot be dismissed — no close button,
    /// and backdrop clicks / Escape are simply never acted on;
    /// only a successful change releases it. The change goes to
    /// POST /v1/me/password with
    /// {current_password, new_password} (G2's implemented
    /// endpoint; aligned at merge time from the G4-side assumed
    /// name). Success returns {ok: true}: the server clears the
    /// flag, KEEPS the current session and logs out the user's
    /// other sessions — no new token is issued, so the client
    /// simply clears its local flag and proceeds.
    fn force_change_pw_ui(&mut self, ui: &mut egui::Ui) {
        if !self.must_change_pw {
            return;
        }
        let mut submit = false;
        egui::Modal::new(egui::Id::new("force-change-password")).show(ui.ctx(), |ui| {
            ui.set_width(360.0);
            ui.label(
                egui::RichText::new(tr("修改密码"))
                    .strong()
                    .size(19.0)
                    .color(palette_color(TEXT_MAIN)),
            );
            ui.add_space(8.0);
            ui.label(
                egui::RichText::new(tr(
                    "当前账号仍在使用初始或临时密码，必须先修改密码才能继续使用。",
                ))
                .size(13.5)
                .color(palette_color(TEXT_MAIN)),
            );
            ui.add_space(10.0);
            ui.label(egui::RichText::new(tr("当前密码")).size(12.5));
            ui.add(
                egui::TextEdit::singleline(&mut self.pw_change_old)
                    .password(true)
                    .desired_width(360.0),
            );
            ui.add_space(6.0);
            ui.label(egui::RichText::new(tr("新密码")).size(12.5));
            ui.add(
                egui::TextEdit::singleline(&mut self.pw_change_new)
                    .password(true)
                    .desired_width(360.0),
            );
            ui.add_space(6.0);
            ui.label(egui::RichText::new(tr("再次输入新密码")).size(12.5));
            ui.add(
                egui::TextEdit::singleline(&mut self.pw_change_new2)
                    .password(true)
                    .desired_width(360.0),
            );
            if !self.pw_change_error.is_empty() {
                ui.add_space(8.0);
                ui.label(
                    egui::RichText::new(&self.pw_change_error)
                        .size(12.5)
                        .color(palette_color(DANGER_BG)),
                );
            }
            ui.add_space(14.0);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if btn_primary(ui, tr("确认修改")).clicked() {
                    submit = true;
                }
            });
        });
        if submit {
            self.submit_change_pw();
        }
    }

    fn submit_change_pw(&mut self) {
        if self.pw_change_new.is_empty() {
            self.pw_change_error = "新密码不能为空".into();
            return;
        }
        if self.pw_change_new != self.pw_change_new2 {
            self.pw_change_error = "两次输入的新密码不一致".into();
            return;
        }
        match self.call(
            "POST",
            "/v1/me/password",
            Some(json!({
                "current_password": self.pw_change_old,
                "new_password": self.pw_change_new,
            })),
        ) {
            Ok(_v) => {
                // G2 contract: no token comes back — the current
                // session stays valid and the server has cleared
                // the must-change flag (other sessions of this
                // user were logged out server-side).
                self.must_change_pw = false;
                self.pw_change_old.clear();
                self.pw_change_new.clear();
                self.pw_change_new2.clear();
                self.pw_change_error.clear();
                self.say("密码已修改");
            }
            Err(e) => self.pw_change_error = format!("修改失败：{e}"),
        }
    }

    /// The status surface: no status bar — a floating toast where
    /// the prototype puts it: top-right, 14px below the 100px
    /// chrome (top = --chrome-h + 14 = 114), --panel3 fill with a
    /// --line2 outline, 8px radius, 13px text, gone after 3.4
    /// seconds. `msg` remains the client's only message channel;
    /// the toast stamps it when it changes.
    fn toast(&mut self, ui: &mut egui::Ui, _screen: egui::Rect) {
        let now = ui.ctx().input(|i| i.time);
        if self.msg != self.toast_msg || self.toast_nonce != self.toast_seen_nonce {
            self.toast_msg = self.msg.clone();
            self.toast_seen_nonce = self.toast_nonce;
            self.toast_since = now;
            self.workspace_controls
                .record_message(&self.msg, now, self.page != Page::Messages);
        }
        if self.toast_msg.is_empty() || now - self.toast_since > 3.4 {
            return;
        }
        egui::Area::new(egui::Id::new("qimu_toast"))
            .anchor(egui::Align2::RIGHT_TOP, egui::vec2(-16.0, 82.0))
            .order(egui::Order::Foreground)
            .show(ui.ctx(), |ui| {
                egui::Frame::NONE
                    .fill(palette_color(PRESSED_BG))
                    .stroke(egui::Stroke::new(1.0, palette_color(FIELD_BORDER)))
                    .corner_radius(egui::CornerRadius::same(8))
                    .shadow(egui::epaint::Shadow {
                        offset: [0, 14],
                        blur: 34,
                        spread: 0,
                        color: egui::Color32::from_black_alpha(128),
                    })
                    .inner_margin(egui::Margin {
                        left: 12,
                        right: 12,
                        top: 7,
                        bottom: 7,
                    })
                    .show(ui, |ui| {
                        ui.label(
                            egui::RichText::new(&self.toast_msg)
                                .size(13.0)
                                .color(palette_color(TEXT_MAIN)),
                        );
                    });
            });
        ui.ctx()
            .request_repaint_after(std::time::Duration::from_millis(500));
    }
}

impl eframe::App for App {
    /// T2 frame forwarding, part 1: while a hosted show runs, the
    /// guest SlideApp's logic drives the frame (show clocks, media
    /// sync). Inert otherwise — this App was ui-only before T2, and
    /// the default trait logic is a no-op, so non-hosted behavior
    /// is unchanged.
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        if let Some(h) = &mut self.hosted {
            h.logic(ctx);
        }
    }

    /// T2 frame forwarding, part 2: the guest's input hook, exactly
    /// as DeckCraft's own desktop host forwards it.
    fn raw_input_hook(&mut self, ctx: &egui::Context, raw: &mut egui::RawInput) {
        if let Some(h) = &mut self.hosted {
            h.raw_input_hook(ctx, raw);
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        // M5 frame timer (design §4.1): logs frames over 50ms.
        let _perf_frame = PerfFrame::new();
        // T2 hosted show: while the guest show is live it owns the
        // whole window (SlideApp::ui short-circuits to the show, so
        // no guest editor chrome is drawn either) and qimu's own
        // UI is not drawn at all. On the first frame the show has
        // ended, drop the guest and restore qimu's fonts/visuals;
        // qimu's drill stack was never touched, so the user is back
        // on the exact frame they left.
        if let Some(mut h) = self.hosted.take() {
            if h.ui(ui) {
                self.hosted = Some(h);
            } else {
                hosted_show::HostedShow::restore_host_ui(ui.ctx());
                self.msg = "放映结束，已回到启幕".into();
            }
            return;
        }
        let ctx = ui.ctx().clone();
        self.egui_ctx = Some(ctx.clone());
        self.frames += 1;
        // Collect finished thumbnail fetches and upload them as textures.
        while let Ok((key, bytes)) = self.thumb_rx.try_recv() {
            let state = bytes
                .and_then(|b| image::load_from_memory(&b).ok())
                .map(|img| {
                    let rgba = img.to_rgba8();
                    let (w, h) = rgba.dimensions();
                    let ci = egui::ColorImage::from_rgba_unmultiplied(
                        [w as usize, h as usize],
                        rgba.as_raw(),
                    );
                    ThumbState::Ready(ctx.load_texture(&key, ci, egui::TextureOptions::LINEAR))
                })
                .unwrap_or_else(|| ThumbState::Failed(std::time::Instant::now()));
            self.thumbs.insert(key, state);
            ctx.request_repaint();
        }
        // Collect finished PDF page renders for the attachment preview.
        while let Ok((att_id, seq, page, bytes, res)) = self.pdf_rx.try_recv() {
            if let Some(v) = &mut self.pdf_view {
                if v.att_id == att_id && v.seq == seq {
                    if bytes.is_some() {
                        v.bytes = bytes;
                    }
                    match res {
                        Ok(img) => {
                            v.pages = img.pages;
                            v.page = page.min(img.pages.saturating_sub(1));
                            let ci =
                                egui::ColorImage::from_rgba_unmultiplied([img.w, img.h], &img.rgba);
                            v.tex = Some(ctx.load_texture(
                                format!("pdf:{att_id}:{page}"),
                                ci,
                                egui::TextureOptions::LINEAR,
                            ));
                            v.note = String::new();
                        }
                        Err(e) => v.note = format!("预览失败：{e}"),
                    }
                }
            }
            ctx.request_repaint();
        }
        // Collect a finished LAN update-mirror check (prompt only).
        while let Ok((manual, res)) = self.upd_rx.try_recv() {
            match res {
                Ok(Some(note)) => {
                    self.msg = note.clone();
                    self.upd_note = Some(note);
                }
                Ok(None) => {
                    self.upd_note = None;
                    if manual {
                        self.msg = "已是最新版本（局域网镜像未提供更新）".into();
                    }
                }
                Err(e) => {
                    if manual {
                        self.msg = format!("检查更新失败：{e}");
                    }
                }
            }
            ctx.request_repaint();
        }
        // Collect a finished folder delete (worker thread; the
        // dialog closed the moment the user picked a mode — see
        // folder_delete_window). Applies exactly what the old
        // synchronous path applied on completion.
        while let Ok((gen, id, mode, res)) = self.folder_del_rx.try_recv() {
            if gen == self.folder_del_gen {
                self.folder_deleting = None;
                match res {
                    Ok(_) => {
                        self.msg = if mode == "trash" {
                            "已删除文件夹，内容已移到回收站".into()
                        } else {
                            "已删除文件夹".into()
                        };
                        if self.content.folder_filter.as_deref() == Some(id.as_str()) {
                            self.content.folder_filter = None;
                        }
                        if self.showroom.folder.as_deref() == Some(id.as_str()) {
                            self.showroom.folder = None;
                        }
                        if self.index.overview_folder.as_deref() == Some(id.as_str()) {
                            self.index.overview_folder = None;
                        }
                        // Plaza assignments homed in the deleted
                        // folder follow the same lift/trash
                        // semantics (read before the refresh
                        // below reloads the folder list).
                        self.apply_plaza_folder_delete(&id, &mode);
                        // Contents moved (lift) or vanished into
                        // the trash (trash): the folder tree and
                        // the unit / index lists are stale either
                        // way — re-read exactly those (plus the
                        // trash page's freshness in trash mode).
                        // The old answer here was refresh_all,
                        // whose org sweep and whole-library cost
                        // on this UI-thread drain were the
                        // remaining delete freeze (round 2).
                        self.refresh_after_folder_delete(&mode);
                    }
                    Err(e) => self.msg = format!("删除失败：{e}"),
                }
            }
            ctx.request_repaint();
        }
        // Collect AI-translation progress / completion.
        while let Ok(ev) = self.translate_rx.try_recv() {
            match ev {
                TranslateEvent::Progress(g, d, t) => {
                    if g == self.translate_gen {
                        if let Some(tr) = &mut self.translating {
                            tr.1 = d;
                            tr.2 = t;
                        }
                    }
                }
                TranslateEvent::Done(g, res) => {
                    // A worker from before an account switch polls
                    // the old server with the old token; its result
                    // belongs to that context and is dropped here
                    // (audit H1).
                    if g != self.translate_gen {
                        continue;
                    }
                    let finished = self.translating.take();
                    match res {
                        Ok(v) => {
                            let runs = i(&v, "translated_runs");
                            let gloss = v.get("glossary");
                            let gget = |k: &str| {
                                gloss
                                    .and_then(|g| g.get(k))
                                    .and_then(|x| x.as_i64())
                                    .unwrap_or(0)
                            };
                            let mut m = if s(&v, "provider") == "stub" {
                                format!("已生成 stub 占位草稿（{runs} 个文本；未配置真实 AI，仅供流程预览，待校对）")
                            } else {
                                format!("译文草稿已生成（{runs} 个文本，待校对）")
                            };
                            if gget("repaired") > 0 {
                                m += &format!("；术语已强制修正 {} 处", gget("repaired"));
                            }
                            if gget("unresolved") > 0 {
                                m += &format!("；术语未命中 {} 处，请人工校对", gget("unresolved"));
                            }
                            if let Some((uid, _, _)) = finished {
                                // Reload in place ONLY when the
                                // translated block's detail frame is
                                // the one on screen (audit H1): from
                                // anywhere else, navigating there
                                // would pop the frame the user is on
                                // and silently discard its unsaved
                                // drafts. The translation is saved
                                // server-side; opening the block
                                // later reads it fresh.
                                let on_frame = matches!(
                                    self.content.stack.top(),
                                    ContentFrame::Unit(st) if st.unit_id == uid
                                );
                                if on_frame {
                                    self.say(m);
                                    self.reload_unit_detail(&uid);
                                } else {
                                    self.say(format!(
                                        "{m}（译文已写入该内容块，打开时即为最新）"
                                    ));
                                }
                            } else {
                                self.say(m);
                            }
                        }
                        Err(e) => self.say(format!("翻译失败：{e}")),
                    }
                }
            }
            ctx.request_repaint();
        }
        // Collect import events (worker thread — the import
        // never runs on the UI thread; see act_import_ppt). An
        // `Opened` beat refreshes the wall onto the freshly
        // landed pages but leaves the import marked as running;
        // only `Done` clears it.
        while let Ok(ev) = self.import_rx.try_recv() {
            match ev {
                ImportEvent::Opened(g, m, units) => {
                    if g == self.import_gen {
                        self.say(m);
                        self.import_open_units = units;
                        // Non-blocking snapshot refresh: the
                        // grouping stage may still hold the store
                        // lock, and this frame must not park on
                        // it (freeze audit F3); a missed snapshot
                        // retries via open_refresh_pending below.
                        self.refresh_opened();
                    }
                    // One beat per frame: a `Done` already
                    // queued behind this `Opened` is handled on
                    // the next frame, so the just-opened cards
                    // get a painted frame of their own instead
                    // of being folded into the completion
                    // refresh (both events draining in one frame
                    // is what made every page appear at once).
                    break;
                }
                ImportEvent::Done(g, Ok(m)) => {
                    if g == self.import_gen {
                        self.importing = None;
                        self.open_refresh_pending = false;
                        self.import_open_units.clear();
                        self.say(m);
                        // Phase-1 (design §3.1 point 3): the
                        // event payload carries no new-index id
                        // set, so IndexMeta goes in its full form;
                        // the wall-selection prune rides along in
                        // apply_dirty's local group. (Gray-off
                        // mode refreshed synchronously above, so
                        // it still prunes here.)
                        self.changed(Change::Units);
                        self.changed(Change::Indexes);
                        self.changed(Change::IndexMeta);
                        if !refresh_v2_enabled() {
                            self.prune_wall_selection();
                        }
                    }
                }
                ImportEvent::Done(g, Err(m)) => {
                    if g == self.import_gen {
                        self.importing = None;
                        self.open_refresh_pending = false;
                        self.import_open_units.clear();
                        // Grouping lands block by block and does
                        // NOT roll back on a mid-way failure
                        // (audit N1): a bare "import failed" would
                        // misreport the library's actual state.
                        // Refresh so the wall shows what really
                        // landed, and say so.
                        self.say(format!(
                            "{m}（已落库的部分不会回滚，墙面已按实际状态刷新）"
                        ));
                        // Phase-1 (design §3.1 point 4): a failed
                        // import may still have landed stage-1
                        // parts — conservatively mark both lists,
                        // but no IndexMeta.
                        self.changed(Change::Units);
                        self.changed(Change::Indexes);
                        if !refresh_v2_enabled() {
                            self.prune_wall_selection();
                        }
                    }
                }
                ImportEvent::LazyOpened(pending_id) => {
                    // The deck is open: its pages are on the wall
                    // (refresh), the split state is adopted, and
                    // the split worker takes it from here. The
                    // "importing" latch only covered the open.
                    self.importing = None;
                    self.lazy_sync_pending(&pending_id);
                    self.lazy_ensure_worker();
                    self.lazy_send(LazyCmd::Kick);
                    // Phase-1 (design §3.1 point 5): the opened
                    // beat formally homes in the event set as
                    // LocalWork consumed at once — the snapshot
                    // path, with the F3 non-blocking semantics.
                    self.changed_now(Change::LocalWork);
                }
                ImportEvent::LazyProgress(info) => {
                    self.lazy_on_progress(info);
                }
            }
            ctx.request_repaint();
        }
        // Phase-1 consumption point (design §2.3/§2.5): every
        // frame, after all channel drains and before drawing,
        // consume the dirty set once. The open_refresh_pending
        // latch is generalized to "local dirt awaiting retry":
        // a snapshot that failed on the store lock keeps its
        // bits, and the 50ms tick below re-runs apply_dirty
        // until it lands — the old refresh_opened retry, widened
        // to the whole local group.
        self.apply_dirty();
        if self.open_refresh_pending {
            // The grouping stage still holds the store lock;
            // retry on a short tick rather than ever blocking a
            // frame on that lock.
            ctx.request_repaint_after(std::time::Duration::from_millis(50));
        }
        if self.importing.is_some() {
            // Keep the frame loop ticking while an import runs so
            // its completion is collected promptly (and the window
            // visibly stays alive) even with no user input.
            ctx.request_repaint_after(std::time::Duration::from_millis(200));
        }
        // Lazy import viewport feed: the unsplit pages the wall
        // rendered LAST frame are queued ahead of the background
        // order (they also render first — a visible card asks
        // for its thumbnail as it is drawn). One frame of lag is
        // inherent and harmless.
        if !self.lazy_visible_boost.is_empty()
            && self.lazy_visible_boost != self.lazy_visible_sent
        {
            let pages: Vec<(String, usize)> = self
                .lazy_visible_boost
                .iter()
                .filter_map(|uid| self.lazy_unit_page.get(uid).cloned())
                .collect();
            if !pages.is_empty() {
                self.lazy_ensure_worker();
                self.lazy_send(LazyCmd::Visible(pages));
            }
            self.lazy_visible_sent = self.lazy_visible_boost.clone();
        }
        self.lazy_visible_boost.clear();
        if !self.lazy_pendings.is_empty() {
            // Keep frames coming while a split runs so progress
            // (and cards flipping usable) lands promptly even
            // with no user input; the worker also wakes frames
            // itself after every page.
            ctx.request_repaint_after(std::time::Duration::from_millis(200));
        }
        // Collect a finished LAN discovery scan, if one is running.
        if self.disc_rx.is_some() {
            let got = self.disc_rx.as_ref().and_then(|rx| rx.try_recv().ok());
            if let Some(found) = got {
                self.discovered = found;
                self.disc_scanning = false;
                self.disc_rx = None;
                self.msg = format!("局域网搜索完成：发现 {} 台服务器", self.discovered.len());
            }
            ctx.request_repaint();
        }
        // DeckCraft handoff: watcher events (show ended /
        // instance gone) and child-exit bookkeeping.
        self.dc_poll();
        if self.dc.is_some() {
            ctx.request_repaint_after(std::time::Duration::from_millis(500));
        }
        // Self-screenshot mode for headless verification (QIMU_SCREENSHOT).
        // egui 0.37 delivers screenshots to a callback instead of an
        // input event; request at frame 240, close a few frames later
        // once the callback has written the file.
        if let Some(path) = self.screenshot_path.clone() {
            if !self.screenshot_done && self.frames == 240 {
                self.screenshot_done = true;
                ctx.request_screenshot(move |image| {
                    if let Some(buf) = image::RgbaImage::from_raw(
                        image.width() as u32,
                        image.height() as u32,
                        image.as_raw().to_vec(),
                    ) {
                        let _ = buf.save(&path);
                    }
                });
            }
            if self.screenshot_done && self.frames >= 245 {
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
            ctx.request_repaint();
        }

        let screen = ui.max_rect();
        self.workspace_ui(ui);
        self.toast(ui, screen);
        self.login_window(ui);
        self.about_window(ui);
        self.overlay_stack_ui(ui, screen);
        self.force_change_pw_ui(ui);
    }
}

impl App {
    /// Real slide thumbnail, resolved on a background thread: a
    /// version that lives in the local library is rendered
    /// in-process (transplant T0, local_render) and served from the
    /// shared deckcraft-v2 cache; anything else is fetched from the
    /// server, which renders it once via DeckCraft and caches it on
    /// disk under the same layout.
    fn thumb(&mut self, version_id: &str, page: usize) -> Option<egui::TextureHandle> {
        if version_id.is_empty() {
            return None;
        }
        let key = format!("{version_id}:{page}");
        match self.thumbs.get(&key) {
            Some(ThumbState::Ready(t)) => Some(t.clone()),
            Some(ThumbState::Loading) => None,
            Some(ThumbState::Failed(at))
                if at.elapsed() < std::time::Duration::from_secs(30) =>
            {
                None
            }
            _ => {
                self.thumbs.insert(key.clone(), ThumbState::Loading);
                let (server, token, tx) = (
                    self.server.clone(),
                    self.token.clone(),
                    self.thumb_tx.clone(),
                );
                let url = format!("{server}/v1/versions/{version_id}/thumb/{page}");
                // Transplant T0: with the local library open, try the
                // in-process render first; a version that is not
                // local (org content) or a local failure falls back
                // to the server fetch below, unchanged.
                let local = self.local.clone();
                let bundle = self.cache_bundle_block(version_id);
                let version_id = version_id.to_string();
                std::thread::spawn(move || {
                    // Audit2 E-M2: the whole fetch runs under
                    // catch_unwind — a panic in here (a poisoned
                    // store lock inside the render path, a renderer
                    // edge) used to kill the worker silently and
                    // pin the card at Loading forever. A panic now
                    // reports as an ordinary failure, which the
                    // bounded retry in thumb() re-attempts.
                    let work = std::panic::catch_unwind(std::panic::AssertUnwindSafe(
                        || -> Option<Vec<u8>> {
                            if let Some(local) = &local {
                                if let Some(bytes) =
                                    local_render::local_thumb(local, &version_id, page)
                                {
                                    return Some(bytes);
                                }
                            }
                            // Offline cache bundle (T1): a cached deck's
                            // blocks render in-process from their cached
                            // fragments when the server is unreachable.
                            if let Some((frag_file, thumb_dir)) = &bundle {
                                let frag = cache_key().and_then(|ck| {
                                    std::fs::read(frag_file)
                                        .ok()
                                        .and_then(|data| decrypt_bytes(&ck, &data))
                                });
                                if let Some(frag) = frag {
                                    if let Some(bytes) =
                                        local_render::cached_thumb(&frag, thumb_dir, page)
                                    {
                                        return Some(bytes);
                                    }
                                }
                            }
                            let mut req = thumb_agent().get(&url);
                            if let Some(t) = &token {
                                req = req.set("Authorization", &format!("Bearer {t}"));
                            }
                            req.call()
                                .ok()
                                .and_then(|r| r.into_json::<Value>().ok())
                                .and_then(|v| {
                                    v.get("image_base64")
                                        .and_then(|x| x.as_str())
                                        .map(str::to_string)
                                })
                                .and_then(|s| b64d(&s).ok())
                        },
                    ));
                    let _ = tx.send((key, work.unwrap_or(None)));
                });
                None
            }
        }
    }

    fn thumb_ui(&mut self, ui: &mut egui::Ui, version_id: &str, page: usize, width: f32) {
        let h = width * 9.0 / 16.0;
        if let Some(tex) = self.thumb(version_id, page) {
            ui.image((tex.id(), egui::vec2(width, h)));
        } else {
            ui.allocate_ui(egui::vec2(width, h), |ui| {
                egui::Frame::NONE
                    .fill(palette_color(HOVER_BG))
                    .show(ui, |ui| {
                        ui.set_min_size(egui::vec2(width, h));
                        ui.centered_and_justified(|ui| {
                            ui.label(
                                egui::RichText::new(format!("第 {} 页", page + 1))
                                    .color(palette_color(TEXT_WEAK)),
                            );
                        });
                    });
            });
        }
    }

    /// Thumbnail for one page of a staged submission fragment
    /// (F1 review preview): the same worker/channel machinery as
    /// `thumb`, keyed by fragment hash; the worker fetches the
    /// fragment bytes from the submission's own staging zone
    /// (GET /v1/submissions/:id/files/:hash) and renders them
    /// in-process, so the preview shows the package's pages —
    /// never a library version's. Once the submission is decided
    /// its staging is cleared (F5) and pages simply stay
    /// unrendered.
    fn sub_thumb(
        &mut self,
        sub_id: &str,
        hash: &str,
        page: usize,
    ) -> Option<egui::TextureHandle> {
        if hash.is_empty() {
            return None;
        }
        let key = format!("sub:{hash}:{page}");
        match self.thumbs.get(&key) {
            Some(ThumbState::Ready(t)) => Some(t.clone()),
            Some(_) => None,
            None => {
                self.thumbs.insert(key.clone(), ThumbState::Loading);
                let (server, token, tx) = (
                    self.server.clone(),
                    self.token.clone(),
                    self.thumb_tx.clone(),
                );
                let url = format!("{server}/v1/submissions/{sub_id}/files/{hash}");
                let dir = cache_dir()
                    .join("subprev")
                    .join(hash.chars().take(16).collect::<String>());
                std::thread::spawn(move || {
                    let mut req = ureq::get(&url);
                    if let Some(t) = &token {
                        req = req.set("Authorization", &format!("Bearer {t}"));
                    }
                    let out = req
                        .call()
                        .ok()
                        .and_then(|r| r.into_json::<Value>().ok())
                        .and_then(|v| {
                            v.get("data_base64")
                                .and_then(|x| x.as_str())
                                .map(str::to_string)
                        })
                        .and_then(|b64| b64d(&b64).ok())
                        .and_then(|frag| local_render::cached_thumb(&frag, &dir, page));
                    let _ = tx.send((key, out));
                });
                None
            }
        }
    }

    fn sub_thumb_ui(
        &mut self,
        ui: &mut egui::Ui,
        sub_id: &str,
        hash: &str,
        page: usize,
        width: f32,
    ) {
        let h = width * 9.0 / 16.0;
        if let Some(tex) = self.sub_thumb(sub_id, hash, page) {
            ui.image((tex.id(), egui::vec2(width, h)));
        } else {
            ui.allocate_ui(egui::vec2(width, h), |ui| {
                egui::Frame::NONE
                    .fill(palette_color(HOVER_BG))
                    .show(ui, |ui| {
                        ui.set_min_size(egui::vec2(width, h));
                        ui.centered_and_justified(|ui| {
                            ui.label(
                                egui::RichText::new(format!("第 {} 页", page + 1))
                                    .color(palette_color(TEXT_WEAK)),
                            );
                        });
                    });
            });
        }
    }

    /// Clickable thumbnail (quick-switch strip / version popup). The
    /// currently resolved version gets an accent outline.
    fn thumb_button(
        &mut self,
        ui: &mut egui::Ui,
        version_id: &str,
        page: usize,
        width: f32,
        current: bool,
    ) -> bool {
        self.thumb_response(ui, version_id, page, width, current)
            .clicked()
    }

    fn thumb_response(
        &mut self,
        ui: &mut egui::Ui,
        version_id: &str,
        page: usize,
        width: f32,
        current: bool,
    ) -> egui::Response {
        let h = width * 9.0 / 16.0;
        let (rect, resp) = ui.allocate_exact_size(egui::vec2(width, h), egui::Sense::click());
        if let Some(tex) = self.thumb(version_id, page) {
            ui.painter().image(
                tex.id(),
                rect,
                egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                egui::Color32::WHITE,
            );
        } else {
            ui.painter().rect_filled(rect, 0.0, palette_color(HOVER_BG));
        }
        if current {
            ui.painter().rect_stroke(
                rect,
                0.0,
                egui::Stroke::new(1.5, ACCENT),
                egui::StrokeKind::Middle,
            );
        } else if resp.hovered() {
            // hover outline sits in the selected-fill tier (#2E2F36),
            // not the text gray — a #9E9E9E stroke reads as a bright
            // white line against the dark panels.
            ui.painter().rect_stroke(
                rect,
                0.0,
                egui::Stroke::new(1.0, palette_color(FIELD_BORDER)),
                egui::StrokeKind::Middle,
            );
        }
        resp
    }

    /// Switch one index entry to a specific version: picking the latest
    /// restores follow-latest; a historical pick locks the entry.
    fn switch_entry_version(
        &mut self,
        pos: usize,
        version_id: &str,
        semver: &str,
        is_latest: bool,
    ) {
        let mut es = self.current_entries();
        if pos >= es.len() {
            return;
        }
        if is_latest {
            es[pos]["policy"] = json!("follow_latest");
            es[pos]["locked_semver"] = Value::Null;
        } else {
            es[pos]["policy"] = json!({ "locked": { "version_id": version_id } });
            es[pos]["locked_semver"] = json!(semver);
        }
        self.commit_entries(es, "切换版本");
    }

    /// Export the open index to a user-chosen file. kind: personal|company
    /// (§3.14 — the server resolves or blocks on personal placeholders).
    fn export_index_file(&mut self, index: &Value, kind: &str) {
        let (id, branch, include_notes) = match self.index.stack.top() {
            IndexFrame::Detail(d) => (d.index_id.clone(), d.branch.clone(), d.include_notes),
            _ => return,
        };
        let Some(path) = rfd::FileDialog::new()
            .set_file_name(format!("{}.pptx", s(index, "name")))
            .save_file()
        else {
            return;
        };
        let r = self.call_work(
            "POST",
            &format!("/v1/indexes/{id}/export"),
            Some(json!({
                "branch": branch,
                "kind": kind,
                "include_notes": include_notes,
            })),
        );
        match r {
            Ok(v) => {
                if let Some(data) = v.get("data_base64").and_then(|x| x.as_str()) {
                    if let Ok(bytes) = b64d(data) {
                        // Audit2 E-M10: the write result decides the message — a failed write no longer reports success.
                        self.msg = match std::fs::write(&path, bytes) {
                            Ok(()) => format!(
                                "已导出 {} 页（{}）",
                                v.get("page_count").and_then(|x| x.as_i64()).unwrap_or(0),
                                if kind == "company" {
                                    "公用版"
                                } else {
                                    "个人版"
                                }
                            ),
                            Err(e) => format!("导出失败：{e}"),
                        };
                        self.load_index_detail();
                    }
                }
            }
            Err(e) => self.msg = format!("导出失败：{e}"),
        }
    }

    /// Write an adopted snapshot into the local offline cache (§3.17).
    /// Administrator policy wins: caching can be disabled server-side and
    /// is capacity-capped; snapshots are stored AES-256-GCM encrypted.
    fn cache_deck(&mut self, key: &str, title: &str, bytes: &[u8], page_count: i64) {
        if self.cache_policy.get("allowed").and_then(|v| v.as_bool()) == Some(false) {
            self.msg = "管理员已关闭离线缓存".into();
            return;
        }
        let max_mb = self
            .cache_policy
            .get("max_mb")
            .and_then(|v| v.as_i64())
            .unwrap_or(500);
        let used: i64 = self
            .cache_entries
            .iter()
            .filter(|e| s(e, "key") != key)
            .map(|e| i(e, "bytes"))
            .sum();
        if used + bytes.len() as i64 > max_mb * 1024 * 1024 {
            self.msg = format!("超过管理员设定的缓存容量上限（{max_mb} MB）");
            return;
        }
        let Some(ck) = cache_key() else {
            self.msg = "无法创建本地缓存密钥".into();
            return;
        };
        let Some(enc) = encrypt_bytes(&ck, bytes) else {
            self.msg = "缓存加密失败".into();
            return;
        };
        let dir = cache_dir();
        let _ = std::fs::create_dir_all(&dir);
        let safe: String = key
            .chars()
            .map(|c| {
                if c.is_alphanumeric() || c == '-' || c == '_' {
                    c
                } else {
                    '_'
                }
            })
            .collect();
        let file = format!("{safe}.pptx.enc");
        match craft_atomic::atomic_write(&dir.join(&file), &enc) {
            Ok(_) => {
                self.cache_entries.retain(|e| s(e, "key") != key);
                self.cache_entries.push(json!({
                    "key": key,
                    "title": title,
                    "page_count": page_count,
                    "file": file,
                    "bytes": bytes.len() as i64,
                    "enc": true,
                    "cached_ms": qimu_core::auth::now_ms(),
                }));
                if save_cache_index(&self.cache_entries) {
                    self.msg = format!("已缓存到本地（加密）：{title}");
                } else {
                    // Audit2 E-M4: the index did not persist — the
                    // entry would vanish on restart, so do not
                    // claim the cache; drop the in-memory entry
                    // again (the encrypted file stays behind as an
                    // unindexed orphan, like a failed first cache).
                    self.cache_entries.retain(|e| s(e, "key") != key);
                    self.msg = "缓存失败：缓存索引保存失败".into();
                }
            }
            Err(e) => self.msg = format!("缓存失败：{e}"),
        }
    }

    /// The full offline cache bundle for one showroom deck (T1
    /// ruling: 缓存 = 索引 + 内容块（页面）): the assembled deck
    /// file, the source index's detail JSON, and every resolved
    /// block's fragment — each AES-256-GCM encrypted exactly like
    /// the deck-only snapshot, and all of it counted against the
    /// same administrator capacity policy. The entry records every
    /// file it owns, so removal and the cache manager's totals
    /// stay truthful. It also records the pin fingerprint of the
    /// formal version it captured (`pin_fp`): a later re-pin or
    /// takedown marks the entry stale (see
    /// `reconcile_cache_staleness`) instead of letting the bundle
    /// pose as the current formal version forever.
    fn cache_deck_bundle(
        &mut self,
        key: &str,
        title: &str,
        deck: &[u8],
        page_count: i64,
        source_index_id: &str,
        index_json: &[u8],
        blocks: &[(String, String, Vec<u8>, i64)],
        pin_fp: &str,
    ) {
        if self.cache_policy.get("allowed").and_then(|v| v.as_bool()) == Some(false) {
            self.msg = "管理员已关闭离线缓存".into();
            return;
        }
        let total: i64 = deck.len() as i64
            + index_json.len() as i64
            + blocks.iter().map(|b| b.2.len() as i64).sum::<i64>();
        let max_mb = self
            .cache_policy
            .get("max_mb")
            .and_then(|v| v.as_i64())
            .unwrap_or(500);
        let used: i64 = self
            .cache_entries
            .iter()
            .filter(|e| s(e, "key") != key)
            .map(|e| i(e, "bytes"))
            .sum();
        if used + total > max_mb * 1024 * 1024 {
            self.msg = format!("超过管理员设定的缓存容量上限（{max_mb} MB）");
            return;
        }
        let Some(ck) = cache_key() else {
            self.msg = "无法创建本地缓存密钥".into();
            return;
        };
        let dir = cache_dir();
        let _ = std::fs::create_dir_all(&dir);
        let safe = |name: &str| -> String {
            name.chars()
                .map(|c| {
                    if c.is_alphanumeric() || c == '-' || c == '_' {
                        c
                    } else {
                        '_'
                    }
                })
                .collect()
        };
        let safe_key = safe(key);
        // Two-phase publish (N5): every file is first written
        // (encrypted) under a `.part` name; only when ALL writes
        // succeed are the parts renamed over the live names and
        // the cache entry swapped (below). A mid-bundle failure
        // — disk full, IO error — therefore leaves the previous
        // bundle, files and entry alike, exactly as it was,
        // instead of the old shape: new deck/index files under
        // an old entry whose block list still names the previous
        // versions, silently read as one consistent bundle.
        let nonce = format!("{}-{}", qimu_core::auth::now_ms(), std::process::id());
        let write_part = |name: &str, plain: &[u8]| -> Result<std::path::PathBuf, String> {
            let enc = encrypt_bytes(&ck, plain).ok_or_else(|| "缓存加密失败".to_string())?;
            let part = dir.join(format!("{name}.part-{nonce}"));
            craft_atomic::atomic_write(&part, &enc).map_err(|e| e.to_string())?;
            Ok(part)
        };
        let deck_file = format!("{safe_key}.pptx.enc");
        let index_file = format!("{safe_key}.index.enc");
        let mut block_entries: Vec<Value> = Vec::new();
        let mut staged: Vec<(std::path::PathBuf, std::path::PathBuf)> = Vec::new();
        let mut result: Result<(), String> = Ok(());
        match write_part(&deck_file, deck) {
            Ok(p) => staged.push((p, dir.join(&deck_file))),
            Err(e) => result = Err(e),
        }
        if result.is_ok() {
            match write_part(&index_file, index_json) {
                Ok(p) => staged.push((p, dir.join(&index_file))),
                Err(e) => result = Err(e),
            }
        }
        if result.is_ok() {
            for (vid, unit_id, frag, pages) in blocks {
                let file = format!("{safe_key}--{}.frag.enc", safe(vid));
                match write_part(&file, frag) {
                    Ok(p) => {
                        staged.push((p, dir.join(&file)));
                        block_entries.push(json!({
                            "version_id": vid,
                            "unit_id": unit_id,
                            "file": file,
                            "bytes": frag.len() as i64,
                            "page_count": pages,
                        }));
                    }
                    Err(e) => {
                        result = Err(e);
                        break;
                    }
                }
            }
        }
        if result.is_ok() {
            // Publish: rename every staged part over its live
            // name. Same-directory renames replace atomically;
            // a failure here is the one residual window (some
            // files new, entry not yet swapped) — it needs a
            // failing rename after every write succeeded.
            for (part, fin) in &staged {
                if let Err(e) = std::fs::rename(part, fin) {
                    result = Err(e.to_string());
                    break;
                }
            }
        }
        if result.is_err() {
            for (part, _) in &staged {
                let _ = std::fs::remove_file(part);
            }
        }
        match result {
            Ok(_) => {
                self.cache_entries.retain(|e| s(e, "key") != key);
                self.cache_entries.push(json!({
                    "key": key,
                    "title": title,
                    "page_count": page_count,
                    "file": deck_file,
                    "bytes": total,
                    "enc": true,
                    "cached_ms": qimu_core::auth::now_ms(),
                    "source_index_id": source_index_id,
                    "index_file": index_file,
                    "blocks": block_entries,
                    "pin_fp": pin_fp,
                }));
                // Audit2 E-M4: an index that failed to save must
                // not report success — roll the in-memory entry
                // back out so the cache does not claim a bundle
                // the next launch will never find.
                if save_cache_index(&self.cache_entries) {
                    self.msg = format!(
                        "已缓存到本地（加密）：{title}（含索引与 {} 个内容块）",
                        blocks.len()
                    );
                } else {
                    self.cache_entries.retain(|e| s(e, "key") != key);
                    self.msg = "缓存失败：缓存索引保存失败".into();
                }
            }
            Err(e) => self.msg = format!("缓存失败：{e}"),
        }
    }

    /// Organization-face reachability (T3): the active
    /// connection is in a live session state — a named session or
    /// an anonymous guest session, the same condition
    /// `sync_from_active` mirrors the token on. Anything else (no
    /// active connection, a failed probe, a logged-out session)
    /// means the showroom's server-backed walls cannot be reached
    /// right now. The reachability latch (`org_unreachable`,
    /// audit issue 4) is the second conjunct: a session that was
    /// live but whose server has stopped answering is offline.
    fn org_online(&self) -> bool {
        // The session status says who we are; the reachability
        // latch says whether the server has answered lately
        // (audit issue 4). Both must hold.
        !self.org_unreachable.get()
            && self
                .active
                .and_then(|i| self.conns.get(i))
                .is_some_and(|c| {
                    matches!(c.status, ConnStatus::Connected | ConnStatus::Anonymous)
                })
    }

    fn require_org_online(&mut self, action: &str) -> bool {
        if self.org_online() {
            return true;
        }
        self.msg = format!(
            "{action}：{}",
            tr("需要连接服务器；离线内容仍可打开。")
        );
        false
    }

    /// True when the item's offline cache is the complete bundle
    /// (T1 cache payload: index + blocks): the entry carries its
    /// source index file, the same marker `cached_index_detail`
    /// keys on, and its `blocks` array rides along in the same
    /// entry. A deck-only entry (an older snapshot, an assembly
    /// export) does not count — offline it cannot open its
    /// detail or drill into blocks. A stale entry (the item was
    /// re-pinned or taken down after the bundle was captured —
    /// see `reconcile_cache_staleness`) does not count either:
    /// offline it must not present itself as the item's current
    /// formal version. The entry's `bytes` is the whole bundle's
    /// total, so capacity and the cache manager already account
    /// for the bundle as one unit.
    fn sr_bundle_cached(&self, item_id: &str) -> bool {
        let key = format!("sr_{item_id}");
        let dir = cache_dir();
        self.cache_entries.iter().any(|e| {
            s(e, "key") == key
                && !s(e, "index_file").is_empty()
                && e.get("stale").and_then(|v| v.as_bool()) != Some(true)
                // Audit2 E-M7: the entry record alone is not proof
                // enough — verify the files it names are really on
                // disk, so a bundle whose files were lost or moved
                // reads as "not cached" (and can simply be
                // re-cached) instead of failing only when opened
                // offline.
                && dir.join(s(e, "file")).is_file()
                && dir.join(s(e, "index_file")).is_file()
        })
    }

    /// Cache-versus-formal-version reconciliation (pin audit
    /// N3), run whenever a fresh live showroom list lands. Each
    /// showroom bundle recorded the pin fingerprint of the
    /// formal version it captured; the entry goes stale when the
    /// item's current fingerprint differs (a re-push or a
    /// re-landing re-pinned it) or when the item no longer
    /// appears in the live list at all (rejected, recalled or
    /// deleted — takedown reaches the offline face here, at the
    /// next successful refresh; a client that never reconnects
    /// never learns, which is the protocol's limit). A matching
    /// fingerprint clears the mark again. Entries recorded
    /// before fingerprints existed carry none and are left alone
    /// — unverifiable, declared as such.
    fn reconcile_cache_staleness(&mut self) {
        let mut changed = false;
        for e in self.cache_entries.iter_mut() {
            let key = s(e, "key");
            let Some(item_id) = key.strip_prefix("sr_") else {
                continue;
            };
            let recorded = s(e, "pin_fp");
            if recorded.is_empty() {
                continue;
            }
            let stale = match self
                .showroom_live
                .iter()
                .find(|it| s(it, "id") == item_id)
            {
                Some(it) => {
                    let now = item_pin_fingerprint(it);
                    !now.is_empty() && now != recorded
                }
                None => true,
            };
            let was = e.get("stale").and_then(|v| v.as_bool()).unwrap_or(false);
            if stale != was {
                e["stale"] = json!(stale);
                changed = true;
            }
        }
        if changed {
            save_cache_index(&self.cache_entries);
        }
    }

    /// Plaza wall backfill (2026-11-25): the wall list comes
    /// from the live showroom list only, so a cold start with
    /// no reachable server rendered an empty wall even when
    /// the machine holds complete offline cache bundles. When
    /// no live list has been fetched this session (the list is
    /// still empty), seed it from the cache index's showroom
    /// bundles (`sr_*` entries): the wall gets a floor of what
    /// this machine actually holds, and each card keeps the
    /// existing offline semantics — a complete bundle opens
    /// offline through the cached detail cascade in
    /// `open_showroom_deck`, an incomplete or stale one reads
    /// through `sr_bundle_cached` and the persisted stale flag
    /// exactly as a card in an in-session list does. A
    /// successful live fetch replaces the list wholesale (the
    /// refresh paths own that assignment), so backfilled rows
    /// never mix into or duplicate live rows; card selection
    /// is keyed by item id, so it survives the swap.
    fn backfill_showroom_from_cache(&mut self) {
        if !self.showroom_live.is_empty() {
            return;
        }
        // The on-disk index is the floor's source: reload it
        // so a backfill after a failed refresh also sees
        // bundles cached earlier in this session. Every cache
        // mutation persists the index, so this never rolls
        // the in-memory state back.
        self.cache_entries = load_cache_index();
        let mut seen = std::collections::HashSet::new();
        let mut items = Vec::new();
        for e in &self.cache_entries {
            let key = s(e, "key");
            let Some(item_id) = key.strip_prefix("sr_") else {
                continue;
            };
            if item_id.is_empty() || !seen.insert(item_id.to_string()) {
                continue;
            }
            items.push(json!({
                "id": item_id,
                "title": s(e, "title"),
                "source_index_id": s(e, "source_index_id"),
                "created_ms": i(e, "cached_ms"),
            }));
        }
        if items.is_empty() {
            return;
        }
        self.showroom_live = items;
        // Seed covers from the bundles' cached index details —
        // the same first-resolved-version extraction the live
        // cover pass performs — so a complete bundle's card
        // shows its cover offline instead of a blank.
        for item in self.showroom_live.clone() {
            let src = s(&item, "source_index_id");
            if src.is_empty() || self.index_covers.contains_key(&src) {
                continue;
            }
            if let Some(d) = self.cached_index_detail(&s(&item, "id")) {
                if let Some(vid) = d
                    .get("resolved_entries")
                    .and_then(|r| r.as_array())
                    .and_then(|a| a.first())
                    .and_then(|e| e.get("resolved"))
                    .and_then(|r| r.get("id"))
                    .and_then(|x| x.as_str())
                {
                    self.index_covers.insert(src, vid.to_string());
                }
            }
        }
    }

    /// The cached index detail for a showroom item, when its
    /// offline bundle carries one (T1 cache payload): this is
    /// what lets a cached deck's detail — its page list and
    /// speaker scripts — resolve with no server at all.
    fn cached_index_detail(&self, item_id: &str) -> Option<Value> {
        let key = format!("sr_{item_id}");
        let e = self.cache_entries.iter().find(|e| s(e, "key") == key)?;
        let file = s(e, "index_file");
        if file.is_empty() {
            return None;
        }
        let ck = cache_key()?;
        let data = std::fs::read(cache_dir().join(file)).ok()?;
        let plain = decrypt_bytes(&ck, &data)?;
        serde_json::from_slice(&plain).ok()
    }

    /// A block version's fragment inside an offline cache bundle:
    /// (encrypted fragment path, thumbnail directory), resolved by
    /// the background thumbnail thread when neither the local
    /// library nor the server can serve the version.
    fn cache_bundle_block(
        &self,
        version_id: &str,
    ) -> Option<(std::path::PathBuf, std::path::PathBuf)> {
        for e in &self.cache_entries {
            if let Some(blocks) = e.get("blocks").and_then(|b| b.as_array()) {
                for b in blocks {
                    if s(b, "version_id") == version_id {
                        let file = s(b, "file");
                        if file.is_empty() {
                            continue;
                        }
                        let dir = cache_dir();
                        return Some((dir.join(file), dir.join("thumbs").join(version_id)));
                    }
                }
            }
        }
        None
    }

    fn remove_cache(&mut self, key: &str) {
        if let Some(e) = self
            .cache_entries
            .iter()
            .find(|e| s(e, "key") == key)
            .cloned()
        {
            let _ = std::fs::remove_file(cache_dir().join(s(&e, "file")));
            // Bundle pieces (T1 cache payload) go with the deck.
            let index_file = s(&e, "index_file");
            if !index_file.is_empty() {
                let _ = std::fs::remove_file(cache_dir().join(index_file));
            }
            if let Some(blocks) = e.get("blocks").and_then(|b| b.as_array()) {
                for b in blocks {
                    let f = s(b, "file");
                    if !f.is_empty() {
                        let _ = std::fs::remove_file(cache_dir().join(f));
                    }
                    // Audit2 E-L7: the bundle's rendered thumbnails
                    // for this block version go with it — they were
                    // never counted in the entry's bytes and used to
                    // outlive every removal, unaccounted forever.
                    let vid = s(b, "version_id");
                    if !vid.is_empty() {
                        let _ = std::fs::remove_dir_all(
                            cache_dir().join("thumbs").join(vid),
                        );
                    }
                }
            }
            // Audit2 E-M6: this entry's decrypted scratch copy in
            // open/ (named by key since the M11 fix) goes too.
            let open_dir = cache_dir().join("open");
            if let Ok(rd) = std::fs::read_dir(&open_dir) {
                let prefix = format!("{}-", key);
                for ent in rd.flatten() {
                    let name = ent.file_name().to_string_lossy().into_owned();
                    if name.starts_with(&prefix) && name.ends_with(".pptx") {
                        let _ = std::fs::remove_file(ent.path());
                    }
                }
            }
        }
        self.cache_entries.retain(|e| s(e, "key") != key);
        save_cache_index(&self.cache_entries);
        self.msg = "已移除缓存".into();
    }

    fn open_cache(&mut self, key: &str) {
        let Some(e) = self
            .cache_entries
            .iter()
            .find(|e| s(e, "key") == key)
            .cloned()
        else {
            return;
        };
        let stored = cache_dir().join(s(&e, "file"));
        // encrypted entries are decrypted to a scratch copy for opening
        let path = if e.get("enc").and_then(|v| v.as_bool()) == Some(true) {
            let Some(ck) = cache_key() else {
                self.msg = "缺少本地缓存密钥，无法打开".into();
                return;
            };
            let data = match std::fs::read(&stored)
                .ok()
                .and_then(|d| decrypt_bytes(&ck, &d))
            {
                Some(d) => d,
                None => {
                    self.msg = "缓存解密失败".into();
                    return;
                }
            };
            let open_dir = cache_dir().join("open");
            let _ = std::fs::create_dir_all(&open_dir);
            // Audit2 E-M11: the title is peer-controlled text and
            // used to be the file name verbatim — a title with `/`
            // or `../` wrote the decrypted copy outside open/, and
            // same-title decks overwrote each other. The name is
            // the entry key plus a sanitized title suffix (both
            // reduced to filesystem-safe characters).
            let sanitize = |t: &str| -> String {
                t.chars()
                    .map(|c| {
                        if c.is_alphanumeric() || c == '-' || c == '_' {
                            c
                        } else {
                            '_'
                        }
                    })
                    .collect()
            };
            let p = open_dir.join(format!(
                "{}-{}.pptx",
                sanitize(key),
                sanitize(&s(&e, "title"))
            ));
            if std::fs::write(&p, data).is_err() {
                self.msg = "写出临时副本失败".into();
                return;
            }
            // Audit2 E-M6: these plaintext copies are scratch, not
            // an archive — sweep the directory's other decks (the
            // same discipline export_for_present applies to
            // present/) instead of accumulating decrypted copies
            // forever. A copy still open in another app simply
            // fails to delete on Windows and is retried next time.
            if let Ok(rd) = std::fs::read_dir(&open_dir) {
                for ent in rd.flatten() {
                    let other = ent.path();
                    if other != p
                        && other.extension().map(|x| x == "pptx").unwrap_or(false)
                    {
                        let _ = std::fs::remove_file(other);
                    }
                }
            }
            p
        } else {
            stored
        };
        // Local decks present in-process first (T2 hosted
        // show), then via the external DeckCraft jump; the system
        // app stays as the final fallback.
        if self.present_local_via_deckcraft(&path, &s(&e, "title")) {
            return;
        }
        let r = if cfg!(target_os = "windows") {
            std::process::Command::new("cmd")
                .args(["/C", "start", ""])
                .arg(&path)
                .spawn()
        } else if cfg!(target_os = "macos") {
            std::process::Command::new("open").arg(&path).spawn()
        } else {
            std::process::Command::new("xdg-open").arg(&path).spawn()
        };
        self.msg = match r {
            Ok(_) => format!("已打开本地副本：{}", path.display()),
            Err(e) => format!("打开失败：{e}（文件在 {}）", path.display()),
        };
    }

    /// Materialize a showroom deck for a DeckCraft jump (or for the
    /// system-app path): the export always carries the speaker
    /// scripts as PPTX notes (`include_notes`) — without them the
    /// DeckCraft presenter view has an empty notes pane (the design's
    /// §1.5 finding: the pre-handoff client never sent this flag, so
    /// exported decks had no notes). The file is a disposable copy
    /// under ~/.qimu/present/, named by item id + a content stamp so
    /// a same-title re-export never reads back a stale deck; older
    /// copies in the directory are swept (best effort — a copy still
    /// open in DeckCraft simply fails to delete on Windows).
    fn export_for_present(&mut self, item_id: &str) -> Result<(std::path::PathBuf, i64), String> {
        let v = self.call(
            "POST",
            &format!("/v1/showroom/{item_id}/export"),
            Some(json!({ "kind": "personal", "include_notes": true })),
        )?;
        let data = v
            .get("data_base64")
            .and_then(|x| x.as_str())
            .ok_or("导出响应缺少稿件数据")?;
        let bytes = b64d(data)?;
        let page_count = i(&v, "page_count");
        let dir = qimu_home().join("present");
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        let path = dir.join(format!("{item_id}-{}.pptx", dc_content_stamp(&bytes)));
        std::fs::write(&path, &bytes).map_err(|e| e.to_string())?;
        // Audit2 E-L4: the sweep used to delete every other copy in
        // present/ — including the file the live DeckCraft session
        // is showing right now (its session record names it), which
        // an external reader re-reading by path would lose mid-show.
        let live_file = self.dc.as_ref().map(|sess| sess.file.clone());
        if let Ok(rd) = std::fs::read_dir(&dir) {
            for e in rd.flatten() {
                let p = e.path();
                if p != path
                    && live_file.as_deref() != Some(&p)
                    && p.extension().map(|x| x == "pptx").unwrap_or(false)
                {
                    let _ = std::fs::remove_file(p);
                }
            }
        }
        Ok((path, page_count))
    }

    /// Start an in-process hosted show (T2, DeckCraft transplant
    /// strategy (b)) on deck bytes and make it the App's hosted
    /// guest: from the next frame the eframe App forwards logic /
    /// raw_input_hook / ui to it until its show ends. The display
    /// count is NOT injected (D1 final ruling, 2026-11-24 14:24:
    /// upstream Windows factory behavior — DeckCraft's own Windows
    /// build never injects it either — so Presenter View opens on
    /// a single screen too). Load damage is reported in the status
    /// message instead of degrading silently.
    fn start_hosted_show(
        &mut self,
        title: &str,
        bytes: &[u8],
        mode: DcMode,
        from: usize,
    ) -> Result<(), String> {
        let show = hosted_show::HostedShow::start(title, bytes, mode, from, None)?;
        let d = show.damage;
        self.hosted = Some(show);
        let mut msg = match mode {
            DcMode::Play => format!("已开始演示：{title}"),
            DcMode::Reading => format!("已打开普通视图：{title}"),
            DcMode::Presenter => format!("已开始演示：{title}（演示者视图）"),
        };
        let mut notes = Vec::new();
        if d.lost_slides > 0 {
            notes.push(format!("{} 页未能读取", d.lost_slides));
        }
        if d.damaged_media > 0 {
            notes.push(format!("{} 处媒体损坏", d.damaged_media));
        }
        if d.missing_fonts > 0 {
            notes.push(format!("{} 个字体不可用", d.missing_fonts));
        }
        if d.import_warnings > 0 {
            notes.push(format!("{} 条导入警告", d.import_warnings));
        }
        if !notes.is_empty() {
            msg += &format!("（注意：{}）", notes.join("、"));
        }
        self.msg = msg;
        Ok(())
    }

    /// Present a showroom item (T2). The export always carries the
    /// speaker scripts as PPTX notes (`include_notes`, see
    /// export_for_present) — without them the presenter view has an
    /// empty notes pane; permission refusals arrive as export
    /// errors, so a refused item never presents (contract §2.2,
    /// unchanged). The primary path is the in-process hosted show
    /// on the exported bytes; when hosting cannot start, the
    /// external DeckCraft jump (dc_jump) remains as the fallback,
    /// and when no DeckCraft desktop app is on this machine either,
    /// the user is prompted to install it (ruling 2026-11-24). The
    /// hosted failure is always surfaced, never silent.
    fn present_via_deckcraft(
        &mut self,
        item_id: &str,
        title: &str,
        mode: DcMode,
        from: usize,
    ) -> bool {
        let (path, page_count) = match self.export_for_present(item_id) {
            Ok(v) => v,
            Err(e) => {
                // Stays `true` on purpose (audit2 E-N2 disposition):
                // an export refusal — no permission, takedown — is
                // a *refused* presentation, not a failed engine.
                // Returning false here would route a deck the user
                // may not present into the built-in presenter and
                // defeat the refusal (the permission-refusal
                // discipline: gates sit at the action point).
                self.msg = format!("演示失败：{e}");
                return true;
            }
        };
        self.open_deck_views_path(title, &path, page_count, mode, from)
    }

    /// DeckCraft as the PRIMARY engine （设置「放映打开方式」=
    /// DeckCraft, ruling 2026-11-25): the external jump, promoted
    /// from its fallback seat. A missing DeckCraft app or a failed
    /// jump is reported and ends the open — the user's chosen
    /// engine never degrades silently into the hosted show, so
    /// every outcome returns true ("handled": callers must not
    /// drop into the built-in presenter behind the user's back).
    /// `dc_jump`'s own failure messages stay as they are.
    fn open_via_deckcraft_primary(
        &mut self,
        path: &std::path::Path,
        title: &str,
        mode: DcMode,
        from: usize,
    ) -> bool {
        if deckcraft_app_path().is_none() {
            self.msg = "未检测到 DeckCraft 桌面应用：放映打开方式已设为 DeckCraft，请先安装 DeckCraft 后再试（或在 设置 → 演示偏好 改回内置放映）".into();
            return true;
        }
        self.dc_jump(path, title, mode, from);
        true
    }

    /// PowerPoint as the engine （设置「放映打开方式」=
    /// PowerPoint, ruling 2026-11-25): the deck bytes — already
    /// resolved with notes by the caller's export path — land in
    /// the cache directory under a content stamp (the same naming
    /// discipline as the present/ copies, so a re-open never reads
    /// back a stale deck), and the bridge opens them in
    /// PowerPoint: 播放 starts the slide show, 演示者视图 also
    /// asks for the presenter view, 普通视图 opens without a
    /// show. Any failure is reported and ends the open; no silent
    /// switch to another engine, so every outcome returns true.
    fn open_via_powerpoint(
        &mut self,
        title: &str,
        bytes: &[u8],
        mode: DcMode,
        from: usize,
    ) -> bool {
        let dir = cache_dir();
        let path = dir.join(format!("present-{}.pptx", dc_content_stamp64(bytes)));
        if let Err(e) =
            std::fs::create_dir_all(&dir).and_then(|()| std::fs::write(&path, bytes))
        {
            self.msg = format!("PowerPoint 打开失败：写出演示文件失败：{e}");
            return true;
        }
        let pmode = match mode {
            DcMode::Play => qimu_bridge::PresentMode::Play,
            DcMode::Reading => qimu_bridge::PresentMode::Reading,
            DcMode::Presenter => qimu_bridge::PresentMode::Presenter,
        };
        match qimu_bridge::platform_bridge().open_and_present(&path, pmode, from) {
            Ok(()) => {
                self.msg = match mode {
                    DcMode::Play => format!("已在 PowerPoint 开始演示：{title}"),
                    DcMode::Reading => format!("已在 PowerPoint 打开：{title}"),
                    DcMode::Presenter => {
                        format!("已在 PowerPoint 开始演示：{title}（演示者视图）")
                    }
                };
            }
            Err(e) => {
                self.msg = format!("PowerPoint 打开失败：{e}");
            }
        }
        true
    }

    /// THE unified view opener (views unification, user ruling
    /// 2026-11-25): the bottom-bar views — 播放 / 普通视图 /
    /// 演示者视图 — are one action, "open an index", and every
    /// object kind resolves to deck bytes + a title and lands
    /// here (a single page is a one-page index, a content block a
    /// multi-page index, an index an index of blocks, a showroom
    /// deck a published index, a history version a frozen page
    /// sequence). The three views are only this entry's mode
    /// parameter. `start_hosted_show` is the one show engine; the
    /// external DeckCraft jump is the next fallback. A non-default
    /// 设置「放映打开方式」pre-empts that order at the top of
    /// this entry (and of its path-fed twin): the chosen engine
    /// takes the open outright, with its own failure reporting.
    /// When NEITHER
    /// engine could start the show this returns false (audit2
    /// E-N2 disposition): showroom callers then fall back to the
    /// built-in presenter, and the doors without a fallback wired
    /// (units, indexes) surface the reason left in `msg` — the
    /// hosted failure is always surfaced, never silent. The
    /// showroom path's discipline is preserved exactly: the start
    /// page is clamped to the deck.
    fn open_deck_views(
        &mut self,
        title: &str,
        bytes: &[u8],
        page_count: i64,
        mode: DcMode,
        from: usize,
    ) -> bool {
        let from = if page_count > 0 {
            from.min(page_count as usize - 1)
        } else {
            0
        };
        // 设置「放映打开方式」routes the open before the default
        // engine order below runs (ruling 2026-11-25): DeckCraft
        // jumps as the primary path, PowerPoint takes the bytes
        // through the bridge. Both report their own failures and
        // never fall through to here.
        if self.present_engine == PresentEngine::DeckCraft {
            let dir = qimu_home().join("present");
            let path = dir.join(format!("open-{}.pptx", dc_content_stamp64(bytes)));
            if let Err(e) =
                std::fs::create_dir_all(&dir).and_then(|()| std::fs::write(&path, bytes))
            {
                self.msg = format!("DeckCraft 打开失败：写出演示副本失败：{e}");
                return true;
            }
            return self.open_via_deckcraft_primary(&path, title, mode, from);
        }
        if self.present_engine == PresentEngine::PowerPoint {
            return self.open_via_powerpoint(title, bytes, mode, from);
        }
        let hosted_err = match self.start_hosted_show(title, bytes, mode, from) {
            Ok(()) => return true,
            Err(e) => e,
        };
        if deckcraft_app_path().is_none() {
            self.msg = format!(
                "进程内放映不可用（{hosted_err}），且未检测到 DeckCraft 桌面应用：请先安装 DeckCraft 后再试"
            );
            return false;
        }
        // The jump needs a file: materialize a disposable copy
        // under the same present/ directory the showroom export
        // uses (its sweep collects these too), named by the
        // full-width content stamp so a re-open never reads back
        // a stale deck (audit2 E-N3).
        let dir = qimu_home().join("present");
        let path = dir.join(format!("open-{}.pptx", dc_content_stamp64(bytes)));
        if let Err(e) = std::fs::create_dir_all(&dir)
            .and_then(|()| std::fs::write(&path, bytes))
        {
            self.msg = format!("进程内放映不可用（{hosted_err}）；写出 DeckCraft 副本失败：{e}");
            return false;
        }
        let jumped = self.dc_jump(&path, title, mode, from);
        if jumped && self.msg.contains("已在 DeckCraft") {
            self.msg = format!("进程内放映不可用（{hosted_err}），已改用 DeckCraft 打开");
        }
        jumped
    }

    /// The path-fed door of the unified opener: the showroom
    /// resolver already materialized its deck (export_for_present
    /// — the pinned export, swept and stamped there), so the jump
    /// reuses that file instead of writing a second copy.
    fn open_deck_views_path(
        &mut self,
        title: &str,
        path: &std::path::Path,
        page_count: i64,
        mode: DcMode,
        from: usize,
    ) -> bool {
        let from = if page_count > 0 {
            from.min(page_count as usize - 1)
        } else {
            0
        };
        // 设置「放映打开方式」routes the open first (ruling
        // 2026-11-25), exactly as in the byte-fed door: the jump
        // reuses this door's already-materialized export file;
        // PowerPoint reads the bytes and re-stamps them into the
        // cache directory inside open_via_powerpoint.
        if self.present_engine == PresentEngine::DeckCraft {
            return self.open_via_deckcraft_primary(path, title, mode, from);
        }
        if self.present_engine == PresentEngine::PowerPoint {
            return match std::fs::read(path) {
                Ok(bytes) => self.open_via_powerpoint(title, &bytes, mode, from),
                Err(e) => {
                    self.msg = format!("PowerPoint 打开失败：读取导出稿失败：{e}");
                    true
                }
            };
        }
        let hosted_err = match std::fs::read(path) {
            Ok(bytes) => match self.start_hosted_show(title, &bytes, mode, from) {
                Ok(()) => return true,
                Err(e) => e,
            },
            Err(e) => format!("读取导出稿失败：{e}"),
        };
        if deckcraft_app_path().is_none() {
            self.msg = format!(
                "进程内放映不可用（{hosted_err}），且未检测到 DeckCraft 桌面应用：请先安装 DeckCraft 后再试"
            );
            return false;
        }
        let jumped = self.dc_jump(path, title, mode, from);
        if jumped && self.msg.contains("已在 DeckCraft") {
            self.msg = format!("进程内放映不可用（{hosted_err}），已改用 DeckCraft 打开");
        }
        jumped
    }

    /// Resolve ONE content-block version — current or a clicked
    /// history version — to deck bytes and open it in the views
    /// (views unification, user ruling 2026-11-25). The version
    /// named by the caller is the version opened: a history
    /// version never falls back to the rendition's latest. The
    /// local library assembles it from its page-reference
    /// sequence with the DB scripts as notes
    /// (`assemble_version_deck`); a version this library does not
    /// hold falls back to the REST fragment, with the scripts the
    /// detail payload already carries written back in-process —
    /// script saves update the DB only, so a bare fragment could
    /// otherwise present stale notes.
    fn present_unit_version(
        &mut self,
        title: &str,
        version: &Value,
        mode: DcMode,
        from: usize,
    ) -> bool {
        let version_id = s(version, "id");
        let page_count = i(version, "page_count");
        let local_outcome = self
            .local
            .as_ref()
            .and_then(|l| l.assemble_version_deck(&version_id));
        let bytes = match local_outcome {
            Some(Ok((bytes, _))) => bytes,
            Some(Err(e)) => {
                self.msg = format!("演示失败：{e}");
                return true;
            }
            None => {
                let resp = match self.call_work(
                    "GET",
                    &format!("/v1/versions/{version_id}/fragment"),
                    None,
                ) {
                    Ok(v) => v,
                    Err(e) => {
                        self.msg = format!("演示失败：{e}");
                        return true;
                    }
                };
                let Some(data) = resp.get("data_base64").and_then(|x| x.as_str()) else {
                    self.msg = "演示失败：片段响应缺少数据".into();
                    return true;
                };
                let Ok(raw) = b64d(data) else {
                    self.msg = "演示失败：片段数据损坏".into();
                    return true;
                };
                let scripts: Vec<String> = version
                    .get("scripts")
                    .and_then(|x| x.as_array())
                    .map(|arr| {
                        arr.iter()
                            .map(|t| t.as_str().unwrap_or("").to_string())
                            .collect()
                    })
                    .unwrap_or_default();
                if scripts.iter().any(|t| !t.is_empty()) {
                    match qimu_core::pptx::Package::load(&raw) {
                        Ok(mut pkg) => {
                            for (slot, script) in scripts.iter().enumerate() {
                                if !script.is_empty() {
                                    qimu_core::pptx::set_notes_text(
                                        &mut pkg,
                                        slot,
                                        script,
                                    );
                                }
                            }
                            pkg.to_bytes().unwrap_or(raw)
                        }
                        Err(_) => raw,
                    }
                } else {
                    raw
                }
            }
        };
        self.open_deck_views(title, &bytes, page_count, mode, from)
    }

    /// Resolve the open index detail to deck bytes and open it in
    /// the views (views unification, user ruling 2026-11-25): the
    /// branch the detail is standing on, at its head commit — the
    /// same resolution the detail's own export runs. The local
    /// library assembles in-process without registering an export
    /// (`assemble_index_deck`); an organization-side index takes
    /// the REST export path (which does register the export, as
    /// any export run from the detail does).
    fn present_index_views(&mut self, mode: DcMode) -> bool {
        let (index_id, branch, name) = match self.index.stack.top() {
            IndexFrame::Detail(d) => (
                d.index_id.clone(),
                if d.branch.is_empty() {
                    "main".to_string()
                } else {
                    d.branch.clone()
                },
                s(&d.detail["index"], "name"),
            ),
            _ => return false,
        };
        let title = if name.is_empty() {
            "索引".to_string()
        } else {
            name
        };
        let local_outcome = self
            .local
            .as_ref()
            .and_then(|l| l.assemble_index_deck(&index_id, &branch));
        let (bytes, page_count) = match local_outcome {
            Some(Ok(v)) => v,
            Some(Err(e)) => {
                self.msg = format!("演示失败：{e}");
                return true;
            }
            None => {
                let resp = match self.call_work(
                    "POST",
                    &format!("/v1/indexes/{index_id}/export"),
                    Some(json!({
                        "branch": branch,
                        "kind": "personal",
                        "include_notes": true,
                    })),
                ) {
                    Ok(v) => v,
                    Err(e) => {
                        self.msg = format!("演示失败：{e}");
                        return true;
                    }
                };
                let Some(data) = resp.get("data_base64").and_then(|x| x.as_str()) else {
                    self.msg = "演示失败：导出响应缺少稿件数据".into();
                    return true;
                };
                let Ok(bytes) = b64d(data) else {
                    self.msg = "演示失败：导出数据损坏".into();
                    return true;
                };
                (bytes, i(&resp, "page_count"))
            }
        };
        self.open_deck_views(&title, &bytes, page_count, mode, 0)
    }

    /// Present an already-local deck file (offline cached decks).
    /// Same order as the showroom path: the in-process hosted show
    /// on the file's bytes first, the external DeckCraft jump as
    /// fallback. Returns false when neither could take it — the
    /// caller then opens the file in the system app, as before T2.
    /// A non-default 设置「放映打开方式」routes this door too
    /// (ruling 2026-11-25) and always returns true: the chosen
    /// engine's outcome — success or a reported failure — never
    /// drops into the system-app fallback behind the user's back.
    fn present_local_via_deckcraft(&mut self, path: &std::path::Path, title: &str) -> bool {
        if self.present_engine == PresentEngine::DeckCraft {
            return self.open_via_deckcraft_primary(path, title, DcMode::Play, 0);
        }
        if self.present_engine == PresentEngine::PowerPoint {
            return match std::fs::read(path) {
                Ok(bytes) => self.open_via_powerpoint(title, &bytes, DcMode::Play, 0),
                Err(e) => {
                    self.msg = format!("PowerPoint 打开失败：读取本地稿失败：{e}");
                    true
                }
            };
        }
        let hosted_err = match std::fs::read(path) {
            Ok(bytes) => match self.start_hosted_show(title, &bytes, DcMode::Play, 0) {
                Ok(()) => return true,
                Err(e) => e,
            },
            Err(e) => format!("读取本地稿失败：{e}"),
        };
        if deckcraft_app_path().is_none() {
            return false;
        }
        // Audit2 E-N2: the jump's own verdict decides — a failed
        // jump now really reaches the caller's system-app fallback
        // instead of being reported as a successful presentation.
        let jumped = self.dc_jump(path, title, DcMode::Play, 0);
        if jumped && self.msg.contains("已在 DeckCraft") {
            self.msg = format!("进程内放映不可用（{hosted_err}），已改用 DeckCraft 打开");
        }
        jumped
    }

    /// Orchestrate one jump into the managed DeckCraft instance
    /// (design §2.3): reuse the instance when its control channel
    /// answers, otherwise launch it with `--control` on a free port;
    /// if the channel never comes up, degrade to a plain
    /// `deckcraft --show` launch and say so. The jump sequence is
    /// `app.open` → presenter flag for this mode → `show.start` →
    /// `ui.focus`; the presenter flag is set explicitly every jump
    /// because it is a persisted DeckCraft preference.
    /// Returns true when DeckCraft actually took the show
    /// (managed jump or the degraded plain launch); false when it
    /// could not be started at all — the doors above use this to
    /// hand the deck to the caller's own fallback (audit2 E-N2).
    fn dc_jump(&mut self, path: &std::path::Path, title: &str, mode: DcMode, from: usize) -> bool {
        let Some(bin) = deckcraft_app_path() else {
            self.msg = "未检测到 DeckCraft 桌面应用：播放、普通视图和演示者视图由 DeckCraft 提供，请先安装 DeckCraft 后再试".into();
            return false;
        };
        // Reuse probe: a fresh connection + inspect on the managed port.
        let mut live: Option<(DcControl, Value)> = None;
        if let Some(sess) = &mut self.dc {
            if let Ok(mut ctl) = DcControl::connect(sess.port) {
                if let Ok(v) = ctl.call("ui.inspect", json!({})) {
                    if sess.presenter_before.is_none() {
                        sess.presenter_before = v
                            .get("ui")
                            // UiState is #[serde(rename_all = "camelCase")]:
                            // the wire key is presenterView; keep the
                            // snake_case spelling as a fallback.
                            .and_then(|u| {
                                u.get("presenterView").or_else(|| u.get("presenter_view"))
                            })
                            .and_then(|x| x.as_bool());
                    }
                    sess.file = path.to_path_buf();
                    sess.auto_quit = self.dc_auto_quit;
                    live = Some((ctl, v));
                }
            }
            if live.is_none() {
                self.dc_forget(false);
            }
        }
        if live.is_none() {
            let Some(port) = dc_free_port() else {
                self.msg = "无法分配本地控制端口".into();
                return false;
            };
            let child = match std::process::Command::new(&bin)
                .arg("--control")
                .arg(port.to_string())
                .spawn()
            {
                Ok(c) => c,
                Err(e) => {
                    self.msg = format!("无法启动 DeckCraft：{e}");
                    return false;
                }
            };
            match dc_wait_ready(port) {
                Some((ctl, v)) => {
                    let presenter_before = v
                        .get("ui")
                        // UiState is #[serde(rename_all = "camelCase")]:
                        // the wire key is presenterView; keep the
                        // snake_case spelling as a fallback.
                        .and_then(|u| u.get("presenterView").or_else(|| u.get("presenter_view")))
                        .and_then(|x| x.as_bool());
                    self.dc = Some(DcSession {
                        child,
                        port,
                        presenter_before,
                        file: path.to_path_buf(),
                        auto_quit: self.dc_auto_quit,
                    });
                    live = Some((ctl, v));
                }
                None => {
                    // The channel never answered (e.g. the port was
                    // taken and DeckCraft only logged the failed
                    // bind): drop our instance and degrade to the
                    // plain command-line show.
                    let mut child = child;
                    let _ = child.kill();
                    let _ = child.wait();
                    return match std::process::Command::new(&bin)
                        .arg("--show")
                        .arg(path)
                        .spawn()
                    {
                        Ok(_) => {
                            self.msg = "已以降级模式在 DeckCraft 打开演示（控制通道不可用）".into();
                            true
                        }
                        Err(e) => {
                            self.msg = format!("无法启动 DeckCraft：{e}");
                            false
                        }
                    };
                }
            }
        }
        let (mut ctl, _inspect) = live.unwrap();
        let port = self.dc.as_ref().map(|s| s.port).unwrap_or(0);
        let run = (|| -> Result<(), String> {
            ctl.call(
                "app.open",
                json!({ "path": path.to_string_lossy().into_owned() }),
            )?;
            match mode {
                DcMode::Play => {
                    ctl.call(
                        "command",
                        json!({ "command": "show.presenter", "params": { "on": false } }),
                    )?;
                }
                DcMode::Presenter => {
                    ctl.call(
                        "command",
                        json!({ "command": "show.presenter", "params": { "on": true } }),
                    )?;
                }
                DcMode::Reading => {}
            }
            ctl.call(
                "show.start",
                json!({ "from": from, "reading": mode == DcMode::Reading }),
            )?;
            let _ = ctl.call("ui.focus", json!({}));
            Ok(())
        })();
        let started = match run {
            Ok(()) => {
                self.msg = match mode {
                    DcMode::Play => format!("已在 DeckCraft 开始演示：{title}"),
                    DcMode::Reading => format!("已在 DeckCraft 打开普通视图：{title}"),
                    DcMode::Presenter => format!(
                        "已在 DeckCraft 开始演示：{title}（演示者视图需第二显示器，将在副屏打开）"
                    ),
                };
                // A fresh watcher for this jump (auto-quit sessions
                // only); bumping the generation retires any watcher
                // from an earlier session.
                if self.dc_auto_quit {
                    let gen = self
                        .dc_gen
                        .fetch_add(1, std::sync::atomic::Ordering::SeqCst)
                        + 1;
                    let gen_arc = self.dc_gen.clone();
                    let tx = self.dc_tx.clone();
                    std::thread::spawn(move || dc_watch(port, gen_arc, gen, tx));
                }
                true
            }
            Err(e) => {
                self.msg = format!("DeckCraft 跳转失败：{e}");
                false
            }
        };
        started
    }

    /// Drop the managed-session record (retires its watcher via the
    /// generation counter). The DeckCraft process itself is left
    /// alone unless `kill` is set — dropping the Child handle does
    /// not kill it.
    fn dc_forget(&mut self, kill: bool) {
        self.dc_gen
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        if let Some(mut sess) = self.dc.take() {
            if kill {
                let _ = sess.child.kill();
            }
        }
    }

    /// Drain watcher events and child-exit bookkeeping; called once
    /// per frame from `eframe::App::ui` (next to the other channel
    /// drains). On show end with auto-quit on, the presenter flag is
    /// restored first, then `app.quit` closes DeckCraft gracefully —
    /// the frame stack underneath was never touched, so the user is
    /// back on the deck frame they left.
    fn dc_poll(&mut self) {
        while let Ok(ev) = self.dc_rx.try_recv() {
            match ev {
                DcEvent::ShowEnded(gen) => {
                    // Audit2 E-M9: a queued event from a superseded
                    // session must not restore flags on, or quit,
                    // the session that replaced it.
                    if gen != self.dc_gen.load(std::sync::atomic::Ordering::SeqCst) {
                        continue;
                    }
                    let auto = self.dc.as_ref().map(|s| s.auto_quit).unwrap_or(false);
                    if auto {
                        if let Some(sess) = &self.dc {
                            if let Ok(mut ctl) = DcControl::connect(sess.port) {
                                if let Some(prev) = sess.presenter_before {
                                    let _ = ctl.call(
                                        "command",
                                        json!({ "command": "show.presenter", "params": { "on": prev } }),
                                    );
                                }
                                let _ = ctl.call("app.quit", json!({}));
                            }
                        }
                        self.dc_forget(false);
                        self.msg = "放映结束，已回到启幕".into();
                    } else if self.dc.is_some() {
                        self.msg = "放映已结束（DeckCraft 仍开着这份稿件）".into();
                    }
                }
                DcEvent::Gone(gen) => {
                    if gen != self.dc_gen.load(std::sync::atomic::Ordering::SeqCst) {
                        continue;
                    }
                    let dead = match &self.dc {
                        Some(sess) => DcControl::connect(sess.port).is_err(),
                        None => false,
                    };
                    if dead {
                        self.dc_forget(false);
                        self.msg = "演示已结束（DeckCraft 已退出）".into();
                    }
                }
            }
        }
        let exited = match &mut self.dc {
            Some(sess) => matches!(sess.child.try_wait(), Ok(Some(_))),
            None => false,
        };
        if exited {
            self.dc_forget(false);
            self.msg = "演示已结束（DeckCraft 已退出）".into();
        }
    }

    /// 用系统应用打开 (§3.17/场景 38): materialize the deck and open
    /// it in the system presentation app. Secondary entry — the
    /// primary presentation path jumps into DeckCraft
    /// (present_via_deckcraft); this stays for PowerPoint/WPS, and
    /// now shares the notes-carrying export with the jump path.
    fn present_showroom(&mut self, item_id: &str, title: &str) {
        match self.export_for_present(item_id) {
            Ok((path, _)) => match spawn_open(&path) {
                Ok(_) => self.msg = format!("已打开演示：{title}（系统默认演示应用）"),
                Err(e) => self.msg = format!("演示文件已生成（{}），打开失败：{e}", path.display()),
            },
            Err(e) => self.msg = format!("演示失败：{e}"),
        }
    }

    /// Open the deck view for a library/inbox/group reference to a
    /// showroom entry: resolve the live entry when this account can see
    /// it, otherwise open with the reference's own fields (the deck view
    /// then states honestly that pages can't be loaded).
    fn open_deck_for_ref(&mut self, item_kind: &str, item_id: &str, title: &str) {
        if item_kind != "showroom" {
            self.msg = "该条目暂不支持逐页查看".into();
            return;
        }
        if let Some(it) = self
            .showroom_live
            .iter()
            .find(|x| s(x, "id") == item_id)
            .cloned()
        {
            self.open_showroom_deck(&it);
        } else {
            self.showroom.stack.pop_to(0);
            // Not in the live list (e.g. offline after a restart):
            // the offline cache bundle still knows this deck's
            // source index and its pages (T1 cache payload).
            let entry = self
                .cache_entries
                .iter()
                .find(|e| s(e, "key") == format!("sr_{item_id}"));
            let src = entry.map(|e| s(e, "source_index_id")).unwrap_or_default();
            if entry
                .map(|e| e.get("stale").and_then(|v| v.as_bool()) == Some(true))
                .unwrap_or(false)
            {
                // A stale bundle still opens — offline access is
                // the cache's point — but it must not present
                // itself as the item's current formal version.
                self.msg =
                    "本地缓存已过期（正式版已更新或条目已下架），打开的是旧版缓存"
                        .into();
            }
            let pages = self
                .cached_index_detail(item_id)
                .map(|v| showroom_pages(&v))
                .unwrap_or_default();
            self.showroom.stack.push(SrFrame::Deck(DeckState {
                item: json!({
                    "id": item_id, "title": title,
                    "source_index_id": src, "submitted_by": "",
                }),
                pages,
                selected_page: None,
                view: DeckView::Thumbnails,
                preview_open: false,
                closed: false,
            }));
        }
    }

    /// Open a showroom entry's whole-deck view (H): page through
    /// the item's resolved entries. The item's own payload is the
    /// primary source — the showroom list resolves it with the
    /// item's publish pins, so the deck view shows the formal
    /// version the push confirmed; the source index detail (the
    /// index layer's own face, following latest) is only a
    /// fallback for item payloads that carry no resolved rows.
    fn open_showroom_deck(&mut self, item: &Value) {
        self.note_recent("showroom", &s(item, "id"), &s(item, "title"), "");
        self.showroom.stack.pop_to(0);
        self.showroom.stack.push(SrFrame::Deck(DeckState {
            item: item.clone(),
            pages: Vec::new(),
            selected_page: None,
            view: DeckView::Thumbnails,
            preview_open: false,
            closed: false,
        }));
        let item_pages = showroom_pages(item);
        if !item_pages.is_empty() {
            if let SrFrame::Deck(d) = self.showroom.stack.top_mut() {
                d.pages = item_pages;
            }
        } else {
            let src = s(item, "source_index_id");
            if !src.is_empty() {
                if let Ok(v) = self.call("GET", &format!("/v1/indexes/{src}"), None) {
                    if let SrFrame::Deck(d) = self.showroom.stack.top_mut() {
                        d.pages = showroom_pages(&v);
                    }
                }
            }
        }
        // Offline: the deck's cache bundle carries the index, so
        // its pages (and their speaker scripts) still resolve.
        // The cache lookup borrows `self`, so it runs before the
        // mutable borrow of the deck frame; it stays gated on the
        // same condition, read through the immutable `top()`: the
        // top frame is a deck whose pages are still empty.
        let pages_empty = match self.showroom.stack.top() {
            SrFrame::Deck(d) => d.pages.is_empty(),
            _ => false,
        };
        let cached_pages = if pages_empty {
            self.cached_index_detail(&s(item, "id"))
                .map(|v| showroom_pages(&v))
        } else {
            None
        };
        if let Some(pages) = cached_pages {
            if let SrFrame::Deck(d) = self.showroom.stack.top_mut() {
                d.pages = pages;
            }
        }
    }

    /// Open the 讲稿视图 (prototype presentDlg) over the open
    /// deck frame: a full-page stream of the deck's pages, each
    /// page's script editable in place. The deck frame stays
    /// beneath untouched — closing this frame restores it
    /// exactly (drill standard: 浮层关闭回发起帧、状态恢复）.
    /// The save base (full per-version scripts) comes from the
    /// same payloads the deck's pages were resolved from, in the
    /// deck loader's own order: the item itself, the offline
    /// cache bundle, then the source index detail.
    fn open_showroom_present(&mut self) {
        let (item, pages) = match self.showroom.stack.top() {
            SrFrame::Deck(d) => (d.item.clone(), d.pages.clone()),
            _ => return,
        };
        if pages.is_empty() {
            self.msg = "该稿件暂无可查看的页面".into();
            return;
        }
        let mut scripts = version_scripts_map(&item);
        let mut titles = showroom_page_titles(&item);
        let uncovered = |scripts: &std::collections::HashMap<String, Vec<String>>| {
            pages.iter().any(|p| !scripts.contains_key(&p.version_id))
        };
        if uncovered(&scripts) || titles.len() < pages.len() {
            if let Some(v) = self.cached_index_detail(&s(&item, "id")) {
                for (k, arr) in version_scripts_map(&v) {
                    scripts.entry(k).or_insert(arr);
                }
                if titles.len() < pages.len() {
                    let t = showroom_page_titles(&v);
                    if t.len() > titles.len() {
                        titles = t;
                    }
                }
            }
        }
        if uncovered(&scripts) || titles.len() < pages.len() {
            let src = s(&item, "source_index_id");
            if !src.is_empty() {
                if let Ok(v) = self.call("GET", &format!("/v1/indexes/{src}"), None) {
                    for (k, arr) in version_scripts_map(&v) {
                        scripts.entry(k).or_insert(arr);
                    }
                    if titles.len() < pages.len() {
                        let t = showroom_page_titles(&v);
                        if t.len() > titles.len() {
                            titles = t;
                        }
                    }
                }
            }
        }
        titles.resize(pages.len(), String::new());
        let edits = pages.iter().map(|p| p.note.clone()).collect();
        self.showroom.stack.push(SrFrame::Present(SrPresentState {
            item,
            pages,
            titles,
            scripts,
            edits,
            script_on: true,
            saved_once: false,
            closed: false,
        }));
    }

    /// Commit one page's edited script （就地保存）: write the
    /// page's slot into its version's full scripts array and PUT
    /// it through the exact path the unit detail's scripts
    /// editor uses — saving never bumps a version. A failed save
    /// keeps the buffer text (the next focus loss retries) and
    /// reports through the status line.
    fn save_present_script(&mut self, st: &mut SrPresentState, idx: usize) {
        let Some(page) = st.pages.get(idx) else { return };
        let new_text = st.edits.get(idx).cloned().unwrap_or_default();
        if new_text == page.note {
            return;
        }
        let (vid, slot) = (page.version_id.clone(), page.slot);
        let Some(scripts) = st.scripts.get_mut(&vid) else {
            return;
        };
        if scripts.len() <= slot {
            scripts.resize(slot + 1, String::new());
        }
        scripts[slot] = new_text.clone();
        let payload = scripts.clone();
        match self.call_work(
            "PUT",
            &format!("/v1/versions/{vid}/scripts"),
            Some(json!({ "scripts": payload })),
        ) {
            Ok(_) => {
                st.pages[idx].note = new_text;
                if !st.saved_once {
                    st.saved_once = true;
                    self.msg = "已就地保存这一页。".into();
                }
            }
            Err(e) => self.msg = format!("讲稿保存失败：{e}"),
        }
    }

    /// Fold a popped 讲稿视图's page scripts back into the deck
    /// frame beneath it (same flattened order — the present
    /// frame's pages were cloned from that deck), so the detail
    /// the user returns to shows the edited scripts without a
    /// reload.
    fn sync_present_notes(&mut self, notes: Vec<String>) {
        if let SrFrame::Deck(d) = self.showroom.stack.top_mut() {
            for (page, note) in d.pages.iter_mut().zip(notes) {
                page.note = note;
            }
        }
    }

    /// Icon-only action button: an IconPark glyph (20px) inside a 30px
    /// hot zone, with a hover tooltip and an accessibility label — an
    /// icon button is never shipped without its name (hard floor).
    fn icon_button(&mut self, ui: &mut egui::Ui, icon: ChromeIcon, label: &str) -> bool {
        self.icon_button_tinted(ui, icon, label, palette_color(ICON_TINT))
    }

    /// The brand logo at `size` px (shell tool row, login popup,
    /// about dialog) — the real Q-ribbon image, not a drawn tile.
    fn logo_ui(&mut self, ui: &mut egui::Ui, size: f32) {
        let id = self.tex_icon(ui, "logo", *LOGO_PNG);
        ui.image((id, egui::vec2(size, size)));
    }

    /// Load the brand PNG into the texture cache once per client.
    fn tex_icon(
        &mut self,
        ui: &egui::Ui,
        name: &'static str,
        bytes: &'static [u8],
    ) -> egui::TextureId {
        if !self.icon_tex.contains_key(name) {
            let img = image::load_from_memory(bytes).expect("embedded icon png");
            let mut rgba = img.to_rgba8();
            if name != "logo" {
                for pixel in rgba.pixels_mut() {
                    pixel.0[..3].fill(255);
                }
            }
            let (w, h) = rgba.dimensions();
            let ci =
                egui::ColorImage::from_rgba_unmultiplied([w as usize, h as usize], rgba.as_raw());
            let tex = ui
                .ctx()
                .load_texture(name, ci, egui::TextureOptions::LINEAR);
            self.icon_tex.insert(name, tex);
        }
        self.icon_tex.get(name).expect("icon texture").id()
    }

    /// Same as `icon_button`, with a texture tint — used to weaken the
    /// glyph when it reports a state (e.g. already cached) rather than
    /// a fresh action.
    fn icon_button_tinted(
        &mut self,
        ui: &mut egui::Ui,
        icon: ChromeIcon,
        label: &str,
        tint: egui::Color32,
    ) -> bool {
        // craft-ui's IconButton (storytold/craft-libs) with its native
        // interaction model — hover/pressed/focus painting and the
        // accessibility response come from the component; qimu supplies
        // the style values and paints its IconPark glyph, as the
        // component is designed for ("application-supplied colors and
        // painting"). 30px hot zone, 20px glyph, tooltip as before.
        let visuals = |fill: egui::Color32| craft_ui::buttons::ButtonVisuals {
            fill,
            stroke: egui::Stroke::NONE,
            icon: tint,
        };
        let style = craft_ui::buttons::IconButtonStyle {
            normal: visuals(egui::Color32::TRANSPARENT),
            hovered: visuals(palette_color(ICON_HOVER)),
            pressed: None,
            selected: visuals(palette_color(ICON_HOVER)),
            corner_radius: egui::CornerRadius::same(6),
            focus_stroke: egui::Stroke::new(1.0, ACCENT),
        };
        let resp = craft_ui::buttons::IconButton::new(label, egui::vec2(30.0, 30.0), &style).show(
            ui,
            |ui, rect, icon_tint| {
                ChromeIcons.paint(ui, icon, rect.center(), 20.0, icon_tint);
            },
        );
        resp.on_hover_text(label).clicked()
    }

    /// Toolbar slot: a 28×26 craft-ui IconButton carrying an
    /// IconPark glyph at 15px — the prototype's .tool geometry
    /// (28×26, 6px radius, muted glyph at rest, --panel3 hover;
    /// the accent-filled selected state matches .ticon.on /
    /// .pagetabs.on). The label doubles as tooltip and
    /// accessibility name.
    fn tool_slot(
        &mut self,
        ui: &mut egui::Ui,
        active: bool,
        label: &str,
        icon: ChromeIcon,
    ) -> bool {
        self.tool_slot_sized(ui, active, label, icon, 26.0)
            .clicked()
    }

    fn tool_slot_sized(
        &mut self,
        ui: &mut egui::Ui,
        active: bool,
        label: &str,
        icon: ChromeIcon,
        height: f32,
    ) -> egui::Response {
        let visuals = |fill: egui::Color32, icon: egui::Color32| craft_ui::buttons::ButtonVisuals {
            fill,
            stroke: egui::Stroke::NONE,
            icon,
        };
        let style = craft_ui::buttons::IconButtonStyle {
            normal: visuals(egui::Color32::TRANSPARENT, palette_color(ICON_TINT)),
            hovered: visuals(palette_color(ICON_HOVER), palette_color(ICON_TINT)),
            pressed: None,
            selected: visuals(ACCENT, egui::Color32::WHITE),
            corner_radius: egui::CornerRadius::same(6),
            focus_stroke: egui::Stroke::new(1.0, ACCENT),
        };
        let resp = craft_ui::buttons::IconButton::new(label, egui::vec2(28.0, height), &style)
            .selected(active)
            .show(ui, |ui, rect, icon_tint| {
                ChromeIcons.paint(ui, icon, rect.center(), 15.0, icon_tint);
            });
        resp.on_hover_text(label)
    }

    /// Toolbar placeholder slot: a tool whose feature is not
    /// built yet, rendered in the tool row's disabled form
    /// (muted glyph at half strength, no fill, no click) with a
    /// hover tooltip that says so — the toolbar counterpart of
    /// shell.rs's deckbar_placeholder, at tool-slot geometry
    /// (28x26, 15px glyph). Painted by hand rather than through
    /// a disabled scope because a disabled scope swallows
    /// hover and the tooltip must still work.
    fn tool_slot_placeholder(&mut self, ui: &mut egui::Ui, label: &str, icon: ChromeIcon) {
        let (rect, resp) =
            ui.allocate_exact_size(egui::vec2(28.0, 26.0), egui::Sense::hover());
        let glyph = palette_color(TEXT_WEAK).gamma_multiply(0.5);
        ChromeIcons.paint(ui, icon, rect.center(), 15.0, glyph);
        resp.on_hover_text(label);
    }

    /// Panel-footer icon button: 28×28 craft-ui IconButton with a
    /// 15px glyph — the prototype's .sp-foot action scale (28px
    /// rows, 15px glyphs, 6px radius).
    fn small_icon_button(&mut self, ui: &mut egui::Ui, icon: ChromeIcon, label: &str) -> bool {
        let visuals = |fill: egui::Color32| craft_ui::buttons::ButtonVisuals {
            fill,
            stroke: egui::Stroke::NONE,
            icon: palette_color(ICON_TINT),
        };
        let style = craft_ui::buttons::IconButtonStyle {
            normal: visuals(egui::Color32::TRANSPARENT),
            hovered: visuals(palette_color(ICON_HOVER)),
            pressed: None,
            selected: visuals(palette_color(ICON_HOVER)),
            corner_radius: egui::CornerRadius::same(6),
            focus_stroke: egui::Stroke::new(1.0, ACCENT),
        };
        let resp = craft_ui::buttons::IconButton::new(label, egui::vec2(28.0, 28.0), &style).show(
            ui,
            |ui, rect, icon_tint| {
                ChromeIcons.paint(ui, icon, rect.center(), 15.0, icon_tint);
            },
        );
        resp.on_hover_text(label).clicked()
    }

    /// PPTX export for the showroom deck on screen: assembled
    /// in-process when the deck lives in the local library,
    /// otherwise the server assembles it; lands through a save
    /// dialog either way. This is also the current phase's
    /// export-style share path — the exported file is the share
    /// artifact (standalone ruling: sharing is export-only for
    /// now) — so the deck header's 导出 and the bottom bar's 分享
    /// call this same handler.
    fn export_showroom_pptx(&mut self, item: &Value) {
        // Local-first P4: a deck that lives in the local library
        // is assembled in-process (the same LocalStore assembly
        // the PDF path uses since T1) — standalone sharing depends
        // on it, there being no server to assemble for us. A local
        // assembly failure falls back to the server path below,
        // unchanged (T1 discipline).
        if let Some(local) = &self.local {
            match local.assemble_showroom_deck(&s(&item, "id"), false) {
                Some(Ok((bytes, _page_count))) => {
                    if let Some(path) = rfd::FileDialog::new()
                        .set_file_name(format!("{}.pptx", s(&item, "title")))
                        .save_file()
                    {
                        // Audit2 E-M10: the write result decides the message — a failed write no longer reports success.
                        self.msg = match std::fs::write(path, bytes) {
                            Ok(()) => "已保存".into(),
                            Err(e) => format!("保存失败：{e}"),
                        };
                    }
                    return;
                }
                Some(Err(e)) => {
                    eprintln!("[qimu] 本地组装稿件失败，回退服务端导出：{e}");
                }
                None => {}
            }
        }
        match self.call(
            "POST",
            &format!("/v1/showroom/{}/export", s(&item, "id")),
            Some(json!({ "kind": "personal" })),
        ) {
            Ok(v) => {
                if let Some(data) = v.get("data_base64").and_then(|x| x.as_str()) {
                    if let (Ok(bytes), Some(path)) = (
                        b64d(data),
                        rfd::FileDialog::new()
                            .set_file_name(format!("{}.pptx", s(&item, "title")))
                            .save_file(),
                    ) {
                        // Audit2 E-M10: the write result decides the message — a failed write no longer reports success.
                        self.msg = match std::fs::write(path, bytes) {
                            Ok(()) => "已保存".into(),
                            Err(e) => format!("保存失败：{e}"),
                        };
                    }
                }
            }
            Err(e) => self.msg = format!("导出失败：{e}"),
        }
    }

    /// PDF export for the showroom deck on screen (Q21): asks the
    /// server for the DeckCraft PDF (the whole deck in the chosen
    /// form, or one deck-global page) and saves it through the same
    /// file dialog flow as the PPTX export. Works in every session
    /// state the PPTX export works in, standalone included.
    fn export_showroom_pdf(&mut self, item: &Value, form: &str, page: Option<usize>) {
        let title = s(item, "title");
        let stem = match (form, page) {
            ("handout", _) => format!("{title}-讲义"),
            ("notes", _) => format!("{title}-备注页"),
            (_, Some(p)) => format!("{title}-第{}页", p + 1),
            _ => title.clone(),
        };
        // Transplant T1: when this deck lives in the local
        // library, assemble and export it in-process instead
        // (try_pdf_local); anything else keeps the server path
        // below, unchanged.
        if self.try_pdf_local(item, form, page, &stem, true) {
            return;
        }
        let mut body = json!({ "kind": "personal", "form": form });
        if let Some(p) = page {
            body["page"] = json!(p);
        }
        match self.call(
            "POST",
            &format!("/v1/showroom/{}/export-pdf", s(item, "id")),
            Some(body),
        ) {
            Ok(v) => {
                if let Some(data) = v.get("data_base64").and_then(|x| x.as_str()) {
                    if let (Ok(bytes), Some(path)) = (
                        b64d(data),
                        rfd::FileDialog::new()
                            .set_file_name(format!("{stem}.pdf"))
                            .save_file(),
                    ) {
                        // Audit2 E-M10: the write result decides the message — a failed write no longer reports success.
                        self.msg = match std::fs::write(path, bytes) {
                            Ok(()) => "已保存".into(),
                            Err(e) => format!("保存失败：{e}"),
                        };
                    }
                }
            }
            Err(e) => self.msg = format!("导出 PDF 失败：{e}"),
        }
    }

    /// Transplant T1 (DESIGN §2.2/§6): the local half of
    /// export_showroom_pdf. When the item lives in the local
    /// library, its deck is assembled in-process (LocalStore
    /// mirrors the server's build_export) and the PDF is rendered
    /// by deckcraft-pdf in this process — no server round-trip, no
    /// deckcraft-cli subprocess. Runs synchronously like the server
    /// call it replaces (the user waits for the save dialog either
    /// way). Returns true when the local path settled the request —
    /// saved, a raster-fallback Confirm queued, or a definitive
    /// local verdict (page out of range, TextLoss with the raster
    /// escape already spent, a non-TextLoss export error); false
    /// when the deck is not local or local assembly failed, in
    /// which case the caller falls back to the server path,
    /// unchanged.
    fn try_pdf_local(
        &mut self,
        item: &Value,
        form: &str,
        page: Option<usize>,
        stem: &str,
        text_layer: bool,
    ) -> bool {
        let outcome = match &self.local {
            Some(local) => local.assemble_showroom_deck(&s(item, "id"), form == "notes"),
            None => None,
        };
        let (deck, page_count) = match outcome {
            None => return false,
            Some(Err(e)) => {
                eprintln!("[qimu] 本地组装稿件失败，回退服务端导出：{e}");
                return false;
            }
            Some(Ok(pair)) => pair,
        };
        if let Some(p) = page {
            if (p as i64) >= page_count {
                self.msg = "导出 PDF 失败：页码超出演示稿页数".into();
                return true;
            }
        }
        match local_render::export_pdf(&deck, form, page, text_layer) {
            Ok(bytes) => {
                if let Some(path) = rfd::FileDialog::new()
                    .set_file_name(format!("{stem}.pdf"))
                    .save_file()
                {
                    // Audit2 E-M10: the write result decides the message — a failed write no longer reports success.
                    self.msg = match std::fs::write(path, bytes) {
                        Ok(()) => {
                            if text_layer {
                                "已保存".into()
                            } else {
                                "已保存（纯图片版：文字不可搜索、复制）".into()
                            }
                        }
                        Err(e) => format!("保存 PDF 失败：{e}"),
                    };
                }
                true
            }
            Err(local_render::PdfFail::TextLoss { count, sample }) => {
                if text_layer {
                    // Never emit a PDF whose text layer silently
                    // lost glyphs: report the loss with its count
                    // and first lost characters, and offer the
                    // raster-only escape as an explicit choice.
                    self.overlays.push(Overlay::Confirm(ConfirmState {
                        title: "导出 PDF".into(),
                        body: format!(
                            "这份稿件有 {count} 个字没有覆盖字体（首个：{sample}），带文本层导出会让这些文字在 PDF 里丢失。改用纯图片导出可以继续：版面不变，但文字不可搜索、复制。"
                        ),
                        confirm_label: "纯图片导出".into(),
                        danger: false,
                        action: ConfirmAction::PdfRasterFallback {
                            deck: deck.clone(),
                            form: form.to_string(),
                            page,
                            stem: stem.to_string(),
                        },
                    }));
                } else {
                    self.msg = format!(
                        "导出 PDF 失败：{count} 个字没有覆盖字体（首个：{sample}），纯图片导出也保留不了这些文字，已取消导出"
                    );
                }
                true
            }
            Err(local_render::PdfFail::Other(e)) => {
                self.msg = format!("导出 PDF 失败：{e}");
                true
            }
        }
    }

    /// Whole-deck page view inside the showroom: every page of the deck
    /// in entry order; clicking a page opens the large preview.
    fn showroom_deck(&mut self, ui: &mut egui::Ui, st: &mut DeckState) {
        let item = st.item.clone();
        let preview_open = st.preview_open;
        let return_label = if preview_open {
            "稿件详情"
        } else {
            "演示厅"
        };
        let current_label = if preview_open {
            "单页预览"
        } else {
            "稿件详情"
        };
        if crumb_bar(ui, &[return_label, current_label]) == Some(0) {
            if preview_open {
                st.preview_open = false;
            } else {
                st.closed = true;
            }
            return;
        }
        ui.add_space(8.0);
        let mut act: Option<&'static str> = None;
        let mut pdf_act: Option<(&'static str, Option<usize>)> = None;
        let cache_key = format!("sr_{}", s(&item, "id"));
        // A stale entry (the formal version moved on or the item
        // was taken down after caching) does not count as cached:
        // the control below offers a fresh 缓存 instead of
        // claiming the current formal version is already held.
        let already_cached = self.cache_entries.iter().any(|e| {
            s(e, "key") == cache_key
                && e.get("stale").and_then(|v| v.as_bool()) != Some(true)
        });
        // T1 transforming button (user rulings 2026-11-24 16:09 and
        // 16:10): this header's save/cache control is ONE button —
        // 保存到我的库 while the deck is not in my library, 缓存
        // once it is, 已缓存（点击更新缓存）in the accent fill once
        // cached. It never greys out: there is no disabled
        // "already saved" state.
        let in_library = self
            .library_refs
            .iter()
            .any(|e| s(e, "item_kind") == "showroom" && s(e, "item_id") == s(&item, "id"));
        ui.horizontal(|ui| {
            ui.label(
                egui::RichText::new(s(&item, "title"))
                    .strong()
                    .size(18.0)
                    .color(palette_color(TEXT_MAIN)),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.spacing_mut().item_spacing.x = 4.0;
                if !in_library {
                    if btn_default(ui, "保存到我的库").clicked() {
                        act = Some("save");
                    }
                } else if already_cached {
                    if btn_primary(ui, "已缓存（点击更新缓存）").clicked() {
                        act = Some("cache");
                    }
                } else if btn_default(ui, "缓存").clicked() {
                    act = Some("cache");
                }
                if self.icon_button(ui, ChromeIcon::Export, "导出") {
                    act = Some("export");
                }
                if self.icon_button(ui, ChromeIcon::Play, "开始演示") {
                    act = Some("play");
                }
                if ChromeIcons
                    .tool(ui, ChromeIcon::Presenter, "演示者视图", true, false, true)
                    .clicked()
                {
                    act = Some("presenter");
                }
                if ChromeIcons
                    .tool(ui, ChromeIcon::Reading, "普通视图", true, false, true)
                    .clicked()
                {
                    act = Some("reading");
                }
            });
        });
        let src_name = self
            .indexes
            .iter()
            .find(|x| s(x, "id") == s(&item, "source_index_id"))
            .map(|x| s(x, "name"))
            .unwrap_or_default();
        // Back-reference line (unified content model, user
        // ruling 2026-11-25): this deck is a published index —
        // name its source index and the published commit (or
        // its follow state), never let it read as a standalone
        // product. Source-less items are personal assemblies
        // and carry no publish segment.
        let mut head_line = if s(&item, "source_index_id").is_empty() {
            "个人组装".to_string()
        } else if src_name.is_empty() {
            "来源索引".to_string()
        } else {
            format!("来源索引 {src_name}")
        };
        if !s(&item, "source_index_id").is_empty() {
            head_line = format!("{head_line} · 发布版 {}", self.deck_commit_label(&item));
        }
        head_line = format!("{head_line} · {}", s(&item, "submitted_by"));
        ui.label(
            egui::RichText::new(head_line)
                .size(11.5)
                .color(palette_color(TEXT_FAINT)),
        );
        if text_action(ui, "用系统应用打开", palette_color(TEXT_WEAK), 11.5) {
            self.present_showroom(&s(&item, "id"), &s(&item, "title"));
        }
        // PDF export (Q21): whole deck in the three DeckCraft forms.
        // The single-page variant lives in the page preview window
        // below. Anonymous (guest) sessions take the same path — the
        // server forces the company edition for them, exactly like
        // the PPTX export.
        ui.horizontal(|ui| {
            if ui.small_button("导出 PDF").clicked() {
                pdf_act = Some(("slides", None));
            }
            if ui.small_button("讲义版 PDF").clicked() {
                pdf_act = Some(("handout", None));
            }
            if ui.small_button("备注页 PDF").clicked() {
                pdf_act = Some(("notes", None));
            }
        });
        ui.add_space(8.0);
        match act {
            Some("play") => {
                let from = st.selected_page.unwrap_or(0);
                if !self.present_via_deckcraft(
                    &s(&item, "id"),
                    &s(&item, "title"),
                    DcMode::Play,
                    from,
                ) {
                    self.start_presentation(ui, &item, &st.pages);
                }
            }
            Some("reading") => {
                let from = st.selected_page.unwrap_or(0);
                if !self.present_via_deckcraft(
                    &s(&item, "id"),
                    &s(&item, "title"),
                    DcMode::Reading,
                    from,
                ) {
                    // No DeckCraft on this machine: the system app is
                    // the reading fallback (it opens the same deck).
                    self.present_showroom(&s(&item, "id"), &s(&item, "title"));
                }
            }
            Some("presenter") => {
                let from = st.selected_page.unwrap_or(0);
                if !self.present_via_deckcraft(
                    &s(&item, "id"),
                    &s(&item, "title"),
                    DcMode::Presenter,
                    from,
                ) {
                    // Fallback is the built-in presenter; the user
                    // can switch its speaker mode on from the menu.
                    self.start_presentation(ui, &item, &st.pages);
                }
            }
            Some("export") => self.export_showroom_pptx(&item),
            Some("save") => self.save_showroom_to_library(&item),
            Some("cache") => self.cache_showroom_deck(&item),
            _ => {}
        }
        if let Some((form, page)) = pdf_act {
            self.export_showroom_pdf(&item, form, page);
        }
        ui.add_space(8.0);
        self.showroom_deck_pages(ui, st, &item);
    }

    /// The 讲稿视图 (prototype presentDlg): a full-page stream —
    /// one row per page, the rendered slide on the left (1.7fr)
    /// and its script panel on the right (1fr), the script
    /// editable in place and committed on focus loss. The
    /// header mirrors the prototype's: title + meta on the left,
    /// 讲稿 toggle / 开始演示 / 导出 PPTX / 关闭预览 on the
    /// right. 开始演示 closes this frame first (the prototype's
    /// closePageView-then-openPlay order) and goes through the
    /// one unified presentation path — never a second engine
    /// entry. The slide face is the real rendered page (the
    /// prototype's big-slide is a text mock of it); the page
    /// title and 第 N 页 sit in a caption row under the image,
    /// where the mock carries its foot line.
    fn showroom_present(&mut self, ui: &mut egui::Ui, st: &mut SrPresentState) {
        let item = st.item.clone();
        if crumb_bar(ui, &["稿件详情", "讲稿视图"]) == Some(0) {
            st.closed = true;
            return;
        }
        ui.add_space(8.0);
        let mut play_act = false;
        let mut export_act = false;
        ui.horizontal(|ui| {
            ui.vertical(|ui| {
                ui.label(
                    egui::RichText::new(format!("预览 · 讲稿视图 · {}", s(&item, "title")))
                        .strong()
                        .size(18.0)
                        .color(palette_color(TEXT_MAIN)),
                );
                let mut segs = vec![format!("{} 页", st.pages.len())];
                if !s(&item, "source_index_id").is_empty() {
                    segs.push(self.deck_commit_label(&item));
                }
                let by = s(&item, "submitted_by");
                if !by.is_empty() {
                    segs.push(format!("维护人 {by}"));
                }
                segs.push("预览不触发缓存".to_string());
                ui.label(
                    egui::RichText::new(segs.join(" · "))
                        .size(11.5)
                        .color(palette_color(TEXT_FAINT)),
                );
            });
            ui.with_layout(egui::Layout::right_to_left(egui::Align::TOP), |ui| {
                ui.spacing_mut().item_spacing.x = 8.0;
                // Right-to-left: added in reverse of the
                // prototype's reading order （讲稿：开 · 开始演示 ·
                // 导出 PPTX · 关闭预览）.
                if btn_default(ui, "关闭预览").clicked() {
                    st.closed = true;
                }
                if btn_default(ui, "导出 PPTX").clicked() {
                    export_act = true;
                }
                if btn_primary(ui, "开始演示").clicked() {
                    play_act = true;
                }
                let script_label = if st.script_on { "讲稿：开" } else { "讲稿：关" };
                if btn_default(ui, script_label).clicked() {
                    st.script_on = !st.script_on;
                }
            });
        });
        if export_act {
            self.export_showroom_pptx(&item);
        }
        if play_act {
            st.closed = true;
            if !self.present_via_deckcraft(
                &s(&item, "id"),
                &s(&item, "title"),
                DcMode::Play,
                0,
            ) {
                let pages = st.pages.clone();
                self.start_presentation(ui, &item, &pages);
            }
            return;
        }
        if st.pages.is_empty() {
            ui.add_space(12.0);
            ui.label(
                egui::RichText::new("该稿件暂无可查看的页面").color(palette_color(TEXT_WEAK)),
            );
            return;
        }
        ui.add_space(12.0);
        let mut save_idx: Option<usize> = None;
        egui::ScrollArea::vertical()
            .id_salt(("showroom-present", item["id"].as_str()))
            .show(ui, |ui| {
                let total = st.pages.len();
                for idx in 0..total {
                    let (vid, slot) = (
                        st.pages[idx].version_id.clone(),
                        st.pages[idx].slot,
                    );
                    let title = st.titles.get(idx).cloned().unwrap_or_default();
                    let editable = st.scripts.contains_key(&vid);
                    ui.horizontal_top(|ui| {
                        let avail = ui.available_width();
                        let gap = 14.0;
                        let slide_w = if st.script_on {
                            ((avail - gap) * 1.7 / 2.7).max(200.0)
                        } else {
                            avail
                        };
                        ui.allocate_ui(
                            egui::vec2(slide_w, slide_w * 9.0 / 16.0 + 24.0),
                            |ui| {
                                ui.vertical(|ui| {
                                    self.thumb_ui(ui, &vid, slot, slide_w);
                                    ui.horizontal(|ui| {
                                        if !title.is_empty() {
                                            ui.label(
                                                egui::RichText::new(&title)
                                                    .size(12.0)
                                                    .color(palette_color(TEXT_WEAK)),
                                            );
                                        }
                                        ui.with_layout(
                                            egui::Layout::right_to_left(egui::Align::Center),
                                            |ui| {
                                                ui.label(
                                                    egui::RichText::new(format!(
                                                        "第 {} 页",
                                                        idx + 1
                                                    ))
                                                    .size(12.0)
                                                    .color(palette_color(TEXT_FAINT)),
                                                );
                                            },
                                        );
                                    });
                                });
                            },
                        );
                        if st.script_on {
                            let note_w = (avail - gap - slide_w).max(160.0);
                            ui.allocate_ui(egui::vec2(note_w, 140.0), |ui| {
                                egui::Frame::NONE
                                    .fill(palette_color(HOVER_BG))
                                    .corner_radius(egui::CornerRadius::same(6))
                                    .inner_margin(egui::Margin {
                                        left: 14,
                                        right: 14,
                                        top: 12,
                                        bottom: 12,
                                    })
                                    .show(ui, |ui| {
                                        ui.set_min_width(note_w - 28.0);
                                        ui.label(
                                            egui::RichText::new(format!(
                                                "讲稿 · 第 {} 页",
                                                idx + 1
                                            ))
                                            .strong()
                                            .size(12.0)
                                            .color(palette_color(TEXT_FAINT)),
                                        );
                                        ui.add_space(4.0);
                                        if editable {
                                            let resp = ui.add(
                                                egui::TextEdit::multiline(
                                                    &mut st.edits[idx],
                                                )
                                                .desired_width(note_w - 28.0)
                                                .desired_rows(5)
                                                .frame(egui::Frame::NONE)
                                                .hint_text("暂无讲稿"),
                                            );
                                            if resp.lost_focus() {
                                                save_idx = Some(idx);
                                            }
                                        } else if st.pages[idx].note.is_empty() {
                                            ui.label(
                                                egui::RichText::new("暂无讲稿")
                                                    .color(palette_color(TEXT_WEAK)),
                                            );
                                        } else {
                                            ui.label(&st.pages[idx].note);
                                        }
                                    });
                            });
                        }
                    });
                    ui.add_space(16.0);
                }
            });
        if let Some(idx) = save_idx {
            self.save_present_script(st, idx);
        }
    }

    fn showroom_deck_pages(&mut self, ui: &mut egui::Ui, st: &mut DeckState, item: &Value) {
        if st.pages.is_empty() {
            ui.label(egui::RichText::new("该稿件暂无可查看的页面").color(palette_color(TEXT_WEAK)));
            return;
        }
        if st.preview_open {
            self.showroom_page_preview(ui, st, item);
            return;
        }
        ui.horizontal(|ui| {
            if let Some(view) = segmented(
                ui,
                &["逐页缩略图", "讲稿"],
                usize::from(st.view == DeckView::Notes),
                SegStyle::Soft,
            ) {
                st.view = if view == 1 {
                    DeckView::Notes
                } else {
                    DeckView::Thumbnails
                };
            }
            ui.label(
                egui::RichText::new(format!("{} 页", st.pages.len()))
                    .color(palette_color(TEXT_WEAK)),
            );
            if ui
                .add_enabled(st.selected_page.is_some(), egui::Button::new("预览所选页"))
                .clicked()
            {
                st.preview_open = true;
            }
        });
        ui.add_space(10.0);
        egui::ScrollArea::vertical()
            .id_salt((item["id"].as_str(), st.view == DeckView::Notes))
            .show(ui, |ui| {
                ui.spacing_mut().item_spacing = egui::vec2(12.0, 12.0);
                let mut open = false;
                if st.view == DeckView::Notes {
                    for (index, page) in st.pages.iter().enumerate() {
                        let width = ui.available_width();
                        card_frame().show(ui, |ui| {
                            ui.set_min_width((width - 26.0).max(0.0));
                            ui.horizontal_top(|ui| {
                                let response = self.showroom_page_thumbnail(
                                    ui,
                                    page,
                                    180.0,
                                    st.selected_page == Some(index),
                                );
                                if response.clicked() {
                                    st.selected_page = Some(index);
                                    open = response.double_clicked();
                                }
                                ui.vertical(|ui| {
                                    ui.label(
                                        egui::RichText::new(format!("第 {} 页", index + 1))
                                            .strong(),
                                    );
                                    ui.label(if page.note.is_empty() {
                                        "暂无讲稿"
                                    } else {
                                        &page.note
                                    });
                                });
                            });
                        });
                    }
                } else {
                    let (columns, width) = showroom_columns(ui, 212.0);
                    for (row_index, row) in st.pages.chunks(columns).enumerate() {
                        ui.horizontal_top(|ui| {
                            for (column, page) in row.iter().enumerate() {
                                let index = row_index * columns + column;
                                ui.vertical(|ui| {
                                    let response = self.showroom_page_thumbnail(
                                        ui,
                                        page,
                                        width,
                                        st.selected_page == Some(index),
                                    );
                                    if response.clicked() {
                                        st.selected_page = Some(index);
                                        open = response.double_clicked();
                                    }
                                    ui.label(
                                        egui::RichText::new(format!("第 {} 页", index + 1))
                                            .size(11.0)
                                            .color(palette_color(TEXT_FAINT)),
                                    );
                                });
                            }
                        });
                    }
                }
                if open {
                    st.preview_open = true;
                }
            });
    }

    fn showroom_page_thumbnail(
        &mut self,
        ui: &mut egui::Ui,
        page: &ShowroomPage,
        width: f32,
        selected: bool,
    ) -> egui::Response {
        self.thumb_response(ui, &page.version_id, page.slot, width, selected)
            .on_hover_text("单击选定，双击预览")
    }

    fn showroom_page_preview(&mut self, ui: &mut egui::Ui, st: &mut DeckState, item: &Value) {
        let last = st.pages.len() - 1;
        let mut index = st.selected_page.unwrap_or(0).min(last);
        if ui.input(|input| input.key_pressed(egui::Key::ArrowLeft)) {
            index = index.saturating_sub(1);
        }
        if ui.input(|input| input.key_pressed(egui::Key::ArrowRight)) {
            index = (index + 1).min(last);
        }
        ui.horizontal(|ui| {
            if ui
                .add_enabled(index > 0, egui::Button::new("上一页"))
                .clicked()
            {
                index -= 1;
            }
            if ui
                .add_enabled(index < last, egui::Button::new("下一页"))
                .clicked()
            {
                index += 1;
            }
            ui.label(format!("{} / {}", index + 1, st.pages.len()));
            if ui.small_button("导出本页 PDF").clicked() {
                self.export_showroom_pdf(item, "slides", Some(index));
            }
        });
        st.selected_page = Some(index);
        let page = &st.pages[index];
        ui.add_space(10.0);
        let width = ui
            .available_width()
            .min(ui.available_height().max(240.0) * 16.0 / 9.0 * 0.8);
        egui::ScrollArea::vertical()
            .id_salt("showroom-page-preview")
            .show(ui, |ui| {
                self.thumb_ui(ui, &page.version_id, page.slot, width);
                if !page.note.is_empty() {
                    ui.add_space(8.0);
                    ui.label(egui::RichText::new("讲稿").strong());
                    ui.label(&page.note);
                }
            });
    }

    fn back_from_showroom_detail(&mut self) {
        if let SrFrame::Deck(st) = self.showroom.stack.top_mut() {
            if st.preview_open {
                st.preview_open = false;
            } else {
                self.showroom.stack.pop();
            }
        }
    }

    /// 保存到我的库 for the deck in the detail header (T1): the
    /// action is saving someone else's shared content into my
    /// library. When the deck reached me as an inbox share, the
    /// save IS accepting that share — the very op the inbox
    /// card's 保存到我的库 button calls. Otherwise (plaza, group
    /// space) the library endpoint creates the same entry under
    /// the showroom access gate. Either way the result is the
    /// library entry the header's saved state reads.
    fn save_showroom_to_library(&mut self, item: &Value) {
        if !self.require_org_online("保存到我的库") {
            return;
        }
        let item_id = s(item, "id");
        if let Some(share) = self
            .inbox
            .iter()
            .find(|sh| s(sh, "item_kind") == "showroom" && s(sh, "item_id") == item_id)
            .cloned()
        {
            let sid = s(&share, "id");
            match self.call("POST", &format!("/v1/shares/{sid}/accept"), Some(json!({}))) {
                Ok(_) => {
                    self.msg = "已保存到我的库".into();
                    self.refresh_all();
                }
                Err(e) => self.msg = format!("保存失败：{e}"),
            }
            return;
        }
        match self.call(
            "POST",
            "/v1/library",
            Some(json!({ "item_kind": "showroom", "item_id": item_id })),
        ) {
            Ok(_) => {
                self.msg = "已保存到我的库".into();
                self.refresh_all();
            }
            Err(e) => self.msg = format!("保存失败：{e}"),
        }
    }

    /// The index half of a showroom cache bundle: the source
    /// index's detail JSON (its resolved rows replaced by the
    /// item's pinned resolution) plus each distinct pinned block
    /// version's fragment bytes (version id, unit id, bytes, page
    /// count). None when any fetch fails — the caller then caches
    /// the assembled deck only, rather than failing the cache.
    fn fetch_cache_bundle(
        &mut self,
        source_index_id: &str,
        item: &Value,
    ) -> Option<(Vec<u8>, Vec<(String, String, Vec<u8>, i64)>)> {
        let mut detail = self
            .call("GET", &format!("/v1/indexes/{source_index_id}"), None)
            .ok()?;
        // The bundle's blocks are the item's pinned resolution
        // (the showroom list payload carries it as the item's
        // resolved_entries), not the source index head's: the
        // cached drill-down holds the formal version. The stored
        // detail is stamped with the same rows, so offline page
        // resolution reads the pinned set too.
        //
        // Single source (N4): the rows ALWAYS come from the
        // item's own pinned resolution. When the caller's payload
        // carries no resolved rows, they are fetched from the
        // item's own face — the showroom list stamps every item
        // with its pinned rows — never from the source index
        // detail, whose rows follow the index head and can name
        // different versions than the pinned deck being cached
        // (the old fallback mixed the two into one bundle). If
        // the item's rows cannot be had either, the bundle is
        // not built (the caller caches the deck alone).
        let pinned_rows = item
            .get("resolved_entries")
            .and_then(|r| r.as_array())
            .filter(|rows| !rows.is_empty())
            .cloned()
            .or_else(|| {
                let list = self.call("GET", "/v1/showroom", None).ok()?;
                let items = list.get("items")?.as_array()?;
                let mine = items.iter().find(|it| s(it, "id") == s(item, "id"))?;
                mine.get("resolved_entries")
                    .and_then(|r| r.as_array())
                    .filter(|rows| !rows.is_empty())
                    .cloned()
            });
        let rows = pinned_rows.clone()?;
        if let Some(rows) = &pinned_rows {
            detail["resolved_entries"] = Value::Array(rows.clone());
        }
        let mut seen: Vec<String> = Vec::new();
        let mut blocks = Vec::new();
        for entry in rows.iter() {
            let version = &entry["resolved"];
            let vid = s(version, "id");
            if vid.is_empty() || seen.contains(&vid) {
                continue;
            }
            seen.push(vid.clone());
            let frag = self
                .call("GET", &format!("/v1/versions/{vid}/fragment"), None)
                .ok()?;
            let bytes = b64d(&s(&frag, "data_base64")).ok()?;
            blocks.push((
                vid,
                s(&entry["entry"], "unit_id"),
                bytes,
                i(version, "page_count"),
            ));
        }
        if blocks.is_empty() {
            return None;
        }
        let detail_bytes = serde_json::to_vec(&detail).ok()?;
        Some((detail_bytes, blocks))
    }

    fn cache_showroom_deck(&mut self, item: &Value) {
        if !self.require_org_online("缓存到本地") {
            return;
        }
        let source = s(item, "source_index_id");
        if source.is_empty() {
            self.msg = "该条目暂不支持缓存到本地".into();
            return;
        }
        // The cached deck is the showroom item's own export —
        // its pinned commit with publish pins applied — not the
        // source index's current main head: the cache holds the
        // formal version, so it must not drift with the index.
        let data = match self.call(
            "POST",
            &format!("/v1/showroom/{}/export", s(item, "id")),
            Some(json!({ "kind": "personal" })),
        ) {
            Ok(d) => d,
            Err(e) => {
                self.msg = format!("缓存失败：{e}");
                return;
            }
        };
        let Ok(bytes) = b64d(&s(&data, "data_base64")) else {
            self.msg = "缓存失败：导出数据损坏".into();
            return;
        };
        // 缓存 = 索引 + 内容块（页面）(the user's definition, T1):
        // the assembled deck alone is not the cache — the bundle
        // also carries the source index and its blocks, so the
        // cached deck opens and drills down offline.
        match self.fetch_cache_bundle(&source, item) {
            Some((index_json, blocks)) => self.cache_deck_bundle(
                &format!("sr_{}", s(item, "id")),
                &s(item, "title"),
                &bytes,
                i(&data, "page_count"),
                &source,
                &index_json,
                &blocks,
                &item_pin_fingerprint(item),
            ),
            None => self.cache_deck(
                &format!("sr_{}", s(item, "id")),
                &s(item, "title"),
                &bytes,
                i(&data, "page_count"),
            ),
        }
    }

    fn library_view(&mut self, ui: &mut egui::Ui) {
        let section = self.showroom.library_view;
        egui::ScrollArea::vertical()
            .id_salt("showroom-library")
            .show(ui, |ui| {
                if section == 0 || section == 1 {
                    self.showroom_cards(
                        ui,
                        &self.library_refs.clone(),
                        |app, ui, e, width| {
                            let cover = app.showroom_cover(&s(e, "item_id"));
                            app.showroom_card(ui, width, &s(e, "title"), &cover, false, |app, ui| {
                                let folder = s(&e, "folder");
                                if !folder.is_empty() {
                                    ui.label(egui::RichText::new(folder).small().weak());
                                }
                                if s(&e, "item_kind") == "showroom"
                                    && !app.org_online()
                                    && !app.sr_bundle_cached(&s(&e, "item_id"))
                                {
                                    // Offline with no complete local
                                    // bundle (T3 ruling): status
                                    // label instead of the actions.
                                    ui.label(
                                        egui::RichText::new("离线不可用").small().weak(),
                                    );
                                    return;
                                }
                                if s(&e, "item_kind") == "showroom" {
                                    if ui.small_button("看整稿").clicked() {
                                        app.open_deck_for_ref(
                                            "showroom",
                                            &s(&e, "item_id"),
                                            &s(&e, "title"),
                                        );
                                    }
                                    if ui.small_button("导出 PPT").clicked() {
                                        let item_id = s(&e, "item_id");
                                        let title = s(&e, "title");
                                        if let Some(path) = rfd::FileDialog::new()
                                            .set_file_name(format!("{title}.pptx"))
                                            .save_file()
                                        {
                                            match app.call(
                                                "POST",
                                                &format!("/v1/showroom/{item_id}/export"),
                                                Some(json!({ "kind": "personal" })),
                                            ) {
                                                Ok(v) => {
                                                    if let Some(data) = v
                                                        .get("data_base64")
                                                        .and_then(|x| x.as_str())
                                                    {
                                                        if let Ok(bytes) = b64d(data) {
                                                            // Audit2 E-M10: the write result decides the message — a failed write no longer reports success.
                                                            app.msg = match std::fs::write(&path, bytes) {
                                                                Ok(()) => "已导出".into(),
                                                                Err(e) => format!("导出失败：{e}"),
                                                            };
                                                        }
                                                    }
                                                }
                                                Err(e2) => app.msg = format!("导出失败：{e2}"),
                                            }
                                        }
                                    }
                                }
                                if ui.small_button("移除").clicked() && app.require_org_online("移除") {
                                    let lid = s(&e, "id");
                                    match app.call(
                                        "POST",
                                        &format!("/v1/library/{lid}/delete"),
                                        Some(json!({})),
                                    ) {
                                        Ok(_) => {
                                            app.msg = "已移除".into();
                                            app.refresh_all();
                                        }
                                        Err(e2) => app.msg = format!("移除失败：{e2}"),
                                    }
                                }
                            });
                        },
                    );
                }
                if section == 0 || section == 2 {
                    if section == 0 {
                        ui.add_space(18.0);
                    }
                    self.showroom_cards(
                        ui,
                        &self.cache_entries.clone(),
                        |app, ui, e, width| {
                            app.showroom_card(ui, width, &s(e, "title"), "", false, |app, ui| {
                                ui.label(format!(
                                    "{} 页 · {}",
                                    i(&e, "page_count"),
                                    fmt_time(i(&e, "cached_ms"))
                                ));
                                if e.get("stale").and_then(|v| v.as_bool()) == Some(true) {
                                    ui.label(
                                        egui::RichText::new(
                                            "已过期：正式版已更新或条目已下架",
                                        )
                                        .small()
                                        .weak(),
                                    );
                                }
                                let key = s(&e, "key");
                                if ui.button("打开").clicked() {
                                    app.open_cache(&key);
                                }
                                if !s(&e, "index_file").is_empty()
                                    && ui.button("打开详情").clicked()
                                {
                                    let item_id =
                                        key.strip_prefix("sr_").unwrap_or("").to_string();
                                    if !item_id.is_empty() {
                                        app.open_deck_for_ref(
                                            "showroom",
                                            &item_id,
                                            &s(&e, "title"),
                                        );
                                    }
                                }
                                if ui.button("移除缓存").clicked() {
                                    app.remove_cache(&key);
                                }
                            });
                        },
                    );
                }
                if section == 0 || section == 3 {
                    if section == 0 {
                        ui.add_space(18.0);
                    }
                    self.showroom_cards(
                        ui,
                        &self.my_assemblies.clone(),
                        |app, ui, a, width| {
                            app.showroom_card(ui, width, &s(a, "name"), "", false, |app, ui| {
                                ui.label(format!(
                                    "{} 个条目 · {}",
                                    i(&a, "entry_count"),
                                    fmt_time(i(&a, "created_ms"))
                                ));
                                if ui.button("缓存到本地").clicked() {
                                    let aid = s(&a, "id");
                                    let name = s(&a, "name");
                                    match app.call(
                                        "POST",
                                        &format!("/v1/assemblies/{aid}/export"),
                                        Some(json!({ "kind": "personal" })),
                                    ) {
                                        Ok(v) => {
                                            if let Some(data) =
                                                v.get("data_base64").and_then(|x| x.as_str())
                                            {
                                                if let Ok(bytes) = b64d(data) {
                                                    let pages = i(&v, "page_count");
                                                    app.cache_deck(&aid, &name, &bytes, pages);
                                                }
                                            }
                                        }
                                        Err(e) => app.msg = format!("缓存失败：{e}"),
                                    }
                                }
                            });
                        },
                    );
                }
                if section == 0 || section == 4 {
                    if section == 0 {
                        ui.add_space(18.0);
                    }
                    // 本地演示: this library's own showroom rows
                    // (local_showroom, fed by refresh_local). Pure
                    // local data — every action below works
                    // offline; only 提交到组织 leaves the machine,
                    // through the existing submission chain.
                    self.showroom_cards(
                        ui,
                        &self.local_showroom.clone(),
                        |app, ui, e, width| {
                            // Cover: the item's pinned first
                            // resolved version; the plaza helper
                            // reads showroom_live only, so local
                            // rows fall back to the source index's
                            // cover here.
                            let cover = e["resolved_entries"]
                                .as_array()
                                .and_then(|a| a.first())
                                .map(|en| s(&en["resolved"], "id"))
                                .filter(|v| !v.is_empty())
                                .unwrap_or_else(|| {
                                    app.index_covers
                                        .get(&s(e, "source_index_id"))
                                        .cloned()
                                        .unwrap_or_default()
                                });
                            app.showroom_card(ui, width, &s(e, "title"), &cover, false, |app, ui| {
                                // Source line: the 本地演示 marker
                                // keeps these rows distinct from
                                // organization items on the shared
                                // 全部 wall; the source index name
                                // follows deck_source_label.
                                ui.label(
                                    egui::RichText::new(format!(
                                        "本地演示 · 来源 {}",
                                        app.deck_source_label(e)
                                    ))
                                    .small()
                                    .weak(),
                                );
                                let intro = s(e, "intro");
                                if !intro.is_empty() {
                                    ui.add(
                                        egui::Label::new(
                                            egui::RichText::new(intro).small().weak(),
                                        )
                                        .truncate(),
                                    );
                                }
                                let state_label = match s(e, "state").as_str() {
                                    "pending" => "待审（本地）",
                                    "rejected" => "已驳回（本地）",
                                    _ => "正式版（本地）",
                                };
                                ui.label(
                                    egui::RichText::new(format!(
                                        "{} · {} · {}",
                                        state_label,
                                        s(e, "submitted_by"),
                                        fmt_time(i(e, "created_ms"))
                                    ))
                                    .small()
                                    .weak(),
                                );
                                if s(e, "state") == "rejected"
                                    && !s(e, "review_note").is_empty()
                                {
                                    ui.label(
                                        egui::RichText::new(format!(
                                            "驳回理由：{}",
                                            s(e, "review_note")
                                        ))
                                        .small()
                                        .weak(),
                                    );
                                }
                                ui.horizontal_wrapped(|ui| {
                                    if ui.small_button("打开详情").clicked() {
                                        app.open_showroom_deck(e);
                                    }
                                    if ui.small_button("开始演示").clicked() {
                                        app.open_showroom_deck(e);
                                        if !app.present_via_deckcraft(
                                            &s(e, "id"),
                                            &s(e, "title"),
                                            DcMode::Play,
                                            0,
                                        ) {
                                            app.start_present_from_deck(ui);
                                        }
                                    }
                                    let src = s(e, "source_index_id");
                                    if !src.is_empty()
                                        && ui.small_button("提交到组织").clicked()
                                    {
                                        let title = s(e, "title");
                                        app.open_push_dialog(&src, &title);
                                    }
                                    if text_action(
                                        ui,
                                        "删除",
                                        palette_color(BADGE_BAD_FG),
                                        12.0,
                                    ) {
                                        app.trash_add("showroom", &s(e, "id"));
                                    }
                                });
                            });
                        },
                    );
                }
            });
    }

    fn inbox_view(&mut self, ui: &mut egui::Ui) {
        let items = self
            .inbox
            .iter()
            .filter(|item| Self::share_matches_source(item, self.showroom.share_source))
            .cloned()
            .collect::<Vec<_>>();
        egui::ScrollArea::vertical()
            .id_salt("showroom-inbox")
            .show(ui, |ui| {
                self.showroom_cards(
                    ui,
                    &items,
                    Self::showroom_shared_card,
                );
            });
    }

    fn showroom_shared_card(&mut self, ui: &mut egui::Ui, item: &Value, width: f32) {
        let title = s(item, "title");
        let id = s(item, "item_id");
        let cover = self.showroom_cover(&id);
        let saved = item["in_library"] == true
            || self
                .library_refs
                .iter()
                .any(|entry| entry["item_id"] == item["item_id"]);
        let cover_response = self.showroom_card(ui, width, &title, &cover, false, |app, ui| {
            ui.label(
                egui::RichText::new(format!(
                    "{} · {}",
                    s(item, "from_name"),
                    fmt_time(i(item, "created_ms"))
                ))
                .small()
                .weak(),
            );
            let note = s(item, "note");
            if !note.is_empty() {
                ui.label(
                    egui::RichText::new(format!("{}：{note}", tr("分享说明")))
                        .size(11.5)
                        .color(palette_color(TEXT_WEAK)),
                );
            }
            if item["item_kind"] == "showroom"
                && !app.org_online()
                && !app.sr_bundle_cached(&id)
            {
                // Offline with no complete local bundle (T3
                // ruling): a status label instead of the action
                // row. Non-showroom shares keep their save action —
                // the bundle test only speaks for showroom decks.
                ui.label(egui::RichText::new("离线不可用").small().weak());
            } else {
                ui.horizontal_wrapped(|ui| {
                    if item["item_kind"] == "showroom" && ui.small_button("打开详情").clicked() {
                        app.open_deck_for_ref("showroom", &id, &title);
                    }
                    if saved {
                        ui.label(egui::RichText::new("已在我的库").small().weak());
                    } else if app.org_online() && ui.small_button("保存到我的库").clicked() {
                        let sid = s(item, "id");
                        app.open_pull_dialog(item, Some(sid));
                    }
                });
            }
        });
        let response = ui.interact(
            cover_response,
            ui.scope_id().with(("share", s(item, "id"))),
            egui::Sense::click(),
        );
        if response.double_clicked()
            && item["item_kind"] == "showroom"
            && (self.org_online() || self.sr_bundle_cached(&id))
        {
            self.open_deck_for_ref("showroom", &id, &title);
        }
    }

    fn groups_view(&mut self, ui: &mut egui::Ui) {
        let users = self
            .org
            .get("users")
            .and_then(|u| u.as_array())
            .cloned()
            .unwrap_or_default();
        let Some(space) = self.active_showroom_group().cloned() else {
            return;
        };
        egui::ScrollArea::vertical()
            .id_salt("showroom-groups")
            .show(ui, |ui| {
                let g = space.get("group").cloned().unwrap_or(json!({}));
                let gid = s(&g, "id");
                ui.label(
                    egui::RichText::new(format!(
                        "{} · 组长 {}",
                        s(&g, "name"),
                        s(&g, "owner_name")
                    ))
                    .strong(),
                );
                let members = g
                    .get("members")
                    .and_then(|m| m.as_array())
                    .map(|arr| {
                        arr.iter()
                            .map(|m| s(m, "name"))
                            .collect::<Vec<_>>()
                            .join("、")
                    })
                    .unwrap_or_default();
                ui.label(
                    egui::RichText::new(format!("成员：{members}"))
                        .small()
                        .weak(),
                );
                // owner can add members
                if s(&g, "owner_id") == self.my_id {
                    ui.horizontal(|ui| {
                        egui::ComboBox::from_id_salt(format!("tm_{gid}"))
                            .selected_text(
                                users
                                    .iter()
                                    .find(|u| s(u, "id") == self.team_member_pick)
                                    .map(|u| s(u, "display_name"))
                                    .unwrap_or_else(|| "选择同事".into()),
                            )
                            .show_ui(ui, |ui| {
                                for u in &users {
                                    ui.selectable_value(
                                        &mut self.team_member_pick,
                                        s(u, "id"),
                                        s(u, "display_name"),
                                    );
                                }
                            });
                        if ui
                            .add_enabled(
                                !self.team_member_pick.is_empty(),
                                egui::Button::new("加成员").small(),
                            )
                            .clicked()
                            && self.require_org_online("添加成员")
                        {
                            let pick = self.team_member_pick.clone();
                            match self.call(
                                "POST",
                                &format!("/v1/custom-groups/{gid}/members"),
                                Some(json!({ "user_id": pick })),
                            ) {
                                Ok(_) => {
                                    self.msg = "成员已加入".into();
                                    self.refresh_all();
                                }
                                Err(e) => self.msg = format!("加成员失败：{e}"),
                            }
                        }
                    });
                }
                ui.add_space(12.0);
                let items = space["items"]
                    .as_array()
                    .map(Vec::as_slice)
                    .unwrap_or_default();
                self.showroom_cards(
                    ui,
                    items,
                    Self::showroom_shared_card,
                );
            });
    }

    /// Showroom content slot, including the optional review queue.
    fn sr_main_body(&mut self, ui: &mut egui::Ui) {
        if self.showroom.zone == "plaza"
            && matches!(self.showroom.stack.top(), SrFrame::Wall)
            && self.role() == "super"
            && (!self.showroom_pending.is_empty() || !self.submissions_pending.is_empty())
        {
            let review_height = ui.available_height() * 0.28;
            egui::Panel::bottom("showroom_review")
                .exact_size(review_height)
                .show(ui, |ui| self.sr_review_body(ui));
        }
        body_frame().show(ui, |ui| self.showroom_body(ui));
    }

    /// Review queue below the showroom wall for super admins.
    fn sr_review_body(&mut self, ui: &mut egui::Ui) {
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
                    ui.label(
                        egui::RichText::new(tr("待审队列"))
                            .size(15.0)
                            .strong()
                            .color(palette_color(TEXT_MAIN)),
                    );
                    ui.add_space(10.0);
                    for sub in self.submissions_pending.clone() {
                        ui.horizontal(|ui| {
                            ui.label(egui::RichText::new(s(&sub, "title")).strong());
                            ui.label(format!("提交人 {}", s(&sub, "submitter_name")));
                            ui.label(tr("提交包"));
                            if ui.button(tr("查看内容")).clicked() {
                                // F1: the package preview lives in
                                // the approval queue as a detail
                                // frame — jump there with the frame
                                // already pushed.
                                self.page = Page::Settings;
                                self.review.jump_source = Some(Page::Showroom);
                                let sections = self.settings_sections();
                                if let Some(pos) =
                                    sections.iter().position(|x| *x == "审批队列")
                                {
                                    self.settings.section = pos;
                                }
                                self.review.stack.pop_to(0);
                                self.review.stack.push(ReviewFrame::Submission(
                                    SubmissionDetailState {
                                        sub_id: s(&sub, "id"),
                                        manifest: None,
                                        loaded: false,
                                        load_error: String::new(),
                                        reject_open: false,
                                        reject_text: String::new(),
                                    },
                                ));
                            }
                            if ui.button("通过").clicked() && self.need_identity("审批演示厅投稿") {
                                self.review_submission_act(&s(&sub, "id"), "live", "");
                            }
                            if ui.button("驳回").clicked() {
                                // Rejection needs a reason — continue
                                // in the approval queue, whose
                                // submission cards carry the form.
                                self.page = Page::Settings;
                                self.review.jump_source = Some(Page::Showroom);
                                let sections = self.settings_sections();
                                if let Some(pos) =
                                    sections.iter().position(|x| *x == "审批队列")
                                {
                                    self.settings.section = pos;
                                }
                                self.review.stack.pop_to(0);
                                self.sub_reject = Some((s(&sub, "id"), String::new()));
                            }
                        });
                    }
                    for item in self.showroom_pending.clone() {
                        ui.horizontal(|ui| {
                            ui.label(egui::RichText::new(s(&item, "title")).strong());
                            ui.label(format!("提交人 {}", s(&item, "submitted_by")));
                            if ui.button("通过").clicked() && self.need_identity("审批演示厅投稿")
                            {
                                let _ = self.call(
                                    "POST",
                                    &format!("/v1/showroom/{}/review", s(&item, "id")),
                                    Some(json!({ "state": "live" })),
                                );
                                self.refresh_all();
                            }
                            if ui.button("驳回").clicked() {
                                // A rejection now requires a reason
                                // (server-enforced) — continue in the
                                // approval queue's detail frame, which
                                // carries the reason form.
                                self.page = Page::Settings;
                                self.review.jump_source = Some(Page::Showroom);
                                let sections = self.settings_sections();
                                if let Some(pos) = sections.iter().position(|x| *x == "审批队列")
                                {
                                    self.settings.section = pos;
                                }
                                self.review
                                    .stack
                                    .push(ReviewFrame::Detail(ReviewDetailState {
                                        item_id: s(&item, "id"),
                                        reject_open: true,
                                        reject_text: String::new(),
                                        deck: None,
                                        deck_loaded: false,
                                    }));
                            }
                        });
                    }
                });
            });
    }

    fn find_showroom_item(&self, id: &str) -> Option<Value> {
        self.showroom_pending
            .iter()
            .chain(self.review.records.iter())
            .chain(self.showroom_live.iter())
            .find(|x| s(x, "id") == id)
            .cloned()
    }

    /// One review decision against the server's manual approval
    /// flow. Rejection requires a reason (server-enforced).
    fn review_act(&mut self, item_id: &str, state: &str, note: &str) -> bool {
        if !self.require_org_online("审批演示厅投稿") {
            return false;
        }
        if !self.need_identity("审批演示厅投稿") {
            return false;
        }
        let mut body = json!({ "state": state });
        if !note.is_empty() {
            body["note"] = json!(note);
        }
        match self.call(
            "POST",
            &format!("/v1/showroom/{item_id}/review"),
            Some(body),
        ) {
            Ok(_) => {
                self.msg = if state == "live" {
                    "已通过".into()
                } else {
                    "已驳回".into()
                };
                self.review.records_loaded = false;
                self.refresh_all();
                true
            }
            Err(e) => {
                self.msg = format!("审批失败：{e}");
                false
            }
        }
    }

    /// One review decision on a P3 submission. Approval runs the
    /// server-side landing transaction — the response only comes
    /// back once the content has actually landed (or rolled back,
    /// leaving the submission pending with the reason recorded).
    /// Rejection requires a reason (server-enforced, returned to
    /// the submitter verbatim).
    fn review_submission_act(&mut self, sub_id: &str, state: &str, note: &str) -> bool {
        if !self.require_org_online("审批演示厅投稿") {
            return false;
        }
        if !self.need_identity("审批演示厅投稿") {
            return false;
        }
        let mut body = json!({ "state": state });
        if !note.is_empty() {
            body["note"] = json!(note);
        }
        match self.call(
            "POST",
            &format!("/v1/submissions/{sub_id}/review"),
            Some(body),
        ) {
            Ok(_) => {
                self.msg = if state == "live" {
                    "已通过并落地".into()
                } else {
                    "已驳回".into()
                };
                self.review.records_loaded = false;
                self.refresh_all();
                true
            }
            Err(e) => {
                self.msg = format!("审批失败：{e}");
                false
            }
        }
    }

    /// Handled records = approved + rejected items that carry a
    /// review stamp (reviewer / time / reason).
    fn review_records_load(&mut self) {
        let mut all: Vec<Value> = Vec::new();
        for st in ["live", "rejected"] {
            if let Ok(v) = self.call("GET", &format!("/v1/showroom?state={st}"), None) {
                if let Some(arr) = v.get("items").and_then(|x| x.as_array()) {
                    all.extend(arr.iter().cloned());
                }
            }
        }
        all.retain(|x| i(x, "reviewed_ms") > 0);
        // P3 submissions: a rejected submission produces no
        // showroom row, so its processed record comes from the
        // submissions table itself (landed submissions are already
        // represented by their live showroom records above — never
        // counted twice).
        if self.role() == "super" {
            if let Ok(v) = self.call("GET", "/v1/submissions?state=rejected", None) {
                if let Some(arr) = v.get("items").and_then(|x| x.as_array()) {
                    for sub in arr {
                        all.push(json!({
                            "id": s(sub, "id"),
                            "title": s(sub, "title"),
                            "state": "rejected",
                            "submitted_by": s(sub, "submitter_name"),
                            "reviewed_by": s(sub, "reviewed_by"),
                            "reviewed_ms": i(sub, "reviewed_ms"),
                            "review_note": s(sub, "review_note"),
                        }));
                    }
                }
            }
        }
        all.sort_by_key(|x| -i(x, "reviewed_ms"));
        self.review.records = all;
        self.review.records_loaded = true;
    }

    /// 「我的提交」 (Settings, named users): the submitter's own
    /// P3 submissions with their review state — pending in the
    /// server's staging zone, landed (as organization content +
    /// a live showroom entry), or rejected with the reviewer's
    /// reason verbatim. A pending submission can be withdrawn
    /// here; resubmission happens from the index detail, where it
    /// automatically supersedes the rejected row.
    fn my_submissions_body(&mut self, ui: &mut egui::Ui) {
        let subs = self.my_submissions.clone();
        if subs.is_empty() {
            return;
        }
        let mut withdraw: Option<String> = None;
        let mut goto_showroom = false;
        let online = self.org_online();
        for sub in &subs {
            let state = s(sub, "state");
            standard_card_v(
                ui,
                false,
                None,
                false,
                if state == "rejected" {
                    CardVariant::Rejected
                } else {
                    CardVariant::Normal
                },
                |ui| {
                    ui.horizontal(|ui| {
                        ui.label(
                            egui::RichText::new(s(sub, "title"))
                                .size(13.0)
                                .color(palette_color(TEXT_MAIN)),
                        );
                        match state.as_str() {
                            "pending" => badge(ui, tr("待审"), BadgeTone::Warn),
                            "landed" => badge(ui, tr("已落地"), BadgeTone::Ok),
                            _ => badge(ui, tr("被驳回"), BadgeTone::Bad),
                        }
                    });
                    ui.label(
                        egui::RichText::new(format!(
                            "{} · {} {} · {} {}",
                            fmt_time(i(sub, "created_ms")),
                            i(sub, "entry_count"),
                            tr("个内容块"),
                            i(sub, "page_count"),
                            tr("页")
                        ))
                        .size(11.0)
                        .color(palette_color(TEXT_FAINT)),
                    );
                    let note = s(sub, "note");
                    if !note.is_empty() {
                        ui.label(
                            egui::RichText::new(format!("{}：{}", tr("提交说明"), note))
                                .size(11.5)
                                .color(palette_color(TEXT_WEAK)),
                        );
                    }
                    if state == "rejected" {
                        let reason = s(sub, "review_note");
                        if !reason.is_empty() {
                            ui.label(
                                egui::RichText::new(format!(
                                    "{}：{}",
                                    tr("驳回理由"),
                                    reason
                                ))
                                .size(11.5)
                                .color(palette_color(TEXT_WEAK)),
                            );
                        }
                        ui.label(
                            egui::RichText::new(tr(
                                "修改后在索引详情重新提交即可（会自动关联本次驳回）",
                            ))
                            .size(11.5)
                            .color(palette_color(TEXT_FAINT)),
                        );
                    }
                    ui.horizontal(|ui| {
                        if state == "pending"
                            && online
                            && btn_danger(ui, tr("撤回")).clicked()
                        {
                            withdraw = Some(s(sub, "id"));
                        }
                        if state == "landed"
                            && btn_default(ui, tr("去演示厅查看")).clicked()
                        {
                            goto_showroom = true;
                        }
                    });
                },
            );
            ui.add_space(8.0);
        }
        if let Some(id) = withdraw {
            match self.call("DELETE", &format!("/v1/submissions/{id}"), None) {
                Ok(_) => {
                    self.msg = "已撤回".into();
                    self.refresh_all();
                }
                Err(e) => {
                    self.msg = format!("撤回失败：{e}");
                }
            }
        }
        if goto_showroom {
            self.page = Page::Showroom;
            self.showroom.zone = "plaza".into();
            self.showroom.folder = None;
        }
    }

    /// Approval queue (Settings, super only): the GUI face of the
    /// server's manual review flow, on a drill-down stack — Queue
    /// frame (待审批 / 已处理记录 tabs) → Detail frame (source pages
    /// + approve / reject-with-reason). Governance settings are a
    /// later phase (route plan); this is the queue only.
    fn review_queue_body(&mut self, ui: &mut egui::Ui) {
        if matches!(self.review.stack.top(), ReviewFrame::Detail(_)) {
            self.review_detail_body(ui);
            return;
        }
        if matches!(self.review.stack.top(), ReviewFrame::Submission(_)) {
            self.submission_detail_body(ui);
            return;
        }
        // Cross-page deep jump (nav ruler 3): this stack was entered
        // from the showroom review bar; at its root, offer the way
        // back to the source frame.
        if self.review.jump_source.is_some() {
            ui.horizontal(|ui| {
                if text_action(ui, "‹ 返回演示厅", palette_color(ACCENT_DEEP), 13.0) {
                    self.page = self.review.jump_source.take().unwrap_or(Page::Showroom);
                }
            });
            ui.add_space(6.0);
        }
        let tab = match self.review.stack.top() {
            ReviewFrame::Queue(q) => q.tab,
            _ => 0,
        };
        if tab == 1 && !self.review.records_loaded {
            self.review_records_load();
        }
        let mut new_tab: Option<usize> = None;
        if let Some(t) = segmented(ui, &[tr("待审批"), tr("已处理记录")], tab, SegStyle::Soft)
        {
            new_tab = Some(t);
        }
        ui.add_space(10.0);
        let mut open_detail: Option<(String, bool)> = None;
        let mut approve: Option<String> = None;
        if tab == 0 {
            let pending = self.showroom_pending.clone();
            let subs = self.submissions_pending.clone();
            let mut approve_sub: Option<String> = None;
            let mut open_sub: Option<String> = None;
            let mut confirm_sub_reject: Option<(String, String)> = None;
            let mut cancel_sub_reject = false;
            let mut reject_draft = self.sub_reject.clone();
            for sub in &subs {
                let sid = s(sub, "id");
                let title = s(sub, "title");
                std_card(ui, false, |ui| {
                    ui.horizontal(|ui| {
                        ui.vertical(|ui| {
                            ui.label(
                                egui::RichText::new(&title).size(13.0).color(palette_color(TEXT_MAIN)),
                            );
                            ui.horizontal(|ui| {
                                badge(ui, tr("待审"), BadgeTone::Warn);
                                badge(ui, tr("提交包"), BadgeTone::Info);
                            });
                        });
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::TOP), |ui| {
                            ui.vertical(|ui| {
                                if btn_primary(ui, tr("通过")).clicked() {
                                    approve_sub = Some(sid.clone());
                                }
                                if btn_danger(ui, tr("驳回…")).clicked() {
                                    reject_draft = Some((sid.clone(), String::new()));
                                }
                                if btn_default(ui, tr("查看内容")).clicked() {
                                    open_sub = Some(sid.clone());
                                }
                            });
                        });
                    });
                    std_card_meta(ui, |ui| {
                        ui.label(
                            egui::RichText::new(format!(
                                "{} {} · {} · {} {} · {} {}",
                                tr("提交人"),
                                s(sub, "submitter_name"),
                                fmt_time(i(sub, "created_ms")),
                                i(sub, "entry_count"),
                                tr("个内容块"),
                                i(sub, "page_count"),
                                tr("页")
                            ))
                            .size(11.0)
                            .color(palette_color(TEXT_FAINT)),
                        );
                    });
                    let note = s(sub, "note");
                    if !note.is_empty() {
                        ui.label(
                            egui::RichText::new(format!("{}：{}", tr("提交说明"), note))
                                .size(11.5)
                                .color(palette_color(TEXT_WEAK)),
                        );
                    }
                    if reject_draft.as_ref().map(|(rid, _)| rid) == Some(&sid) {
                        if let Some((_, draft)) = &mut reject_draft {
                            ui.add_space(6.0);
                            ui.label(
                                egui::RichText::new(tr("驳回理由（必填，会原样回到提交人）"))
                                    .size(11.5)
                                    .color(palette_color(TEXT_WEAK)),
                            );
                            ui.text_edit_singleline(draft);
                            ui.horizontal(|ui| {
                                if ui
                                    .add_enabled(
                                        !draft.trim().is_empty(),
                                        egui::Button::new(tr("确认驳回")),
                                    )
                                    .clicked()
                                {
                                    confirm_sub_reject = Some((sid.clone(), draft.clone()));
                                }
                                if ui.button(tr("取消")).clicked() {
                                    cancel_sub_reject = true;
                                }
                            });
                        }
                    }
                });
                ui.add_space(8.0);
            }
            for item in &pending {
                let iid = s(item, "id");
                let cover_vid = self
                    .index_covers
                    .get(&s(item, "source_index_id"))
                    .cloned()
                    .unwrap_or_default();
                let tex = if cover_vid.is_empty() {
                    None
                } else {
                    self.thumb(&cover_vid, 0)
                };
                let title = s(item, "title");
                let src_name = self
                    .indexes
                    .iter()
                    .find(|x| s(x, "id") == s(item, "source_index_id"))
                    .map(|x| s(x, "name"))
                    .unwrap_or_else(|| s(item, "source_index_id"));
                std_card(ui, false, |ui| {
                    ui.horizontal(|ui| {
                        slide_cover(ui, tex, tr("演示稿"), &title, "", 84.0);
                        ui.vertical(|ui| {
                            ui.label(
                                egui::RichText::new(&title)
                                    .size(13.0)
                                    .color(palette_color(TEXT_MAIN)),
                            );
                            ui.horizontal(|ui| {
                                badge(ui, tr("待审"), BadgeTone::Warn);
                            });
                        });
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::TOP), |ui| {
                            ui.vertical(|ui| {
                                if btn_primary(ui, tr("通过")).clicked() {
                                    approve = Some(iid.clone());
                                }
                                if btn_danger(ui, tr("驳回…")).clicked() {
                                    open_detail = Some((iid.clone(), true));
                                }
                                if btn_default(ui, tr("查看")).clicked() {
                                    open_detail = Some((iid.clone(), false));
                                }
                            });
                        });
                    });
                    std_card_meta(ui, |ui| {
                        ui.label(
                            egui::RichText::new(format!(
                                "{} {} · {} · {} {}",
                                tr("提交人"),
                                s(item, "submitted_by"),
                                fmt_time(i(item, "created_ms")),
                                tr("来源索引"),
                                src_name
                            ))
                            .size(11.0)
                            .color(palette_color(TEXT_FAINT)),
                        );
                    });
                });
                ui.add_space(8.0);
            }
            if let Some(sid) = open_sub {
                self.review.stack.push(ReviewFrame::Submission(SubmissionDetailState {
                    sub_id: sid,
                    manifest: None,
                    loaded: false,
                    load_error: String::new(),
                    reject_open: false,
                    reject_text: String::new(),
                }));
            }
            if let Some(sid) = approve_sub {
                self.review_submission_act(&sid, "live", "");
            }
            if let Some((sid, note)) = confirm_sub_reject {
                if self.review_submission_act(&sid, "rejected", &note) {
                    reject_draft = None;
                }
            }
            if cancel_sub_reject {
                reject_draft = None;
            }
            self.sub_reject = reject_draft;
        } else {
            let records = self.review.records.clone();
            for item in &records {
                let rejected = s(item, "state") == "rejected";
                standard_card_v(
                    ui,
                    false,
                    None,
                    false,
                    if rejected {
                        CardVariant::Rejected
                    } else {
                        CardVariant::Normal
                    },
                    |ui| {
                        ui.horizontal(|ui| {
                            ui.label(
                                egui::RichText::new(s(item, "title"))
                                    .size(13.0)
                                    .color(palette_color(TEXT_MAIN)),
                            );
                            badge(
                                ui,
                                if rejected {
                                    tr("已驳回")
                                } else {
                                    tr("已通过")
                                },
                                if rejected {
                                    BadgeTone::Bad
                                } else {
                                    BadgeTone::Ok
                                },
                            );
                        });
                        ui.label(
                            egui::RichText::new(format!(
                                "{} {} · {}",
                                tr("审批人"),
                                s(item, "reviewed_by"),
                                fmt_time(i(item, "reviewed_ms"))
                            ))
                            .size(11.0)
                            .color(palette_color(TEXT_FAINT)),
                        );
                        let note = s(item, "review_note");
                        if !note.is_empty() {
                            ui.label(
                                egui::RichText::new(format!("{}：{}", tr("理由"), note))
                                    .size(11.5)
                                    .color(palette_color(TEXT_WEAK)),
                            );
                        }
                    },
                );
                ui.add_space(8.0);
            }
        }
        if let Some(t) = new_tab {
            if let ReviewFrame::Queue(q) = self.review.stack.top_mut() {
                q.tab = t;
            }
        }
        if let Some(iid) = approve {
            self.review_act(&iid, "live", "");
        }
        if let Some((iid, reject)) = open_detail {
            self.review
                .stack
                .push(ReviewFrame::Detail(ReviewDetailState {
                    item_id: iid,
                    reject_open: reject,
                    reject_text: String::new(),
                    deck: None,
                    deck_loaded: false,
                }));
        }
    }

    fn review_detail_body(&mut self, ui: &mut egui::Ui) {
        let item_id = match self.review.stack.top() {
            ReviewFrame::Detail(d) => d.item_id.clone(),
            _ => return,
        };
        if self.find_showroom_item(&item_id).is_none() && !self.review.records_loaded {
            self.review_records_load();
        }
        // Load the source deck once per detail frame.
        let need_deck = match self.review.stack.top() {
            ReviewFrame::Detail(d) => !d.deck_loaded,
            _ => false,
        };
        if need_deck {
            let src = self
                .find_showroom_item(&item_id)
                .map(|it| s(&it, "source_index_id"))
                .unwrap_or_default();
            let deck = if src.is_empty() {
                None
            } else {
                self.call("GET", &format!("/v1/indexes/{src}"), None).ok()
            };
            if let ReviewFrame::Detail(d) = self.review.stack.top_mut() {
                d.deck = deck;
                d.deck_loaded = true;
            }
        }
        let item = self.find_showroom_item(&item_id);
        let (deck, reject_open) = match self.review.stack.top() {
            ReviewFrame::Detail(d) => (d.deck.clone(), d.reject_open),
            _ => (None, false),
        };
        let mut go_back = false;
        let mut approve = false;
        let mut open_reject = false;
        let mut cancel_reject = false;
        let mut confirm_reject = false;
        egui::ScrollArea::vertical().show(ui, |ui| {
            let title = item
                .as_ref()
                .map(|it| s(it, "title"))
                .unwrap_or_else(|| item_id.clone());
            if let Some(d) = crumb_bar(ui, &[tr("审批队列"), &title]) {
                if d == 0 {
                    go_back = true;
                }
            }
            ui.add_space(10.0);
            let Some(it) = &item else {
                ui.label(
                    egui::RichText::new(tr("这条提交已不在队列中（可能已被处理或删除）。"))
                        .size(12.5)
                        .color(palette_color(TEXT_WEAK)),
                );
                return;
            };
            let state = s(it, "state");
            card_frame().show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(
                        egui::RichText::new(s(it, "title"))
                            .size(14.0)
                            .color(palette_color(TEXT_MAIN)),
                    );
                    match state.as_str() {
                        "pending" => badge(ui, tr("待审"), BadgeTone::Warn),
                        "live" => badge(ui, tr("已通过"), BadgeTone::Ok),
                        "rejected" => badge(ui, tr("已驳回"), BadgeTone::Bad),
                        _ => {}
                    }
                });
                let intro = s(it, "intro");
                if !intro.is_empty() {
                    ui.label(
                        egui::RichText::new(intro)
                            .size(12.0)
                            .color(palette_color(TEXT_WEAK)),
                    );
                }
                ui.label(
                    egui::RichText::new(format!(
                        "{} {} · {}",
                        tr("提交人"),
                        s(it, "submitted_by"),
                        fmt_time(i(it, "created_ms"))
                    ))
                    .size(11.0)
                    .color(palette_color(TEXT_FAINT)),
                );
                if state != "pending" && i(it, "reviewed_ms") > 0 {
                    ui.label(
                        egui::RichText::new(format!(
                            "{} {} · {}",
                            tr("审批人"),
                            s(it, "reviewed_by"),
                            fmt_time(i(it, "reviewed_ms"))
                        ))
                        .size(11.0)
                        .color(palette_color(TEXT_FAINT)),
                    );
                    let note = s(it, "review_note");
                    if !note.is_empty() {
                        ui.label(
                            egui::RichText::new(format!("{}：{}", tr("理由"), note))
                                .size(11.5)
                                .color(palette_color(TEXT_WEAK)),
                        );
                    }
                }
            });
            ui.add_space(12.0);
            // Source pages, resolved exactly like the deck view.
            if let Some(deck) = &deck {
                let resolved = deck
                    .get("resolved_entries")
                    .and_then(|r| r.as_array())
                    .cloned()
                    .unwrap_or_default();
                let mut pages: Vec<(String, usize, usize)> = Vec::new();
                for re in &resolved {
                    let ver = re.get("resolved").cloned().unwrap_or(Value::Null);
                    if !ver.is_object() {
                        continue;
                    }
                    let vid = s(&ver, "id");
                    let total = i(&ver, "page_count");
                    for p in 0..total {
                        pages.push((vid.clone(), p as usize, pages.len() + 1));
                    }
                }
                ui.horizontal_wrapped(|ui| {
                    for (vid, p, n) in &pages {
                        ui.vertical(|ui| {
                            self.thumb_ui(ui, vid, *p, 216.0);
                            ui.label(
                                egui::RichText::new(format!("{} {n} {}", tr("第"), tr("页")))
                                    .size(11.0)
                                    .color(palette_color(TEXT_FAINT)),
                            );
                        });
                    }
                });
            } else {
                ui.label(
                    egui::RichText::new(tr("来源索引暂不可用，无法预览页面。"))
                        .size(12.0)
                        .color(palette_color(TEXT_WEAK)),
                );
            }
            ui.add_space(14.0);
            if state == "pending" {
                ui.horizontal(|ui| {
                    if btn_primary(ui, tr("通过")).clicked() {
                        approve = true;
                    }
                    if !reject_open && btn_danger(ui, tr("驳回…")).clicked() {
                        open_reject = true;
                    }
                });
                if reject_open {
                    ui.add_space(8.0);
                    ui.label(
                        egui::RichText::new(tr("驳回理由（必填，将随结果一并记录）"))
                            .size(12.0)
                            .color(palette_color(TEXT_MAIN)),
                    );
                    if let ReviewFrame::Detail(d) = self.review.stack.top_mut() {
                        ui.add(
                            egui::TextEdit::multiline(&mut d.reject_text)
                                .desired_width(420.0)
                                .desired_rows(3),
                        );
                    }
                    ui.horizontal(|ui| {
                        let empty = match self.review.stack.top() {
                            ReviewFrame::Detail(d) => d.reject_text.trim().is_empty(),
                            _ => true,
                        };
                        if ui
                            .add_enabled(!empty, egui::Button::new(tr("确认驳回")))
                            .clicked()
                        {
                            confirm_reject = true;
                        }
                        if btn_default(ui, tr("取消")).clicked() {
                            cancel_reject = true;
                        }
                    });
                }
            }
            ui.add_space(14.0);
        });
        if open_reject {
            if let ReviewFrame::Detail(d) = self.review.stack.top_mut() {
                d.reject_open = true;
            }
        }
        if cancel_reject {
            if let ReviewFrame::Detail(d) = self.review.stack.top_mut() {
                d.reject_open = false;
            }
        }
        if approve && self.review_act(&item_id, "live", "") {
            self.review.stack.pop();
        }
        if confirm_reject {
            let note = match self.review.stack.top() {
                ReviewFrame::Detail(d) => d.reject_text.trim().to_string(),
                _ => String::new(),
            };
            if !note.is_empty() && self.review_act(&item_id, "rejected", &note) {
                self.review.stack.pop();
            }
        }
        if go_back {
            self.review.stack.pop();
        }
    }

    /// F1: the submission-package preview (approval-queue detail
    /// frame for a P3 package). Everything shown comes from the
    /// staged package itself: the manifest from
    /// GET /v1/submissions/:id/manifest, page renders from the
    /// package's own fragment bytes — never from the submitter's
    /// source library, whose current state may already have
    /// moved on. This is the face that makes review non-blind:
    /// approve / reject act from here exactly like the queue
    /// cards, on the content the reviewer is looking at.
    fn submission_detail_body(&mut self, ui: &mut egui::Ui) {
        let sub_id = match self.review.stack.top() {
            ReviewFrame::Submission(d) => d.sub_id.clone(),
            _ => return,
        };
        // Load the manifest once per detail frame.
        let need = match self.review.stack.top() {
            ReviewFrame::Submission(d) => !d.loaded,
            _ => false,
        };
        if need {
            let got = self.call(
                "GET",
                &format!("/v1/submissions/{sub_id}/manifest"),
                None,
            );
            if let ReviewFrame::Submission(d) = self.review.stack.top_mut() {
                match got {
                    Ok(v) => d.manifest = Some(v),
                    Err(e) => d.load_error = e,
                }
                d.loaded = true;
            }
        }
        let (payload, load_error, reject_open) = match self.review.stack.top() {
            ReviewFrame::Submission(d) => {
                (d.manifest.clone(), d.load_error.clone(), d.reject_open)
            }
            _ => (None, String::new(), false),
        };
        let mut go_back = false;
        let mut approve = false;
        let mut open_reject = false;
        let mut cancel_reject = false;
        let mut confirm_reject = false;
        egui::ScrollArea::vertical().show(ui, |ui| {
            let sub = payload
                .as_ref()
                .map(|p| p["submission"].clone())
                .unwrap_or(Value::Null);
            let title = {
                let t = s(&sub, "title");
                if t.is_empty() {
                    sub_id.clone()
                } else {
                    t
                }
            };
            if let Some(d) = crumb_bar(ui, &[tr("审批队列"), &title]) {
                if d == 0 {
                    go_back = true;
                }
            }
            ui.add_space(10.0);
            if !load_error.is_empty() {
                ui.label(
                    egui::RichText::new(format!("{}：{}", tr("读取提交包失败"), load_error))
                        .size(12.5)
                        .color(palette_color(TEXT_WEAK)),
                );
                return;
            }
            let Some(payload) = &payload else {
                ui.label(
                    egui::RichText::new(tr("正在读取提交包…"))
                        .size(12.5)
                        .color(palette_color(TEXT_WEAK)),
                );
                return;
            };
            let manifest = &payload["manifest"];
            let state = s(&sub, "state");
            card_frame().show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(
                        egui::RichText::new(s(&sub, "title"))
                            .size(14.0)
                            .color(palette_color(TEXT_MAIN)),
                    );
                    match state.as_str() {
                        "pending" => badge(ui, tr("待审"), BadgeTone::Warn),
                        "landed" => badge(ui, tr("已落地"), BadgeTone::Ok),
                        "rejected" => badge(ui, tr("已驳回"), BadgeTone::Bad),
                        _ => {}
                    }
                    badge(ui, tr("提交包"), BadgeTone::Info);
                });
                let intro = s(&sub, "intro");
                if !intro.is_empty() {
                    ui.label(
                        egui::RichText::new(intro)
                            .size(12.0)
                            .color(palette_color(TEXT_WEAK)),
                    );
                }
                ui.label(
                    egui::RichText::new(format!(
                        "{} {} · {} · {} {} · {} {}",
                        tr("提交人"),
                        s(&sub, "submitter_name"),
                        fmt_time(i(&sub, "created_ms")),
                        i(&sub, "entry_count"),
                        tr("个内容块"),
                        i(&sub, "page_count"),
                        tr("页")
                    ))
                    .size(11.0)
                    .color(palette_color(TEXT_FAINT)),
                );
                let note = s(&sub, "note");
                if !note.is_empty() {
                    ui.label(
                        egui::RichText::new(format!("{}：{}", tr("提交说明"), note))
                            .size(11.5)
                            .color(palette_color(TEXT_WEAK)),
                    );
                }
                let land_error = s(&sub, "land_error");
                if !land_error.is_empty() {
                    ui.label(
                        egui::RichText::new(format!("{}：{}", tr("上次落地失败"), land_error))
                            .size(11.5)
                            .color(palette_color(TEXT_WEAK)),
                    );
                }
                ui.label(
                    egui::RichText::new(format!(
                        "{}：{} · {} {}",
                        tr("落地索引"),
                        s(manifest, "index_name"),
                        tr("可见范围"),
                        s(manifest, "index_scope")
                    ))
                    .size(11.5)
                    .color(palette_color(TEXT_WEAK)),
                );
            });
            ui.add_space(12.0);
            // The package's entries, in commit order, each with
            // its resolved version's pages and speaker scripts —
            // the exact set an approval lands.
            let find_in = |arr: &Value, id: &str| -> Value {
                arr.as_array()
                    .into_iter()
                    .flatten()
                    .find(|x| s(x, "id") == id)
                    .cloned()
                    .unwrap_or(Value::Null)
            };
            let entries = manifest["commit"]["entries"]
                .as_array()
                .cloned()
                .unwrap_or_default();
            for (n, e) in entries.iter().enumerate() {
                let entry = &e["entry"];
                let unit = find_in(&manifest["units"], &s(entry, "unit_id"));
                let rend = find_in(&manifest["renditions"], &s(entry, "rendition_id"));
                let ver = find_in(&manifest["versions"], &s(e, "version_id"));
                let policy = if entry["policy"].is_object() {
                    tr("版本锁定")
                } else {
                    tr("跟随最新（落地钉为审核版本）")
                };
                ui.label(
                    egui::RichText::new(format!(
                        "{}. {} · {}/{} · v{} · {}",
                        n + 1,
                        s(&unit, "name"),
                        s(&rend, "form"),
                        s(&rend, "lang"),
                        s(&ver, "semver"),
                        policy
                    ))
                    .size(13.0)
                    .strong()
                    .color(palette_color(TEXT_MAIN)),
                );
                let total = i(&ver, "page_count");
                let slots = entry["excerpt_slots"]
                    .as_array()
                    .map(|a| a.len())
                    .unwrap_or(0);
                if slots > 0 {
                    ui.label(
                        egui::RichText::new(format!(
                            "{}：{} {}，{} {}；{}",
                            tr("摘页条目"),
                            tr("版本共"),
                            total,
                            tr("本条目选用"),
                            slots,
                            tr("下方为版本全部页面，落地按摘页选用")
                        ))
                        .size(11.5)
                        .color(palette_color(TEXT_WEAK)),
                    );
                }
                let hash = s(&ver, "fragment_hash");
                let scripts = ver["scripts"].as_array().cloned().unwrap_or_default();
                ui.horizontal_wrapped(|ui| {
                    for p in 0..total.max(0) as usize {
                        ui.vertical(|ui| {
                            ui.set_max_width(216.0);
                            self.sub_thumb_ui(ui, &sub_id, &hash, p, 216.0);
                            ui.label(
                                egui::RichText::new(format!(
                                    "{} {} {}",
                                    tr("第"),
                                    p + 1,
                                    tr("页")
                                ))
                                .size(11.0)
                                .color(palette_color(TEXT_FAINT)),
                            );
                            let script = scripts
                                .get(p)
                                .and_then(|x| x.as_str())
                                .unwrap_or_default();
                            if !script.is_empty() {
                                ui.label(
                                    egui::RichText::new(script)
                                        .size(11.0)
                                        .color(palette_color(TEXT_WEAK)),
                                );
                            }
                        });
                    }
                });
                ui.add_space(12.0);
            }
            if state == "pending" {
                ui.horizontal(|ui| {
                    if btn_primary(ui, tr("通过")).clicked() {
                        approve = true;
                    }
                    if !reject_open && btn_danger(ui, tr("驳回…")).clicked() {
                        open_reject = true;
                    }
                });
                if reject_open {
                    ui.add_space(8.0);
                    ui.label(
                        egui::RichText::new(tr("驳回理由（必填，会原样回到提交人）"))
                            .size(12.0)
                            .color(palette_color(TEXT_MAIN)),
                    );
                    if let ReviewFrame::Submission(d) = self.review.stack.top_mut() {
                        ui.add(
                            egui::TextEdit::multiline(&mut d.reject_text)
                                .desired_width(420.0)
                                .desired_rows(3),
                        );
                    }
                    ui.horizontal(|ui| {
                        let empty = match self.review.stack.top() {
                            ReviewFrame::Submission(d) => {
                                d.reject_text.trim().is_empty()
                            }
                            _ => true,
                        };
                        if ui
                            .add_enabled(!empty, egui::Button::new(tr("确认驳回")))
                            .clicked()
                        {
                            confirm_reject = true;
                        }
                        if btn_default(ui, tr("取消")).clicked() {
                            cancel_reject = true;
                        }
                    });
                }
            }
            ui.add_space(14.0);
        });
        if open_reject {
            if let ReviewFrame::Submission(d) = self.review.stack.top_mut() {
                d.reject_open = true;
            }
        }
        if cancel_reject {
            if let ReviewFrame::Submission(d) = self.review.stack.top_mut() {
                d.reject_open = false;
            }
        }
        if approve && self.review_submission_act(&sub_id, "live", "") {
            self.review.stack.pop();
        }
        if confirm_reject {
            let note = match self.review.stack.top() {
                ReviewFrame::Submission(d) => d.reject_text.trim().to_string(),
                _ => String::new(),
            };
            if !note.is_empty() && self.review_submission_act(&sub_id, "rejected", &note) {
                self.review.stack.pop();
            }
        }
        if go_back {
            self.review.stack.pop();
        }
    }

    fn showroom_body(&mut self, ui: &mut egui::Ui) {
        // Frame-state take-out, same pattern as content/index.
        // The deck view carries its own header (title + icon
        // actions); no "演示厅" heading above it (James 2026-11-15).
        if matches!(self.showroom.stack.top(), SrFrame::Wall) {
            self.showroom_wall(ui);
            return;
        }
        // The 讲稿视图 frame sits on top of the deck frame: the
        // same take-out pattern, and on close its saved scripts
        // fold back into the deck frame it restores.
        if matches!(self.showroom.stack.top(), SrFrame::Present(_)) {
            if let SrFrame::Present(mut pv) =
                self.showroom.stack.replace_top(SrFrame::Wall)
            {
                self.showroom_present(ui, &mut pv);
                if pv.closed {
                    let notes =
                        pv.pages.iter().map(|p| p.note.clone()).collect::<Vec<_>>();
                    self.showroom.stack.pop();
                    self.sync_present_notes(notes);
                } else {
                    self.showroom.stack.replace_top(SrFrame::Present(pv));
                }
            }
            return;
        }
        if let SrFrame::Deck(mut deck) = self.showroom.stack.replace_top(SrFrame::Wall) {
            self.showroom_deck(ui, &mut deck);
            if deck.closed {
                self.showroom.stack.pop();
            } else {
                self.showroom.stack.replace_top(SrFrame::Deck(deck));
            }
        }
    }

    /// The zone wall (root frame body): the plaza card grid, or
    /// the library / inbox / groups sub-view for the zone.
    fn showroom_wall(&mut self, ui: &mut egui::Ui) {
        // No zone-name heading here: content areas never display
        // board/zone names (user ruling 2026-11-24); the sidebar
        // segmented control already says which zone this is.
        if self.deep_source == Some(Page::Home) {
            ui.horizontal(|ui| {
                if text_action(ui, "‹ 返回主页", palette_color(ACCENT_DEEP), 13.0) {
                    self.deep_source = None;
                    self.page = Page::Home;
                }
            });
            ui.add_space(4.0);
        }
        if self.anon_active() && self.showroom.zone == "plaza" {
            ui.label(
                egui::RichText::new(tr("匿名连接：仅显示公开内容，发布与审批需实名登录"))
                    .size(12.0)
                    .color(palette_color(TEXT_WEAK)),
            );
            ui.add_space(4.0);
        }
        if self.showroom.zone == "library" {
            self.library_view(ui);
            return;
        }
        if self.showroom.zone == "inbox" {
            self.inbox_view(ui);
            return;
        }
        if self.showroom.zone == "groups" {
            self.groups_view(ui);
            return;
        }
        // Plaza folder filtering is by the item's OWN folder
        // assignment (option (a), 2026-11-25): the sidebar's
        // "showroom"-scope folders are the plaza's own tree, so
        // filtering by the source index's folder — a disjoint
        // id space since sidebar isolation — could only ever
        // empty the wall. 全部 (None) passes everything;
        // unassigned items pass only there.
        let items = self
            .showroom_live
            .iter()
            .filter(|item| {
                self.folder_filter_ok(
                    &self.showroom.folder,
                    &self.plaza_folder_of(&s(item, "id")),
                )
            })
            .cloned()
            .collect::<Vec<_>>();
        egui::ScrollArea::vertical()
            .id_salt("showroom-plaza")
            .show(ui, |ui| {
                self.showroom_cards(
                    ui,
                    &items,
                    Self::showroom_plaza_card,
                );
            });
    }

    fn showroom_plaza_card(&mut self, ui: &mut egui::Ui, item: &Value, width: f32) {
        let id = s(item, "id");
        let title = s(item, "title");
        let cover = self.showroom_cover(&id);
        let selected = self.showroom.selected_card.as_deref() == Some(id.as_str());
        let cover_rect = self.showroom_card(ui, width, &title, &cover, selected, |app, ui| {
            let intro = s(item, "intro");
            if !intro.is_empty() {
                ui.add(egui::Label::new(egui::RichText::new(intro).small().weak()).truncate());
            }
            if !app.org_online() && !app.sr_bundle_cached(&id) {
                // Offline with no complete local bundle (T3
                // ruling): the action row is a status label, not
                // buttons — every action here would land on an
                // empty deck or a failed request. The cover and
                // title stay as they are.
                ui.label(egui::RichText::new("离线不可用").small().weak());
                return;
            }
            ui.horizontal_wrapped(|ui| {
                if ui.small_button("打开详情").clicked() {
                    app.open_showroom_deck(item);
                }
                if ui.small_button("开始演示").clicked() {
                    // The deck frame stays as the return context;
                    // presenting tries the hosted show, then
                    // DeckCraft, and the built-in presenter is the
                    // fallback when neither engine could start it.
                    app.open_showroom_deck(item);
                    if !app.present_via_deckcraft(&id, &title, DcMode::Play, 0) {
                        app.start_present_from_deck(ui);
                    }
                }
                if app.org_online()
                    && !app
                        .library_refs
                        .iter()
                        .any(|entry| entry["item_id"] == item["id"])
                    && ui.small_button("保存到我的库").clicked()
                {
                    app.open_pull_dialog(item, None);
                }
                if app.org_online() && !s(item, "source_index_id").is_empty() {
                    let key = format!("sr_{id}");
                    let cached = app.cache_entries.iter().any(|entry| entry["key"] == key);
                    let label = if cached {
                        "已缓存（点击更新缓存）"
                    } else {
                        "缓存到本地"
                    };
                    if app.tool_slot(ui, cached, label, ChromeIcon::Download) {
                        app.cache_showroom_deck(item);
                    }
                }
                if app.org_online()
                    && (app.role() == "super"
                        || app
                            .user
                            .as_ref()
                            .is_some_and(|u| s(u, "id") == s(item, "submitted_by_id")))
                {
                    if text_action(ui, "删除", palette_color(BADGE_BAD_FG), 12.0) {
                        app.trash_add("showroom", &id);
                    }
                }
            });
        });
        let response = ui
            .interact(
                cover_rect,
                ui.scope_id().with(("plaza", &id)),
                egui::Sense::click(),
            )
            .on_hover_text(format!(
                "{} · {}",
                s(item, "submitted_by"),
                fmt_time(i(item, "created_ms"))
            ));
        // Move-to-folder (option (a) intake): a right-click menu
        // on the cover — the same response that takes the card's
        // click/double-click, and the same context-menu form the
        // sidebar folder rows use. The assignment is local state,
        // so the menu is offered offline and on cache-backfilled
        // cards too. The pick is captured and applied after the
        // menu closes (the folder_tree discipline).
        let mut move_pick: Option<String> = None;
        response.context_menu(|ui| {
            ui.menu_button("移动到文件夹", |ui| {
                let cur = self.plaza_folder_of(&id);
                let mut pick = |ui: &mut egui::Ui, target: &str, label: String| {
                    let text = if cur == target {
                        format!("✓ {label}")
                    } else {
                        label
                    };
                    if ui.button(text).clicked() {
                        move_pick = Some(target.to_string());
                        ui.close();
                    }
                };
                pick(ui, "", "无文件夹（仅在「全部」显示）".to_string());
                let options = self.plaza_folder_options();
                if !options.is_empty() {
                    ui.separator();
                    for (fid, name, depth) in options {
                        pick(
                            ui,
                            &fid,
                            format!("{}{name}", "　".repeat(depth)),
                        );
                    }
                }
            });
        });
        if let Some(target) = move_pick {
            self.set_plaza_item_folder(&id, &target);
        }
        if response.clicked() {
            self.showroom.selected_card = Some(id.clone());
        }
        if response.double_clicked()
            && (self.org_online() || self.sr_bundle_cached(&id))
        {
            self.open_showroom_deck(item);
        }
    }

    fn showroom_cover(&self, item_id: &str) -> String {
        let Some(item) = self.showroom_live.iter().find(|item| item["id"] == item_id) else {
            return String::new();
        };
        self.index_covers
            .get(&s(item, "source_index_id"))
            .cloned()
            .unwrap_or_default()
    }

    fn showroom_cards(
        &mut self,
        ui: &mut egui::Ui,
        items: &[Value],
        mut card: impl FnMut(&mut Self, &mut egui::Ui, &Value, f32),
    ) {
        // Empty walls render blank: no placeholder text
        // (user ruling 2026-11-24).
        if items.is_empty() {
            return;
        }
        ui.spacing_mut().item_spacing = egui::vec2(12.0, 12.0);
        let (columns, width) = showroom_columns(ui, 216.0);
        for row in items.chunks(columns) {
            ui.horizontal_top(|ui| {
                for item in row {
                    card(self, ui, item, (width - 2.0).max(1.0));
                }
            });
        }
    }

    fn showroom_card(
        &mut self,
        ui: &mut egui::Ui,
        width: f32,
        title: &str,
        cover: &str,
        selected: bool,
        body: impl FnOnce(&mut Self, &mut egui::Ui),
    ) -> egui::Rect {
        self.wall_card(
            ui,
            WallCard {
                width,
                min_height: 0.0,
                title,
                cover,
                selected,
                scope: None,
                variant: CardVariant::Normal,
            },
            body,
        )
        .inner
    }

    fn wall_card(
        &mut self,
        ui: &mut egui::Ui,
        card: WallCard<'_>,
        body: impl FnOnce(&mut Self, &mut egui::Ui),
    ) -> egui::InnerResponse<egui::Rect> {
        standard_card_v(ui, card.selected, card.scope, true, card.variant, |ui| {
            ui.set_width(card.width);
            ui.set_min_height(card.min_height);
            ui.spacing_mut().item_spacing.y = 0.0;
            let cover_rect = ui
                .allocate_ui(egui::vec2(card.width, card.width * 9.0 / 16.0), |ui| {
                    if card.cover.is_empty() {
                        slide_cover(ui, None, "", "", "", card.width);
                    } else {
                        self.thumb_ui(ui, card.cover, 0, card.width);
                    }
                })
                .response
                .rect;
            egui::Frame::NONE
                .inner_margin(egui::Margin {
                    left: 12,
                    right: 12,
                    top: 11,
                    bottom: 12,
                })
                .show(ui, |ui| {
                    ui.spacing_mut().item_spacing = egui::vec2(8.0, 5.0);
                    ui.add(
                        egui::Label::new(egui::RichText::new(card.title).strong().size(13.0))
                            .truncate(),
                    );
                    body(self, ui);
                });
            cover_rect
        })
    }

    /// 智能分块导入 (AI-analysis split). Retained for the
    /// future AI proposer: since the 2026-11-24 ruling its only
    /// toolbar slot renders as a greyed placeholder
    /// (tool_slot_placeholder in shell.rs's Content arm), so
    /// this has no live caller until the AI feature lands.
    fn act_import_split(&mut self) {
        if !self.need_identity_local("智能分块导入") {
            return;
        }
        if self.importing.is_some() {
            self.msg = "正在导入，请稍候…".into();
            return;
        }
        if let Some(path) = rfd::FileDialog::new()
            .add_filter("PowerPoint", &["pptx"])
            .pick_file()
        {
            if let Ok(bytes) = std::fs::read(&path) {
                let name = path
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_default();
                let folder_id = self.content.folder_filter.clone();
                let group_id = self.content.import_group.clone();
                let (local, server, token, tx) = (
                    self.local.clone(),
                    self.server.clone(),
                    self.token.clone(),
                    self.import_tx.clone(),
                );
                let gen = self.import_gen;
                self.importing = Some(name.clone());
                self.msg = format!("正在导入「{name}」…");
                // The propose + import sequence runs on a worker
                // thread (the translation precedent): both calls
                // parse and rebuild the whole deck, which froze
                // the interface for the duration when it ran here.
                let wake = self.egui_ctx.clone();
                std::thread::spawn(move || {
                    let data = b64e(&bytes);
                    let done = (|| -> Result<String, String> {
                        let p = work_call_thread(
                            &local,
                            &server,
                            token.as_deref(),
                            "POST",
                            "/v1/import/propose",
                            Some(json!({ "data_base64": &data })),
                        )
                        .map_err(|e| format!("提块失败：{e}"))?;
                        let blocks = p.get("blocks").cloned().unwrap_or(json!([]));
                        let names: Vec<String> = blocks
                            .as_array()
                            .map(|arr| arr.iter().map(|b| s(b, "name")).collect())
                            .unwrap_or_default();
                        staged_import_thread(
                            &local,
                            &server,
                            token.as_deref(),
                            &tx,
                            gen,
                            &name,
                            &bytes,
                            &folder_id,
                            &group_id,
                            Some(blocks),
                            wake.as_ref(),
                        )
                        .map_err(|e| format!("导入失败：{e}"))?;
                        Ok(format!(
                            "智能分块导入完成：{} 块（{}）",
                            names.len(),
                            names.join("、")
                        ))
                    })();
                    let _ = tx.send(ImportEvent::Done(gen, done));
                    if let Some(ctx) = &wake {
                        ctx.request_repaint();
                    }
                });
                if let Some(ctx) = &self.egui_ctx {
                    ctx.request_repaint();
                }
            }
        }
    }

    // ---- lazy import (2026-11-25) ----

    fn lazy_send(&self, cmd: LazyCmd) {
        if let Some(tx) = &self.lazy_cmd_tx {
            let _ = tx.send(cmd);
        }
    }

    fn lazy_ensure_worker(&mut self) {
        if self.lazy_cmd_tx.is_some() {
            return;
        }
        let Some(local) = self.local.clone() else {
            return;
        };
        let (tx, rx) = std::sync::mpsc::channel();
        let event_tx = self.import_tx.clone();
        let wake = self.lazy_wake_ctx.clone();
        let turbo = self.lazy_turbo.clone();
        std::thread::spawn(move || {
            lazy_worker_thread(local, rx, event_tx, wake, turbo);
        });
        self.lazy_cmd_tx = Some(tx);
    }

    /// Adopt one durable split state (from lazy-list) into the
    /// UI mirror: the deck's unsplit unit -> page map is rebuilt
    /// wholesale, so a sync after grouping can never leave stale
    /// "unusable" entries behind.
    fn lazy_adopt_state(&mut self, st: &Value) {
        let pid = s(st, "pending_id");
        if pid.is_empty() {
            return;
        }
        self.lazy_unit_page.retain(|_, (p, _)| p != &pid);
        if let Some(pages) = st.get("pages").and_then(|p| p.as_array()) {
            for pg in pages {
                if pg.get("split").and_then(|x| x.as_bool()) != Some(true) {
                    let uid = s(pg, "unit_id");
                    if !uid.is_empty() {
                        self.lazy_unit_page
                            .insert(uid, (pid.clone(), i(pg, "index") as usize));
                    }
                }
            }
        }
        self.lazy_pendings.insert(
            pid,
            LazyPendingUi {
                stem: s(st, "stem"),
                slide_count: i(st, "slide_count") as usize,
                split_count: i(st, "split_count") as usize,
                failed: s(st, "state") == "failed",
                error: s(st, "error"),
            },
        );
    }

    fn lazy_sync_pending(&mut self, pending_id: &str) {
        if let Ok(v) = self.call_work("POST", "/v1/import/lazy-list", Some(json!({}))) {
            if let Some(st) = v
                .as_array()
                .and_then(|a| a.iter().find(|x| s(x, "pending_id") == pending_id))
            {
                let st = st.clone();
                self.lazy_adopt_state(&st);
            }
        }
    }

    /// Startup resume: re-adopt every durable pending import
    /// and restart the worker if any deck is still splitting.
    fn lazy_resume(&mut self) {
        if self.local.is_none() {
            return;
        }
        if let Ok(v) = self.call_work("POST", "/v1/import/lazy-list", Some(json!({}))) {
            if let Some(arr) = v.as_array() {
                let states = arr.clone();
                for st in &states {
                    self.lazy_adopt_state(st);
                }
            }
        }
        if !self.lazy_pendings.is_empty() {
            self.lazy_ensure_worker();
            self.lazy_send(LazyCmd::Kick);
        }
    }

    /// Clicking an unsplit page card: the page jumps to the
    /// front of the split queue — the user's one explicit
    /// priority entry — with visible feedback.
    fn lazy_boost(&mut self, pending_id: &str, index: usize) {
        self.lazy_ensure_worker();
        self.lazy_send(LazyCmd::Boost(vec![(pending_id.to_string(), index)]));
        let stem = self
            .lazy_pendings
            .get(pending_id)
            .map(|p| p.stem.clone())
            .unwrap_or_default();
        self.msg = format!("已优先拆分「{stem}」第 {} 页，马上就好", index + 1);
    }

    fn lazy_on_progress(&mut self, info: LazyProgressInfo) {
        if info.done {
            let stem = self
                .lazy_pendings
                .get(&info.pending_id)
                .map(|p| p.stem.clone())
                .unwrap_or_default();
            self.lazy_pendings.remove(&info.pending_id);
            self.lazy_unit_page
                .retain(|_, (p, _)| p != &info.pending_id);
            self.msg = format!("「{stem}」分割完成");
            // Phase-1 (design §3.1 point 2): the terminal beat —
            // full local snapshot + index meta, consumed at once;
            // the split throttle never defers completion.
            self.mark(Change::LocalWork);
            self.mark(Change::IndexMeta);
            self.flush_dirty_now();
            return;
        }
        if info.failed {
            if let Some(p) = self.lazy_pendings.get_mut(&info.pending_id) {
                p.failed = true;
                p.error = info.error.clone();
                p.split_count = info.split_count;
            }
            self.msg = format!("分割失败：{}", info.error);
            return;
        }
        // The page that just split leaves the unusable set.
        self.lazy_unit_page
            .retain(|_, (p, idx)| !(p == &info.pending_id && *idx == info.index));
        if let Some(p) = self.lazy_pendings.get_mut(&info.pending_id) {
            p.split_count = info.split_count;
            p.slide_count = info.slide_count;
        }
        if info.grouped {
            // Folding absorbed wrappers into a block unit: the
            // wall's unit set changed, and this deck's remaining
            // map entries are rebuilt exactly.
            self.lazy_sync_pending(&info.pending_id);
            // Phase-1 (design §3.1 point 1): the old code paid a
            // full refresh_all per folded block (S per deck);
            // now the beat only marks Units+Indexes — the frame
            // point consumes them, throttled to one local
            // re-read per 500ms while the split runs.
            self.changed(Change::Units);
            self.changed(Change::Indexes);
        }
    }

    /// Toolbar split status (shell tool row): present only
    /// while some deck is still splitting (or one failed) —
    /// aggregate progress, a thin bar, and the turbo toggle;
    /// disappears on its own when the last deck finishes.
    fn lazy_status_slot(&mut self, ui: &mut egui::Ui) {
        if self.lazy_pendings.is_empty() {
            return;
        }
        let total: usize = self.lazy_pendings.values().map(|p| p.slide_count).sum();
        let done_n: usize = self.lazy_pendings.values().map(|p| p.split_count).sum();
        let failed: Vec<String> = self
            .lazy_pendings
            .iter()
            .filter(|(_, p)| p.failed)
            .map(|(id, _)| id.clone())
            .collect();
        ui.add_space(10.0);
        ui.separator();
        if !failed.is_empty() {
            let first_error = failed
                .first()
                .and_then(|id| self.lazy_pendings.get(id))
                .map(|p| p.error.clone())
                .unwrap_or_default();
            ui.label(
                egui::RichText::new("分割失败")
                    .size(12.5)
                    .color(egui::Color32::from_rgb(0xD9, 0x5F, 0x5F)),
            )
            .on_hover_text(first_error);
            if ui.small_button("忽略").clicked() {
                for pid in &failed {
                    let _ = self.call_work(
                        "POST",
                        "/v1/import/lazy-forget",
                        Some(json!({ "pending_id": pid })),
                    );
                    self.lazy_pendings.remove(pid);
                    self.lazy_unit_page.retain(|_, (p, _)| p != pid);
                }
                self.msg = "已忽略分割失败的导入记录（已拆好的页不受影响）".into();
            }
            return;
        }
        ui.label(
            egui::RichText::new(format!("分割中 {done_n}/{total} 页"))
                .size(12.5)
                .color(palette_color(TEXT_WEAK)),
        );
        let frac = if total > 0 {
            done_n as f32 / total as f32
        } else {
            0.0
        };
        let (rect, _) = ui.allocate_exact_size(egui::vec2(84.0, 5.0), egui::Sense::hover());
        let painter = ui.painter();
        painter.rect_filled(rect, 2.5, palette_color(CHIP_GRAY_BG));
        let mut fill = rect;
        fill.max.x = rect.min.x + rect.width() * frac.clamp(0.0, 1.0);
        painter.rect_filled(fill, 2.5, ACCENT);
        let turbo = self.lazy_turbo.load(std::sync::atomic::Ordering::Relaxed);
        let label = if turbo { "极速分割·开" } else { "极速分割" };
        if ui.small_button(label).clicked() {
            let on = !turbo;
            self.lazy_turbo
                .store(on, std::sync::atomic::Ordering::Relaxed);
            self.msg = if on {
                "极速分割：全速拆分中".into()
            } else {
                "已退出极速分割".into()
            };
        }
    }

    /// 导入 PPT (content page action, also its toolbar slot): import
    /// a whole deck into the current folder/group. The import call
    /// itself runs on a worker thread (see act_import_split); the
    /// completion event lands in the drain in `ui`, which shows
    /// the message and refreshes.
    fn act_import_ppt(&mut self) {
        if !self.need_identity_local("导入 PPT") {
            return;
        }
        if self.importing.is_some() {
            self.msg = "正在导入，请稍候…".into();
            return;
        }
        if let Some(path) = rfd::FileDialog::new()
            .add_filter("PowerPoint", &["pptx"])
            .pick_file()
        {
            if let Ok(bytes) = std::fs::read(&path) {
                let name = path
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_default();
                let folder_id = self.content.folder_filter.clone();
                let group_id = self.content.import_group.clone();
                let (local, server, token, tx) = (
                    self.local.clone(),
                    self.server.clone(),
                    self.token.clone(),
                    self.import_tx.clone(),
                );
                let gen = self.import_gen;
                self.importing = Some(name.clone());
                if local.is_some() {
                    // Lazy import (user ruling 2026-11-25): open
                    // only — the deck lands on the wall at once,
                    // backed by the whole file, and the split
                    // worker turns its pages into entities in the
                    // background. The REST path below is
                    // unchanged (server imports stay staged
                    // one-shot this round).
                    self.msg = format!("正在打开「{name}」…");
                    let wake = self.lazy_wake_ctx.clone();
                    std::thread::spawn(move || {
                        let body = json!({
                            "filename": name,
                            "data_base64": b64e(&bytes),
                            "folder_id": folder_id,
                            "group_id": group_id,
                        });
                        let r = work_call_thread(
                            &local,
                            &server,
                            token.as_deref(),
                            "POST",
                            "/v1/import/lazy-open",
                            Some(body),
                        );
                        match r {
                            Ok(v) => {
                                let _ = tx.send(ImportEvent::LazyOpened(s(&v, "pending_id")));
                            }
                            Err(e) => {
                                let _ =
                                    tx.send(ImportEvent::Done(gen, Err(format!("导入失败：{e}"))));
                            }
                        }
                        if let Some(ctx) = &wake {
                            ctx.request_repaint();
                        }
                    });
                    if let Some(ctx) = &self.egui_ctx {
                        ctx.request_repaint();
                    }
                    return;
                }
                self.msg = format!("正在导入「{name}」…");
                let wake = self.egui_ctx.clone();
                std::thread::spawn(move || {
                    let r = staged_import_thread(
                        &local,
                        &server,
                        token.as_deref(),
                        &tx,
                        gen,
                        &name,
                        &bytes,
                        &folder_id,
                        &group_id,
                        None,
                        wake.as_ref(),
                    );
                    let done = match r {
                        Ok(v) => Ok(format!(
                            "导入完成：{} 页，校验通过",
                            v.get("slide_count").and_then(|x| x.as_i64()).unwrap_or(0)
                        )),
                        Err(e) => Err(format!("导入失败：{e}")),
                    };
                    let _ = tx.send(ImportEvent::Done(gen, done));
                    if let Some(ctx) = &wake {
                        ctx.request_repaint();
                    }
                });
                if let Some(ctx) = &self.egui_ctx {
                    ctx.request_repaint();
                }
            }
        }
    }

    fn ct_main_body(&mut self, ui: &mut egui::Ui) {
        // Frame-state take-out: a frame's form state is rendered
        // alongside &mut self, which borrowing the stack would
        // forbid. A placeholder wall frame sits in the taken
        // frame's place meanwhile — it is never rendered; the
        // taken frame is what gets drawn.
        let depth = self.content.stack.depth();
        let mut frame = self
            .content
            .stack
            .replace_top(ContentFrame::Wall(WallState::default()));
        let closed = match &mut frame {
            ContentFrame::Unit(st) => {
                self.rendering_unit = Some(st.unit_id.clone());
                body_frame().show(ui, |ui| {
                    if self.pick.is_some() {
                        self.pick_bar(ui);
                    }
                    self.unit_detail(ui, st);
                });
                self.rendering_unit = None;
                st.closed
            }
            ContentFrame::Wall(w) => {
                if self.content.cache_open && self.pick.is_none() {
                    self.content_cache_body(ui);
                } else {
                    self.content_wall(ui, w);
                }
                false
            }
        };
        if self.content.stack.depth() == depth {
            if closed {
                self.content.stack.pop();
            } else {
                self.content.stack.replace_top(frame);
            }
        }
        if let Some(unit_id) = self.pending_unit_reload.take() {
            self.reload_unit_detail(&unit_id);
        }
    }

    /// The maintenance wall (root frame body): the block card
    /// grid (plus the deep-jump back link when arriving from Home).
    fn content_wall(&mut self, ui: &mut egui::Ui, w: &mut WallState) {
        body_frame().show(ui, |ui| {
            if self.pick.is_some() {
                self.pick_bar(ui);
            }
            if self.pick.is_none() {
                ui.horizontal(|ui| {
                    if self.deep_source == Some(Page::Home) {
                        if text_action(ui, "‹ 返回主页", palette_color(ACCENT_DEEP), 13.0) {
                            self.deep_source = None;
                            self.page = Page::Home;
                        }
                    }
                    // No search box, no import actions and no
                    // import-group picker on the wall (user ruling
                    // 2026-11-24): the wall is the block grid only.
                    // Imports stay on the home / sidebar entries
                    // (act_import_ppt is wired there).
                });
            } // pick.is_none() toolbar block
            ui.add_space(12.0);
            if let Some(hits) = w.search_hits.clone() {
                ui.label(egui::RichText::new("搜索结果").strong());
                if let Some(arr) = hits.get("hits").and_then(|h| h.as_array()) {
                    for h in arr {
                        let u = h.get("unit").cloned().unwrap_or(json!({}));
                        let v = h.get("version").cloned().unwrap_or(json!({}));
                        if ui
                            .button(format!("{} · v{}", s(&u, "name"), s(&v, "semver")))
                            .clicked()
                        {
                            self.open_unit_detail(&s(&u, "id"));
                        }
                    }
                }
                ui.add_space(8.0);
            }
            let my_name = self
                .user
                .as_ref()
                .map(|u| s(u, "display_name"))
                .unwrap_or_default();
            egui::ScrollArea::vertical()
                .id_salt("content-wall")
                .show(ui, |ui| {
                    ui.spacing_mut().item_spacing = egui::vec2(12.0, 12.0);
                    // Card grid (prototype .grid): fluid columns, min
                    // 220px wide, 12px gaps.
                    let avail = ui.available_width();
                    let per_row = ((avail + 12.0) / 232.0).floor().max(1.0);
                    let card_w = (avail + 12.0) / per_row - 12.0;
                    let cover_h = card_w * 9.0 / 16.0;
                    let card_h = cover_h + 89.0;
                    // Folder flow (user ruling 2026-11-24): the wall
                    // does NOT filter to the sidebar's folder any
                    // more. Every folder renders as one continuous
                    // block — header row (folder name + its note in
                    // small type), a rule under the header, then the
                    // folder's cards — in depth-first tree order;
                    // a folder with no blocks keeps a one-card-high
                    // placeholder so its section still shows.
                    // Folder-less blocks close the flow as one
                    // untitled group (no fake folder name). The
                    // sidebar selection only scrolls the matching
                    // block into view; it stays the create / import
                    // target folder, never a display filter.
                    // Scope isolation (user ruling 2026-11-25): the
                    // flow shows the content board's own folders
                    // only — the same set as the page's left
                    // sidebar tree (scope "content"). Other boards'
                    // folders (showroom plaza, index, …) never
                    // render as blocks here.
                    let mut folders = self.folders.clone();
                    folders.retain(|f| s(f, "scope") == "content");
                    folders.sort_by_key(|f| i(f, "sort"));
                    fn depth_of(folders: &[Value], f: &Value) -> usize {
                        let mut d = 0;
                        let mut cur = s(f, "parent_id");
                        while !cur.is_empty() && d < 8 {
                            d += 1;
                            match folders.iter().find(|x| s(x, "id") == cur) {
                                Some(p) => cur = s(p, "parent_id"),
                                None => break,
                            }
                        }
                        d
                    }
                    let mut ordered: Vec<Value> = Vec::new();
                    fn walk(folders: &[Value], parent: &str, out: &mut Vec<Value>) {
                        for f in folders {
                            if s(f, "parent_id") == parent {
                                out.push(f.clone());
                                let id = s(f, "id");
                                walk(folders, &id, out);
                            }
                        }
                    }
                    walk(&folders, "", &mut ordered);
                    for f in &folders {
                        if !ordered.iter().any(|x| s(x, "id") == s(f, "id")) {
                            ordered.push(f.clone());
                        }
                    }
                    let all_units = self.units.clone();
                    let unit_folder = |it: &Value| -> String {
                        s(&it.get("unit").cloned().unwrap_or(json!({})), "folder_id")
                    };
                    // Scroll anchoring: position only when the
                    // sidebar selection changed since the wall was
                    // last positioned for it.
                    let want_anchor = self.content.folder_filter.clone();
                    let do_scroll = w.flow_anchor.as_ref() != Some(&want_anchor);
                    if do_scroll && want_anchor.is_none() {
                        ui.scroll_to_cursor(Some(egui::Align::TOP));
                    }
                    // Folder-flow virtualization (2026-11-25): once
                    // the wall holds FLOW_VIRTUALIZE_MIN blocks or
                    // more, a folder header or a card row whose rect
                    // misses the visible band (the scroll viewport,
                    // widened by one screenful above and below) is
                    // replaced by an equal-height spacer instead of
                    // built widgets. Positions, the total content
                    // height and the scrollbar are unchanged: every
                    // spacer advances the cursor by exactly the
                    // amount the real widgets would have (see
                    // FlowMetrics for where the heights come from,
                    // and flow_grid_virtual for the row geometry).
                    // Anything not yet measured builds for real, so
                    // the first frames render exactly like the
                    // un-virtualized wall; below the threshold the
                    // wall takes the original path verbatim.
                    let virt = all_units.len() >= FLOW_VIRTUALIZE_MIN;
                    let clip = ui.clip_rect();
                    let band = egui::Rect::from_min_max(
                        egui::pos2(clip.min.x, clip.min.y - clip.height()),
                        egui::pos2(clip.max.x, clip.max.y + clip.height()),
                    );
                    if (w.flow_metrics_ppp - ui.ctx().pixels_per_point()).abs() > 0.001 {
                        w.flow_metrics = FlowMetrics::default();
                        w.flow_metrics_ppp = ui.ctx().pixels_per_point();
                    }
                    let per_row_n = (per_row as usize).max(1);
                    let mut first_block = true;
                    for f in &ordered {
                        let fid = s(f, "id");
                        let depth = depth_of(&folders, f);
                        let items: Vec<Value> = all_units
                            .iter()
                            .filter(|it| unit_folder(it) == fid)
                            .cloned()
                            .collect();
                        if !first_block {
                            ui.add_space(14.0);
                        }
                        first_block = false;
                        let is_anchor =
                            do_scroll && want_anchor.as_deref() == Some(fid.as_str());
                        let editing_this =
                            matches!(&w.flow_edit, Some(e) if e.folder_id == fid);
                        // On the virtualized path a header fully
                        // outside the band becomes one spacer
                        // allocation whose advance equals the
                        // header's measured advance (a single
                        // allocation advances by its height plus
                        // the layout's trailing item spacing,
                        // hence the −12). The scroll-anchor target
                        // and the folder under edit always build
                        // for real: the anchor needs the header's
                        // rect, and the editor must not be dropped.
                        let header_spacer = if virt && !is_anchor && !editing_this {
                            let top = ui.cursor().min.y;
                            w.flow_metrics.header_adv_for(&fid, depth).filter(|adv| {
                                !band.intersects(egui::Rect::from_min_size(
                                    egui::pos2(band.min.x, top),
                                    egui::vec2(band.width(), *adv),
                                ))
                            })
                        } else {
                            None
                        };
                        if let Some(adv) = header_spacer {
                            ui.allocate_exact_size(
                                egui::vec2(ui.available_width(), (adv - 12.0).max(0.0)),
                                egui::Sense::hover(),
                            );
                        } else {
                            let y0 = ui.cursor().min.y;
                            self.flow_folder_header(ui, w, f, depth, is_anchor);
                            if !editing_this {
                                let adv = ui.cursor().min.y - y0;
                                w.flow_metrics.record_header(&fid, depth, adv);
                            }
                        }
                        if items.is_empty() {
                            // Empty folder: a one-card-high frame
                            // where its cards would be (the
                            // content_add_card placeholder's frame,
                            // without its actions).
                            let (rect, _) = ui.allocate_exact_size(
                                egui::vec2(card_w, card_h),
                                egui::Sense::hover(),
                            );
                            ui.painter().rect_stroke(
                                rect,
                                6.0,
                                egui::Stroke::new(1.0, palette_color(CARD_BORDER)),
                                egui::StrokeKind::Middle,
                            );
                        } else if virt {
                            self.flow_grid_virtual(
                                ui,
                                w,
                                &items,
                                card_w,
                                card_h,
                                cover_h,
                                per_row_n,
                                &my_name,
                                band,
                                is_anchor,
                            );
                        } else {
                            ui.horizontal_wrapped(|ui| {
                                for item in &items {
                                    self.flow_unit_card(ui, w, item, card_w, card_h, &my_name);
                                }
                            });
                        }
                        ui.add_space(12.0);
                    }
                    // Folder-less blocks (and blocks whose folder
                    // no longer exists): one untitled card group
                    // closing the flow. A block filed under another
                    // board's folder is NOT loose — it belongs to
                    // that board and stays out of this flow.
                    let root_items: Vec<Value> = all_units
                        .iter()
                        .filter(|it| {
                            let fid = unit_folder(it);
                            fid.is_empty()
                                || !self.folders.iter().any(|f| s(f, "id") == fid)
                        })
                        .cloned()
                        .collect();
                    if !root_items.is_empty() {
                        if !first_block {
                            ui.add_space(14.0);
                        }
                        if virt {
                            self.flow_grid_virtual(
                                ui,
                                w,
                                &root_items,
                                card_w,
                                card_h,
                                cover_h,
                                per_row_n,
                                &my_name,
                                band,
                                false,
                            );
                        } else {
                            ui.horizontal_wrapped(|ui| {
                                for item in &root_items {
                                    self.flow_unit_card(ui, w, item, card_w, card_h, &my_name);
                                }
                            });
                        }
                    }
                    if do_scroll {
                        w.flow_anchor = Some(want_anchor);
                    }
                });
        });
    }

    /// The folder-flow card grid on the virtualized path: the
    /// same rows `horizontal_wrapped` would form — fluid columns
    /// of `card_w` with 12pt gaps, a row's height the tallest
    /// card in it, cards centred within their row (`ui.horizontal`
    /// is the wrapped row's own layout, minus the wrap) — but a
    /// row whose rect misses `band` is one equal-height spacer
    /// allocation instead of built cards. A spacer row advances
    /// the cursor by its height plus the layout's trailing item
    /// spacing, exactly the advance of the real row widget, so
    /// rows below it land at the same positions either way. Row
    /// heights come from the measured per-variant card heights
    /// (FlowMetrics); a row holding an unmeasured variant builds
    /// for real, as does every row of the scroll-anchor target
    /// block (`force_real`).
    #[allow(clippy::too_many_arguments)]
    fn flow_grid_virtual(
        &mut self,
        ui: &mut egui::Ui,
        w: &mut WallState,
        items: &[Value],
        card_w: f32,
        card_h: f32,
        cover_h: f32,
        per_row: usize,
        my_name: &str,
        band: egui::Rect,
        force_real: bool,
    ) {
        let insert = matches!(self.pick, Some(PickState::Insert { .. }));
        for row in items.chunks(per_row) {
            let row_h: Option<f32> = if force_real {
                None
            } else {
                let mut acc = 0.0f32;
                let mut measured = true;
                for item in row {
                    match w
                        .flow_metrics
                        .card_h(insert, flow_item_has_version(item), cover_h)
                    {
                        Some(h) => acc = acc.max(h),
                        None => {
                            measured = false;
                            break;
                        }
                    }
                }
                if measured {
                    Some(acc)
                } else {
                    None
                }
            };
            let top = ui.cursor().min.y;
            let spacer = match row_h {
                Some(h)
                    if !band.intersects(egui::Rect::from_min_size(
                        egui::pos2(band.min.x, top),
                        egui::vec2(band.width(), h),
                    )) =>
                {
                    Some(h)
                }
                _ => None,
            };
            if let Some(h) = spacer {
                ui.allocate_exact_size(
                    egui::vec2(ui.available_width(), h),
                    egui::Sense::hover(),
                );
            } else {
                ui.horizontal(|ui| {
                    for item in row {
                        self.flow_unit_card(ui, w, item, card_w, card_h, my_name);
                    }
                });
            }
        }
    }

    /// One folder block header in the content wall's folder flow:
    /// the folder name on the left (strong; its size steps down
    /// with tree depth) with the folder's note in small type
    /// beside it, and a solid rule underneath. Both texts edit in
    /// place on double-click (FlowEdit). `scroll_here` positions
    /// this block at the top of the wall (sidebar selection).
    fn flow_folder_header(
        &mut self,
        ui: &mut egui::Ui,
        w: &mut WallState,
        f: &Value,
        depth: usize,
        scroll_here: bool,
    ) {
        let fid = s(f, "id");
        let name = s(f, "name");
        let note = s(f, "note");
        let name_size = match depth {
            0 => 15.0,
            1 => 13.5,
            _ => 12.5,
        };
        let editing_name = matches!(
            &w.flow_edit,
            Some(e) if e.folder_id == fid && e.field == FlowField::Name
        );
        let editing_note = matches!(
            &w.flow_edit,
            Some(e) if e.folder_id == fid && e.field == FlowField::Note
        );
        let row = ui.horizontal(|ui| {
            if depth > 0 {
                ui.add_space(depth as f32 * 16.0);
            }
            if editing_name {
                self.flow_edit_field(ui, w, 240.0);
            } else {
                let resp = ui.add(
                    egui::Label::new(
                        egui::RichText::new(&name)
                            .strong()
                            .size(name_size)
                            .color(palette_color(TEXT_MAIN)),
                    )
                    .sense(egui::Sense::click()),
                );
                if resp.double_clicked() {
                    w.flow_edit = Some(FlowEdit {
                        folder_id: fid.clone(),
                        field: FlowField::Name,
                        text: name.clone(),
                    });
                    w.flow_edit_focus = true;
                }
            }
            ui.add_space(4.0);
            if editing_note {
                self.flow_edit_field(ui, w, 300.0);
            } else if note.is_empty() {
                // An empty note still needs its double-click
                // target: a faint hint inside the control (a
                // control placeholder, not a wall empty-state
                // text — the distinction the 2026-11-24
                // empty-text ruling keeps).
                let resp = ui.add(
                    egui::Label::new(
                        egui::RichText::new(tr("添加备注"))
                            .size(12.0)
                            .color(palette_color(TEXT_WEAK).gamma_multiply(0.55)),
                    )
                    .sense(egui::Sense::click()),
                );
                if resp.double_clicked() {
                    w.flow_edit = Some(FlowEdit {
                        folder_id: fid.clone(),
                        field: FlowField::Note,
                        text: String::new(),
                    });
                    w.flow_edit_focus = true;
                }
            } else {
                let resp = ui.add(
                    egui::Label::new(
                        egui::RichText::new(&note)
                            .size(12.0)
                            .color(palette_color(TEXT_WEAK)),
                    )
                    .sense(egui::Sense::click()),
                );
                if resp.double_clicked() {
                    w.flow_edit = Some(FlowEdit {
                        folder_id: fid.clone(),
                        field: FlowField::Note,
                        text: note.clone(),
                    });
                    w.flow_edit_focus = true;
                }
            }
        });
        if scroll_here {
            ui.scroll_to_rect(row.response.rect, Some(egui::Align::TOP));
        }
        ui.add_space(6.0);
        let (rect, _) =
            ui.allocate_exact_size(egui::vec2(ui.available_width(), 1.0), egui::Sense::hover());
        ui.painter().rect_filled(rect, 0.0, palette_color(SEPARATOR));
    }

    /// The in-place text field of a folder-flow header edit.
    /// Losing focus commits (Enter surrenders focus, so it
    /// commits too — the unit-detail header editor's contract);
    /// Esc cancels.
    fn flow_edit_field(&mut self, ui: &mut egui::Ui, w: &mut WallState, width: f32) {
        let mut commit = false;
        let mut cancel = false;
        if let Some(edit) = &mut w.flow_edit {
            let resp = ui.add_sized([width, 26.0], egui::TextEdit::singleline(&mut edit.text));
            if w.flow_edit_focus {
                resp.request_focus();
                w.flow_edit_focus = false;
            }
            if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                cancel = true;
            } else if resp.lost_focus() {
                commit = true;
            }
        }
        if commit {
            self.flow_edit_commit(w);
        }
        if cancel {
            w.flow_edit = None;
        }
    }

    /// Commit a folder-flow header edit: the name goes through
    /// the rename endpoint, the note through the note endpoint —
    /// both served by the core store on the local dispatch and
    /// the server alike. An emptied name is refused (folder names
    /// are never blank); an emptied note simply clears the note.
    fn flow_edit_commit(&mut self, w: &mut WallState) {
        if !self.need_identity_local("编辑文件夹") {
            return;
        }
        let Some(edit) = w.flow_edit.take() else {
            return;
        };
        match edit.field {
            FlowField::Name => {
                let name = edit.text.trim().to_string();
                if name.is_empty() {
                    self.msg = "文件夹名不能为空".into();
                    return;
                }
                match self.call_work(
                    "POST",
                    &format!("/v1/folders/{}/rename", edit.folder_id),
                    Some(json!({ "name": name })),
                ) {
                    Ok(_) => {
                        self.msg = "已重命名".into();
                        self.reload_folders();
                    }
                    Err(e) => self.msg = format!("重命名失败：{e}"),
                }
            }
            FlowField::Note => {
                let note = edit.text.trim().to_string();
                match self.call_work(
                    "POST",
                    &format!("/v1/folders/{}/note", edit.folder_id),
                    Some(json!({ "note": note })),
                ) {
                    Ok(_) => {
                        self.msg = "已保存备注".into();
                        self.reload_folders();
                    }
                    Err(e) => self.msg = format!("备注保存失败：{e}"),
                }
            }
        }
    }

    /// One block card inside a folder-flow section (or the
    /// folder-less tail group): the standard wall card and its
    /// click contract, unchanged from the filtered wall — single
    /// click selects, Ctrl/⌘-click toggles the multi-selection,
    /// double-click opens the block's drill-down.
    fn flow_unit_card(
        &mut self,
        ui: &mut egui::Ui,
        w: &mut WallState,
        item: &Value,
        card_w: f32,
        card_h: f32,
        my_name: &str,
    ) {
        let unit = item.get("unit").cloned().unwrap_or(json!({}));
        let uid = s(&unit, "id");
        let rends = item
            .get("renditions")
            .and_then(|r| r.as_array())
            .cloned()
            .unwrap_or_default();
        let latest = rends
            .first()
            .and_then(|r| r.get("latest"))
            .cloned()
            .unwrap_or(Value::Null);
        let title = s(&unit, "name");
        let cover = s(&latest, "id");
        // Lazy import: a wrapper whose page has not split yet is
        // display-only — it renders (its cover comes from the
        // backing deck) but is not usable: no selection ring, no
        // open, no insert; clicking it boosts its split.
        let lazy_page = self.lazy_unit_page.get(&uid).cloned();
        let unsplit = lazy_page.is_some();
        let outer = self.wall_card(
            ui,
            WallCard {
                width: card_w,
                min_height: card_h,
                title: &title,
                cover: &cover,
                selected: !unsplit
                    && (w.selected.as_deref() == Some(uid.as_str()) || w.multi.contains(&uid)),
                scope: None,
                variant: if !my_name.is_empty() && s(&unit, "maintainer") == my_name {
                    CardVariant::Mine
                } else {
                    CardVariant::Normal
                },
            },
            |app, ui| {
                ui.horizontal(|ui| {
                    if unsplit {
                        chip(
                            ui,
                            "待拆分",
                            palette_color(CHIP_ORANGE_FG),
                            CHIP_ORANGE_BG,
                        );
                    }
                    if latest.is_object() {
                        chip(
                            ui,
                            &format!("v{}", s(&latest, "semver")),
                            palette_color(ACCENT_DEEP),
                            CHIP_BLUE_BG,
                        );
                        if let Some(tag) = unit_kind_tag(i(&latest, "page_count")) {
                            chip(ui, tag, palette_color(CHIP_GRAY_FG), CHIP_GRAY_BG);
                        }
                        ui.label(
                            egui::RichText::new(format!("{} 页", i(&latest, "page_count")))
                                .small()
                                .color(palette_color(TEXT_WEAK)),
                        );
                    } else {
                        ui.label(
                            egui::RichText::new("暂无版本")
                                .small()
                                .color(palette_color(TEXT_WEAK)),
                        );
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.label(
                            egui::RichText::new(format!(
                                "{} · {}",
                                app.folder_name(
                                    unit.get("folder_id").and_then(|x| x.as_str()).unwrap_or("")
                                ),
                                s(&unit, "maintainer")
                            ))
                            .small()
                            .color(palette_color(TEXT_WEAK)),
                        );
                    });
                });
                if !unsplit && matches!(app.pick, Some(PickState::Insert { .. })) {
                    ui.add_space(8.0);
                    if ui
                        .add_sized(
                            [72.0, 28.0],
                            egui::Button::new(
                                egui::RichText::new("插入")
                                    .size(13.0)
                                    .color(egui::Color32::WHITE),
                            )
                            .fill(ACCENT)
                            .corner_radius(6),
                        )
                        .clicked()
                    {
                        app.insert_pick_unit(item);
                    }
                }
            },
        );
        // Feed the folder-flow virtualization's card metric:
        // this card's height over its cover, filed under its
        // variant (pick-insert mode × has a latest version).
        let insert = matches!(self.pick, Some(PickState::Insert { .. }));
        w.flow_metrics.card_body[insert as usize][latest.is_object() as usize] =
            Some(outer.response.rect.height() - card_w * 9.0 / 16.0);
        // Click handling rides on an explicit interact over
        // the card rect: allocate_ui's own response carries no
        // click sense, which is why double-click never fired
        // (James 2026-11-15). Single click selects, double
        // click opens the maintenance drill-down — the same
        // contract as the web prototype.
        if unsplit {
            // Viewport priority feed: this card was just
            // rendered, i.e. it sits in (or near) the visible
            // band — its page is queued ahead of the background
            // order at the next drain, and its thumbnail was
            // already requested by the cover draw above.
            self.lazy_visible_boost.insert(uid.clone());
        }
        let click = ui.interact(
            outer.response.rect,
            egui::Id::unique(("unit_card", &uid)),
            egui::Sense::click(),
        );
        let import_locked =
            self.importing.is_some() && self.import_open_units.iter().any(|u| u == &uid);
        if let Some((pid, idx)) = &lazy_page {
            // Unsplit page: any click only moves the page to the
            // front of the split queue (the user's one explicit
            // priority entry) and says so. No selection, no open.
            if click.clicked() || click.double_clicked() {
                let (pid, idx) = (pid.clone(), *idx);
                self.lazy_boost(&pid, idx);
            }
        } else if import_locked && (click.clicked() || click.double_clicked()) {
            self.say("该页正在导入成块，完成后再操作");
        } else if click.double_clicked() {
            if self.pick.is_none() {
                self.open_unit_detail(&uid);
            }
        } else if click.clicked() {
            // Ctrl/⌘-click toggles the card in the
            // multi-selection （组合 / 剪贴板的前置）;
            // a plain click stays the single selection.
            let multi_key = ui.ctx().input(|inp| inp.modifiers.command || inp.modifiers.ctrl);
            if multi_key {
                if let Some(pos) = w.multi.iter().position(|u| u == &uid) {
                    w.multi.remove(pos);
                } else {
                    w.multi.push(uid.clone());
                }
            }
            w.selected = Some(uid.clone());
        }
    }

    /// Content page · local cache management (prototype #6b
    /// wallCacheView): the same on-device cache the showroom 我的
    /// zone lists as cards, here as management rows — stats hero,
    /// capacity bar, and per-entry 打开 / 常驻 / 移除 actions on
    /// the existing cache machinery (open_cache / remove_cache /
    /// the persisted cache index). Pinning persists a flag on the
    /// entry; the client cache has no eviction pass yet, so today
    /// the flag is state (shown in the stats and the row badge),
    /// not yet a behavior difference.
    fn content_cache_body(&mut self, ui: &mut egui::Ui) {
        body_frame().show(ui, |ui| {
            ui.label(
                egui::RichText::new(tr("本地缓存"))
                    .strong()
                    .size(16.0)
                    .color(palette_color(TEXT_MAIN)),
            );
            ui.add_space(8.0);
            let entries = self.cache_entries.clone();
            let used_bytes: i64 = entries.iter().map(|e| i(e, "bytes")).sum();
            let cap_mb = self
                .cache_policy
                .get("max_mb")
                .and_then(|v| v.as_i64())
                .unwrap_or(500);
            let pinned = entries
                .iter()
                .filter(|e| e.get("pinned").and_then(|v| v.as_bool()) == Some(true))
                .count();
            ui.horizontal(|ui| {
                ui.label(
                    egui::RichText::new(format!("已缓存条目 · {}", entries.len()))
                        .size(12.5)
                        .color(palette_color(TEXT_WEAK)),
                );
                ui.add_space(16.0);
                ui.label(
                    egui::RichText::new(format!(
                        "总占用 {:.1} MB / 上限 {} MB",
                        used_bytes as f64 / 1_048_576.0,
                        cap_mb
                    ))
                    .size(12.5)
                    .color(palette_color(TEXT_WEAK)),
                );
                ui.add_space(16.0);
                ui.label(
                    egui::RichText::new(format!("常驻条数 · {}", pinned))
                        .size(12.5)
                        .color(palette_color(TEXT_WEAK)),
                );
            });
            ui.add_space(10.0);
            let pct = if cap_mb > 0 {
                (used_bytes as f64 / (cap_mb as f64 * 1024.0 * 1024.0)).clamp(0.0, 1.0) as f32
            } else {
                0.0
            };
            let bar_w = ui.available_width().min(560.0);
            let (track, _) = ui.allocate_exact_size(egui::vec2(bar_w, 6.0), egui::Sense::hover());
            ui.painter()
                .rect_filled(track, 3.0, palette_color(HOVER_BG));
            if pct > 0.0 {
                let fill_rect =
                    egui::Rect::from_min_size(track.min, egui::vec2(track.width() * pct, 6.0));
                ui.painter().rect_filled(fill_rect, 3.0, ACCENT);
            }
            ui.label(
                egui::RichText::new(format!(
                    "本地已用 {:.1} MB / 上限 {} MB",
                    used_bytes as f64 / 1_048_576.0,
                    cap_mb
                ))
                .size(11.5)
                .color(palette_color(TEXT_WEAK)),
            );
            ui.add_space(14.0);
            if entries.is_empty() {
                return;
            }
            // 0 open · 1 toggle pin · 2 remove — applied after the
            // render pass, like the review queue's action locals.
            let mut act: Option<(String, u8)> = None;
            egui::ScrollArea::vertical()
                .id_salt("content_cache")
                .show(ui, |ui| {
                    for e in &entries {
                        let key = s(e, "key");
                        let is_pinned = e.get("pinned").and_then(|v| v.as_bool()) == Some(true);
                        egui::Frame::NONE
                            .fill(palette_color(PANEL_BG))
                            .stroke(egui::Stroke::new(1.0, palette_color(FIELD_BORDER)))
                            .corner_radius(egui::CornerRadius::same(6))
                            .inner_margin(egui::Margin::same(12))
                            .show(ui, |ui| {
                                ui.horizontal(|ui| {
                                    ui.vertical(|ui| {
                                        ui.label(
                                            egui::RichText::new(s(e, "title"))
                                                .size(13.0)
                                                .strong()
                                                .color(palette_color(TEXT_MAIN)),
                                        );
                                        ui.label(
                                            egui::RichText::new(format!(
                                                "{} 页 · {:.1} MB · 缓存于 {}{}",
                                                i(e, "page_count"),
                                                i(e, "bytes") as f64 / 1_048_576.0,
                                                fmt_time(i(e, "cached_ms")),
                                                match e
                                                    .get("blocks")
                                                    .and_then(|b| b.as_array())
                                                {
                                                    Some(b) if !b.is_empty() => {
                                                        format!(" · {} 个内容块", b.len())
                                                    }
                                                    _ => String::new(),
                                                }
                                            ))
                                            .size(11.5)
                                            .color(palette_color(TEXT_WEAK)),
                                        );
                                    });
                                    ui.with_layout(
                                        egui::Layout::right_to_left(egui::Align::Center),
                                        |ui| {
                                            if btn_default(ui, tr("移除缓存")).clicked() {
                                                act = Some((key.clone(), 2));
                                            }
                                            if btn_default(
                                                ui,
                                                if is_pinned {
                                                    tr("取消常驻")
                                                } else {
                                                    tr("设为常驻")
                                                },
                                            )
                                            .clicked()
                                            {
                                                act = Some((key.clone(), 1));
                                            }
                                            if btn_default(ui, tr("打开")).clicked() {
                                                act = Some((key.clone(), 0));
                                            }
                                        },
                                    );
                                });
                                ui.add_space(6.0);
                                if is_pinned {
                                    chip(
                                        ui,
                                        tr("已缓存 · 常驻本地"),
                                        palette_color(ACCENT_DEEP),
                                        CHIP_BLUE_BG,
                                    );
                                } else {
                                    chip(
                                        ui,
                                        tr("已缓存"),
                                        palette_color(CHIP_GREEN_FG),
                                        CHIP_GREEN_BG,
                                    );
                                }
                            });
                        ui.add_space(8.0);
                    }
                });
            if let Some((key, what)) = act {
                match what {
                    0 => self.open_cache(&key),
                    2 => self.remove_cache(&key),
                    _ => {
                        if let Some(e) = self.cache_entries.iter_mut().find(|e| s(e, "key") == key)
                        {
                            let now = e.get("pinned").and_then(|v| v.as_bool()).unwrap_or(false);
                            e["pinned"] = json!(!now);
                        }
                        save_cache_index(&self.cache_entries);
                    }
                }
            }
        });
    }

    /// Inline header meta editor (prototype data-dbl-edit): a
    /// single-line field standing in for the header text while
    /// editing. Focus loss commits (the prototype's focusout
    /// save); Esc cancels. Empty or unchanged text saves
    /// nothing, exactly as saveDrillMeta treats it.
    fn meta_edit_ui(&mut self, ui: &mut egui::Ui, st: &mut UnitState, field: MetaField) {
        let mut commit = false;
        let mut cancel = false;
        if let Some((f, text)) = &mut st.meta_edit {
            if *f == field {
                let resp = ui.add(egui::TextEdit::singleline(text).desired_width(360.0));
                if st.meta_edit_focus {
                    resp.request_focus();
                    st.meta_edit_focus = false;
                }
                if resp.lost_focus() {
                    commit = true;
                }
                if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                    cancel = true;
                }
            }
        }
        if cancel {
            st.meta_edit = None;
        } else if commit {
            self.commit_meta_edit(st);
        }
    }

    fn commit_meta_edit(&mut self, st: &mut UnitState) {
        let Some((field, text)) = st.meta_edit.take() else {
            return;
        };
        let text = text.trim().to_string();
        let unit = st.detail.get("unit").cloned().unwrap_or(json!({}));
        let current = match field {
            MetaField::Name => s(&unit, "name"),
            MetaField::Description => s(&unit, "description"),
        };
        if text.is_empty() || text == current {
            return;
        }
        let body = match field {
            MetaField::Name => json!({ "name": text }),
            MetaField::Description => json!({ "description": text }),
        };
        let uid = st.unit_id.clone();
        match self.call_work("PATCH", &format!("/v1/units/{uid}"), Some(body)) {
            Ok(_) => self.reload_unit_detail(&uid),
            Err(e) => self.msg = format!("保存失败：{e}"),
        }
    }

    /// Open the page-level editor overlay for version `v` at
    /// `page` — the same PageEditState the version rows'
    /// 编辑页面 button builds, landed on the requested page.
    fn open_page_editor(&mut self, unit: &Value, v: &Value, page: usize) {
        let mut scripts: Vec<String> = v
            .get("scripts")
            .and_then(|x| x.as_array())
            .map(|arr| {
                arr.iter()
                    .map(|t| t.as_str().unwrap_or("").to_string())
                    .collect()
            })
            .unwrap_or_default();
        let pages = i(v, "page_count") as usize;
        while scripts.len() < pages {
            scripts.push(String::new());
        }
        self.overlays.push(Overlay::PageEdit(PageEditState {
            version_id: s(v, "id"),
            unit_id: s(unit, "id"),
            unit_name: s(unit, "name"),
            semver: s(v, "semver"),
            page_count: pages,
            original_page_count: pages,
            page,
            base: Default::default(),
            work: Default::default(),
            scripts_base: scripts.clone(),
            scripts,
            bump: 1,
            note: String::new(),
            elems: Default::default(),
            elems_base: Default::default(),
            slide_cx: 12192000,
            slide_cy: 6858000,
            sel: None,
            selection: Default::default(),
            preview: Default::default(),
            drag: None,
            undo: Vec::new(),
            redo: Vec::new(),
        }));
    }

    fn unit_detail(&mut self, ui: &mut egui::Ui, st: &mut UnitState) {
        if ui.input(|input| input.key_pressed(egui::Key::Escape)) && st.bind_pick.is_some() {
            st.bind_pick = None;
            self.msg = "已取消页面挑选".into();
        }
        let detail = st.detail.clone();
        let unit = detail.get("unit").cloned().unwrap_or(json!({}));
        // Breadcrumb (drill-down standard): 页面 › block name, the
        // root crumb being the way back — this frame was the last
        // one without a crumb bar, headed by a plain back button.
        let unit_name = s(&unit, "name");
        if crumb_bar(ui, &[tr("页面"), &unit_name]) == Some(0) {
            st.closed = true;
            self.refresh_all();
            return;
        }
        // Type tag (unified content model, user ruling
        // 2026-11-25): the head rendition's latest version
        // decides — one page reads 单页, more read 内容块.
        let head_latest = detail
            .get("renditions")
            .and_then(|r| r.as_array())
            .and_then(|arr| arr.first())
            .and_then(|r| r.get("latest"))
            .cloned()
            .unwrap_or(Value::Null);
        let kind_tag = if head_latest.is_object() {
            unit_kind_tag(i(&head_latest, "page_count"))
        } else {
            None
        };
        ui.add_space(6.0);
        self.unit_header_ui(ui, st, &unit, kind_tag);

        egui::ScrollArea::vertical().show(ui, |ui| {
            // pick-pages mode: the entry's currently resolved version,
            // page by page with checkboxes (initial state = the entry's
            // existing excerpt, all pages when there is none)
            let pick_vid = match &self.pick {
                Some(PickState::Pages {
                    unit_id,
                    version_id,
                    ..
                }) if *unit_id == s(&unit, "id") => Some(version_id.clone()),
                _ => None,
            };
            if let Some(vid) = pick_vid {
                let total = detail
                    .get("renditions")
                    .and_then(|r| r.as_array())
                    .and_then(|arr| {
                        arr.iter().find_map(|r| {
                            r.get("versions").and_then(|v| v.as_array()).and_then(|vs| {
                                vs.iter()
                                    .find(|v| s(v, "id") == vid)
                                    .map(|v| i(v, "page_count") as usize)
                            })
                        })
                    })
                    .unwrap_or(0);
                ui.label(
                    egui::RichText::new("当前解析版本 · 逐页勾选")
                        .strong()
                        .size(16.0),
                );
                ui.add_space(6.0);
                ui.horizontal_wrapped(|ui| {
                    for p in 0..total {
                        ui.vertical(|ui| {
                            self.thumb_ui(ui, &vid, p, 200.0);
                            if let Some(PickState::Pages { sel, .. }) = &mut self.pick {
                                if p < sel.len() {
                                    ui.checkbox(&mut sel[p], format!("第 {} 页", p + 1));
                                }
                            }
                        });
                    }
                });
                ui.horizontal(|ui| {
                    if ui.small_button("全选").clicked() {
                        if let Some(PickState::Pages { sel, .. }) = &mut self.pick {
                            for b in sel.iter_mut() {
                                *b = true;
                            }
                        }
                    }
                    if ui.small_button("清空").clicked() {
                        if let Some(PickState::Pages { sel, .. }) = &mut self.pick {
                            for b in sel.iter_mut() {
                                *b = false;
                            }
                        }
                    }
                });
                ui.add_space(10.0);
                ui.separator();
                ui.add_space(6.0);
            }
            self.unit_101_body(ui, st);
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new("AI 翻译（生成草稿）").strong().size(16.0));
                // Configuration state from /v1/ai/status: a stub draft
                // must never look like a real translation.
                if let Some(st) = &self.ai_status {
                    if s(st, "provider") == "stub" {
                        ui.label(
                            egui::RichText::new("当前为 stub 占位（未配置真实 AI）")
                                .small()
                                .color(palette_color(TEXT_WEAK)),
                        );
                    } else if st
                        .get("configured")
                        .and_then(|x| x.as_bool())
                        .unwrap_or(false)
                    {
                        ui.label(
                            egui::RichText::new(format!("AI 已配置：{}", s(st, "model")))
                                .small()
                                .color(palette_color(TEXT_WEAK)),
                        );
                    }
                }
                let cur_to = if st.translate_to.is_empty() {
                    "zh".to_string()
                } else {
                    st.translate_to.clone()
                };
                let cur_label = self
                    .lang_options
                    .iter()
                    .find(|(code, _)| *code == cur_to)
                    .map(|(_, label)| label.clone())
                    .unwrap_or_else(|| match cur_to.as_str() {
                        "en" => "英语".to_string(),
                        "ja" => "日语".to_string(),
                        _ => "中文".to_string(),
                    });
                egui::ComboBox::from_id_salt("translate_to")
                    .selected_text(cur_label)
                    .show_ui(ui, |ui| {
                        if self.lang_options.is_empty() {
                            ui.selectable_value(&mut st.translate_to, "zh".into(), "中文");
                            ui.selectable_value(&mut st.translate_to, "en".into(), "英语");
                            ui.selectable_value(&mut st.translate_to, "ja".into(), "日语");
                        } else {
                            for (code, label) in self.lang_options.clone() {
                                ui.selectable_value(&mut st.translate_to, code, label);
                            }
                        }
                    });
                let busy = self.translating.is_some();
                let translate_ready = st
                    .detail
                    .get("renditions")
                    .and_then(|r| r.as_array())
                    .and_then(|arr| arr.first())
                    .and_then(|r| r.get("versions"))
                    .and_then(|v| v.as_array())
                    .map(|arr| !arr.is_empty())
                    .unwrap_or(false);
                let btn_text = match &self.translating {
                    Some((_, d, t)) if *t > 0 => format!("翻译中… {d}/{t}"),
                    Some(_) => "翻译中…".to_string(),
                    None => "翻译最新版本".to_string(),
                };
                if ui
                    .add_enabled(!busy && translate_ready, egui::Button::new(btn_text))
                    .clicked()
                {
                    // latest version of the first rendition in the detail
                    let first_version = st
                        .detail
                        .get("renditions")
                        .and_then(|r| r.as_array())
                        .and_then(|arr| arr.first())
                        .and_then(|r| r.get("versions"))
                        .and_then(|v| v.as_array())
                        .and_then(|arr| arr.first())
                        .map(|v| s(v, "id"));
                    match first_version {
                        Some(vid) => {
                            let uid = st.unit_id.clone();
                            let to = if st.translate_to.is_empty() {
                                "zh".to_string()
                            } else {
                                st.translate_to.clone()
                            };
                            match self.call(
                                "POST",
                                &format!("/v1/units/{uid}/translate-jobs"),
                                Some(json!({ "version_id": vid, "target_lang": to })),
                            ) {
                                Ok(v) => {
                                    let job_id = s(&v, "job_id");
                                    self.translating = Some((uid, 0, 0));
                                    let server = self.server.clone();
                                    let token = self.token.clone();
                                    let tx = self.translate_tx.clone();
                                    let gen = self.translate_gen;
                                    std::thread::spawn(move || loop {
                                        match raw_call(
                                            &server,
                                            token.as_deref(),
                                            "GET",
                                            &format!("/v1/translate-jobs/{job_id}"),
                                            None,
                                            false,
                                        ) {
                                            Ok(p) => {
                                                let _ = tx.send(TranslateEvent::Progress(
                                                    gen,
                                                    i(&p, "done") as usize,
                                                    i(&p, "total") as usize,
                                                ));
                                                match s(&p, "state").as_str() {
                                                    "succeeded" => {
                                                        let _ = tx.send(TranslateEvent::Done(
                                                            gen,
                                                            Ok(p.get("result")
                                                                .cloned()
                                                                .unwrap_or(Value::Null)),
                                                        ));
                                                        break;
                                                    }
                                                    "failed" => {
                                                        let _ = tx.send(TranslateEvent::Done(
                                                            gen,
                                                            Err(s(&p, "error")),
                                                        ));
                                                        break;
                                                    }
                                                    _ => std::thread::sleep(
                                                        std::time::Duration::from_millis(700),
                                                    ),
                                                }
                                            }
                                            Err(e) => {
                                                let _ =
                                                    tx.send(TranslateEvent::Done(gen, Err(e)));
                                                break;
                                            }
                                        }
                                    });
                                }
                                Err(e) => self.msg = format!("翻译失败：{e}"),
                            }
                        }
                        _ => self.msg = "暂无可翻译的版本".into(),
                    }
                }
            });
            if let Some(vid) = st.scripts_for.clone() {
                ui.add_space(12.0);
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new("逐页讲稿（保存不升版本）").strong());
                    if ui.button("保存讲稿").clicked() {
                        let scripts = st.scripts_edit.clone();
                        match self.call_work(
                            "PUT",
                            &format!("/v1/versions/{vid}/scripts"),
                            Some(json!({ "scripts": scripts })),
                        ) {
                            Ok(_) => {
                                self.msg = "讲稿已保存".into();
                                self.reload_unit_detail(&st.unit_id.clone());
                            }
                            Err(e) => self.msg = format!("保存讲稿失败：{e}"),
                        }
                    }
                    if ui.button("关闭").clicked() {
                        st.scripts_for = None;
                    }
                });
                let n = st.scripts_edit.len();
                for idx in 0..n {
                    ui.horizontal(|ui| {
                        ui.label(format!("第 {} 页", idx + 1));
                        ui.add(
                            egui::TextEdit::multiline(&mut st.scripts_edit[idx])
                                .desired_width(560.0)
                                .desired_rows(2),
                        );
                    });
                }
            }
            ui.add_space(20.0);
            ui.label(egui::RichText::new("发布新版本").strong().size(16.0));
            ui.horizontal(|ui| {
                if ui.button("选择片段文件").clicked() {
                    st.publish_file = rfd::FileDialog::new()
                        .add_filter("PowerPoint", &["pptx"])
                        .pick_file();
                }
                if let Some(p) = &st.publish_file {
                    ui.label(
                        p.file_name()
                            .map(|n| n.to_string_lossy().to_string())
                            .unwrap_or_default(),
                    );
                }
            });
            ui.horizontal(|ui| {
                ui.label("形态");
                ui.add(egui::TextEdit::singleline(&mut st.publish_form).desired_width(80.0));
                ui.label("语言");
                ui.add(egui::TextEdit::singleline(&mut st.publish_lang).desired_width(60.0));
                ui.label("升版方式");
                for (idx, label) in [(0usize, "大版本"), (1, "小版本"), (2, "文字调整")] {
                    ui.radio_value(&mut st.publish_bump, idx, label);
                }
                ui.label("备注");
                ui.text_edit_singleline(&mut st.publish_note);
            });
            if ui
                .add_enabled(st.publish_file.is_some(), egui::Button::new("发布"))
                .clicked()
            {
                if let Some(path) = st.publish_file.clone() {
                    let uid = st.unit_id.clone();
                    if let Ok(bytes) = std::fs::read(&path) {
                        let bump = ["major", "minor", "patch"][st.publish_bump];
                        let r = self.call_work(
                            "POST",
                            &format!("/v1/units/{uid}/publish"),
                            Some(json!({
                                "data_base64": b64e(&bytes),
                                "form": st.publish_form,
                                "lang": st.publish_lang,
                                "bump": bump,
                                "note": st.publish_note,
                            })),
                        );
                        match r {
                            Ok(v) => {
                                self.msg = format!("已发布 v{}", s(&v, "semver"));
                                self.reload_unit_detail(&uid);
                            }
                            Err(e) => self.msg = format!("发布失败：{e}"),
                        }
                    }
                } else {
                    self.msg = "先选择片段文件".into();
                }
            }
        });
        self.attachments_ui(ui, "unit", &st.unit_id.clone());
    }
}

impl App {
    /// Folder tree used by the index page, the content page and the
    /// showroom: the left column holds folders; the right side shows what
    /// lives in the selected folder. `selected` None = 全部. Folder rows
    /// carry a right-click menu （新建子文件夹 / 新建文件夹 / 重命名 / 删除）;
    /// right-clicking the blank area below the tree offers 新建文件夹.
    fn folder_tree(
        &mut self,
        ui: &mut egui::Ui,
        selected: &mut Option<String>,
        uncat: bool,
        scope: &str,
    ) {
        ui.spacing_mut().item_spacing.y = 2.0;
        enum Act {
            NewChild(String),
            NewSibling(Option<String>),
            Rename(String, String),
            Delete(String, String),
            Color(String, Option<String>),
        }
        let mut action: Option<Act> = None;
        // Long-press gate (user ruling 2026-11-24): a folder drag
        // starts only after the pointer has rested on the row for
        // FOLDER_LONG_PRESS_SECS without wandering past
        // FOLDER_PRESS_MOVE_TOLERANCE. Until then the row is a
        // plain click target and the sidebar scrolls exactly as
        // before — the drag state is managed here rather than by
        // egui's drag detection, which arms on the first few
        // points of movement (the old "一点就拖"). While a press
        // is pending we ask for steady repaints so the threshold
        // still fires with the pointer held perfectly still.
        enum PressStep {
            Keep,
            Cancel,
            Arm(String),
        }
        let step = match &self.folder_press {
            None => PressStep::Keep,
            Some(press) => {
                let (down, pos, now) = ui.input(|i| {
                    (
                        i.pointer.primary_down(),
                        i.pointer.interact_pos().or_else(|| i.pointer.hover_pos()),
                        i.time,
                    )
                });
                if !down || press.armed {
                    PressStep::Keep
                } else if pos.map_or(false, |p| {
                    p.distance(press.origin) > FOLDER_PRESS_MOVE_TOLERANCE
                }) {
                    // The gesture is a scroll, not a press.
                    PressStep::Cancel
                } else if now - press.since >= FOLDER_LONG_PRESS_SECS {
                    PressStep::Arm(press.id.clone())
                } else {
                    PressStep::Keep
                }
            }
        };
        match step {
            PressStep::Cancel => self.folder_press = None,
            PressStep::Arm(id) => {
                if let Some(press) = &mut self.folder_press {
                    press.armed = true;
                }
                *selected = Some(id.clone());
                egui::DragAndDrop::set_payload(ui.ctx(), FolderDrag(id));
            }
            PressStep::Keep => {}
        }
        if let Some(press) = &self.folder_press {
            if press.armed {
                // Keep the payload warm every frame, the way
                // dnd_drag_source does for its own drags.
                egui::DragAndDrop::set_payload(ui.ctx(), FolderDrag(press.id.clone()));
            }
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_millis(30));
        }
        let all_icon = ChromeIcon::Settings;
        let width = ui.available_width();
        ui.horizontal(|ui| {
            ui.set_width(width);
            ui.spacing_mut().item_spacing.x = 0.0;
            // The 全部 row spans the full width: the new-folder
            // button moved to the sidebar's 目录 header row (user
            // ruling 2026-11-24).
            let (rect, response) = ui.allocate_exact_size(
                egui::vec2(width.max(1.0), 28.0),
                egui::Sense::click(),
            );
            let selected_all = selected.is_none();
            if selected_all {
                ui.painter().rect_filled(rect, 6.0, palette_color(SEL_BG));
            } else if response.hovered() {
                ui.painter().rect_filled(rect, 6.0, palette_color(HOVER_BG));
            }
            let icon_rect = egui::Rect::from_min_size(
                egui::pos2(rect.min.x + 10.0, rect.center().y - 7.5),
                egui::vec2(15.0, 15.0),
            );
            ChromeIcons.paint(
                ui,
                all_icon,
                icon_rect.center(),
                15.0,
                if selected_all {
                    palette_color(ACCENT)
                } else {
                    palette_color(ICON_TINT)
                },
            );
            let text_color = if selected_all {
                palette_color(ACCENT)
            } else {
                palette_color(TEXT_MAIN)
            };
            ui.painter().text(
                egui::pos2(rect.min.x + 31.0, rect.center().y),
                egui::Align2::LEFT_CENTER,
                tr("全部"),
                egui::FontId::proportional(12.5),
                text_color,
            );
            if response.clicked() {
                *selected = None;
            }

            // (No add button on this row any more — it lives on
            // the 目录 header row; see the allocation note above.)
        });
        // an inline editor for a NEW root folder sits right under 全部
        if matches!(&self.folder_edit, Some(e) if e.target.is_none() && e.parent.is_none()) {
            self.folder_edit_row(ui, 0.0, ChromeIcon::Folder, palette_color(ICON_TINT));
            ui.add_space(2.0);
        }
        // Index page only (uncat): 未分类, the home of every index
        // whose folder_id is NULL — where every new index lands by
        // default. It is a filter sentinel (selected == Some("")),
        // not a folder row: it sits with 全部 above the user's own
        // folders, carries the folder icon like its neighbours, and
        // has no context menu, so it cannot be renamed or deleted.
        if uncat {
            let uncat_sel = selected.as_deref() == Some("");
            let uncat_icon = if uncat_sel {
                ChromeIcon::FolderOpen
            } else {
                ChromeIcon::Folder
            };
            if tree_row(ui, uncat_sel, 0.0, Some(uncat_icon), "未分类", 12.5).clicked() {
                *selected = Some(String::new());
            }
        }
        // Sidebar isolation (user ruling 2026-11-24): the tree
        // lists only its own scope's folders; self.folders keeps
        // the cross-scope union for id-based name lookups.
        let mut folders: Vec<Value> = self
            .folders
            .iter()
            .filter(|f| s(*f, "scope") == scope)
            .cloned()
            .collect();
        folders.sort_by_key(|f| i(f, "sort"));
        // depth-first order by parent chain
        fn depth_of(folders: &[Value], f: &Value) -> usize {
            let mut d = 0;
            let mut cur = s(f, "parent_id");
            while !cur.is_empty() && d < 8 {
                d += 1;
                match folders.iter().find(|x| s(x, "id") == cur) {
                    Some(p) => cur = s(p, "parent_id"),
                    None => break,
                }
            }
            d
        }
        let mut ordered: Vec<Value> = Vec::new();
        fn walk(folders: &[Value], parent: &str, out: &mut Vec<Value>) {
            for f in folders {
                if s(f, "parent_id") == parent {
                    out.push(f.clone());
                    let id = s(f, "id");
                    walk(folders, &id, out);
                }
            }
        }
        walk(&folders, "", &mut ordered);
        for f in &folders {
            if !ordered.iter().any(|x| s(x, "id") == s(f, "id")) {
                ordered.push(f.clone());
            }
        }
        // Drag-and-drop (user ruling 2026-11-24; the prototype tree
        // behaves the same way): drag a folder ONTO another folder
        // to make it that folder's child, or into the SEAM between
        // two rows to insert it at that position among the below
        // row's siblings. Row geometry is collected during the
        // loop and hover/drop is resolved after it, when every row
        // rect is known. The 全部 and 未分类 rows take no part:
        // they are neither drag sources nor drop targets.
        //
        // The payload is managed by hand (set at the long-press
        // gate, read here) instead of via Ui::dnd_drag_source:
        // at the pinned egui rev that helper repaints the dragged
        // body on a Tooltip layer translated by
        // `pointer_pos - response.rect.center()` — it re-anchors
        // the row's CENTRE on the pointer, so the row jumps by up
        // to half its width the moment a drag starts and then
        // drifts freely in both axes (the user's "文件夹偏移出去
        // 很远"). Painting the lifted row ourselves (below) keeps
        // it locked to the sidebar and to vertical motion only.
        let drag_id: Option<String> =
            egui::DragAndDrop::payload::<FolderDrag>(ui.ctx()).map(|p| p.0.clone());
        // Escape aborts a drag at the plugin level (the payload
        // is cleared at pass start): mirror that in the press
        // state so the floating row disappears too.
        if matches!(&self.folder_press, Some(p) if p.armed)
            && drag_id.is_none()
            && ui.input(|i| i.pointer.primary_down())
        {
            self.folder_press = None;
        }
        // True when `start` is `needle` itself or sits inside
        // `needle`'s subtree (walk start's parent chain up).
        let chain_hits = |folders: &[Value], start: &str, needle: &str| -> bool {
            let mut cur = start.to_string();
            for _ in 0..64 {
                if cur == needle {
                    return true;
                }
                match folders.iter().find(|x| s(x, "id") == cur) {
                    Some(f) => {
                        cur = s(f, "parent_id");
                        if cur.is_empty() {
                            return false;
                        }
                    }
                    None => return false,
                }
            }
            false
        };
        let mut row_rects: Vec<(String, String, egui::Rect)> = Vec::new();
        for f in &ordered {
            let fid = s(f, "id");
            let fname = s(f, "name");
            let fparent = s(f, "parent_id");
            let d = depth_of(&folders, f);
            let editing_this =
                matches!(&self.folder_edit, Some(e) if e.target.as_deref() == Some(fid.as_str()));
            if editing_this {
                // The editor row carries the same icon and tint
                // the normal row (else branch) paints for this
                // folder: selected = FolderOpen in accent, a
                // custom folder color next, default tint last.
                let esel = selected.as_deref() == Some(fid.as_str());
                let eicon = if esel {
                    ChromeIcon::FolderOpen
                } else {
                    ChromeIcon::Folder
                };
                let etint = if esel {
                    palette_color(ACCENT)
                } else {
                    folder_color_of(f).unwrap_or_else(|| palette_color(ICON_TINT))
                };
                self.folder_edit_row(ui, d as f32 * 15.0, eicon, etint);
            } else {
                let is_sel = selected.as_deref() == Some(fid.as_str());
                let ficon = if is_sel {
                    ChromeIcon::FolderOpen
                } else {
                    ChromeIcon::Folder
                };
                // The row itself is a plain click target (see
                // the long-press gate at the top): it is NOT a
                // dnd_drag_source, so clicks and sidebar scrolling
                // behave exactly like every other tree row.
                let fcolor = folder_color_of(f);
                let resp = tree_row_ex(
                    ui,
                    is_sel,
                    d as f32 * 15.0,
                    Some(ficon),
                    fcolor,
                    &fname,
                    12.5,
                    egui::Sense::click(),
                );
                let is_dragged =
                    matches!(&self.folder_press, Some(p) if p.armed && p.id == fid);
                if is_dragged {
                    // The lifted copy is painted after the loop;
                    // the row's own slot stays behind, dimmed.
                    ui.painter().rect_filled(
                        resp.rect,
                        6.0,
                        palette_color(APP_BG).gamma_multiply(0.55),
                    );
                }
                row_rects.push((fid.clone(), fparent.clone(), resp.rect));
                // A fresh primary press on this row starts the
                // long-press watch.
                if self.folder_press.is_none()
                    && ui.input(|i| {
                        i.pointer.primary_pressed()
                            && i.pointer
                                .interact_pos()
                                .is_some_and(|p| resp.rect.contains(p))
                    })
                {
                    let origin = ui
                        .input(|i| i.pointer.interact_pos())
                        .unwrap_or(resp.rect.center());
                    let now = ui.input(|i| i.time);
                    self.folder_press = Some(FolderPress {
                        id: fid.clone(),
                        name: fname.clone(),
                        depth: d,
                        since: now,
                        origin,
                        armed: false,
                    });
                }
                if resp.clicked() {
                    *selected = Some(fid.clone());
                }
                if resp.double_clicked() {
                    // Double-click renames (user ruling
                    // 2026-11-24) — the same inline editor as the
                    // context menu's 重命名. Folder rows have no
                    // other double-click behaviour (the tree is
                    // always fully expanded), so nothing is
                    // displaced; the click half still just selects.
                    action = Some(Act::Rename(fid.clone(), fname.clone()));
                }
                let (a, b, c, dd) = (
                    fid.clone(),
                    if fparent.is_empty() {
                        None
                    } else {
                        Some(fparent.clone())
                    },
                    fname.clone(),
                    fid.clone(),
                );
                let mut color_pick: Option<Option<String>> = None;
                resp.context_menu(|ui| {
                    if ui.button("新建子文件夹").clicked() {
                        action = Some(Act::NewChild(a.clone()));
                        ui.close();
                    }
                    if ui.button("新建文件夹").clicked() {
                        action = Some(Act::NewSibling(b.clone()));
                        ui.close();
                    }
                    if ui.button("重命名").clicked() {
                        action = Some(Act::Rename(dd.clone(), c.clone()));
                        ui.close();
                    }
                    ui.separator();
                    ui.menu_button("颜色", |ui| {
                        ui.horizontal(|ui| {
                            if ui.button("默认").clicked() {
                                color_pick = Some(None);
                                ui.close();
                            }
                            for (cname, hex) in FOLDER_COLORS {
                                let col = parse_folder_hex(hex)
                                    .unwrap_or_else(|| palette_color(ICON_TINT));
                                let (srect, sresp) = ui.allocate_exact_size(
                                    egui::vec2(20.0, 20.0),
                                    egui::Sense::click(),
                                );
                                ui.painter().rect_filled(srect, 5.0, col);
                                if fcolor == Some(col) {
                                    ui.painter().rect_stroke(
                                        srect,
                                        5.0,
                                        egui::Stroke::new(2.0, palette_color(ACCENT)),
                                        egui::StrokeKind::Inside,
                                    );
                                }
                                if sresp.on_hover_text(cname).clicked() {
                                    color_pick = Some(Some(hex.to_string()));
                                    ui.close();
                                }
                            }
                        });
                    });
                    if ui.button("删除").clicked() {
                        action = Some(Act::Delete(dd.clone(), c.clone()));
                        ui.close();
                    }
                });
                if let Some(cp) = color_pick {
                    action = Some(Act::Color(dd.clone(), cp));
                }
            }
            // an inline editor for a NEW child of this folder
            if matches!(&self.folder_edit, Some(e) if e.target.is_none() && e.parent.as_deref() == Some(fid.as_str()))
            {
                self.folder_edit_row(
                    ui,
                    (d + 1) as f32 * 15.0,
                    ChromeIcon::Folder,
                    palette_color(ICON_TINT),
                );
                ui.add_space(2.0);
            }
        }
        // blank area: right-click creates a root folder
        let blank = ui.allocate_response(
            egui::vec2(ui.available_width(), ui.available_height().max(16.0)),
            egui::Sense::click(),
        );
        blank.context_menu(|ui| {
            if ui.button("新建文件夹").clicked() {
                action = Some(Act::NewSibling(None));
                ui.close();
            }
        });
        // Resolve the folder drag against the collected geometry.
        // The two targets are mutually exclusive by pointer
        // position: a row's centre band means ONTO (becomes that
        // folder's child), its edge bands mean the seam next to
        // that row (same-level insert in front of the below row,
        // among the below row's siblings). Feedback while
        // hovering: a thin DASHED frame around the ONTO row; a
        // horizontal line in the exact middle of the gap between
        // the two rows for a seam. Drops onto the dragged folder
        // itself or into its own subtree show no feedback and do
        // nothing; dropping in front of itself is a no-op.
        if let (Some(drag), Some(pos)) = (
            &drag_id,
            ui.input(|i| i.pointer.interact_pos().or_else(|| i.pointer.hover_pos())),
        ) {
            if !row_rects.is_empty() {
                let first = row_rects[0].2;
                let last = row_rects[row_rects.len() - 1].2;
                let in_x = pos.x >= first.left() - 4.0 && pos.x <= first.right() + 4.0;
                let in_y = pos.y >= first.top() - 6.0 && pos.y <= blank.rect.bottom();
                if in_x && in_y {
                    enum Drop {
                        Onto(usize),
                        Seam(usize),
                    }
                    let mut target: Option<Drop> = None;
                    if pos.y < first.top() {
                        target = Some(Drop::Seam(0));
                    } else if pos.y > last.bottom() {
                        target = Some(Drop::Seam(row_rects.len()));
                    } else {
                        for (idx, (_, _, rect)) in row_rects.iter().enumerate() {
                            if pos.y >= rect.top() && pos.y <= rect.bottom() {
                                let rel = (pos.y - rect.top()) / rect.height().max(1.0);
                                target = Some(if rel < 0.25 {
                                    Drop::Seam(idx)
                                } else if rel > 0.75 {
                                    Drop::Seam(idx + 1)
                                } else {
                                    Drop::Onto(idx)
                                });
                                break;
                            }
                            if pos.y > rect.bottom()
                                && idx + 1 < row_rects.len()
                                && pos.y < row_rects[idx + 1].2.top()
                            {
                                target = Some(Drop::Seam(idx + 1));
                                break;
                            }
                        }
                    }
                    let mut go: Option<(Option<String>, Option<String>)> = None;
                    match target {
                        Some(Drop::Onto(idx)) => {
                            let (tid, _, rect) = &row_rects[idx];
                            if !chain_hits(&folders, tid, drag) {
                                let r = rect.shrink(0.5);
                                let stroke = egui::Stroke::new(1.0, palette_color(ACCENT));
                                let corners = [
                                    r.left_top(),
                                    r.right_top(),
                                    r.right_bottom(),
                                    r.left_bottom(),
                                    r.left_top(),
                                ];
                                for shape in
                                    egui::Shape::dashed_line(&corners, stroke, 5.0, 4.0)
                                {
                                    ui.painter().add(shape);
                                }
                                go = Some((Some(tid.clone()), None));
                            }
                        }
                        Some(Drop::Seam(idx)) => {
                            let (parent, before) = if idx < row_rects.len() {
                                let (bid, bparent, _) = &row_rects[idx];
                                (bparent.clone(), Some(bid.clone()))
                            } else {
                                let (_, lparent, _) = &row_rects[row_rects.len() - 1];
                                (lparent.clone(), None)
                            };
                            let parent_opt = if parent.is_empty() {
                                None
                            } else {
                                Some(parent)
                            };
                            let invalid = parent_opt
                                .as_deref()
                                .map_or(false, |p| chain_hits(&folders, p, drag));
                            let noop = before.as_deref() == Some(drag.as_str());
                            if !invalid && !noop {
                                // The line sits in the exact middle
                                // of the gap between the two rows —
                                // equidistant from the row above and
                                // the row below.
                                let y = if idx == 0 {
                                    first.top() - 1.0
                                } else if idx >= row_rects.len() {
                                    last.bottom() + 1.0
                                } else {
                                    (row_rects[idx - 1].2.bottom() + row_rects[idx].2.top())
                                        / 2.0
                                };
                                ui.painter().line_segment(
                                    [
                                        egui::pos2(first.left() + 4.0, y),
                                        egui::pos2(first.right() - 4.0, y),
                                    ],
                                    egui::Stroke::new(2.0, palette_color(ACCENT)),
                                );
                                go = Some((parent_opt, before));
                            }
                        }
                        None => {}
                    }
                    if let Some((parent, before)) = go {
                        if ui.input(|i| i.pointer.any_released()) {
                            self.folder_drag_move(drag, parent, before);
                        }
                    }
                }
            }
        }
        // The lifted row: painted after the drop resolution so
        // it floats above the tree. Its x and width are locked to
        // the row's own slot; its y is the slot's y plus the
        // pointer's vertical travel, clamped to the tree's
        // extent — it moves only up and down, inside the sidebar.
        // The lift is drawn transparent — accent outline, icon and
        // name only — so the drop seam and the rows underneath
        // stay visible through it (user report 2026-11-25).
        if let Some(press) = &self.folder_press {
            if press.armed {
                let slot = row_rects
                    .iter()
                    .find(|(id, _, _)| *id == press.id)
                    .map(|(_, _, r)| *r);
                let pos = ui.input(|i| {
                    i.pointer.interact_pos().or_else(|| i.pointer.hover_pos())
                });
                if let (Some(slot), Some(pos)) = (slot, pos) {
                    let top_limit = row_rects
                        .first()
                        .map_or(slot.top(), |(_, _, r)| r.top());
                    let bottom_limit = blank.rect.bottom();
                    let y = (slot.top() + (pos.y - press.origin.y))
                        .clamp(top_limit, (bottom_limit - slot.height()).max(top_limit));
                    let ghost = egui::Rect::from_min_size(
                        egui::pos2(slot.left(), y),
                        slot.size(),
                    );
                    let painter = ui.painter();
                    // No plate under the ghost (user report
                    // 2026-11-25): this used to stack a drop
                    // shadow and an opaque APP_BG fill (SEL_BG
                    // tint on top), which hid the insertion seam
                    // and the rows below while dragging. Only the
                    // outline, icon and name are painted now, all
                    // in accent, exactly as before.
                    painter.rect_stroke(
                        ghost,
                        6.0,
                        egui::Stroke::new(1.0, palette_color(ACCENT)),
                        egui::StrokeKind::Inside,
                    );
                    let indent = press.depth as f32 * 15.0;
                    let ir = egui::Rect::from_min_size(
                        egui::pos2(ghost.min.x + 10.0 + indent, ghost.center().y - 7.5),
                        egui::vec2(15.0, 15.0),
                    );
                    ChromeIcon::FolderOpen
                        .image(15.0, palette_color(ACCENT))
                        .paint_at(ui, ir);
                    let font = egui::FontId::new(12.5, egui::FontFamily::Proportional);
                    let galley = painter.layout_no_wrap(
                        press.name.clone(),
                        font,
                        palette_color(ACCENT),
                    );
                    let ty = ghost.center().y - galley.size().y / 2.0;
                    painter.galley(
                        egui::pos2(ghost.min.x + 31.0 + indent, ty),
                        galley,
                        palette_color(ACCENT),
                    );
                }
            }
        }
        // The press watch ends with the gesture (the drop above
        // has already read the payload for this release frame).
        if !ui.input(|i| i.pointer.primary_down()) {
            self.folder_press = None;
        }
        match action {
            Some(Act::NewChild(parent)) => {
                self.folder_edit = Some(FolderEdit {
                    target: None,
                    parent: Some(parent),
                    text: "新文件夹".into(),
                    scope: scope.into(),
                });
                self.folder_edit_focus = true;
            }
            Some(Act::NewSibling(parent)) => {
                self.folder_edit = Some(FolderEdit {
                    target: None,
                    parent,
                    text: "新文件夹".into(),
                    scope: scope.into(),
                });
                self.folder_edit_focus = true;
            }
            Some(Act::Rename(id, name)) => {
                self.folder_edit = Some(FolderEdit {
                    target: Some(id),
                    parent: None,
                    text: name,
                    scope: scope.into(),
                });
                self.folder_edit_focus = true;
            }
            Some(Act::Color(id, color)) => self.folder_set_color(&id, color),
            Some(Act::Delete(id, name)) => self.folder_try_delete(&id, &name),
            None => {}
        }
        self.folder_delete_window(ui);
    }

    /// One inline folder-name editor row (rename or new folder).
    /// Built exactly like a normal tree row (`tree_row_ex`): the
    /// folder icon leads at the same offset under the same tint
    /// rules and the text field starts where the label would, so
    /// entering edit mode never makes the icon vanish (user
    /// report 2026-11-25).
    fn folder_edit_row(
        &mut self,
        ui: &mut egui::Ui,
        indent: f32,
        icon: ChromeIcon,
        icon_tint: egui::Color32,
    ) {
        let mut commit = false;
        let mut cancel = false;
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 0.0;
            ui.add_space(10.0 + indent);
            let (irect, _) =
                ui.allocate_exact_size(egui::vec2(15.0, 15.0), egui::Sense::empty());
            icon.image(15.0, icon_tint).paint_at(ui, irect);
            ui.add_space(6.0);
            if let Some(edit) = &mut self.folder_edit {
                let w = ui.available_width().min(190.0).max(60.0);
                let resp =
                    ui.add_sized([w, 30.0], egui::TextEdit::singleline(&mut edit.text));
                if self.folder_edit_focus {
                    resp.request_focus();
                    self.folder_edit_focus = false;
                }
                // Ending the edit (user report 2026-11-25): Enter
                // commits, clicking anywhere else commits too, Esc
                // cancels — every path clears `folder_edit` so the
                // row snaps back to its normal rendering. The old
                // code committed ONLY on lost_focus+Enter, so a
                // plain click-away left the editor open forever:
                // the row stayed in editor form (no icon, no
                // normal styling) until some later Enter happened
                // by. Esc is checked first because egui also
                // surrenders the TextEdit's focus on Escape, and
                // that blur must not be misread as a commit.
                if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                    cancel = true;
                } else if resp.lost_focus() {
                    commit = true;
                }
            }
        });
        if commit {
            self.folder_edit_commit();
        }
        if cancel {
            self.folder_edit = None;
        }
    }

    fn folder_edit_commit(&mut self) {
        if !self.need_identity_local("编辑文件夹") {
            return;
        }
        // Validate BEFORE taking the edit state (audit L6): an
        // empty-name commit used to consume the editor row along
        // with the error, so the user lost the name they were
        // typing along with the message. The row now survives a
        // rejected commit.
        let name = match self.folder_edit.as_ref() {
            Some(edit) => edit.text.trim().to_string(),
            None => return,
        };
        if name.is_empty() {
            self.say("文件夹名不能为空");
            return;
        }
        let Some(edit) = self.folder_edit.take() else {
            return;
        };
        match &edit.target {
            Some(id) => {
                match self.call_work(
                    "POST",
                    &format!("/v1/folders/{id}/rename"),
                    Some(json!({ "name": name })),
                ) {
                    Ok(_) => {
                        self.msg = "已重命名".into();
                        self.reload_folders();
                    }
                    Err(e) => self.msg = format!("重命名失败：{e}"),
                }
            }
            None => {
                match self.call_work(
                    "POST",
                    "/v1/folders",
                    Some(json!({
                        "name": name,
                        "parent_id": edit.parent,
                        "scope": edit.scope
                    })),
                ) {
                    Ok(_) => {
                        self.msg = "已新建文件夹".into();
                        self.reload_folders();
                    }
                    Err(e) => self.msg = format!("新建失败：{e}"),
                }
            }
        }
    }

    /// Drag-and-drop move from the sidebar folder tree: reparent
    /// and/or reorder through the store's move endpoint (shared by
    /// the local dispatch and the server REST route), then reload
    /// the folder list so the tree shows the persisted order.
    fn folder_drag_move(&mut self, id: &str, parent: Option<String>, before: Option<String>) {
        if !self.need_identity_local("移动文件夹") {
            return;
        }
        match self.call_work(
            "POST",
            &format!("/v1/folders/{id}/move"),
            Some(json!({ "parent_id": parent, "before_id": before })),
        ) {
            Ok(_) => {
                self.msg = "已移动文件夹".into();
                self.reload_folders();
            }
            Err(e) => self.msg = format!("移动失败：{e}"),
        }
    }

    /// Set or clear a folder's sidebar color (the context menu's
    /// 颜色 submenu; None restores the default icon tint).
    fn folder_set_color(&mut self, id: &str, color: Option<String>) {
        if !self.need_identity_local("设置文件夹颜色") {
            return;
        }
        match self.call_work(
            "POST",
            &format!("/v1/folders/{id}/color"),
            Some(json!({ "color": color })),
        ) {
            Ok(_) => {
                self.msg = "已设置文件夹颜色".into();
                self.reload_folders();
            }
            Err(e) => self.msg = format!("设置颜色失败：{e}"),
        }
    }

    fn reload_folders(&mut self) {
        if let Ok(v) = self.call_work("GET", "/v1/folders", None) {
            self.folders = v
                .get("folders")
                .and_then(|f| f.as_array())
                .cloned()
                .unwrap_or_default();
        }
    }

    /// Deleting a folder always opens the two-choice dialog
    /// (user ruling 2026-11-24): 只删除文件夹 (contents lift to
    /// the parent) or 连同内容移到回收站. There is deliberately
    /// no non-empty pre-check any more — both shapes are valid
    /// for empty and non-empty folders alike, and the store
    /// performs the chosen one atomically.
    fn folder_try_delete(&mut self, id: &str, name: &str) {
        self.folder_delete_confirm = Some((id.to_string(), name.to_string()));
    }

    /// The overlay stack: modal tasks rendered above the frame
    /// stacks, in push order. Esc routing lives here — it closes
    /// the top overlay first (with the page editor's unsaved
    /// guard) and never touches the frame stacks below.
    fn overlay_stack_ui(&mut self, ui: &mut egui::Ui, screen: egui::Rect) {
        if ui.input(|i| i.key_pressed(egui::Key::Escape)) && !self.overlays.is_empty() {
            match self.overlays.last() {
                Some(Overlay::Confirm(_)) => {
                    self.overlays.pop();
                }
                Some(Overlay::Light(_)) => {
                    self.overlays.pop();
                }
                Some(Overlay::PageEdit(_)) => {
                    let dirty = match self.overlays.last() {
                        Some(Overlay::PageEdit(pe)) => pe.dirty(),
                        _ => false,
                    };
                    if dirty {
                        self.overlays.push(Overlay::Confirm(ConfirmState {
                            title: "放弃未保存的修改？".into(),
                            body: "页面编辑尚未保存为新版本，关闭将丢弃这些修改。".into(),
                            confirm_label: "放弃修改".into(),
                            danger: true,
                            action: ConfirmAction::DiscardPageEdit,
                        }));
                    } else {
                        self.overlays.pop();
                    }
                }
                Some(Overlay::Present(_)) => {
                    self.overlays.pop();
                    self.end_present(ui);
                }
                Some(Overlay::PickTemplate(_)) => {
                    self.overlays.pop();
                }
                Some(Overlay::AllVersions { .. }) => {
                    self.overlays.pop();
                }
                Some(Overlay::DeckInfo(_)) => {
                    self.overlays.pop();
                }
                None => {}
            }
        }
        // Render bottom-to-top. Each surface takes ownership of its
        // state and hands it back when it stays open, so surfaces
        // can freely call back into the app while rendering.
        let mut idx = 0;
        while idx < self.overlays.len() {
            let ov = self.overlays.remove(idx);
            match self.overlay_ui(ui, screen, ov) {
                Some(back) => {
                    self.overlays.insert(idx, back);
                    idx += 1;
                }
                None => {}
            }
        }
    }

    /// 全部历史版本 (drill retrofit): the old index.more_for
    /// full-history Window, now an overlay above the detail frame.
    /// Data comes from the open Detail frame; picking a version
    /// commits through switch_entry_version and closes.
    fn all_versions_ui(&mut self, ui: &mut egui::Ui, entry_pos: usize, index_id: &str) -> bool {
        // Opened against one index's detail frame; if that frame
        // is no longer current, close instead of rebinding to
        // whatever detail the user opened meanwhile (audit M2).
        let frame_matches = matches!(
            self.index.stack.top(),
            IndexFrame::Detail(d) if d.index_id == index_id
        );
        if !frame_matches {
            return false;
        }
        let entries_now = self.current_entries();
        let rid = entries_now
            .get(entry_pos)
            .map(|e| s(e, "rendition_id"))
            .unwrap_or_default();
        let (versions, cur_id) = match self.index.stack.top() {
            IndexFrame::Detail(d) => (
                d.unit_versions.get(&rid).cloned().unwrap_or_default(),
                d.detail
                    .get("resolved_entries")
                    .and_then(|r| r.as_array())
                    .and_then(|a| a.get(entry_pos))
                    .and_then(|re| re.get("resolved"))
                    .map(|v| s(v, "id"))
                    .unwrap_or_default(),
            ),
            _ => (Vec::new(), String::new()),
        };
        let latest_id = versions.first().map(|v| s(v, "id")).unwrap_or_default();
        let mut open = true;
        let mut switch: Option<(String, String, bool)> = None;
        egui::Window::new("全部历史版本")
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
            .show(ui.ctx(), |ui| {
                egui::ScrollArea::vertical()
                    .max_height(430.0)
                    .show(ui, |ui| {
                        ui.horizontal_wrapped(|ui| {
                            for v in &versions {
                                let vid = s(v, "id");
                                let is_cur = vid == cur_id;
                                ui.vertical(|ui| {
                                    if self.thumb_button(ui, &vid, 0, 110.0, is_cur) {
                                        switch =
                                            Some((vid.clone(), s(v, "semver"), vid == latest_id));
                                    }
                                    ui.label(
                                        egui::RichText::new(format!(
                                            "v{} · {} 页",
                                            s(v, "semver"),
                                            i(v, "page_count")
                                        ))
                                        .small()
                                        .color(
                                            palette_color(if is_cur {
                                                ACCENT_DEEP
                                            } else {
                                                TEXT_WEAK
                                            }),
                                        ),
                                    );
                                });
                            }
                        });
                    });
            });
        if let Some((vid, sv, is_latest)) = switch {
            self.switch_entry_version(entry_pos, &vid, &sv, is_latest);
            return false;
        }
        open
    }

    fn overlay_ui(
        &mut self,
        ui: &mut egui::Ui,
        screen: egui::Rect,
        ov: Overlay,
    ) -> Option<Overlay> {
        match ov {
            Overlay::Confirm(c) => self.confirm_ui(ui, c),
            Overlay::AllVersions { entry_pos, index_id } => {
                if self.all_versions_ui(ui, entry_pos, &index_id) {
                    Some(Overlay::AllVersions { entry_pos, index_id })
                } else {
                    None
                }
            }
            Overlay::DeckInfo(st) => self.deck_info_ui(ui, st),
            Overlay::PickTemplate(t) => self.pick_template_ui(ui, t),
            Overlay::PageEdit(pe) => self.page_edit_ui(ui, screen, pe),
            Overlay::Light(light) => self.light_ui(ui, screen, light),
            Overlay::Present(p) => self.present_ui(ui, screen, p),
        }
    }

    fn confirm_ui(&mut self, ui: &mut egui::Ui, c: ConfirmState) -> Option<Overlay> {
        let mut open = true;
        let mut confirmed = false;
        let mut cancelled = false;
        egui::Window::new(&c.title)
            .open(&mut open)
            .title_bar(false)
            .frame(dlg_frame())
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
            .show(ui.ctx(), |ui| {
                // Same C30 dialog treatment as the folder-delete
                // confirm (dlg_frame, in-body 19px title, 13.5
                // body, 8/14 gaps, weak-form cancel); the content
                // width stays at this family's established 360 —
                // its bodies are single sentences. The ✕ feeds
                // `cancelled`, the old title-bar X's outcome.
                ui.set_width(360.0);
                ui.spacing_mut().item_spacing.y = 0.0;
                ui.horizontal(|ui| {
                    ui.vertical(|ui| {
                        ui.set_width(320.0);
                        ui.label(
                            egui::RichText::new(&c.title)
                                .strong()
                                .size(19.0)
                                .color(palette_color(TEXT_MAIN)),
                        );
                    });
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Min), |ui| {
                        if dlg_close_x(ui) {
                            cancelled = true;
                        }
                    });
                });
                ui.add_space(8.0);
                ui.label(
                    egui::RichText::new(&c.body)
                        .size(13.5)
                        .color(palette_color(TEXT_MAIN)),
                );
                ui.add_space(14.0);
                ui.horizontal(|ui| {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.spacing_mut().item_spacing.x = 8.0;
                        let b = if c.danger {
                            btn_danger(ui, &c.confirm_label)
                        } else {
                            btn_primary(ui, &c.confirm_label)
                        };
                        if b.clicked() {
                            confirmed = true;
                        }
                        if btn_weak(ui, tr("取消")).clicked() {
                            cancelled = true;
                        }
                    });
                });
            });
        if !open || cancelled {
            return None;
        }
        if confirmed {
            match c.action {
                ConfirmAction::PurgeTrash(id) => self.trash_purge(&id),
                ConfirmAction::PurgeTrashMany(ids) => {
                    let mut ok = 0usize;
                    for id in &ids {
                        if self
                            .call_work("POST", &format!("/v1/trash/{id}/purge"), Some(json!({})))
                            .is_ok()
                        {
                            ok += 1;
                        }
                    }
                    self.msg = format!("{} {} {}", tr("已彻底删除"), ok, tr("项"));
                    if let TrashFrame::List(ls) = self.trash.stack.top_mut() {
                        ls.selected.clear();
                    }
                    self.trash.loaded = false;
                    // Phase-1 (design §3.1 point 22): no wall
                    // refresh — purged rows were already absent
                    // from every wall (same reasoning as
                    // trash_purge, delete-freeze round 2); the
                    // Trash mark is the bin's own bookkeeping.
                    self.changed(Change::Trash);
                }
                ConfirmAction::ShutdownServer(i) => {
                    self.do_shutdown_server(i);
                }
                ConfirmAction::EmptyTrash => {
                    // F6: rows merged from the server (marked
                    // "org" at load) are not the local face's to
                    // purge-all — purge them individually through
                    // REST so the merged list really empties. In
                    // REST-fallback mode (no local library) the
                    // purge-all below already hit the server and
                    // no row carries the mark.
                    let org_ids: Vec<String> = if self.local.is_some() {
                        self.trash
                            .items
                            .iter()
                            .filter(|x| {
                                x.get("org").and_then(|v| v.as_bool()) == Some(true)
                            })
                            .map(|x| s(x, "id"))
                            .collect()
                    } else {
                        Vec::new()
                    };
                    match self.call_work("POST", "/v1/trash/purge-all", Some(json!({}))) {
                        Ok(_) => {
                            for id in &org_ids {
                                let _ = self.call(
                                    "POST",
                                    &format!("/v1/trash/{id}/purge"),
                                    Some(json!({})),
                                );
                            }
                            self.msg = "已清空回收站".into();
                            self.trash.stack = NavStack::new(TrashFrame::List(TrashListState {
                                filter: 0,
                                selected: Default::default(),
                            }));
                            self.trash.loaded = false;
                            // Phase-1 (design §3.1 point 23): the
                            // per-row REST purges above are the
                            // operation itself (phase-3 territory);
                            // only the trailing refresh is replaced
                            // (no wall refresh — every purged row
                            // was already absent from the walls,
                            // delete-freeze round 2).
                            self.changed(Change::Trash);
                        }
                        Err(e) => self.msg = format!("清空失败：{e}"),
                    }
                }
                ConfirmAction::DeleteOldLibrary(path) => {
                    // Execution-time revalidation (audit P2): the
                    // prompt is a non-modal window that may have
                    // sat open while the user changed the data
                    // directory — never delete the directory the
                    // client is using NOW, and never delete a
                    // directory that no longer probes as a library.
                    let current = self
                        .local
                        .as_ref()
                        .map(|l| l.root.clone())
                        .unwrap_or_else(local_store::library_dir);
                    let configured = local_store::read_bootstrap()
                        .data_dir
                        .map(std::path::PathBuf::from);
                    if path == current
                        || configured.as_deref() == Some(path.as_path())
                        || current.starts_with(&path)
                    {
                        self.say("已取消删除：该目录是当前正在使用的数据目录");
                    } else if local_store::probe_library(&path).is_none() {
                        self.say(
                            "已取消删除：该目录已不是可识别的启幕库（可能已被移动或改作他用）",
                        );
                    } else {
                        match std::fs::remove_dir_all(&path) {
                            Ok(()) => {
                                self.say(format!("旧库已删除：{}", path.display()));
                            }
                            Err(e) => {
                                self.say(format!(
                                    "旧库删除失败（{e}），旧库仍保留在：{}",
                                    path.display()
                                ));
                            }
                        }
                    }
                }
                ConfirmAction::DiscardPageEdit => {
                    if let Some(pos) = self
                        .overlays
                        .iter()
                        .rposition(|o| matches!(o, Overlay::PageEdit(_)))
                    {
                        self.overlays.remove(pos);
                    }
                }
                ConfirmAction::MovePageBinding(req) => self.apply_page_binding(req),
                ConfirmAction::PdfRasterFallback { deck, form, page, stem } => {
                    // Audit2 E-L5: raster-export the deck bytes
                    // captured when the TextLoss warning was raised.
                    // The old handler re-ran the whole local export
                    // against the CURRENT library, so a deck edited
                    // (or replaced) between the warning and the
                    // click was exported instead of the one the user
                    // accepted the warning for.
                    match local_render::export_pdf(&deck, &form, page, false) {
                        Ok(bytes) => {
                            if let Some(path) = rfd::FileDialog::new()
                                .set_file_name(format!("{stem}.pdf"))
                                .save_file()
                            {
                                self.msg = match std::fs::write(path, bytes) {
                                    Ok(()) => "已保存（纯图片版：文字不可搜索、复制）".into(),
                                    Err(e) => format!("保存 PDF 失败：{e}"),
                                };
                            }
                        }
                        Err(local_render::PdfFail::TextLoss { count, sample }) => {
                            self.msg = format!(
                                "导出 PDF 失败：{count} 个字没有覆盖字体（首个：{sample}），纯图片导出也保留不了这些文字，已取消导出"
                            );
                        }
                        Err(local_render::PdfFail::Other(e)) => {
                            self.msg = format!("导出 PDF 失败：{e}");
                        }
                    }
                }
            }
            return None;
        }
        Some(Overlay::Confirm(c))
    }

    /// sdInfo panel (T5): the four metadata fields of the
    /// user's definition — page count, version, submitter,
    /// visible scope — as a read-only overlay in the
    /// Confirm / AllVersions form (centered Window; closes
    /// via its X, the close button, or Esc, like its siblings).
    fn deck_info_ui(&mut self, ui: &mut egui::Ui, st: DeckInfoState) -> Option<Overlay> {
        let mut open = true;
        let mut close = false;
        egui::Window::new(tr("演示稿信息"))
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
            .show(ui.ctx(), |ui| {
                ui.set_width(340.0);
                ui.label(
                    egui::RichText::new(&st.title)
                        .size(13.5)
                        .color(palette_color(TEXT_MAIN)),
                );
                ui.add_space(8.0);
                egui::Grid::new("deck_info_grid")
                    .num_columns(2)
                    .spacing(egui::vec2(14.0, 5.0))
                    .show(ui, |ui| {
                        for (label, value) in [
                            ("页数", &st.pages),
                            ("来源索引", &st.source_index),
                            ("发布版本", &st.version),
                            ("提交人", &st.submitter),
                            ("可见范围", &st.scope),
                        ] {
                            ui.label(
                                egui::RichText::new(tr(label))
                                    .size(12.5)
                                    .color(palette_color(TEXT_WEAK)),
                            );
                            ui.label(
                                egui::RichText::new(value.as_str())
                                    .size(12.5)
                                    .color(palette_color(TEXT_MAIN)),
                            );
                            ui.end_row();
                        }
                    });
                ui.add_space(10.0);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if btn_default(ui, tr("关闭")).clicked() {
                        close = true;
                    }
                });
            });
        if !open || close {
            return None;
        }
        Some(Overlay::DeckInfo(st))
    }

    /// Back-reference to the source index of a showroom item
    /// (unified content model, user ruling 2026-11-25): the
    /// showroom holds a published index, not a standalone
    /// product, and its surfaces name the index it came from.
    /// The index's name when the local index list knows it,
    /// else the id's short form; "—" for source-less items
    /// (hand-made assemblies).
    fn deck_source_label(&self, item: &Value) -> String {
        let sid = s(item, "source_index_id");
        if sid.is_empty() {
            return "—".to_string();
        }
        self.indexes
            .iter()
            .find(|x| s(x, "id") == sid)
            .map(|x| s(x, "name"))
            .unwrap_or_else(|| sid.chars().take(8).collect())
    }

    /// The published-commit label of a showroom item: the
    /// pinned commit's message when the open index detail
    /// holds the commit list (commits are labelled by message
    /// in this client), else the commit's short id. An item
    /// with no pinned commit follows its source index's
    /// current head, and the label says so.
    fn deck_commit_label(&self, item: &Value) -> String {
        let cid = s(item, "commit_id");
        if cid.is_empty() {
            return "跟随索引最新".to_string();
        }
        if let IndexFrame::Detail(d) = self.index.stack.top() {
            if d.index_id == s(item, "source_index_id") {
                if let Some(list) = d.commits.as_ref().and_then(|c| c["commits"].as_array()) {
                    if let Some(c) = list.iter().find(|c| s(c, "id") == cid) {
                        let msg = s(c, "message");
                        if !msg.is_empty() {
                            return msg;
                        }
                    }
                }
            }
        }
        format!("提交 {}", cid.chars().take(8).collect::<String>())
    }

    /// sdInfo click: snapshot the target item's metadata and
    /// open the panel. Page count is the open deck's loaded
    /// page list — a plaza-selected deck has not been loaded
    /// and the item record carries no page count, so the field
    /// stays "—" rather than guessing. The panel also carries
    /// the back-reference to the published index (source index
    /// name + published commit, via the helpers above).
    fn open_deck_info(&mut self, item: &Value, pages: Option<usize>) {
        let version = self.deck_commit_label(item);
        let submitter = s(item, "submitted_by");
        self.overlays.push(Overlay::DeckInfo(DeckInfoState {
            title: s(item, "title"),
            pages: pages.map(|n| n.to_string()).unwrap_or_else(|| "—".into()),
            source_index: self.deck_source_label(item),
            version,
            submitter: if submitter.is_empty() {
                "—".into()
            } else {
                submitter
            },
            scope: scope_label(&s(item, "scope")),
        }));
    }

    fn pick_template_ui(
        &mut self,
        ui: &mut egui::Ui,
        mut t: PickTemplateState,
    ) -> Option<Overlay> {
        let mut open = true;
        let mut create = false;
        let mut cancelled = false;
        egui::Window::new(tr("从模板新建"))
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
            .show(ui.ctx(), |ui| {
                ui.set_width(380.0);
                ui.label(
                    egui::RichText::new(tr("选择模板（现有索引将作为模板）"))
                        .size(12.5)
                        .color(palette_color(TEXT_MAIN)),
                );
                ui.add_space(6.0);
                if self.indexes.is_empty() {
                    ui.label(
                        egui::RichText::new(tr(
                            "暂无可用模板：先创建一个索引，之后就能以它为模板新建。",
                        ))
                        .size(12.0)
                        .color(palette_color(TEXT_WEAK)),
                    );
                } else {
                    egui::ScrollArea::vertical()
                        .max_height(180.0)
                        .show(ui, |ui| {
                            for idx in self.indexes.clone() {
                                ui.radio_value(&mut t.template_id, s(&idx, "id"), s(&idx, "name"));
                            }
                        });
                }
                ui.add_space(10.0);
                ui.label(
                    egui::RichText::new(tr("新索引名称"))
                        .size(12.5)
                        .color(palette_color(TEXT_MAIN)),
                );
                ui.add(
                    egui::TextEdit::singleline(&mut t.new_name)
                        .desired_width(360.0)
                        .margin(egui::vec2(10.0, 6.0)),
                );
                ui.add_space(10.0);
                ui.horizontal(|ui| {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let ready = !t.new_name.trim().is_empty() && !t.template_id.is_empty();
                        if ui
                            .add_enabled(ready, egui::Button::new(tr("创建")))
                            .clicked()
                        {
                            create = true;
                        }
                        if btn_default(ui, tr("取消")).clicked() {
                            cancelled = true;
                        }
                    });
                });
            });
        if !open || cancelled {
            return None;
        }
        if create {
            let group_id = self
                .indexes
                .iter()
                .find(|x| s(x, "id") == t.template_id)
                .map(|x| s(x, "group_id"))
                .unwrap_or_default();
            // The template's resolved entries: the create call takes
            // the minimal shape; a follow-up commit restores the
            // locked policies and page excerpts verbatim.
            let mut entries_min: Vec<Value> = Vec::new();
            let mut entries_raw: Vec<Value> = Vec::new();
            if let Ok(detail) =
                self.call_work("GET", &format!("/v1/indexes/{}", t.template_id), None)
            {
                if let Some(res) = detail.get("resolved_entries").and_then(|r| r.as_array()) {
                    for re in res {
                        if let Some(e) = re.get("entry") {
                            entries_raw.push(e.clone());
                            entries_min.push(json!({
                                "unit_id": s(e, "unit_id"),
                                "rendition_id": s(e, "rendition_id"),
                            }));
                        }
                    }
                }
            }
            let r = self.call_work(
                "POST",
                "/v1/indexes",
                Some(json!({
                    "name": t.new_name.trim(),
                    "group_id": group_id,
                    "scope": "company",
                    "entries": entries_min,
                })),
            );
            match r {
                Ok(v) => {
                    let new_id = s(&v, "id");
                    if !entries_raw.is_empty() {
                        let _ = self.call_work(
                            "POST",
                            &format!("/v1/indexes/{new_id}/commits"),
                            Some(json!({
                                "branch": "main",
                                "message": "从模板创建",
                                "entries": entries_raw,
                            })),
                        );
                    }
                    self.msg = "已从模板创建".into();
                    self.page = Page::Index;
                    // Phase-1 (design §3.1 point 17): read-after-
                    // write — the detail open below navigates
                    // from the refreshed lists, so consume at
                    // once (new index's meta by id).
                    self.mark(Change::Indexes);
                    self.mark(Change::Units);
                    if new_id.is_empty() {
                        self.mark(Change::IndexMeta);
                    } else {
                        self.mark_index_meta(&new_id);
                    }
                    self.flush_dirty_now();
                    if !new_id.is_empty() {
                        self.open_index_detail(&new_id);
                    }
                    return None;
                }
                Err(e) => {
                    self.msg = format!("创建失败：{e}");
                }
            }
        }
        Some(Overlay::PickTemplate(t))
    }

    /// Playback uses the same DeckCraft thumbnails as the detail page until
    /// the DeckCraft playback view is connected.
    fn start_present_from_deck(&mut self, ui: &egui::Ui) {
        let SrFrame::Deck(deck) = self.showroom.stack.top() else {
            return;
        };
        let item = deck.item.clone();
        let pages = deck.pages.clone();
        self.start_presentation(ui, &item, &pages);
    }

    fn start_presentation(&mut self, ui: &egui::Ui, detail: &Value, deck: &[ShowroomPage]) {
        if deck.is_empty() {
            self.msg = "没有可放映的页面".into();
            return;
        }
        let (pages, scripts): (Vec<_>, Vec<_>) = deck
            .iter()
            .map(|page| ((page.version_id.clone(), page.slot), page.note.clone()))
            .unzip();
        ui.ctx()
            .send_viewport_cmd(egui::ViewportCommand::Fullscreen(true));
        self.overlays.push(Overlay::Present(PresentState {
            title: s(detail, "title"),
            pages,
            scripts,
            cur: 0,
            speaker: false,
            blackout: false,
            started: std::time::Instant::now(),
        }));
    }

    fn end_present(&mut self, ui: &egui::Ui) {
        ui.ctx()
            .send_viewport_cmd(egui::ViewportCommand::Fullscreen(false));
    }

    fn present_ui(
        &mut self,
        ui: &mut egui::Ui,
        screen: egui::Rect,
        mut p: PresentState,
    ) -> Option<Overlay> {
        if p.pages.is_empty() {
            self.end_present(ui);
            return None;
        }
        // Navigation input: → / ↓ / Space / Enter advance, ← / ↑
        // go back, wheel too; B toggles the blackout.
        let (fwd, back, black) = ui.input(|i| {
            (
                i.key_pressed(egui::Key::ArrowRight)
                    || i.key_pressed(egui::Key::ArrowDown)
                    || i.key_pressed(egui::Key::Space)
                    || i.key_pressed(egui::Key::Enter),
                i.key_pressed(egui::Key::ArrowLeft) || i.key_pressed(egui::Key::ArrowUp),
                i.key_pressed(egui::Key::B),
            )
        });
        let wheel = ui.input(|i| i.smooth_scroll_delta.y);
        if fwd || wheel < -10.0 {
            p.cur = (p.cur + 1).min(p.pages.len() - 1);
        }
        if back || wheel > 10.0 {
            p.cur = p.cur.saturating_sub(1);
        }
        if black {
            p.blackout = !p.blackout;
        }
        ui.ctx()
            .request_repaint_after(std::time::Duration::from_millis(500));

        let mut exit = false;
        let mut toggle_speaker = false;
        let mut toggle_black = false;
        egui::Area::new(egui::Id::new("presenter"))
            .fixed_pos(screen.min)
            .order(egui::Order::Foreground)
            .show(ui.ctx(), |ui| {
                let (rect, resp) = ui.allocate_exact_size(screen.size(), egui::Sense::click());
                let painter = ui.painter();
                // A28 stage black (prototype value, an intentional
                // exception to the app token set).
                let stage_bg = egui::Color32::from_rgb(0x0B, 0x0D, 0x0C);
                painter.rect_filled(rect, 0.0, stage_bg);
                let top_h = if p.speaker { 40.0 } else { 0.0 };
                let rail_w = if p.speaker { 300.0 } else { 0.0 };
                let stage = egui::Rect::from_min_max(
                    egui::pos2(rect.min.x, rect.min.y + top_h),
                    egui::pos2(rect.max.x - rail_w, rect.max.y),
                );
                if !p.blackout {
                    let (vid, page) = &p.pages[p.cur];
                    if let Some(tex) = self.thumb(vid, *page) {
                        let ts = tex.size_vec2();
                        if ts.x > 0.0 && ts.y > 0.0 {
                            let scale = (stage.width() / ts.x).min(stage.height() / ts.y);
                            let r = egui::Rect::from_center_size(stage.center(), ts * scale);
                            painter.image(
                                tex.id(),
                                r,
                                egui::Rect::from_min_max(
                                    egui::pos2(0.0, 0.0),
                                    egui::pos2(1.0, 1.0),
                                ),
                                egui::Color32::WHITE,
                            );
                        }
                    }
                }
                // Progress: a thin accent line along the bottom.
                let frac = (p.cur + 1) as f32 / p.pages.len() as f32;
                painter.rect_filled(
                    egui::Rect::from_min_max(
                        egui::pos2(rect.min.x, rect.max.y - 3.0),
                        egui::pos2(rect.min.x + rect.width() * frac, rect.max.y),
                    ),
                    0.0,
                    ACCENT,
                );
                if p.speaker {
                    // Speaker top bar: title / timer / position.
                    let secs = p.started.elapsed().as_secs();
                    let bar_text = format!(
                        "{}    {:02}:{:02}    {} / {}",
                        p.title,
                        secs / 60,
                        secs % 60,
                        p.cur + 1,
                        p.pages.len()
                    );
                    let font = egui::FontId::new(13.0, egui::FontFamily::Proportional);
                    let g = painter.layout_no_wrap(bar_text, font, egui::Color32::from_gray(220));
                    painter.galley(
                        egui::pos2(rect.min.x + 16.0, rect.min.y + 12.0),
                        g,
                        egui::Color32::from_gray(220),
                    );
                    // Right rail: next page preview + this page's
                    // speaker notes.
                    let rail = egui::Rect::from_min_max(
                        egui::pos2(rect.max.x - rail_w, rect.min.y + top_h),
                        rect.max,
                    );
                    painter.rect_filled(rail, 0.0, egui::Color32::from_rgb(0x14, 0x17, 0x16));
                    let mut y = rail.min.y + 14.0;
                    let small = egui::FontId::new(11.5, egui::FontFamily::Proportional);
                    let gray = egui::Color32::from_gray(160);
                    let g = painter.layout_no_wrap(tr("下一页").to_string(), small.clone(), gray);
                    painter.galley(egui::pos2(rail.min.x + 14.0, y), g, gray);
                    y += 22.0;
                    if p.cur + 1 < p.pages.len() {
                        let (nvid, npage) = &p.pages[p.cur + 1];
                        if let Some(tex) = self.thumb(nvid, *npage) {
                            let w = rail.width() - 28.0;
                            let h = w * tex.size_vec2().y / tex.size_vec2().x.max(1.0);
                            let r = egui::Rect::from_min_size(
                                egui::pos2(rail.min.x + 14.0, y),
                                egui::vec2(w, h),
                            );
                            painter.image(
                                tex.id(),
                                r,
                                egui::Rect::from_min_max(
                                    egui::pos2(0.0, 0.0),
                                    egui::pos2(1.0, 1.0),
                                ),
                                egui::Color32::WHITE,
                            );
                            y += h + 14.0;
                        }
                    } else {
                        let g = painter.layout_no_wrap(
                            tr("（最后一页）").to_string(),
                            small.clone(),
                            gray,
                        );
                        painter.galley(egui::pos2(rail.min.x + 14.0, y), g, gray);
                        y += 22.0;
                    }
                    let g = painter.layout_no_wrap(tr("讲稿").to_string(), small.clone(), gray);
                    painter.galley(egui::pos2(rail.min.x + 14.0, y), g, gray);
                    y += 22.0;
                    let script = p.scripts.get(p.cur).cloned().unwrap_or_default();
                    let body_font = egui::FontId::new(12.5, egui::FontFamily::Proportional);
                    let g = painter.layout(
                        if script.is_empty() {
                            tr("（本页无讲稿）").to_string()
                        } else {
                            script
                        },
                        body_font,
                        egui::Color32::from_gray(225),
                        rail.width() - 28.0,
                    );
                    painter.galley(
                        egui::pos2(rail.min.x + 14.0, y),
                        g,
                        egui::Color32::from_gray(225),
                    );
                }
                if resp.clicked() && !p.speaker {
                    p.cur = (p.cur + 1).min(p.pages.len() - 1);
                }
                resp.context_menu(|ui| {
                    if ui
                        .button(if p.blackout {
                            tr("黑屏 ✓")
                        } else {
                            tr("黑屏")
                        })
                        .clicked()
                    {
                        toggle_black = true;
                        ui.close();
                    }
                    if ui
                        .button(if p.speaker {
                            tr("演讲者视图 ✓")
                        } else {
                            tr("演讲者视图")
                        })
                        .clicked()
                    {
                        toggle_speaker = true;
                        ui.close();
                    }
                    if ui.button(tr("结束放映")).clicked() {
                        exit = true;
                        ui.close();
                    }
                });
            });
        if toggle_black {
            p.blackout = !p.blackout;
        }
        if toggle_speaker {
            p.speaker = !p.speaker;
        }
        if exit {
            self.end_present(ui);
            return None;
        }
        Some(Overlay::Present(p))
    }

    /// The page editor's save — ONE implementation shared by
    /// the editor footer's 保存并发布新版本 and the bottom bar's
    /// 保存到索引 (sdBlockSave): the §8 bump-rule payload, the
    /// edit-pages call, the result message, the unit-detail
    /// reopen and the refresh. Returns Some(version id the
    /// save landed on) when the editor should close (saved, or
    /// nothing to save); None keeps it open after a failed
    /// call, with the failure in self.msg.
    fn save_page_edit(&mut self, pe: &PageEditState) -> Option<String> {
        let edits = pe.legacy_edits();
        let ops = pe.page_ops(None);
        let mut new_semver = String::new();
        // The version the save lands on: the newly inserted
        // version for content edits, the edited version itself
        // for script-only and no-op saves. Returned for
        // sdBlockPublish's resolved-version check; None is a
        // failed save (its message is already set).
        let mut saved_version_id = pe.version_id.clone();
        if !edits.is_empty() || !ops.is_empty() || pe.added_pages() > 0 {
            let bump = ["major", "minor", "patch"][pe.bump];
            let payload = json!({
                "edits": edits.iter().map(|(p, r, t)| json!({
                    "page": p, "run": r, "text": t,
                })).collect::<Vec<_>>(),
                "ops": ops,
                "added_pages": pe.added_pages(),
                "scripts": pe.scripts,
                "bump": bump,
                "note": pe.note,
            });
            match self.call_work(
                "POST",
                &format!("/v1/versions/{}/edit-pages", pe.version_id),
                Some(payload),
            ) {
                Ok(v) => {
                    let ver = v.get("version").cloned().unwrap_or(json!({}));
                    new_semver = s(&ver, "semver");
                    let id = s(&ver, "id");
                    if !id.is_empty() {
                        saved_version_id = id;
                    }
                }
                Err(e) => {
                    self.msg = format!("保存失败：{e}");
                    return None;
                }
            }
        } else if pe.scripts != pe.scripts_base {
            if let Err(e) = self.call_work(
                "PUT",
                &format!("/v1/versions/{}/scripts", pe.version_id),
                Some(json!({ "scripts": pe.scripts })),
            ) {
                self.msg = format!("讲稿保存失败：{e}");
                return None;
            }
        }
        self.msg = if new_semver.is_empty() {
            "已保存".into()
        } else {
            format!("已保存为新版本 v{new_semver}")
        };
        let open_uid = match self.content.stack.top() {
            ContentFrame::Unit(st) => Some(st.unit_id.clone()),
            _ => None,
        };
        if let Some(uid) = open_uid {
            self.reload_unit_detail(&uid);
        }
        // Phase-1 (design §3.1 point 16): the detail reload
        // above is the D1 path; the wall only needs Units. The
        // caller reads nothing but the returned version id.
        self.changed(Change::Units);
        Some(saved_version_id)
    }

    /// Page-level visual editor (prototype A29 shell): page rail,
    /// large preview, per-run text editing and the speaker notes
    /// of the page. Saving publishes a NEW version under the §8
    /// bump rule (the bump choice + note ride along) — the edited
    /// version itself is never mutated.
    fn page_edit_ui(
        &mut self,
        ui: &mut egui::Ui,
        screen: egui::Rect,
        mut pe: PageEditState,
    ) -> Option<Overlay> {
        // Load the current page's runs (v1) and element model (v2)
        // once per page.
        if !pe.work.contains_key(&pe.page) {
            let vid = pe.version_id.clone();
            let page = pe.page;
            match self.call_work(
                "GET",
                &format!("/v1/versions/{vid}/page-text?page={page}"),
                None,
            ) {
                Ok(v) => {
                    let runs: Vec<String> = v
                        .get("runs")
                        .and_then(|x| x.as_array())
                        .map(|arr| {
                            arr.iter()
                                .map(|t| t.as_str().unwrap_or("").to_string())
                                .collect()
                        })
                        .unwrap_or_default();
                    pe.base.insert(page, runs.clone());
                    pe.work.insert(page, runs);
                    if let Some(els) = v.get("elements").and_then(|x| x.as_array()) {
                        let list: Vec<ElWork> = read_elements(els);
                        pe.elems.insert(page, list.clone());
                        pe.elems_base.insert(page, list);
                        pe.slide_cx = i(&v, "slide_cx");
                        pe.slide_cy = i(&v, "slide_cy");
                        if pe.slide_cx <= 0 {
                            pe.slide_cx = 12192000;
                        }
                        if pe.slide_cy <= 0 {
                            pe.slide_cy = 6858000;
                        }
                    }
                }
                Err(e) => {
                    self.msg = format!("读取页面文本失败：{e}");
                    pe.base.insert(page, Vec::new());
                    pe.work.insert(page, Vec::new());
                }
            }
        }
        while pe.scripts.len() < pe.page_count {
            pe.scripts.push(String::new());
        }
        let v2 = pe.elems.contains_key(&pe.page);
        let mut close = false;
        let mut save = false;
        let mut undo_req = false;
        let mut redo_req = false;
        let mut nav = None;
        let mut add_page = false;
        let mut add_element = None;
        let mut alignment = None;
        // Set when a control gains focus (text/drag fields snapshot
        // on the gain frame, before any character changes) or when
        // the inspector's clone is about to be committed.
        let mut want_snapshot = false;
        let mut commit: Option<(usize, ElWork)> = None;
        let mut sel_req: Option<Option<usize>> = None;
        egui::Area::new(egui::Id::new("page_edit"))
            .fixed_pos(screen.min)
            .order(egui::Order::Foreground)
            .show(ui.ctx(), |ui| {
                egui::Frame::NONE
                    .fill(palette_color(PANEL_BG))
                    .show(ui, |ui| {
                        ui.set_min_size(screen.size());
                        ui.add_space(10.0);
                        ui.horizontal(|ui| {
                            ui.add_space(14.0);
                            ui.label(
                                egui::RichText::new(format!(
                                    "{} · v{} · {}",
                                    pe.unit_name,
                                    pe.semver,
                                    tr("页面编辑")
                                ))
                                .size(14.0)
                                .color(palette_color(TEXT_MAIN)),
                            );
                            ui.add_space(10.0);
                            if ui
                                .add_enabled(!pe.undo.is_empty(), egui::Button::new(tr("撤销")))
                                .clicked()
                            {
                                undo_req = true;
                            }
                            if ui
                                .add_enabled(!pe.redo.is_empty(), egui::Button::new(tr("重做")))
                                .clicked()
                            {
                                redo_req = true;
                            }
                            ui.add_enabled_ui(v2, |ui| {
                                ui.menu_button(tr("添加元素"), |ui| {
                                    for (kind, label) in [
                                        ("text", "文本"),
                                        ("shape", "形状"),
                                        ("line", "线条"),
                                        ("picture", "图片"),
                                    ] {
                                        if ui.button(tr(label)).clicked() {
                                            add_element = Some(kind);
                                            ui.close();
                                        }
                                    }
                                });
                            });
                            ui.add_enabled_ui(pe.can_align(), |ui| {
                                ui.menu_button(tr("多选对齐"), |ui| {
                                    for (direction, label) in ALIGNMENTS {
                                        if ui.button(tr(label)).clicked() {
                                            alignment = Some(*direction);
                                            ui.close();
                                        }
                                    }
                                });
                            });
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    ui.add_space(14.0);
                                    if text_action(ui, "✕ 关闭", palette_color(TEXT_MAIN), 13.0)
                                    {
                                        close = true;
                                    }
                                },
                            );
                        });
                        ui.add_space(8.0);
                        ui.separator();
                        ui.horizontal_top(|ui| {
                            ui.vertical(|ui| {
                                if btn_default_enabled(ui, pe.added_pages() < 100, tr("＋ 加页"))
                                    .clicked()
                                {
                                    add_page = true;
                                }
                                egui::ScrollArea::vertical()
                                    .id_salt("editor-page-rail")
                                    .max_width(124.0)
                                    .max_height((screen.height() - 176.0).max(132.0))
                                    .show(ui, |ui| {
                                        for pno in 0..pe.page_count {
                                            let clicked = if pno < pe.original_page_count {
                                                self.thumb_button(
                                                    ui,
                                                    &pe.version_id,
                                                    pno,
                                                    104.0,
                                                    pno == pe.page,
                                                )
                                            } else {
                                                ui.add_sized(
                                                    [104.0, 58.0],
                                                    egui::Button::new(format!(
                                                        "{} {}",
                                                        tr("第"),
                                                        pno + 1
                                                    ))
                                                    .selected(pno == pe.page),
                                                )
                                                .clicked()
                                            };
                                            if clicked {
                                                nav = Some(pno);
                                            }
                                        }
                                    });
                            });
                            ui.separator();
                            // Center: the stage with element boxes (v2) or
                            // the v1 thumbnail + flat run editors.
                            if v2 {
                                ui.vertical(|ui| {
                                    self.page_edit_stage(ui, &mut pe);
                                    pe.preview.status(ui);
                                    ui.add_space(8.0);
                                    let hint =
                                        match pe.sel.and_then(|s| {
                                            pe.elems.get(&pe.page).and_then(|l| l.get(s))
                                        }) {
                                            Some(el) => format!(
                                                "{} · {}",
                                                el_kind_label(&el.kind),
                                                if el.name.is_empty() {
                                                    tr("未命名元素")
                                                } else {
                                                    el.name.as_str()
                                                }
                                            ),
                                            None => tr("点选元素，按住 Ctrl 或 Shift 多选。")
                                                .to_string(),
                                        };
                                    ui.label(
                                        egui::RichText::new(hint)
                                            .size(12.0)
                                            .color(palette_color(TEXT_WEAK)),
                                    );
                                });
                            } else {
                                egui::ScrollArea::vertical().show(ui, |ui| {
                                    self.thumb_ui(ui, &pe.version_id.clone(), pe.page, 620.0);
                                    ui.add_space(10.0);
                                    ui.label(
                                        egui::RichText::new(tr(
                                            "文本内容（逐段编辑，保存后生成新版本）",
                                        ))
                                        .size(12.5)
                                        .color(palette_color(TEXT_MAIN)),
                                    );
                                    ui.add_space(6.0);
                                    let has_runs = pe
                                        .work
                                        .get(&pe.page)
                                        .map(|r| !r.is_empty())
                                        .unwrap_or(false);
                                    if !has_runs {
                                        ui.label(
                                            egui::RichText::new(tr("本页没有可编辑的文本段。"))
                                                .size(12.0)
                                                .color(palette_color(TEXT_WEAK)),
                                        );
                                    }
                                    if let Some(runs) = pe.work.get_mut(&pe.page) {
                                        for (ri, run) in runs.iter_mut().enumerate() {
                                            ui.horizontal(|ui| {
                                                ui.label(
                                                    egui::RichText::new(format!(
                                                        "{} {}",
                                                        tr("文本"),
                                                        ri + 1
                                                    ))
                                                    .size(11.5)
                                                    .color(palette_color(TEXT_FAINT)),
                                                );
                                                let resp = ui.add(
                                                    egui::TextEdit::singleline(run)
                                                        .desired_width(520.0)
                                                        .margin(egui::vec2(8.0, 5.0)),
                                                );
                                                if resp.gained_focus() {
                                                    want_snapshot = true;
                                                }
                                            });
                                        }
                                    }
                                    ui.add_space(12.0);
                                });
                            }
                            ui.separator();
                            // Side panel: element list + inspector (v2)
                            // or the page position alone (v1), then the
                            // speaker notes for both.
                            ui.vertical(|ui| {
                                ui.set_width(if v2 { 308.0 } else { 210.0 });
                                egui::ScrollArea::vertical()
                                    .id_salt("editor-inspector")
                                    .max_height((screen.height() - 176.0).max(132.0))
                                    .show(ui, |ui| {
                                        ui.label(
                                            egui::RichText::new(format!(
                                                "{} {} / {} {}",
                                                tr("第"),
                                                pe.page + 1,
                                                pe.page_count,
                                                tr("页")
                                            ))
                                            .size(12.0)
                                            .color(palette_color(TEXT_WEAK)),
                                        );
                                        if v2 {
                                            ui.add_space(8.0);
                                            page_edit_inspector(
                                                ui,
                                                &pe,
                                                &mut want_snapshot,
                                                &mut commit,
                                                &mut sel_req,
                                            );
                                        }
                                        ui.add_space(10.0);
                                        ui.label(
                                            egui::RichText::new(tr("讲稿"))
                                                .size(12.5)
                                                .color(palette_color(TEXT_MAIN)),
                                        );
                                        if pe.page < pe.scripts.len() {
                                            let resp = ui.add(
                                                egui::TextEdit::multiline(&mut pe.scripts[pe.page])
                                                    .desired_width(if v2 { 292.0 } else { 196.0 })
                                                    .desired_rows(10),
                                            );
                                            if resp.gained_focus() {
                                                want_snapshot = true;
                                            }
                                        }
                                    });
                            });
                        });
                        ui.separator();
                        // Footer (B5): bump choice per the §8 rule, the
                        // publish note, cancel / save-as-new-version.
                        ui.horizontal(|ui| {
                            ui.add_space(14.0);
                            if let Some(b) = segmented(
                                ui,
                                &[tr("大版本"), tr("小版本"), tr("修订号")],
                                pe.bump,
                                SegStyle::Soft,
                            ) {
                                pe.bump = b;
                            }
                            ui.add(
                                egui::TextEdit::singleline(&mut pe.note)
                                    .hint_text(tr("发布说明"))
                                    .desired_width(220.0)
                                    .margin(egui::vec2(8.0, 5.0)),
                            );
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    ui.add_space(14.0);
                                    let can_save = pe.dirty();
                                    if ui
                                        .add_enabled(
                                            can_save,
                                            egui::Button::new(tr("保存并发布新版本")),
                                        )
                                        .clicked()
                                    {
                                        save = true;
                                    }
                                    if btn_default(ui, tr("取消")).clicked() {
                                        close = true;
                                    }
                                },
                            );
                        });
                        ui.add_space(10.0);
                    });
            });
        // Keyboard undo/redo, only when no text field holds focus
        // (a focused TextEdit keeps Ctrl+Z for its own buffer).
        if ui.ctx().memory(|m| m.focused().is_none()) {
            let (undo_key, redo_key) = ui.input(|inp| {
                (
                    inp.modifiers.ctrl && !inp.modifiers.shift && inp.key_pressed(egui::Key::Z),
                    inp.modifiers.ctrl
                        && (inp.key_pressed(egui::Key::Y)
                            || (inp.modifiers.shift && inp.key_pressed(egui::Key::Z))),
                )
            });
            if undo_key {
                undo_req = true;
            }
            if redo_key {
                redo_req = true;
            }
        }
        // Deferred mutations, in undo order: snapshot the pre-edit
        // state first, then land the inspector's committed clone,
        // then any undo/redo request.
        if want_snapshot {
            pe.push_undo();
        }
        if let Some((idx, elw)) = commit {
            if let Some(list) = pe.elems.get_mut(&pe.page) {
                if idx < list.len() {
                    list[idx] = elw;
                }
            }
        }
        if let Some(selection) = sel_req {
            pe.select_element(
                selection,
                ui.input(|input| {
                    input.modifiers.command || input.modifiers.ctrl || input.modifiers.shift
                }),
            );
        }
        if let Some(page) = nav {
            pe.page = page;
            pe.select_element(None, false);
            pe.drag = None;
        }
        if add_page {
            pe.add_page();
        }
        if let Some(kind) = add_element {
            let picture = if kind == "picture" {
                pick_editor_picture()
            } else {
                None
            };
            if kind != "picture" || picture.is_some() {
                pe.add_element(kind, picture);
            }
        }
        if let Some(direction) = alignment {
            pe.align(direction);
        }
        self.update_page_edit_preview(ui.ctx(), &mut pe);
        if undo_req {
            pe.do_undo();
        }
        if redo_req {
            pe.do_redo();
        }
        if close {
            if pe.dirty() {
                self.overlays.push(Overlay::Confirm(ConfirmState {
                    title: "放弃未保存的修改？".into(),
                    body: "页面编辑尚未保存为新版本，关闭将丢弃这些修改。".into(),
                    confirm_label: "放弃修改".into(),
                    danger: true,
                    action: ConfirmAction::DiscardPageEdit,
                }));
                return Some(Overlay::PageEdit(pe));
            }
            return None;
        }
        if save {
            if self.save_page_edit(&pe).is_some() {
                return None;
            }
            return Some(Overlay::PageEdit(pe));
        }
        Some(Overlay::PageEdit(pe))
    }

    fn page_edit_stage(&mut self, ui: &mut egui::Ui, pe: &mut PageEditState) {
        let page = pe.page;
        let width = (ui.ctx().content_rect().width() - 480.0).clamp(300.0, 850.0);
        let height = width * pe.slide_cy.max(1) as f32 / pe.slide_cx.max(1) as f32;
        let (rect, response) =
            ui.allocate_exact_size(egui::vec2(width, height), egui::Sense::click_and_drag());
        let painter = ui.painter().with_clip_rect(rect.intersect(ui.clip_rect()));
        let picture = pe.preview.texture(page).or_else(|| {
            (page < pe.original_page_count)
                .then(|| self.thumb(&pe.version_id, page))
                .flatten()
        });
        if let Some(texture) = picture {
            painter.image(
                texture.id(),
                rect,
                egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0)),
                egui::Color32::WHITE,
            );
        } else {
            painter.rect_filled(
                rect,
                0.0,
                if page >= pe.original_page_count {
                    egui::Color32::WHITE
                } else {
                    palette_color(HOVER_BG)
                },
            );
        }
        let elements = pe.elems.get(&page).map(Vec::as_slice).unwrap_or(&[]);
        let scale = width / pe.slide_cx.max(1) as f32;
        let root = StageTransform {
            origin: rect.min,
            x_axis: egui::vec2(scale, 0.0),
            y_axis: egui::vec2(0.0, scale),
        };
        let transforms = stage_transforms(elements, root);
        let bounds: Vec<_> = elements
            .iter()
            .zip(&transforms)
            .map(|(element, transform)| transform.project(element))
            .collect();
        let handle = |element: egui::Rect| {
            egui::Rect::from_min_size(element.max - egui::vec2(9.0, 9.0), egui::vec2(9.0, 9.0))
        };
        for (index, area) in bounds.iter().copied().enumerate() {
            let selected = pe.selection.contains(&index);
            if elements[index].has_xfrm {
                painter.rect_stroke(
                    area,
                    0.0,
                    egui::Stroke::new(
                        if selected { 1.8 } else { 0.6 },
                        if selected {
                            ACCENT
                        } else {
                            palette_color(TEXT_FAINT).gamma_multiply(0.55)
                        },
                    ),
                    egui::StrokeKind::Middle,
                );
                if pe.sel == Some(index) && pe.selection.len() == 1 {
                    painter.rect_filled(handle(area), 0.0, ACCENT);
                }
            }
        }
        let Some(pointer) = response.interact_pointer_pos() else {
            if response.drag_stopped() {
                pe.drag = None;
            }
            return;
        };
        let hit = || {
            bounds
                .iter()
                .enumerate()
                .rev()
                .find(|(index, area)| elements[*index].has_xfrm && area.contains(pointer))
                .map(|(index, _)| index)
        };
        if response.drag_started() {
            let resize = pe.sel.filter(|&index| {
                pe.selection.len() == 1
                    && bounds
                        .get(index)
                        .is_some_and(|area| handle(*area).expand(2.0).contains(pointer))
            });
            if let Some(index) = resize.or_else(hit) {
                let element = &elements[index];
                let drag = ElDrag {
                    el: index,
                    resize: resize.is_some(),
                    start_pos: pointer,
                    transform: transforms[index],
                    x: element.x,
                    y: element.y,
                    cx: element.cx,
                    cy: element.cy,
                };
                pe.select_element(Some(index), false);
                pe.push_undo();
                pe.drag = Some(drag);
            }
        } else if response.clicked() {
            let extend = ui.input(|input| {
                input.modifiers.command || input.modifiers.ctrl || input.modifiers.shift
            });
            pe.select_element(hit(), extend);
        }
        if response.dragged() {
            if let Some(drag) = pe.drag {
                let delta = drag.transform.world_delta(pointer - drag.start_pos);
                if let Some(element) = pe
                    .elems
                    .get_mut(&page)
                    .and_then(|items| items.get_mut(drag.el))
                {
                    if drag.resize {
                        element.cx = (drag.cx + delta.x as i64).max(20_000);
                        element.cy = (drag.cy + delta.y as i64).max(20_000);
                    } else {
                        element.x = drag.x + delta.x as i64;
                        element.y = drag.y + delta.y as i64;
                    }
                }
            }
        }
        if response.drag_stopped() {
            pe.drag = None;
        }
    }

    fn folder_delete_window(&mut self, ui: &mut egui::Ui) {
        let Some((id, name)) = self.folder_delete_confirm.clone() else {
            return;
        };
        let mut open = true;
        let mut mode: Option<&'static str> = None;
        let mut cancel = false;
        egui::Window::new("删除文件夹")
            .open(&mut open)
            .title_bar(false)
            .frame(dlg_frame())
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
            .show(ui.ctx(), |ui| {
                // C30 native-dialog metrics (prototype <dialog> /
                // bindConfirmDlg): 680 total width = 640 content +
                // 2×20 frame padding, the title drawn in-body as
                // the prototype's h2 (19px, folder name in accent),
                // body copy at 13.5, and the .btn row right-aligned
                // with an 8px gap. Vertical gaps are explicit
                // add_space values (item_spacing.y is zeroed) so
                // they land exactly on the prototype's margins:
                // title→body 8, paragraph→paragraph 10, body→
                // buttons 14. The ✕ keeps the old title-bar X's
                // close path (dismiss via `cancel`, nothing runs).
                ui.set_width(640.0);
                ui.spacing_mut().item_spacing.y = 0.0;
                ui.horizontal(|ui| {
                    ui.vertical(|ui| {
                        ui.set_width(600.0);
                        ui.horizontal_wrapped(|ui| {
                            ui.spacing_mut().item_spacing.x = 0.0;
                            ui.label(
                                egui::RichText::new("删除文件夹「")
                                    .strong()
                                    .size(19.0)
                                    .color(palette_color(TEXT_MAIN)),
                            );
                            ui.label(
                                egui::RichText::new(&name)
                                    .strong()
                                    .size(19.0)
                                    .color(ACCENT),
                            );
                            ui.label(
                                egui::RichText::new("」？")
                                    .strong()
                                    .size(19.0)
                                    .color(palette_color(TEXT_MAIN)),
                            );
                        });
                    });
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Min), |ui| {
                        if dlg_close_x(ui) {
                            cancel = true;
                        }
                    });
                });
                ui.add_space(8.0);
                ui.label(
                    egui::RichText::new("只删除文件夹：里面的子文件夹、内容块和索引提升到上一级；顶层文件夹的内容回到根目录（索引回到「未分类」）。")
                        .size(13.5)
                        .color(palette_color(TEXT_MAIN)),
                );
                ui.add_space(10.0);
                ui.label(
                    egui::RichText::new("连同内容删除：文件夹、子文件夹和里面的全部内容一起移到回收站。")
                        .size(13.5)
                        .color(palette_color(TEXT_MAIN)),
                );
                ui.add_space(14.0);
                ui.horizontal(|ui| {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.spacing_mut().item_spacing.x = 8.0;
                        if btn_danger(ui, "连同内容移到回收站").clicked() {
                            mode = Some("trash");
                        }
                        if btn_default(ui, "只删除文件夹").clicked() {
                            mode = Some("lift");
                        }
                        if btn_weak(ui, "取消").clicked() {
                            cancel = true;
                        }
                    });
                });
            });
        if let Some(mode) = mode {
            // Picking a mode closes the dialog IMMEDIATELY and the
            // delete runs on a worker thread (freeze fix,
            // 2026-11-25): this used to call call_work
            // synchronously right here, with the dialog held open
            // until it returned. In "trash" mode the work scales
            // with the subtree — per-folder content queries,
            // per-index showroom scans, one trash insert per
            // object — and in REST-fallback mode it is a network
            // call on the REST agent's 120s read timeout, so a
            // big folder (or a wedged server) parked the UI
            // thread inside the click, dialog frozen on screen,
            // indistinguishable from a hang. "lift" is four SQL
            // statements, which is why it never showed the
            // symptom. The worker posts its result to
            // folder_del_rx; the per-frame drain applies the same
            // message / filter clearing / refresh the synchronous
            // path did. Semantics are unchanged: same route, same
            // body, same core operation with its guards.
            self.folder_delete_confirm = None;
            if self.folder_deleting.is_some() {
                self.msg = "正在删除文件夹，请稍候".into();
            } else {
                self.folder_deleting = Some((id.clone(), mode.to_string()));
                let gen = self.folder_del_gen;
                let local = self.local.clone();
                let server = self.server.clone();
                let token = self.token.clone();
                let tx = self.folder_del_tx.clone();
                let wake = self.egui_ctx.clone();
                let path = format!("/v1/folders/{id}/delete");
                let body = json!({ "mode": mode });
                std::thread::spawn(move || {
                    let res = work_call_thread(
                        &local,
                        &server,
                        token.as_deref(),
                        "POST",
                        &path,
                        Some(body),
                    );
                    let _ = tx.send((gen, id, mode.to_string(), res));
                    if let Some(ctx) = &wake {
                        ctx.request_repaint();
                    }
                });
            }
        }
        if cancel || !open {
            self.folder_delete_confirm = None;
        }
    }

    /// The login popup (user rulings 2026-11-23): login is a modal
    /// dialog over the main window — never a page, never an entry
    /// gate. The server address is the primary field: with an
    /// address alone, 「匿名进入」 connects a guest session that can
    /// browse the server's public content; the account fields are
    /// for a named login, which publishing / review / signed
    /// actions require. It reuses the pre-existing login form state
    /// (server / username / password live on App and feed do_login),
    /// so the connection restore and local-server autostart logic
    /// are untouched, and closing + reopening keeps whatever was
    /// typed.
    ///
    /// Close behavior (each path only flips `login_open` — nothing
    /// is submitted, cleared or rolled back on close, at any stage,
    /// including mid-typing and after a failed submit):
    /// - the title-bar X (egui's .open binding);
    /// - the 「取消」 button;
    /// - the Esc key.
    /// There is no mask layer (the codebase's other dialogs have
    /// none either); the window itself is the modal surface. A
    /// failed login keeps the popup open with the error shown
    /// inside; a successful one closes it via do_login.
    fn login_window(&mut self, ui: &mut egui::Ui) {
        if !self.login_open {
            return;
        }
        if ui.ctx().input(|i| i.key_pressed(egui::Key::Escape)) {
            self.login_open = false;
            return;
        }
        let mut open = true;
        let mut cancel = false;
        egui::Window::new("登录")
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
            .show(ui.ctx(), |ui| {
                ui.vertical_centered(|ui| {
                    self.logo_ui(ui, 56.0);
                    ui.add_space(10.0);
                    ui.label(
                        egui::RichText::new("启幕")
                            .strong()
                            .size(24.0)
                            .color(palette_color(TEXT_MAIN)),
                    );
                    ui.add_space(4.0);
                    ui.label(
                        egui::RichText::new(tr("PPT 内容管理与更新系统"))
                            .size(13.0)
                            .color(palette_color(TEXT_WEAK)),
                    );
                });
                ui.add_space(18.0);
                egui::Grid::new("login_grid")
                    .num_columns(2)
                    .spacing([10.0, 14.0])
                    .show(ui, |ui| {
                        for (label, val, pw) in [
                            ("服务器", &mut self.server, false),
                            ("账号", &mut self.username, false),
                            ("密码", &mut self.password, true),
                        ] {
                            ui.label(
                                egui::RichText::new(tr(label))
                                    .size(13.0)
                                    .color(palette_color(TEXT_WEAK)),
                            );
                            ui.add_sized(
                                [300.0, 34.0],
                                egui::TextEdit::singleline(val)
                                    .password(pw)
                                    .margin(egui::vec2(10.0, 8.0)),
                            );
                            ui.end_row();
                        }
                    });
                if !self.msg.is_empty() {
                    ui.add_space(10.0);
                    ui.label(
                        egui::RichText::new(&self.msg)
                            .size(12.0)
                            .color(palette_color(TEXT_WEAK)),
                    );
                }
                ui.add_space(10.0);
                ui.label(
                    egui::RichText::new(tr(
                        "只填服务器地址可「匿名进入」：浏览公开内容；发布、审批需实名登录。",
                    ))
                    .size(11.5)
                    .color(palette_color(TEXT_WEAK)),
                );
                ui.add_space(16.0);
                ui.horizontal(|ui| {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let login_btn = egui::Button::new(
                            egui::RichText::new(tr("登录"))
                                .size(14.0)
                                .color(egui::Color32::WHITE),
                        )
                        .fill(ACCENT)
                        .corner_radius(6)
                        .min_size(egui::vec2(110.0, 36.0));
                        if ui
                            .add_enabled(
                                !self.server.trim().is_empty()
                                    && !self.username.trim().is_empty()
                                    && !self.password.is_empty(),
                                login_btn,
                            )
                            .clicked()
                        {
                            self.do_login();
                        }
                        if ui.button(tr("取消")).clicked() {
                            cancel = true;
                        }
                        if ui
                            .add_enabled(
                                !self.server.trim().is_empty(),
                                egui::Button::new(tr("匿名进入")),
                            )
                            .clicked()
                        {
                            self.enter_anonymous();
                        }
                    });
                });
            });
        if cancel || !open {
            self.login_open = false;
        }
    }

    /// About dialog: brand logo + name + version (the 帮助 menu's
    /// 关于启幕 used to only flash a toast line).
    fn about_window(&mut self, ui: &mut egui::Ui) {
        if !self.about_open {
            return;
        }
        let mut open = true;
        egui::Window::new("关于启幕")
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
            .show(ui.ctx(), |ui| {
                ui.vertical_centered(|ui| {
                    self.logo_ui(ui, 64.0);
                    ui.add_space(10.0);
                    ui.label(
                        egui::RichText::new("启幕")
                            .strong()
                            .size(22.0)
                            .color(palette_color(TEXT_MAIN)),
                    );
                    ui.add_space(4.0);
                    ui.label(
                        egui::RichText::new(format!(
                            "{} · v{}",
                            tr("PPT 内容管理与更新系统"),
                            env!("CARGO_PKG_VERSION")
                        ))
                        .size(12.5)
                        .color(palette_color(TEXT_WEAK)),
                    );
                });
            });
        if !open {
            self.about_open = false;
        }
    }

    fn index_folder_of(&self, index_id: &str) -> String {
        self.indexes
            .iter()
            .find(|x| s(x, "id") == index_id)
            .map(|x| s(x, "folder_id"))
            .unwrap_or_default()
    }

    /// A showroom item's own plaza folder home (client-side
    /// assignment; empty = unassigned, shows only under 全部).
    fn plaza_folder_of(&self, item_id: &str) -> String {
        self.plaza_item_folders
            .get(item_id)
            .cloned()
            .unwrap_or_default()
    }

    /// Set (or, with an empty folder id, clear) a showroom
    /// item's plaza folder assignment and persist the mapping.
    fn set_plaza_item_folder(&mut self, item_id: &str, folder_id: &str) {
        if item_id.is_empty() {
            return;
        }
        let changed = if folder_id.is_empty() {
            self.plaza_item_folders.remove(item_id).is_some()
        } else {
            self.plaza_item_folders
                .insert(item_id.to_string(), folder_id.to_string())
                .as_deref()
                != Some(folder_id)
        };
        if changed {
            save_client_settings(json!({
                "plaza_item_folders": self.plaza_item_folders.clone(),
            }));
        }
    }

    /// The plaza sidebar's folders ("showroom" scope) in tree
    /// order, as (id, name, depth) — the move menu's targets.
    fn plaza_folder_options(&self) -> Vec<(String, String, usize)> {
        let mut folders: Vec<Value> = self
            .folders
            .iter()
            .filter(|f| s(*f, "scope") == "showroom")
            .cloned()
            .collect();
        folders.sort_by_key(|f| i(f, "sort"));
        fn depth_of(folders: &[Value], f: &Value) -> usize {
            let mut d = 0;
            let mut cur = s(f, "parent_id");
            while !cur.is_empty() && d < 8 {
                d += 1;
                match folders.iter().find(|x| s(x, "id") == cur) {
                    Some(p) => cur = s(p, "parent_id"),
                    None => break,
                }
            }
            d
        }
        let mut ordered: Vec<Value> = Vec::new();
        fn walk(folders: &[Value], parent: &str, out: &mut Vec<Value>) {
            for f in folders {
                if s(f, "parent_id") == parent {
                    out.push(f.clone());
                    let id = s(f, "id");
                    walk(folders, &id, out);
                }
            }
        }
        walk(&folders, "", &mut ordered);
        for f in &folders {
            if !ordered.iter().any(|x| s(x, "id") == s(f, "id")) {
                ordered.push(f.clone());
            }
        }
        ordered
            .iter()
            .map(|f| (s(f, "id"), s(f, "name"), depth_of(&folders, f)))
            .collect()
    }

    /// Folder-delete bookkeeping for plaza assignments, run on
    /// the delete drain BEFORE the refresh reloads the folder
    /// list (the deleted record is still readable here). Mirrors
    /// the core's own semantics: "lift" promotes the deleted
    /// folder's items to its parent (subfolder items keep their
    /// folders — the subfolders themselves are promoted, not
    /// deleted); "trash" removes the whole subtree, so items
    /// homed anywhere inside it become unassigned.
    fn apply_plaza_folder_delete(&mut self, id: &str, mode: &str) {
        if self.plaza_item_folders.is_empty() {
            return;
        }
        let showroom_folders: Vec<Value> = self
            .folders
            .iter()
            .filter(|f| s(*f, "scope") == "showroom")
            .cloned()
            .collect();
        let parent = showroom_folders
            .iter()
            .find(|f| s(f, "id") == id)
            .map(|f| s(f, "parent_id"))
            .unwrap_or_default();
        // subtree of `id` within the showroom scope (id included)
        let mut subtree: Vec<String> = vec![id.to_string()];
        let mut grew = true;
        while grew {
            grew = false;
            for f in &showroom_folders {
                let fid = s(f, "id");
                if !subtree.contains(&fid) && subtree.contains(&s(f, "parent_id")) {
                    subtree.push(fid);
                    grew = true;
                }
            }
        }
        let mut changed = false;
        if mode == "trash" {
            let before = self.plaza_item_folders.len();
            self.plaza_item_folders
                .retain(|_, fid| !subtree.contains(fid));
            changed = self.plaza_item_folders.len() != before;
        } else {
            for fid in self.plaza_item_folders.values_mut() {
                if *fid == id {
                    *fid = parent.clone();
                    changed = true;
                }
            }
            // A root folder's parent is "": promoted items
            // become unassigned (the empty value is not a home).
            if parent.is_empty() {
                let before = self.plaza_item_folders.len();
                self.plaza_item_folders.retain(|_, fid| !fid.is_empty());
                changed = changed || self.plaza_item_folders.len() != before;
            }
        }
        if changed {
            save_client_settings(json!({
                "plaza_item_folders": self.plaza_item_folders.clone(),
            }));
        }
    }

    /// Does an item whose home is `folder_id` pass the folder filter?
    /// The filter Some("") is the index page's 未分类 node: it
    /// matches exactly the items whose folder_id is empty (NULL).
    fn folder_filter_ok(&self, filter: &Option<String>, folder_id: &str) -> bool {
        match filter {
            None => true,
            Some(f) => f == folder_id,
        }
    }

    /// 新建索引 (the index page's pill button, also its toolbar
    /// slot): enter the create flow with the pick list primed from
    /// every unit's renditions.
    fn act_new_index(&mut self) {
        if !self.need_identity_local("新建索引") {
            return;
        }
        // Capture the sidebar folder BEFORE entering the page:
        // entering a board resets its selection to 全部 (user
        // ruling 2026-11-24), but a create started from inside a
        // folder keeps that folder as the new index's home.
        let entry_folder = self.index.overview_folder.clone();
        if self.page != Page::Index {
            self.navigate_to(Page::Index);
        }
        if matches!(self.index.stack.top(), IndexFrame::Create(_)) {
            return;
        }
        let mut pick: Vec<(String, String, bool)> = Vec::new();
        for item in &self.units {
            let unit = item.get("unit").cloned().unwrap_or(json!({}));
            if let Some(rends) = item.get("renditions").and_then(|r| r.as_array()) {
                for r in rends {
                    let rend = r.get("rendition").cloned().unwrap_or(json!({}));
                    pick.push((s(&unit, "id"), s(&rend, "id"), false));
                }
            }
        }
        // The create form's group defaults to the first group (the
        // old page-level new_group defaulting, now per-frame).
        let new_group = self
            .groups
            .first()
            .map(|g| s(g, "id"))
            .unwrap_or_default();
        // Folder context, captured at entry: a real folder selected
        // in the index sidebar is an explicit choice and becomes
        // the new index's home; 全部 / 未分类 (None / Some(""))
        // fall through to None — the default home, 未分类.
        let folder = entry_folder.filter(|f| !f.is_empty());
        self.index.stack.push(IndexFrame::Create(CreateState {
            new_group,
            folder,
            pick,
            ..Default::default()
        }));
    }

    /// Index content for the overview, detail, or create frame.
    fn ix_main_body(&mut self, ui: &mut egui::Ui) {
        // Frame-state take-out, same pattern as the content page:
        // a placeholder overview frame sits in the taken frame's
        // place meanwhile and is never rendered.
        let mut frame = self
            .index
            .stack
            .replace_top(IndexFrame::Overview(OverviewState::default()));
        let mut outcome: Option<Option<String>> = None;
        let mut action = None;
        body_frame()
            .inner_margin(egui::Margin {
                left: PAGE_PAD_L as i8,
                right: 22,
                top: 12,
                bottom: 0,
            })
            .show(ui, |ui| match &mut frame {
                IndexFrame::Create(st) => {
                    self.index_create(ui, st);
                    if st.closed {
                        outcome = Some(st.open_after.clone());
                    }
                }
                IndexFrame::Detail(st) => {
                    let name = self
                        .indexes
                        .iter()
                        .find(|x| s(x, "id") == st.index_id)
                        .map(|x| s(x, "name"))
                        .unwrap_or_default();
                    if crumb_bar(ui, &[tr("索引"), &name]) == Some(0) {
                        st.closed = true;
                    }
                    if !st.closed {
                        self.index_detail(ui, st, &mut action);
                        self.index_more_dialog(ui.ctx(), st, &mut action);
                    }
                    if st.closed {
                        outcome = Some(None);
                    }
                }
                IndexFrame::Overview(ov) => {
                    if let Some(id) = self.index_overview(ui, ov, &mut action) {
                        outcome = Some(Some(id));
                    }
                }
            });

        // Put the frame back before dispatching actions that read or reload it.
        self.index.stack.replace_top(frame);
        match outcome {
            Some(open_after) => {
                self.index.stack.pop();
                if let Some(id) = open_after {
                    self.open_index_detail_as(&id, "main".into());
                }
            }
            None => match action {
                Some(IndexDetailAction::Reload) => self.load_index_detail(),
                Some(IndexDetailAction::ChangeBranch(branch)) => self.change_index_branch(branch),
                Some(IndexDetailAction::FocusCommit { branch, id }) => {
                    let branch_changed = matches!(
                        self.index.stack.top(),
                        IndexFrame::Detail(st) if st.branch != branch
                    );
                    if branch_changed {
                        self.change_index_branch(branch.clone());
                    }
                    if let IndexFrame::Detail(st) = self.index.stack.top_mut() {
                        if st.branch == branch {
                            st.view = IndexDetailView::Commits;
                            st.selected_commit = Some(id);
                        }
                    }
                }
                Some(IndexDetailAction::OpenGraphTarget {
                    index,
                    branch,
                    commit,
                }) => {
                    self.open_index_detail_as(&index, branch.clone());
                    if let IndexFrame::Detail(st) = self.index.stack.top_mut() {
                        if st.index_id == index && st.branch == branch {
                            st.view = if commit.is_some() {
                                IndexDetailView::Commits
                            } else {
                                IndexDetailView::Lineage
                            };
                            st.selected_commit = commit;
                        }
                    }
                }
                Some(IndexDetailAction::Export(kind)) => {
                    if let IndexFrame::Detail(st) = self.index.stack.top() {
                        let index = st.detail.get("index").cloned().unwrap_or_default();
                        self.export_index_file(&index, kind);
                    }
                }
                Some(IndexDetailAction::Commit(entries, message)) => {
                    self.commit_entries(entries, message);
                }
                Some(IndexDetailAction::Insert(at)) => {
                    self.pick = Some(PickState::Insert {
                        index_name: self.current_index_name(),
                        at,
                    });
                    self.close_unit_detail();
                    self.page = Page::Content;
                }
                Some(IndexDetailAction::PickPages(pos, entry, selected)) => {
                    self.start_pick_pages(pos, &entry, selected);
                }
                Some(IndexDetailAction::SwitchVersion(pos, id, semver, is_latest)) => {
                    self.switch_entry_version(pos, &id, &semver, is_latest);
                }
                None => {}
            },
        }
    }

    /// Index overview: standard cards for the indexes living in the
    /// selected folder. Single click selects, double click opens.
    fn index_overview(
        &mut self,
        ui: &mut egui::Ui,
        ov: &mut OverviewState,
        action: &mut Option<IndexDetailAction>,
    ) -> Option<String> {
        let filter = self.index.overview_folder.clone();
        ui.add_space(2.0);
        if self.deep_source == Some(Page::Home) {
            ui.horizontal(|ui| {
                if text_action(ui, "‹ 返回主页", palette_color(ACCENT_DEEP), 13.0) {
                    self.deep_source = None;
                    self.page = Page::Home;
                }
            });
            ui.add_space(6.0);
        }
        ui.add_space(8.0);
        let items: Vec<Value> = self
            .indexes
            .iter()
            .filter(|x| self.folder_filter_ok(&filter, &s(x, "folder_id")))
            .filter(|x| self.index.overview_controls.matches(x))
            .cloned()
            .collect();
        if items.is_empty() {
            ov.selected = None;
            ui.label(egui::RichText::new(tr("当前筛选下没有索引")).color(palette_color(TEXT_WEAK)));
            return None;
        }
        if self.index.overview_controls.relationships {
            self.index_overview_relationships(ui, ov, &items, action);
            return None;
        }
        let my_name = self
            .user
            .as_ref()
            .map(|u| s(u, "display_name"))
            .unwrap_or_default();
        let mut open = None;
        egui::ScrollArea::vertical().show(ui, |ui| {
            ui.spacing_mut().item_spacing = egui::vec2(12.0, 12.0);
            // Card grid (prototype .idx-grid): fluid columns,
            // min 236px wide, 12px gaps.
            let avail = ui.available_width();
            let per_row = ((avail + 12.0) / 248.0).floor().max(1.0);
            let outer_width = ((avail + 12.0) / per_row - 12.0).floor().max(3.0);
            // egui frames include the border in their outer allocation.
            let card_w = outer_width - 2.0;
            let cover_h = card_w * 9.0 / 16.0;
            let card_h = cover_h + 70.0;
            let mut groups: Vec<(String, Vec<Value>)> = Vec::new();
            for item in items {
                let folder_id = s(&item, "folder_id");
                let folder = if folder_id.is_empty() {
                    "根目录".to_string()
                } else {
                    self.folders
                        .iter()
                        .find(|folder| s(folder, "id") == folder_id)
                        .map(|folder| {
                            let path = s(folder, "path");
                            if path.is_empty() {
                                s(folder, "name")
                            } else {
                                path
                            }
                        })
                        .unwrap_or_else(|| "未分类".into())
                };
                if let Some((_, group)) = groups.iter_mut().find(|(name, _)| *name == folder) {
                    group.push(item);
                } else {
                    groups.push((folder, vec![item]));
                }
            }
            let mut first_group = true;
            for (_folder, mut group) in groups {
                group.sort_by_key(|item| {
                    let id = s(item, "id");
                    let folder_id = s(item, "folder_id");
                    !self.folders.iter().any(|folder| {
                        s(folder, "id") == folder_id && s(folder, "main_index_id") == id
                    })
                });
                if first_group {
                    first_group = false;
                } else {
                    ui.add_space(10.0);
                }
                ui.with_layout(
                    egui::Layout::left_to_right(egui::Align::Min).with_main_wrap(true),
                    |ui| {
                        for x in group {
                            let xid = s(&x, "id");
                            let selected = ov.selected.as_deref() == Some(xid.as_str());
                            let scope_color = IndexScope::of(&x).color();
                            let title = s(&x, "name");
                            let cover = self.index_covers.get(&xid).cloned().unwrap_or_default();
                            let resp = self.wall_card(
                                ui,
                                WallCard {
                                    width: card_w,
                                    min_height: card_h,
                                    title: &title,
                                    cover: &cover,
                                    selected,
                                    scope: Some(scope_color),
                                    variant: if !my_name.is_empty()
                                        && s(&x, "maintainer") == my_name
                                    {
                                        CardVariant::Mine
                                    } else {
                                        CardVariant::Normal
                                    },
                                },
                                |app, ui| {
                                    ui.horizontal(|ui| {
                                        if let Some(tag) = app.index_tags.get(&xid) {
                                            chip(
                                                ui,
                                                &format!("V{tag}"),
                                                palette_color(ACCENT_DEEP),
                                                CHIP_BLUE_BG,
                                            );
                                        }
                                        // 主索引 capsule: this index is
                                        // its home folder's designated
                                        // main index (spec §3.10).
                                        let home = s(&x, "folder_id");
                                        let is_main = !home.is_empty()
                                            && app.folders.iter().any(|f| {
                                                s(f, "id") == home && s(f, "main_index_id") == xid
                                            });
                                        if is_main {
                                            chip(
                                                ui,
                                                "主索引",
                                                palette_color(ACCENT_DEEP),
                                                CHIP_BLUE_BG,
                                            );
                                        }
                                        match s(&x, "lifecycle").as_str() {
                                            "history" => chip(
                                                ui,
                                                "历史",
                                                palette_color(TEXT_WEAK),
                                                palette_color(CHIP_GRAY_BG),
                                            ),
                                            "void" => chip(
                                                ui,
                                                "已作废",
                                                palette_color(ROSE),
                                                ROSE_SOFT_BG,
                                            ),
                                            _ => {}
                                        }
                                    });
                                },
                            );
                            let rect = resp.response.rect;
                            let click = ui
                                .interact(
                                    rect,
                                    ui.scope_id().with(("idx_card", &xid)),
                                    egui::Sense::click(),
                                )
                                .on_hover_text(format!(
                                    "{} · {}",
                                    s(&x, "maintainer"),
                                    fmt_time(i(&x, "created_ms"))
                                ));
                            let stroke = if !selected && (click.hovered() || click.has_focus()) {
                                Some(egui::Stroke::new(1.0, palette_color(FIELD_BORDER)))
                            } else {
                                None
                            };
                            if let Some(stroke) = stroke {
                                ui.painter().rect_stroke(
                                    rect,
                                    6.0,
                                    stroke,
                                    egui::StrokeKind::Inside,
                                );
                            }
                            if click.clicked() {
                                ov.selected = Some(xid.clone());
                            }
                            if click.double_clicked() {
                                open = Some(xid);
                            }
                        }
                    },
                );
                ui.add_space(8.0);
            }
        });
        open
    }

    fn index_create(&mut self, ui: &mut egui::Ui, st: &mut CreateState) {
        ui.heading("新建索引 · 选内容");
        ui.horizontal(|ui| {
            ui.label("名称");
            ui.text_edit_singleline(&mut st.new_name);
            ui.label("内容组");
            egui::ComboBox::from_id_salt("new_index_group")
                .selected_text(self.group_name(&st.new_group.clone()))
                .show_ui(ui, |ui| {
                    for g in self.groups.clone() {
                        ui.selectable_value(&mut st.new_group, s(&g, "id"), s(&g, "name"));
                    }
                });
            if ui
                .add_enabled(
                    st.pick.iter().any(|(_, _, on)| *on)
                        && !st.new_name.trim().is_empty(),
                    egui::Button::new("创建索引"),
                )
                .clicked()
            {
                let entries: Vec<Value> = st.pick
                    .iter()
                    .filter(|(_, _, on)| *on)
                    .map(|(u, r, _)| json!({ "unit_id": u, "rendition_id": r, "policy": "follow_latest", "locked_semver": Value::Null, "excerpt_slots": Value::Null }))
                    .collect();
                if entries.is_empty() {
                    self.msg = "至少选一个内容块".into();
                } else {
                    let r = self.call_work(
                        "POST",
                        "/v1/indexes",
                        Some(json!({
                            "name": st.new_name,
                            "group_id": st.new_group,
                            "folder_id": st.folder,
                            "scope": "company",
                            "entries": entries,
                        })),
                    );
                    match r {
                        Ok(v) => {
                            st.open_after = Some(s(&v, "id"));
                            st.closed = true;
                            self.refresh_all();
                        }
                        Err(e) => self.msg = format!("创建失败：{e}"),
                    }
                }
            }
            if ui.button("取消").clicked() {
                st.closed = true;
            }
        });
        ui.separator();
        let mut pick = st.pick.clone();
        egui::ScrollArea::vertical().show(ui, |ui| {
            for item in self.units.clone() {
                let unit = item.get("unit").cloned().unwrap_or(json!({}));
                if let Some(rends) = item.get("renditions").and_then(|r| r.as_array()) {
                    for r in rends {
                        let rend = r.get("rendition").cloned().unwrap_or(json!({}));
                        if let Some(slot) = pick
                            .iter_mut()
                            .find(|(u, rr, _)| u == &s(&unit, "id") && rr == &s(&rend, "id"))
                        {
                            ui.checkbox(
                                &mut slot.2,
                                format!(
                                    "{} · {} · {}",
                                    s(&unit, "name"),
                                    s(&rend, "form"),
                                    s(&rend, "lang")
                                ),
                            );
                        }
                    }
                }
            }
        });
        st.pick = pick;
    }

    fn index_detail(
        &mut self,
        ui: &mut egui::Ui,
        st: &mut IndexDetailState,
        action: &mut Option<IndexDetailAction>,
    ) {
        let detail = st.detail.clone();
        let index = detail.get("index").cloned().unwrap_or(json!({}));
        let branches = detail
            .get("branches")
            .and_then(|b| b.as_array())
            .cloned()
            .unwrap_or_default();
        ui.horizontal(|ui| {
            ui.heading(s(&index, "name"));
            ui.label(
                egui::RichText::new(format!("· {}", s(&index, "maintainer")))
                    .size(11.5)
                    .color(palette_color(TEXT_WEAK)),
            );
        });
        ui.add_space(2.0);
        ui.horizontal_wrapped(|ui| {
            ui.label(
                egui::RichText::new("分支")
                    .size(11.5)
                    .color(palette_color(TEXT_WEAK)),
            );
            let current_branch = if st.branch.is_empty() {
                "main"
            } else {
                &st.branch
            };
            let mut branch = current_branch.to_owned();
            egui::ComboBox::from_id_salt("branch")
                .width(96.0)
                .selected_text(branch.clone())
                .show_ui(ui, |ui| {
                    for b in &branches {
                        if let Some(bs) = b.as_str() {
                            ui.selectable_value(&mut branch, bs.to_string(), bs);
                        }
                    }
                });
            if branch != current_branch {
                *action = Some(IndexDetailAction::ChangeBranch(branch));
                return;
            }
            let version = ui.add(
                egui::TextEdit::singleline(&mut st.tag_name)
                    .hint_text("新版本号")
                    .desired_width(176.0)
                    .margin(egui::vec2(10.0, 5.0)),
            );
            if std::mem::take(&mut st.focus_tag) {
                version.request_focus();
            }
            let publish = ui
                .add_enabled(
                    !st.tag_name.trim().is_empty(),
                    egui::Button::new(
                        egui::RichText::new("发布版本")
                            .size(13.0)
                            .color(egui::Color32::WHITE),
                    )
                    .fill(ACCENT)
                    .corner_radius(6)
                    .min_size(egui::vec2(88.0, 30.0)),
                )
                .clicked();
            let enter = (version.has_focus() || version.lost_focus())
                && ui.input(|input| input.key_pressed(egui::Key::Enter));
            if publish || (enter && !st.tag_name.trim().is_empty()) {
                if !self.need_identity_local("发布版本") {
                    // Anonymous session: the login popup is open;
                    // the typed tag name stays for after the login.
                } else {
                    let id = st.index_id.clone();
                    let r = self.call_work(
                        "POST",
                        &format!("/v1/indexes/{id}/tags"),
                        Some(json!({ "branch": st.branch, "name": st.tag_name.trim() })),
                    );
                    match r {
                        Ok(_) => {
                            self.msg = "已发布".into();
                            *action = Some(IndexDetailAction::Reload);
                        }
                        Err(e) => self.msg = e,
                    }
                }
            }
        });
        ui.add_space(2.0);
        ui.horizontal_wrapped(|ui| {
            ui.checkbox(&mut st.include_notes, "添加备注");
            if text_action(ui, "导出（个人版）", palette_color(TEXT_MAIN), 13.0) {
                *action = Some(IndexDetailAction::Export("personal"));
            }
            if text_action(ui, "导出（公用版）", palette_color(TEXT_MAIN), 13.0) {
                *action = Some(IndexDetailAction::Export("company"));
            }
            ui.add_space(24.0);
            ui.add_space(24.0);
            ui.add(
                egui::TextEdit::singleline(&mut st.new_branch)
                    .hint_text("新分支名")
                    .desired_width(110.0)
                    .margin(egui::vec2(8.0, 2.0)),
            );
            if text_action_enabled(
                ui,
                !st.new_branch.is_empty(),
                "拉分支",
                palette_color(TEXT_MAIN),
                13.0,
            ) {
                let id = st.index_id.clone();
                let r = self.call_work(
                    "POST",
                    &format!("/v1/indexes/{id}/branches"),
                    Some(json!({ "name": st.new_branch, "from": st.branch })),
                );
                match r {
                    Ok(_) => {
                        self.msg = "已拉分支".into();
                        *action = Some(IndexDetailAction::Reload);
                    }
                    Err(e) => self.msg = e,
                }
            }
        });
        ui.add_space(8.0);
        self.index_detail_tabs(ui, st);
        ui.add_space(10.0);
        if st.view != IndexDetailView::Wall {
            self.index_history_view(ui, st, action);
            return;
        }
        // entry tile wall: horizontal cards, one per row
        let entries: Vec<Value> = detail
            .get("resolved_entries")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|row| row.get("entry").cloned())
            .collect();
        // An armed remove confirmation whose entry is gone
        // (list shrank under it) disarms instead of pointing
        // at whatever entry now sits at that position.
        if let Some(p) = st.confirm_remove_entry {
            if p >= entries.len() {
                st.confirm_remove_entry = None;
            }
        }
        let resolved = detail
            .get("resolved_entries")
            .and_then(|r| r.as_array())
            .cloned()
            .unwrap_or_default();
        egui::ScrollArea::vertical().id_salt(&st.index_id).show(ui, |ui| {
            let cover_width = if ui.available_width() >= 900.0 { 340.0 } else { 300.0 };
            let card_content_width = (ui.available_width() - 25.0).max(0.0);
            let thumb_gap = ui.spacing().item_spacing.x;
            let small_width = (cover_width - 2.0 * thumb_gap) / 3.0;
            for (pos, re) in resolved.iter().enumerate() {
                let entry = re.get("entry").cloned().unwrap_or(json!({}));
                let ver = re.get("resolved").cloned().unwrap_or(Value::Null);
                let locked = entry
                    .get("policy")
                    .map(|p| p.get("locked").is_some())
                    .unwrap_or(false);
                if pos > 0 {
                    ui.vertical_centered(|ui| {
                        if text_action(ui, "＋ 插入条目", palette_color(TEXT_WEAK), 13.0) {
                            *action = Some(IndexDetailAction::Insert(pos));
                        }
                    });
                }
                card_frame_square()
                    .inner_margin(egui::Margin {
                        left: 10,
                        right: 13,
                        top: 9,
                        bottom: 9,
                    })
                    .show(ui, |ui| {
                    ui.set_min_width(card_content_width);
                    ui.horizontal(|ui| {
                        ui.vertical(|ui| {
                            ui.set_width(cover_width);
                            ui.label(
                                egui::RichText::new(format!("#{}", pos + 1))
                                    .size(11.5)
                                    .color(palette_color(TEXT_WEAK)),
                            );
                            if ver.is_object() {
                                let vid = s(&ver, "id");
                                let total = i(&ver, "page_count");
                                self.thumb_ui(ui, &vid, 0, cover_width);
                                if total > 1 {
                                    ui.horizontal_wrapped(|ui| {
                                        for page in 1..total.min(7) as usize {
                                            self.thumb_ui(ui, &vid, page, small_width);
                                        }
                                    });
                                }
                            } else {
                                ui.label("无版本");
                            }
                        });
                        ui.add_space(6.0);
                        ui.vertical(|ui| {
                            ui.horizontal_wrapped(|ui| {
                                ui.label(
                                    egui::RichText::new(re.get("unit_name").and_then(|x| x.as_str()).unwrap_or(""))
                                        .strong()
                                        .size(13.5)
                                        .color(palette_color(TEXT_MAIN)),
                                );
                                ui.label(
                                    egui::RichText::new(format!(
                                        "{} · {} · {}",
                                        re.get("form").and_then(|x| x.as_str()).unwrap_or(""),
                                        re.get("lang").and_then(|x| x.as_str()).unwrap_or(""),
                                        if ver.is_object() {
                                            format!("v{}", s(&ver, "semver"))
                                        } else {
                                            "无版本".into()
                                        }
                                    ))
                                    .size(11.5)
                                    .color(palette_color(TEXT_WEAK)),
                                );
                            });
                            ui.horizontal_wrapped(|ui| {
                            if ui
                                .button(
                                    egui::RichText::new("×")
                                        .color(palette_color(ROSE))
                                        .size(17.0)
                                        .strong(),
                                )
                                .on_hover_text("从这个索引移除这个条目（内容块保留）")
                                .clicked()
                            {
                                // Prototype .del-pop (drill
                                // re-audit N2): × only arms the
                                // inline confirmation rendered
                                // below the action row; the
                                // removal commits from its
                                // 确认删除 button.
                                st.confirm_remove_entry = Some(pos);
                            }
                            if ui
                                .add_enabled(
                                    pos + 1 < entries.len(),
                                    egui::Button::new("↓"),
                                )
                                .clicked()
                            {
                                let mut es = entries.clone();
                                es.swap(pos, pos + 1);
                                *action = Some(IndexDetailAction::Commit(es, "调整顺序"));
                                return;
                            }
                            if ui
                                .add_enabled(pos > 0, egui::Button::new("↑"))
                                .clicked()
                            {
                                let mut es = entries.clone();
                                es.swap(pos, pos - 1);
                                *action = Some(IndexDetailAction::Commit(es, "调整顺序"));
                                return;
                            }
                            if ui.button("选页").clicked() {
                                // page picking happens on the
                                // content page (pick mode), not
                                // inline in the card (James)
                                let total = i(&ver, "page_count") as usize;
                                let mut sel = vec![true; total];
                                if let Some(slots) = entry
                                    .get("excerpt_slots")
                                    .and_then(|x| x.as_array())
                                {
                                    sel = vec![false; total];
                                    for pos in excerpt_positions_json(&ver, slots) {
                                        sel[pos] = true;
                                    }
                                }
                                *action = Some(IndexDetailAction::PickPages(pos, re.clone(), sel));
                            }
                            if locked {
                                if ui
                                    .button(
                                        egui::RichText::new("解除锁定")
                                            .color(palette_color(CHIP_GREEN_FG)),
                                    )
                                    .clicked()
                                {
                                    let mut es = entries.clone();
                                    es[pos]["policy"] = json!("follow_latest");
                                    es[pos]["locked_semver"] = Value::Null;
                                    *action = Some(IndexDetailAction::Commit(es, "解除锁定"));
                                    return;
                                }
                                chip(ui, "版本锁定", palette_color(CHIP_ORANGE_FG), CHIP_ORANGE_BG);
                            } else {
                                if ui
                                    .add_enabled(
                                        ver.is_object(),
                                        egui::Button::new("锁定当前版本"),
                                    )
                                    .clicked()
                                {
                                    if ver.is_object() {
                                        let mut es = entries.clone();
                                        es[pos]["policy"] = json!({ "locked": { "version_id": s(&ver, "id") } });
                                        es[pos]["locked_semver"] =
                                            json!(s(&ver, "semver"));
                                        *action = Some(IndexDetailAction::Commit(es, "锁定版本"));
                                        return;
                                    }
                                }
                                chip(ui, "跟随最新", palette_color(CHIP_GREEN_FG), CHIP_GREEN_BG);
                            }
                            });
                            // Inline remove confirmation
                            // (prototype .del-pop inrow, drill
                            // re-audit N2): rendered in the card
                            // directly under the action row while
                            // armed; copy follows the prototype
                            // verbatim. 取消 just disarms; only
                            // 确认删除 commits the removal (the
                            // commit itself is the same one the
                            // × used to fire directly).
                            if st.confirm_remove_entry == Some(pos) {
                                ui.add_space(4.0);
                                let entry_name = re
                                    .get("unit_name")
                                    .and_then(|x| x.as_str())
                                    .unwrap_or("");
                                ui.label(
                                    egui::RichText::new(format!(
                                        "删除条目「{entry_name}」？"
                                    ))
                                    .strong()
                                    .size(12.5),
                                );
                                ui.label(
                                    egui::RichText::new(
                                        "只从这个索引里移除这一个条目，内容库里的内容块不受影响；删除会生成一条提交，可在下方提交图 / 历史版本里找回。",
                                    )
                                    .size(12.0)
                                    .color(palette_color(TEXT_WEAK)),
                                );
                                ui.horizontal(|ui| {
                                    if btn_weak(ui, "取消").clicked() {
                                        st.confirm_remove_entry = None;
                                    }
                                    if btn_danger(ui, "确认删除").clicked() {
                                        let mut es = entries.clone();
                                        es.remove(pos);
                                        *action = Some(IndexDetailAction::Commit(
                                            es,
                                            "删除条目",
                                        ));
                                        st.confirm_remove_entry = None;
                                        return;
                                    }
                                });
                            }
                            ui.horizontal(|ui| {
                                if let Some(slots) =
                                    entry.get("excerpt_slots").and_then(|x| x.as_array())
                                {
                                    chip(
                                        ui,
                                        &format!("摘录 {} 页", slots.len()),
                                        palette_color(ACCENT_DEEP),
                                        CHIP_BLUE_BG,
                                    );
                                }
                                if re.get("substituted").and_then(|x| x.as_bool()) == Some(true) {
                                    chip(ui, "原版已删 · 已替代", palette_color(CHIP_ORANGE_FG), CHIP_ORANGE_BG);
                                }
                            });
                            // quick-switch strip: history versions, newest
                            // first; last item opens the full-history popup
                            if let Some(versions) = st.unit_versions
                                .get(&s(&entry, "rendition_id"))
                                .cloned()
                            {
                                if versions.len() > 1 {
                                    let cur_id = if ver.is_object() {
                                        s(&ver, "id")
                                    } else {
                                        String::new()
                                    };
                                    let latest_id = s(&versions[0], "id");
                                    ui.horizontal_wrapped(|ui| {
                                        for v in versions.iter().take(7) {
                                            let vid = s(v, "id");
                                            let is_cur = vid == cur_id;
                                            ui.vertical(|ui| {
                                                if self.thumb_button(ui, &vid, 0, 54.0, is_cur) {
                                                    let is_latest = vid == latest_id;
                                                    *action = Some(IndexDetailAction::SwitchVersion(
                                                        pos, vid, s(v, "semver"), is_latest,
                                                    ));
                                                }
                                                ui.label(
                                                    egui::RichText::new(format!(
                                                        "v{}",
                                                        s(v, "semver")
                                                    ))
                                                    .size(10.5)
                                                    .color(palette_color(if is_cur {
                                                        ACCENT_DEEP
                                                    } else {
                                                        TEXT_WEAK
                                                    })),
                                                );
                                            });
                                        }
                                        if ui.button("更多").clicked() {
                                            let cur_index_id = match self.index.stack.top() {
                                                IndexFrame::Detail(d) => d.index_id.clone(),
                                                _ => String::new(),
                                            };
                                            self.overlays.push(Overlay::AllVersions {
                                                entry_pos: pos,
                                                index_id: cur_index_id,
                                            });
                                        }
                                    });
                                }
                            }
                        });
                    });
                });
                ui.add_space(8.0);
            }
            let index_id = s(&index, "id");
            if !index_id.is_empty() {
                self.attachments_ui(ui, "index", &index_id);
            }
        });
    }

    fn index_merge_ui(
        &mut self,
        ui: &mut egui::Ui,
        st: &mut IndexDetailState,
        action: &mut Option<IndexDetailAction>,
    ) {
        let resolved = st.detail["resolved_entries"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        ui.add_space(6.0);
        ui.label(egui::RichText::new("分支合并").strong().size(16.0));
        ui.horizontal(|ui| {
            ui.label("来源分支");
            let source = ui.text_edit_singleline(&mut st.merge_from);
            if std::mem::take(&mut st.focus_merge) {
                source.scroll_to_me(Some(egui::Align::Center));
                source.request_focus();
            }
            if ui
                .add_enabled(!st.merge_from.is_empty(), egui::Button::new("预览合并"))
                .clicked()
            {
                let id = st.index_id.clone();
                let into = st.branch.clone();
                let from = st.merge_from.clone();
                match self.call_work(
                    "POST",
                    &format!("/v1/indexes/{id}/merge/preview"),
                    Some(json!({ "from": from, "into": into })),
                ) {
                    Ok(v) => {
                        st.merge_preview = Some(v);
                        st.merge_resolutions.clear();
                    }
                    Err(e) => self.msg = format!("合并预览失败：{e}"),
                }
            }
        });
        if let Some(preview) = st.merge_preview.clone() {
            let conflicts = preview
                .get("conflicts")
                .and_then(|c| c.as_array())
                .cloned()
                .unwrap_or_default();
            let mut names: std::collections::HashMap<String, String> =
                std::collections::HashMap::new();
            for re in &resolved {
                if let Some(en) = re.get("entry") {
                    names.insert(
                        s(en, "unit_id"),
                        re.get("unit_name")
                            .and_then(|x| x.as_str())
                            .unwrap_or("")
                            .to_string(),
                    );
                }
            }
            if conflicts.is_empty() {
                ui.label(
                    egui::RichText::new("无冲突，可自动合并").color(palette_color(CHIP_GREEN_FG)),
                );
            } else {
                ui.label(format!("{} 个条目冲突，逐条裁决：", conflicts.len()));
                for c in &conflicts {
                    let key = s(c, "key");
                    let uname = names
                        .get(&s(c, "unit_id"))
                        .cloned()
                        .unwrap_or_else(|| "条目".to_string());
                    let ours = c.get("ours").cloned().unwrap_or(Value::Null);
                    let theirs = c.get("theirs").cloned().unwrap_or(Value::Null);
                    ui.horizontal(|ui| {
                        ui.label(format!(
                            "{}：本分支「{}」 / 对方「{}」",
                            uname,
                            entry_label(&ours),
                            entry_label(&theirs)
                        ));
                        let cur = st.merge_resolutions.get(&key).cloned();
                        if ui
                            .selectable_label(cur.as_deref() == Some("ours"), "用本分支")
                            .clicked()
                        {
                            st.merge_resolutions.insert(key.clone(), "ours".to_string());
                        }
                        if ui
                            .selectable_label(cur.as_deref() == Some("theirs"), "用对方")
                            .clicked()
                        {
                            st.merge_resolutions
                                .insert(key.clone(), "theirs".to_string());
                        }
                    });
                }
            }
            let unresolved = conflicts
                .iter()
                .filter(|c| !st.merge_resolutions.contains_key(&s(c, "key")))
                .count();
            ui.horizontal(|ui| {
                if ui
                    .add_enabled(unresolved == 0, egui::Button::new("执行合并"))
                    .clicked()
                {
                    let id = st.index_id.clone();
                    let resolutions: serde_json::Map<String, Value> = st
                        .merge_resolutions
                        .iter()
                        .map(|(k, v)| (k.clone(), json!(v)))
                        .collect();
                    let r = self.call_work(
                        "POST",
                        &format!("/v1/indexes/{id}/merge"),
                        Some(json!({
                            "from": st.merge_from,
                            "into": st.branch,
                            "resolutions": resolutions,
                        })),
                    );
                    match r {
                        Ok(_) => {
                            self.msg = "合并完成".into();
                            st.merge_preview = None;
                            *action = Some(IndexDetailAction::Reload);
                        }
                        Err(e) => self.msg = format!("合并失败：{e}"),
                    }
                }
                if unresolved > 0 {
                    ui.label(format!("还剩 {unresolved} 条未裁决"));
                }
            });
        }
    }

    /// 设置 → 服务器：saved connections, add-by-login, LAN discovery.
    fn settings_servers(&mut self, ui: &mut egui::Ui) {
        ui.label(
            egui::RichText::new("已保存的服务器")
                .strong()
                .size(16.0)
                .color(palette_color(TEXT_MAIN)),
        );
        ui.add_space(6.0);
        if self.conns.is_empty() {
            ui.label(
                egui::RichText::new("暂无保存的服务器。在下方手动添加，或搜索局域网服务器。")
                    .size(11.5)
                    .color(palette_color(TEXT_WEAK)),
            );
        }
        let mut action: Option<(usize, &'static str)> = None;
        for i in 0..self.conns.len() {
            let c = self.conns[i].clone();
            let is_active = self.active == Some(i);
            egui::Frame::NONE
                .fill(palette_color(CARD_BG))
                .stroke(egui::Stroke::new(1.0, palette_color(CARD_BORDER)))
                .corner_radius(egui::CornerRadius::same(12))
                .inner_margin(egui::Margin::same(12))
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.label(
                            egui::RichText::new(&c.name)
                                .strong()
                                .size(13.5)
                                .color(palette_color(TEXT_MAIN)),
                        );
                        if is_active {
                            ui.label(
                                egui::RichText::new("当前")
                                    .size(11.0)
                                    .color(palette_color(ACCENT_DEEP)),
                            );
                        }
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui.button("删除").clicked() {
                                action = Some((i, "delete"));
                            }
                            if c.token.is_some() && ui.button("断开").clicked() {
                                action = Some((i, "disconnect"));
                            }
                            // 关闭服务器 (user ruling 2026-11-25):
                            // shown on every card and never greyed
                            // for permission — the click prompts
                            // when the identity cannot close it
                            // (button-state standard).
                            if ui.button("关闭服务器").clicked() {
                                action = Some((i, "shutdown"));
                            }
                            if !is_active && ui.button("设为当前").clicked() {
                                action = Some((i, "activate"));
                            }
                        });
                    });
                    ui.add_space(4.0);
                    let (status, color) = match c.status {
                        ConnStatus::Connected => ("已连接", palette_color(CHIP_GREEN_FG)),
                        ConnStatus::Failed => ("连接失败", palette_color(ROSE)),
                        ConnStatus::LoggedOut => ("未登录", palette_color(TEXT_WEAK)),
                        ConnStatus::Anonymous => {
                            ("匿名连接", egui::Color32::from_rgb(0x2F, 0x6B, 0xF5))
                        }
                    };
                    ui.horizontal(|ui| {
                        ui.label(
                            egui::RichText::new(if c.anon {
                                format!("{} · 匿名", c.url)
                            } else {
                                format!("{} · {}", c.url, c.username)
                            })
                            .size(11.5)
                            .color(palette_color(TEXT_WEAK)),
                        );
                        ui.label(egui::RichText::new(status).size(11.5).color(color));
                    });
                });
            ui.add_space(8.0);
        }
        if let Some((i, op)) = action {
            match op {
                "activate" => self.activate(i),
                "disconnect" => self.disconnect_conn(i),
                "shutdown" => self.ask_shutdown_server(i),
                "delete" => {
                    if i < self.conns.len() {
                        let was_active = self.active == Some(i);
                        self.conns.remove(i);
                        if was_active {
                            // The active identity's server is gone:
                            // reset the account context (the old code
                            // kept the deleted server's user object
                            // and every org surface on screen), then
                            // re-enter through the first still-connected
                            // saved server, if any — activate()
                            // revalidates it and loads that account.
                            self.active = None;
                            self.user = None;
                            self.token = None;
                            self.clear_account_state();
                            if let Some(j) = (0..self.conns.len())
                                .find(|&j| self.conns[j].status == ConnStatus::Connected)
                            {
                                self.activate(j);
                            }
                        } else if let Some(a) = self.active {
                            if a > i {
                                self.active = Some(a - 1);
                            }
                        }
                        self.persist_conns();
                        self.msg = "已删除该服务器".into();
                    }
                }
                _ => {}
            }
        }
        ui.add_space(10.0);
        ui.separator();
        ui.add_space(10.0);
        ui.label(
            egui::RichText::new("添加服务器")
                .strong()
                .size(16.0)
                .color(palette_color(TEXT_MAIN)),
        );
        ui.add_space(6.0);
        let mut fetch_name = false;
        egui::Grid::new("add_server_grid")
            .num_columns(2)
            .spacing([10.0, 10.0])
            .show(ui, |ui| {
                ui.label("服务器地址");
                ui.add_sized(
                    [380.0, 30.0],
                    egui::TextEdit::singleline(&mut self.add_url)
                        .hint_text("http://192.168.1.10:8730"),
                );
                ui.end_row();
                ui.label("服务器名称");
                ui.horizontal(|ui| {
                    ui.set_width(380.0);
                    ui.add_sized(
                        [282.0, 30.0],
                        egui::TextEdit::singleline(&mut self.add_name)
                            .hint_text("自定义名称，或从服务器获取"),
                    );
                    fetch_name = ui
                        .add_enabled(
                            !self.add_url.trim().is_empty(),
                            egui::Button::new("获取名称")
                                .corner_radius(6)
                                .min_size(egui::vec2(90.0, 30.0)),
                        )
                        .clicked();
                });
                ui.end_row();
                ui.label("账号");
                ui.add_sized(
                    [380.0, 30.0],
                    egui::TextEdit::singleline(&mut self.add_user),
                );
                ui.end_row();
                ui.label("密码");
                ui.add_sized(
                    [380.0, 30.0],
                    egui::TextEdit::singleline(&mut self.add_pass).password(true),
                );
                ui.end_row();
            });
        if fetch_name {
            self.fetch_add_server_name();
        }
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            if ui
                .add_enabled(
                    !self.add_url.trim().is_empty(),
                    egui::Button::new(
                        egui::RichText::new("登录并添加").color(egui::Color32::WHITE),
                    )
                    .fill(ACCENT)
                    .corner_radius(6)
                    .min_size(egui::vec2(120.0, 30.0)),
                )
                .clicked()
            {
                self.add_server_login();
            }
            if ui
                .add_enabled(
                    !self.add_url.trim().is_empty(),
                    egui::Button::new(egui::RichText::new("匿名添加"))
                        .corner_radius(6)
                        .min_size(egui::vec2(120.0, 30.0)),
                )
                .clicked()
            {
                self.add_server_anon();
            }
        });
        ui.add_space(14.0);
        ui.separator();
        ui.add_space(10.0);
        ui.horizontal(|ui| {
            ui.label(
                egui::RichText::new("局域网发现")
                    .strong()
                    .size(16.0)
                    .color(palette_color(TEXT_MAIN)),
            );
            if self.disc_scanning {
                ui.label(
                    egui::RichText::new("搜索中…")
                        .size(11.5)
                        .color(palette_color(TEXT_WEAK)),
                );
            } else if ui.button("搜索局域网服务器").clicked() {
                self.start_discovery();
            }
        });
        ui.add_space(6.0);
        if self.discovered.is_empty() {
            ui.label(
                egui::RichText::new("点「搜索局域网服务器」自动发现同一局域网内的启幕服务器。")
                    .size(11.5)
                    .color(palette_color(TEXT_WEAK)),
            );
        }
        let mut use_server: Option<(String, String)> = None;
        for d in self.discovered.clone() {
            let name = s(&d, "name");
            let url = s(&d, "url");
            let ver = s(&d, "version");
            egui::Frame::NONE
                .fill(palette_color(CARD_BG))
                .stroke(egui::Stroke::new(1.0, palette_color(CARD_BORDER)))
                .corner_radius(egui::CornerRadius::same(12))
                .inner_margin(egui::Margin::same(12))
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.label(
                            egui::RichText::new(if name.is_empty() {
                                "启幕服务器"
                            } else {
                                &name
                            })
                            .size(13.0)
                            .color(palette_color(TEXT_MAIN)),
                        );
                        ui.label(
                            egui::RichText::new(format!("{url} · v{ver}"))
                                .size(11.5)
                                .color(palette_color(TEXT_WEAK)),
                        );
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui.button("添加并登录").clicked() {
                                use_server = Some((url.clone(), name.clone()));
                            }
                        });
                    });
                });
            ui.add_space(8.0);
        }
        if let Some((url, name)) = use_server {
            self.add_url = url;
            self.add_name = if name.is_empty() {
                "启幕服务器".to_string()
            } else {
                name
            };
            self.msg =
                "服务器名称和地址已填入上方表单：输入账号密码点「登录并添加」，或点「匿名添加」浏览公开内容"
                    .into();
        }
        ui.add_space(14.0);
        ui.label(
            egui::RichText::new("软件更新（局域网镜像）")
                .strong()
                .size(16.0)
                .color(palette_color(TEXT_MAIN)),
        );
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            ui.label(
                egui::RichText::new(format!("当前版本 v{}", env!("CARGO_PKG_VERSION")))
                    .size(11.5)
                    .color(palette_color(TEXT_WEAK)),
            );
            if ui.button("检查更新").clicked() {
                self.check_lan_update(true);
            }
        });
        if let Some(note) = &self.upd_note {
            ui.add_space(4.0);
            ui.label(
                egui::RichText::new(note.clone())
                    .size(11.5)
                    .color(palette_color(ACCENT_DEEP)),
            );
        }
    }

    /// 缓存路径 (local setting; the admin cache policy lives elsewhere
    /// and is not touched by this).
    fn cache_path_ui(&mut self, ui: &mut egui::Ui) {
        ui.label(
            egui::RichText::new("缓存路径（本机设置）")
                .strong()
                .size(16.0)
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
                [460.0, 30.0],
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
        // 数据目录 (library-discovery line, 2026-11-25; P1
        // rework): where the local library lives, per the
        // bootstrap config. Saving a new directory only registers
        // the move; the relocation runs at the next launch, before
        // the library opens, with count verification — stated in
        // the copy below.
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
                [460.0, 30.0],
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
        // 旧库处置: the post-relocation source disposition policy,
        // stored in the bootstrap config next to `data_dir`.
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

    /// Settings 「保存并搬迁」/「恢复默认目录」 for the data
    /// directory (audit P1 rework): this only REGISTERS the move —
    /// the target is validated by read-only probe and the intent
    /// is stored in the bootstrap config; the relocation itself
    /// runs at the next launch, inside library resolution, before
    /// the store opens. Copying the *running* library (the old
    /// behaviour) forked the session: the copy was taken from a
    /// live database while this session kept writing to the old
    /// directory, so everything done after the "move" vanished
    /// from the user's view at the next launch. Deferred to
    /// startup, the source is static, the counts check arbitrates
    /// a complete copy, and this session is untouched.
    fn apply_data_dir_change(&mut self, target: std::path::PathBuf) {
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

    /// Settings nav frame body: the section list, unchanged.
    /// Settings content for the section selected in the sidebar.
    fn st_main_body(&mut self, ui: &mut egui::Ui) {
        // Board layout: left section nav, one section at a time on the
        // right; sections a role may not see are not rendered at all.
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
                        "服务器" => {
                            self.settings_servers(ui);
                        }
                        "离线缓存" => {
                            self.cache_path_ui(ui);
                        }
                        "演示偏好" => {
                            // 放映打开方式 (ruling 2026-11-25): the
                            // radio row follows the settings page's
                            // existing 旧库处置 radio pattern; the
                            // choice persists to client-settings.json
                            // like the checkbox below it.
                            ui.label(
                                egui::RichText::new("放映打开方式")
                                    .strong()
                                    .size(14.0)
                                    .color(palette_color(TEXT_MAIN)),
                            );
                            ui.add_space(4.0);
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
                                        save_client_settings(
                                            json!({ "present_engine": val.key() }),
                                        );
                                        self.msg = format!("放映打开方式已设为「{label}」");
                                    }
                                }
                            });
                            ui.add_space(4.0);
                            ui.label(
                                egui::RichText::new(
                                    "播放、普通视图、演示者视图打开稿件时用哪种方式：「内置放映」在启幕内放映（默认）；「DeckCraft」用 DeckCraft 桌面应用打开；「PowerPoint」把稿件写到缓存目录并用 PowerPoint 打开，播放与演示者视图会自动开始放映。所选应用不可用时会明确提示，不会自动改用其他方式。",
                                )
                                .size(11.5)
                                .color(palette_color(TEXT_WEAK)),
                            );
                            ui.add_space(12.0);
                            if ui
                                .checkbox(&mut self.dc_auto_quit, tr("放映结束自动关闭 DeckCraft"))
                                .changed()
                            {
                                save_client_settings(
                                    json!({ "deckcraft_auto_quit": self.dc_auto_quit }),
                                );
                            }
                        }
                        "我的提交" => {
                            self.my_submissions_body(ui);
                        }
                        "审批队列" => {
                            self.review_queue_body(ui);
                        }
                        "个人资料" => {
                            // Board: label column + 380×34 fields on a 54
                            // pitch, blue pill save under the field column,
                            // personal-binding preview card on the right.
                            let mut save_clicked = false;
                            ui.horizontal_top(|ui| {
                                egui::Grid::new("profile_grid")
                                    .num_columns(2)
                                    .spacing([16.0, 20.0])
                                    .show(ui, |ui| {
                                        for (key, label) in [
                                            ("name", "姓名"),
                                            ("title", "职务"),
                                            ("english_name", "英文名"),
                                            ("phone", "手机号"),
                                            ("email", "邮箱"),
                                            ("wechat", "微信二维码"),
                                        ] {
                                            ui.label(
                                                egui::RichText::new(label)
                                                    .size(13.0)
                                                    .color(palette_color(TEXT_WEAK)),
                                            );
                                            let v = self
                                                .settings
                                                .profile
                                                .entry(key.to_string())
                                                .or_default();
                                            ui.add_sized(
                                                [380.0, 34.0],
                                                egui::TextEdit::singleline(v)
                                                    .margin(egui::vec2(10.0, 8.0)),
                                            );
                                            ui.end_row();
                                        }
                                        ui.label("");
                                        if ui
                                            .add(
                                                egui::Button::new(
                                                    egui::RichText::new("保存资料")
                                                        .size(13.0)
                                                        .color(egui::Color32::WHITE),
                                                )
                                                .fill(ACCENT)
                                                .corner_radius(6)
                                                .min_size(egui::vec2(120.0, 38.0)),
                                            )
                                            .clicked()
                                        {
                                            save_clicked = true;
                                        }
                                        ui.end_row();
                                    });
                                ui.add_space(64.0);
                                ui.vertical(|ui| {
                                    ui.label(
                                        egui::RichText::new("个人绑定预览").strong().size(16.0),
                                    );
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
                                        .corner_radius(egui::CornerRadius::same(8))
                                        .inner_margin(egui::Margin::same(14))
                                        .show(ui, |ui| {
                                            ui.set_min_size(egui::vec2(312.0, 163.0));
                                            let (bar, _) = ui.allocate_exact_size(
                                                egui::vec2(150.0, 8.0),
                                                egui::Sense::hover(),
                                            );
                                            ui.painter().rect_filled(
                                                bar,
                                                0.0,
                                                egui::Color32::from_rgb(0x1F, 0x38, 0x64),
                                            );
                                            ui.add_space(10.0);
                                            ui.label(
                                                egui::RichText::new(format!(
                                                    "汇报人：{} · {}",
                                                    pname,
                                                    get("title")
                                                ))
                                                .size(11.0)
                                                .color(egui::Color32::from_rgb(0x33, 0x33, 0x33)),
                                            );
                                            ui.add_space(4.0);
                                            ui.label(
                                                egui::RichText::new(format!(
                                                    "{} · {}",
                                                    get("phone"),
                                                    get("email")
                                                ))
                                                .size(11.0)
                                                .color(egui::Color32::from_rgb(0x66, 0x66, 0x66)),
                                            );
                                            ui.add_space(12.0);
                                            let (img, _) = ui.allocate_exact_size(
                                                egui::vec2(90.0, 60.0),
                                                egui::Sense::hover(),
                                            );
                                            ui.painter().rect_filled(
                                                img,
                                                0.0,
                                                egui::Color32::from_rgb(0xB8, 0xC4, 0xD4),
                                            );
                                        });
                                });
                            });
                            if save_clicked {
                                let fields: Value = json!(self.settings.profile);
                                match self.call(
                                    "PUT",
                                    "/v1/me/profile",
                                    Some(json!({ "fields": fields })),
                                ) {
                                    Ok(_) => self.msg = "已保存".into(),
                                    Err(e) => self.msg = e,
                                }
                            }
                        }
                        "组织管理" => {
                            let depts = self
                                .org
                                .get("departments")
                                .and_then(|d| d.as_array())
                                .cloned()
                                .unwrap_or_default();
                            for d in &depts {
                                ui.label(format!("· {}", s(d, "name")));
                            }
                            ui.horizontal(|ui| {
                                ui.label("新部门");
                                ui.add_sized(
                                    [240.0, 26.0],
                                    egui::TextEdit::singleline(&mut self.settings.dept_name),
                                );
                                if ui
                                    .add_enabled(
                                        !self.settings.dept_name.is_empty(),
                                        egui::Button::new("添加"),
                                    )
                                    .clicked()
                                    && self.require_org_online("添加部门")
                                {
                                    match self.call(
                                        "POST",
                                        "/v1/org/departments",
                                        Some(json!({ "name": self.settings.dept_name })),
                                    ) {
                                        Ok(_) => {
                                            self.settings.dept_name.clear();
                                            self.refresh_all();
                                        }
                                        Err(e) => self.msg = e,
                                    }
                                }
                            });
                            ui.add_space(10.0);
                            let users = self
                                .org
                                .get("users")
                                .and_then(|d| d.as_array())
                                .cloned()
                                .unwrap_or_default();
                            for u in &users {
                                ui.label(format!(
                                    "{} · {} · {}",
                                    s(u, "username"),
                                    s(u, "display_name"),
                                    s(u, "role")
                                ));
                            }
                            ui.horizontal(|ui| {
                                ui.label("新账号");
                                ui.add_sized(
                                    [150.0, 26.0],
                                    egui::TextEdit::singleline(&mut self.settings.user_name),
                                );
                                ui.label("姓名");
                                ui.add_sized(
                                    [150.0, 26.0],
                                    egui::TextEdit::singleline(&mut self.settings.user_display),
                                );
                                ui.label("密码");
                                ui.add_sized(
                                    [150.0, 26.0],
                                    egui::TextEdit::singleline(&mut self.settings.user_password)
                                        .password(true),
                                );
                                if ui
                                    .add_enabled(
                                        !self.settings.user_name.trim().is_empty()
                                            && !self.settings.user_display.trim().is_empty()
                                            && !self.settings.user_password.is_empty(),
                                        egui::Button::new("创建账号"),
                                    )
                                    .clicked()
                                    && self.require_org_online("创建账号")
                                {
                                    match self.call(
                                        "POST",
                                        "/v1/org/users",
                                        Some(json!({
                                            "username": self.settings.user_name,
                                            "display_name": self.settings.user_display,
                                            "password": self.settings.user_password,
                                            "role": "member",
                                        })),
                                    ) {
                                        Ok(_) => {
                                            self.settings.user_name.clear();
                                            self.refresh_all();
                                        }
                                        Err(e) => self.msg = e,
                                    }
                                }
                            });
                        }
                        "内容组与授权" => {
                            for g in self.groups.clone() {
                                ui.label(format!("· {}", s(&g, "name")));
                            }
                            ui.horizontal(|ui| {
                                ui.label("新内容组");
                                ui.add_sized(
                                    [240.0, 26.0],
                                    egui::TextEdit::singleline(&mut self.settings.group_name),
                                );
                                if ui
                                    .add_enabled(
                                        !self.settings.group_name.is_empty(),
                                        egui::Button::new("添加"),
                                    )
                                    .clicked()
                                    && self.require_org_online("添加内容组")
                                {
                                    match self.call(
                                        "POST",
                                        "/v1/groups",
                                        Some(json!({ "name": self.settings.group_name })),
                                    ) {
                                        Ok(_) => {
                                            self.settings.group_name.clear();
                                            self.refresh_all();
                                        }
                                        Err(e) => self.msg = e,
                                    }
                                }
                            });
                            ui.add_space(10.0);
                            if ui.button("载入授权列表").clicked() {
                                match self.call("GET", "/v1/grants", None) {
                                    Ok(v) => {
                                        self.grants = v
                                            .get("grants")
                                            .and_then(|g| g.as_array())
                                            .cloned()
                                            .unwrap_or_default()
                                    }
                                    Err(e) => self.msg = e,
                                }
                            }
                            for g in self.grants.clone() {
                                ui.label(format!(
                                    "{} → {}:{} {}",
                                    self.group_name(&s(&g, "group_id")),
                                    s(&g, "subject_type"),
                                    s(&g, "subject_id"),
                                    s(&g, "capability")
                                ));
                            }
                            ui.horizontal(|ui| {
                                egui::ComboBox::from_id_salt("grant_group")
                                    .selected_text(
                                        self.group_name(&self.settings.grant_group.clone()),
                                    )
                                    .show_ui(ui, |ui| {
                                        for g in self.groups.clone() {
                                            ui.selectable_value(
                                                &mut self.settings.grant_group,
                                                s(&g, "id"),
                                                s(&g, "name"),
                                            );
                                        }
                                    });
                                egui::ComboBox::from_id_salt("grant_st")
                                    .selected_text(self.settings.grant_subject_type.clone())
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
                                ui.text_edit_singleline(&mut self.settings.grant_subject_id);
                                egui::ComboBox::from_id_salt("grant_cap")
                                    .selected_text(self.settings.grant_capability.clone())
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
                                if ui
                                    .add_enabled(
                                        !self.settings.grant_subject_id.trim().is_empty()
                                            && !self.settings.grant_group.is_empty(),
                                        egui::Button::new("添加授权"),
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
                                        Ok(_) => self.msg = "已添加授权".into(),
                                        Err(e) => self.msg = e,
                                    }
                                }
                            });
                        }
                        "公司公共资料" => {
                            egui::Grid::new("company_grid")
                                .num_columns(2)
                                .spacing([14.0, 10.0])
                                .show(ui, |ui| {
                                    for (key, label) in [
                                        ("phone", "公司电话"),
                                        ("website", "官网"),
                                        ("address", "地址"),
                                        ("gongzhonghao", "公众号"),
                                    ] {
                                        ui.label(label);
                                        let v = self
                                            .settings
                                            .company
                                            .entry(key.to_string())
                                            .or_default();
                                        ui.add_sized([300.0, 26.0], egui::TextEdit::singleline(v));
                                        ui.end_row();
                                    }
                                });
                            ui.add_space(12.0);
                            if ui
                                .add(
                                    egui::Button::new(
                                        egui::RichText::new("保存公司资料")
                                            .color(egui::Color32::WHITE),
                                    )
                                    .fill(ACCENT)
                                    .corner_radius(6),
                                )
                                .clicked()
                                && self.require_org_online("保存公司资料")
                            {
                                let fields: Value = json!(self.settings.company);
                                match self.call(
                                    "PUT",
                                    "/v1/company-profile",
                                    Some(json!({ "fields": fields })),
                                ) {
                                    Ok(_) => self.msg = "已保存".into(),
                                    Err(e) => self.msg = e,
                                }
                            }
                        }
                        "内容语言" => {
                            ui.horizontal(|ui| {
                                ui.label("内容语言清单");
                                ui.add_sized(
                                    [300.0, 26.0],
                                    egui::TextEdit::singleline(&mut self.settings.languages_text),
                                );
                                if ui.button("保存").clicked() && self.require_org_online("保存内容语言") {
                                    let langs: Vec<String> = self
                                        .settings
                                        .languages_text
                                        .split(',')
                                        .map(|x| x.trim().to_string())
                                        .filter(|x| !x.is_empty())
                                        .collect();
                                    match self.call(
                                        "PUT",
                                        "/v1/settings/languages",
                                        Some(json!({ "languages": langs })),
                                    ) {
                                        Ok(_) => self.msg = "已保存".into(),
                                        Err(e) => self.msg = e,
                                    }
                                }
                            });
                        }
                        "AI 翻译" => {
                            if !self.settings.ai_loaded {
                                self.settings.ai_loaded = true;
                                if let Ok(v) = self.call("GET", "/v1/settings/ai", None) {
                                    self.settings.ai_provider = s(&v, "provider");
                                    self.settings.ai_endpoint = s(&v, "endpoint");
                                    self.settings.ai_model = s(&v, "model");
                                    self.settings.ai_has_key =
                                        v.get("has_key").and_then(|x| x.as_bool()).unwrap_or(false);
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
                                        [300.0, 26.0],
                                        egui::TextEdit::singleline(&mut self.settings.ai_endpoint),
                                    );
                                    ui.end_row();
                                    ui.label("模型");
                                    ui.add_sized(
                                        [300.0, 26.0],
                                        egui::TextEdit::singleline(&mut self.settings.ai_model),
                                    );
                                    ui.end_row();
                                    ui.label("API 密钥");
                                    ui.add_sized(
                                        [300.0, 26.0],
                                        egui::TextEdit::singleline(&mut self.settings.ai_key)
                                            .password(true),
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
                            if ui
                                .add(
                                    egui::Button::new(
                                        egui::RichText::new("保存 AI 设置")
                                            .color(egui::Color32::WHITE),
                                    )
                                    .fill(ACCENT)
                                    .corner_radius(6),
                                )
                                .clicked()
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
                                        self.msg = "AI 设置已保存".into();
                                    }
                                    Err(e) => self.msg = e,
                                }
                            }
                            ui.add_space(16.0);
                            ui.label(egui::RichText::new("术语表（每行一条：源词=译词）").strong());
                            ui.add_sized(
                                [440.0, 120.0],
                                egui::TextEdit::multiline(&mut self.settings.glossary_text),
                            );
                            ui.add_space(8.0);
                            if ui.button("保存术语表").clicked() && self.require_org_online("保存术语表") {
                                let pairs: Vec<Value> = self
                                    .settings
                                    .glossary_text
                                    .lines()
                                    .filter_map(|l| {
                                        let (a, b) = l.split_once('=')?;
                                        Some(json!({
                                            "src": a.trim(),
                                            "dst": b.trim(),
                                        }))
                                    })
                                    .collect();
                                match self.call(
                                    "PUT",
                                    "/v1/settings/glossary",
                                    Some(json!({ "pairs": pairs })),
                                ) {
                                    Ok(_) => self.msg = "术语表已保存".into(),
                                    Err(e) => self.msg = e,
                                }
                            }
                        }
                        "离线缓存策略" => {
                            self.cache_path_ui(ui);
                            ui.add_space(10.0);
                            ui.separator();
                            ui.add_space(10.0);
                            ui.label(
                                egui::RichText::new("管理员策略（服务器下发）")
                                    .strong()
                                    .size(16.0)
                                    .color(palette_color(TEXT_MAIN)),
                            );
                            ui.add_space(8.0);
                            let mut allowed = self
                                .cache_policy
                                .get("allowed")
                                .and_then(|v| v.as_bool())
                                .unwrap_or(true);
                            let mut max_mb = self
                                .cache_policy
                                .get("max_mb")
                                .and_then(|v| v.as_i64())
                                .unwrap_or(500);
                            ui.horizontal(|ui| {
                                ui.checkbox(&mut allowed, "允许离线缓存");
                                ui.label("容量上限（MB）");
                                ui.add(egui::DragValue::new(&mut max_mb).range(10..=100000));
                                if ui.button("保存策略").clicked() && self.require_org_online("保存缓存策略") {
                                    match self.call(
                                        "PUT",
                                        "/v1/settings/cache-policy",
                                        Some(json!({
                                            "allowed": allowed,
                                            "max_mb": max_mb
                                        })),
                                    ) {
                                        Ok(_) => {
                                            self.cache_policy =
                                                json!({ "allowed": allowed, "max_mb": max_mb });
                                            self.msg = "缓存策略已保存".into();
                                        }
                                        Err(e) => self.msg = e,
                                    }
                                }
                            });
                        }
                        _ => {}
                    }
                });
            });
    }
}

/// Append one line to the startup log under ~/.qimu (best effort: a
/// logging failure must never take the app down with it). The GUI
/// subsystem has no console, so without this file a startup failure is
/// a silent vanish ("闪一下就没") with zero evidence left behind.
/// Phase-1 measurement switch (design §4.1): every perf
/// instrumentation point in the client (M1–M5) hangs off this one
/// flag — `QIMU_PERF_LOG=1` writes to `client-perf.log` in the
/// qimu home; unset/anything else is off and costs one cached
/// flag read per call site.
pub(crate) fn perf_enabled() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| {
        std::env::var("QIMU_PERF_LOG")
            .map(|v| v == "1")
            .unwrap_or(false)
    })
}

/// Append one line to the perf log (no-op when the switch is
/// off). Same home and line shape as `startup_log`, separate
/// file so metric extraction never wades through launch noise.
pub(crate) fn perf_log(msg: &str) {
    if !perf_enabled() {
        return;
    }
    let dir = qimu_home();
    let _ = std::fs::create_dir_all(&dir);
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let line = format!("[{secs}] {msg}\n");
    let _ = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(dir.join("client-perf.log"))
        .and_then(|mut f| std::io::Write::write_all(&mut f, line.as_bytes()));
}

thread_local! {
    /// M3 REST counters for the calling thread: (call count,
    /// total ms). The UI thread's refresh summaries diff two
    /// snapshots; worker threads accumulate into their own cells
    /// and never pollute a UI-thread summary.
    static REST_STAT: std::cell::Cell<(u64, u128)> = std::cell::Cell::new((0, 0));
}

fn perf_rest_note(ms: u128) {
    REST_STAT.with(|c| {
        let (n, t) = c.get();
        c.set((n + 1, t + ms));
    });
}

fn perf_rest_snapshot() -> (u64, u128) {
    REST_STAT.with(|c| c.get())
}

/// The `refresh <kind> …` summary line (design §4.1 M3/M4):
/// REST calls and REST wait inside one refresh_all / apply_dirty
/// pass, plus the pass's total wall time and — for apply_dirty —
/// the per-slice breakdown.
fn perf_refresh_summary(
    kind: &str,
    start: std::time::Instant,
    before: (u64, u128),
    slices: &[String],
) {
    if !perf_enabled() {
        return;
    }
    let (n, t) = perf_rest_snapshot();
    let mut line = format!(
        "refresh {kind} rest_count={} rest_ms={} total_ms={}",
        n.saturating_sub(before.0),
        t.saturating_sub(before.1),
        start.elapsed().as_millis()
    );
    if !slices.is_empty() {
        line += &format!(" slices={}", slices.join(","));
    }
    perf_log(&line);
}

/// M5 frame timer: logs `frame <ms>` on drop when the frame
/// exceeded 50ms, so slow frames can be aligned by timestamp
/// with the M1–M4 lines that caused them.
struct PerfFrame(std::time::Instant);

impl PerfFrame {
    fn new() -> Self {
        Self(std::time::Instant::now())
    }
}

impl Drop for PerfFrame {
    fn drop(&mut self) {
        let ms = self.0.elapsed().as_millis();
        if ms > 50 {
            perf_log(&format!("frame {ms}ms"));
        }
    }
}

/// Phase-1 gray switch (design §5.2): `QIMU_REFRESH_V2=0`
/// turns the event machinery off — `changed`/`changed_now`
/// then fall back to the legacy `refresh_all()` at their single
/// shared entry, never at the migration call sites. Missing =
/// on.
fn refresh_v2_enabled() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| {
        std::env::var("QIMU_REFRESH_V2")
            .map(|v| v != "0")
            .unwrap_or(true)
    })
}

fn startup_log(msg: &str) {
    let dir = qimu_home();
    let _ = std::fs::create_dir_all(&dir);
    // Audit2 F-N6: the log used to grow without bound (one line per
    // launch plus one per panic, forever). Rotate once past ~1 MB,
    // keeping a single previous generation.
    let log_path = dir.join("client-startup.log");
    if std::fs::metadata(&log_path)
        .map(|m| m.len() > 1_000_000)
        .unwrap_or(false)
    {
        let _ = std::fs::rename(&log_path, dir.join("client-startup.log.1"));
    }
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let line = format!("[{secs}] {msg}\n");
    let _ = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(dir.join("client-startup.log"))
        .and_then(|mut f| std::io::Write::write_all(&mut f, line.as_bytes()));
}

/// Route panics into the startup log before the default hook runs, so
/// a startup panic leaves a diagnosable trace instead of just a flash.
fn install_panic_logger() {
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        startup_log(&format!("PANIC: {info}"));
        default_hook(info);
    }));
}

/// The brand logo as the OS window / taskbar icon, decoded from
/// the same embedded asset the shell renders (assets/logo.png.b64).
fn app_icon() -> Option<egui::IconData> {
    let img = image::load_from_memory(*LOGO_PNG).ok()?;
    let rgba = img.to_rgba8();
    let (width, height) = rgba.dimensions();
    Some(egui::IconData {
        rgba: rgba.into_raw(),
        width,
        height,
    })
}

fn main() {
    install_panic_logger();
    startup_log("qimu-client starting");
    let mut viewport = egui::ViewportBuilder::default()
        .with_inner_size([1440.0, 900.0])
        .with_min_inner_size([960.0, 600.0])
        .with_title("启幕 · PPT 管理器");
    if let Some(icon) = app_icon() {
        viewport = viewport.with_icon(icon);
    }
    let options = eframe::NativeOptions {
        system_font_fallback: false, // The client installs and seeds its own provider.
        viewport,
        ..Default::default()
    };
    match eframe::run_native("启幕", options, Box::new(|cc| Ok(Box::new(App::new(cc))))) {
        Ok(()) => startup_log("qimu-client exited normally"),
        Err(e) => {
            // Previously main returned this Err and the process died
            // silently (no console under the Windows GUI subsystem).
            // Leave a log line and tell the user what happened.
            startup_log(&format!("run_native error: {e:?}"));
            let log_path = qimu_home().join("client-startup.log");
            let _ = rfd::MessageDialog::new()
                .set_level(rfd::MessageLevel::Error)
                .set_title("启幕启动失败")
                .set_description(format!(
                    "图形界面初始化失败：{e}\n\n详细信息已写入日志：{}",
                    log_path.display()
                ))
                .show();
        }
    }
}

#[cfg(test)]
mod i18n_tests {
    use super::*;

    #[test]
    fn catalogs_parse_and_switch() {
        let (i18n, _problems) = craft_i18n::I18n::new(&I18N_EMBEDDED, &["zh-hans", "en-us"]);
        assert!(
            i18n.check_all().is_empty(),
            "catalog problems: {:?}",
            i18n.check_all()
        );
        let zh = i18n.from_code("zh-hans").expect("zh-hans offered");
        let en = i18n.from_code("en-us").expect("en-us offered");
        // Default locale is the identity: UI text renders verbatim.
        assert_eq!(i18n.t(zh, "演示厅"), "演示厅");
        assert_eq!(i18n.t(zh, "离线缓存策略"), "离线缓存策略");
        // The switch point: the same source string renders in English.
        assert_eq!(i18n.t(en, "演示厅"), "Showroom");
        assert_eq!(i18n.t(en, "设置"), "Settings");
        // Unrouted strings fall back to the source in every locale.
        assert_eq!(i18n.t(en, "组装工作台"), "组装工作台");
        // Placeholder templates substitute through craft_i18n::fmt.
        assert_eq!(
            craft_i18n::fmt(i18n.t(en, "服务器：{name}"), &[("name", "nas-01")]),
            "Server: nas-01"
        );
        assert_eq!(
            craft_i18n::fmt(i18n.t(zh, "服务器：{name}"), &[("name", "nas-01")]),
            "服务器：nas-01"
        );
    }
}

#[cfg(test)]
mod cache_crypto_tests {
    use super::*;

    #[test]
    fn encrypt_decrypt_round_trip() {
        let key = [7u8; 32];
        let plain = b"PK\x03\x04 fake pptx bytes for round trip";
        let enc = encrypt_bytes(&key, plain).expect("encrypt");
        assert_ne!(&enc[12..], &plain[..]);
        let back = decrypt_bytes(&key, &enc).expect("decrypt");
        assert_eq!(back, plain.to_vec());
        let wrong = [8u8; 32];
        assert!(decrypt_bytes(&wrong, &enc).is_none());
    }
}

#[cfg(test)]
mod appearance_tests {
    use super::*;

    #[test]
    fn app_theme_overrides_system_theme_for_text_and_controls() {
        let ctx = egui::Context::default();
        for (light, theme, system_theme, foreground) in [
            (
                false,
                egui::Theme::Dark,
                egui::Theme::Light,
                egui::Color32::WHITE,
            ),
            (
                true,
                egui::Theme::Light,
                egui::Theme::Dark,
                egui::Color32::BLACK,
            ),
            (
                false,
                egui::Theme::Dark,
                egui::Theme::Light,
                egui::Color32::WHITE,
            ),
        ] {
            LIGHT_THEME.set(light);
            apply_modern_style(&ctx);
            let mut output = ctx.run_ui(
                egui::RawInput {
                    system_theme: Some(system_theme),
                    ..Default::default()
                },
                |ui| {
                    assert_eq!(ui.ctx().theme(), theme);
                    assert_eq!(palette_color(ICON_TINT), foreground);
                    assert_eq!(ui.visuals().override_text_color, Some(foreground));
                    assert_eq!(ui.visuals().widgets.hovered.fg_stroke.color, foreground);
                    assert_eq!(ui.visuals().weak_text_color(), palette_color(TEXT_FAINT));
                },
            );
            output.textures_delta.clear();
        }
    }

    #[test]
    #[ignore = "requires installed Latin and Simplified Chinese system fonts"]
    fn system_fonts_render_chinese_and_latin() {
        let ctx = egui::Context::default();
        install_system_fonts(&ctx);
        let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
            ui.fonts_mut(|fonts| {
                for family in [egui::FontFamily::Proportional, egui::FontFamily::Monospace] {
                    let primary = &fonts.definitions().families[&family][0];
                    assert!(primary.starts_with("system"), "primary font: {primary}");
                    let id = egui::FontId::new(13.5, family);
                    assert!(fonts.has_glyphs(&id, "启幕 · Showroom"));
                }
            });
        });
        output.textures_delta.clear();
    }
}

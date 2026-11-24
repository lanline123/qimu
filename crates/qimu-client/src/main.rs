//! qimu-client — the Rust desktop client (spec §4): four top-level pages
//! 演示厅 / 内容页 / 索引页 / 设置, talking to qimu-server over HTTP.
//! All logic lives in qimu-core / the server; this crate is a frontend,
//! exactly like the CLI (spec §4.4 parity).
#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

mod chrome_icons;
mod index_controls;
mod index_lineage;
mod index_history;
mod shell;
mod page_editor;

use page_editor::*;
mod workspace_controls;

use chrome_icons::{ChromeIcon, ChromeIcons};
use index_controls::{IndexMoreAction, IndexOverviewControls, IndexScope};
use index_lineage::IndexGraphs;
use qimu_core::store::IndexGraph;
use workspace_controls::{palette_color, query_component, WorkspaceControls, ENGLISH_UI, LIGHT_THEME};

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
pub const CARD_BG: egui::Color32 = egui::Color32::from_rgb(0x15, 0x15, 0x19);
pub const CARD_BORDER: egui::Color32 = egui::Color32::from_rgb(0x28, 0x28, 0x30);
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
/// Layout tokens: page content inset inside a dock frame — the
/// prototype's main padding (20px top / 22px sides); the sidebar
/// column is 268px on every page (.appbody grid).
pub const PAGE_PAD_L: f32 = 22.0;

fn apply_modern_style(ctx: &egui::Context) {
    let light = LIGHT_THEME.get();
    ctx.set_theme(if light { egui::Theme::Light } else { egui::Theme::Dark });
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
    style
        .text_styles
        .insert(TextStyle::Monospace, FontId::new(12.0, FontFamily::Monospace));
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

/// The one standard card: the prototype's --panel surface over the
/// --bg canvas, 1px --line hairline as the only finish, 6px radius
/// (the final tightening layer puts .std-card and every panel-class
/// surface at 6px), NO shadow (the final .std-card rule sets
/// box-shadow:none) — cover-first layout. Inner margin follows the
/// final .card-body padding (10px top / 12px sides / 11px bottom).
fn card_frame() -> egui::Frame {
    egui::Frame::NONE
        .fill(palette_color(CARD_BG))
        .stroke(egui::Stroke::new(1.0, palette_color(CARD_BORDER)))
        .corner_radius(egui::CornerRadius::same(6))
        .inner_margin(egui::Margin {
            left: 12,
            right: 12,
            top: 10,
            bottom: 11,
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

/// Status chips: the prototype's semantic pairs (fg over its *-soft
/// bg; gray = --muted on --panel2).
pub const CHIP_GREEN_FG: egui::Color32 = egui::Color32::from_rgb(0x3F, 0xAE, 0x74);
pub const CHIP_GREEN_BG: egui::Color32 = egui::Color32::from_rgba_unmultiplied_const(0x3F, 0xAE, 0x74, 38);
pub const CHIP_ORANGE_FG: egui::Color32 = egui::Color32::from_rgb(0xE0, 0x91, 0x3A);
pub const CHIP_ORANGE_BG: egui::Color32 = egui::Color32::from_rgba_unmultiplied_const(0xE0, 0x91, 0x3A, 36);
pub const CHIP_BLUE_BG: egui::Color32 = egui::Color32::from_rgba_unmultiplied_const(0x5B, 0x8C, 0xFF, 41);
pub const CHIP_GRAY_FG: egui::Color32 = TEXT_WEAK;
pub const CHIP_GRAY_BG: egui::Color32 = egui::Color32::from_rgb(0x1D, 0x1D, 0x23);
/// Danger pair — the prototype's --rose / --rose-soft.
pub const ROSE: egui::Color32 = egui::Color32::from_rgb(0xFF, 0x72, 0x76);
pub const ROSE_SOFT_BG: egui::Color32 = egui::Color32::from_rgba_unmultiplied_const(0xF2, 0x55, 0x5A, 36);
/// Accent-soft fill (prototype --accent-soft — the same value as
/// SEL_BG): the .show-seg selected state and accent-tinted fills.
pub const ACCENT_SOFT: egui::Color32 = egui::Color32::from_rgba_unmultiplied_const(0x2F, 0x6B, 0xF5, 41);
/// Badge Bad tone: the prototype's --rose over its soft pair
/// (identical values to ROSE / ROSE_SOFT_BG above).
pub const BADGE_BAD_FG: egui::Color32 = ROSE;
pub const BADGE_BAD_BG: egui::Color32 = egui::Color32::from_rgba_unmultiplied_const(0xF2, 0x55, 0x5A, 36);
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
    let width = ui.available_width();
    let (rect, resp) =
        ui.allocate_exact_size(egui::vec2(width, 28.0), egui::Sense::click());
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
        icon.image(15.0, palette_color(ICON_TINT)).paint_at(ui, ir);
        tx += 15.0 + 6.0;
    }
    let font = egui::FontId::new(size, egui::FontFamily::Proportional);
    let color = if selected {
        palette_color(ACCENT_DEEP)
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
    egui::Frame::NONE.fill(palette_color(APP_BG)).inner_margin(egui::Margin {
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
    std::fs::read_to_string(qimu_home().join("client-settings.json"))
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_else(|| json!({}))
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
        serde_json::to_string_pretty(&cur).unwrap_or_default().as_bytes(),
    );
}

/// Offline cache (§3.17): adopted snapshots live in the cache directory
/// with a small JSON index — usable with no server at all. The directory
/// is a local setting: default is a `cache` folder beside the client
/// executable (the install dir); the user can move it in 设置.
fn default_cache_dir() -> std::path::PathBuf {
    exe_dir().join("cache")
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
                    let rel = p.strip_prefix(base).unwrap_or(&p).to_string_lossy().into_owned();
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
    if client_settings().get("cache_dir").and_then(|v| v.as_str()).is_some() {
        return;
    }
    let legacy = legacy_cache_dir();
    let target = default_cache_dir();
    if legacy == target || !legacy.is_dir() {
        return;
    }
    let has_content = std::fs::read_dir(&legacy).map(|mut d| d.next().is_some()).unwrap_or(false);
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
    std::fs::read_to_string(cache_dir().join("index.json"))
        .ok()
        .and_then(|t| serde_json::from_str::<Value>(&t).ok())
        .and_then(|v| v.as_array().cloned())
        .unwrap_or_default()
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
    use aes_gcm::aead::OsRng;
    use aes_gcm::aead::rand_core::RngCore;
    let dir = cache_dir();
    let path = dir.join("cache.key");
    if let Some(key) = read_key_file(&path) {
        return Some(key);
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
    use aes_gcm::aead::{Aead, OsRng};
    use aes_gcm::aead::rand_core::RngCore;
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
    /// The show was observed running and then ended.
    ShowEnded,
    /// The control channel is dead (instance closed or crashed).
    Gone,
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
        if !p.is_empty() {
            return Some(std::path::PathBuf::from(p));
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

/// Device-local preference. Preserve explicit choices from the old settings file.
fn load_dc_auto_quit() -> bool {
    client_settings()["deckcraft_auto_quit"].as_bool().or_else(|| {
        let text = std::fs::read_to_string(qimu_home().join("present.json")).ok()?;
        serde_json::from_str::<Value>(&text).ok()?["auto_quit"].as_bool()
    }).unwrap_or(false)
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
        let stream = std::net::TcpStream::connect_timeout(
            &addr,
            std::time::Duration::from_millis(600),
        )?;
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
                    let _ = tx.send(DcEvent::ShowEnded);
                }
            }
            Err(_) => {
                ctl = None;
                errors += 1;
                if errors >= 5 {
                    let _ = tx.send(DcEvent::Gone);
                    return;
                }
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(1200));
    }
}

fn save_cache_index(entries: &[Value]) {
    let dir = cache_dir();
    let _ = std::fs::create_dir_all(&dir);
    let _ = craft_atomic::atomic_write(
        &dir.join("index.json"),
        serde_json::to_string_pretty(&json!(entries)).unwrap_or_default().as_bytes(),
    );
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
            e.get("locked_semver").and_then(|x| x.as_str()).unwrap_or("?")
        )
    } else {
        "跟随最新".to_string()
    };
    if let Some(slots) = e.get("excerpt_slots").and_then(|x| x.as_array()) {
        base += &format!(" · 摘录 {} 页", slots.len());
    }
    base
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
    stack: NavStack<ContentFrame>,
}

impl Default for ContentState {
    fn default() -> Self {
        Self {
            folder_filter: None,
            import_group: String::new(),
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

#[derive(Default)]
struct WallState {
    search_text: String,
    search_hits: Option<Value>,
    /// Single-click selection on the wall (the accent ring). The
    /// drill-down itself is the Unit frame, not this field.
    selected: Option<String>,
}

/// One open content block: its detail payload plus every
/// detail-period form (publish, per-page language bindings,
/// scripts editor, AI translation target). Drafts die with the
/// frame — what used to leak from one block into the next through
/// ContentState now cannot.
struct UnitState {
    unit_id: String,
    detail: Value,
    publish_bump: usize, // 0 major 1 minor 2 patch
    publish_form: String,
    publish_lang: String,
    publish_note: String,
    publish_file: Option<std::path::PathBuf>,
    bindings: Vec<Value>,
    bind_from: String,
    bind_to: String,
    bind_from_slot: Option<i64>,
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
            publish_bump: 1, // default preselect: minor （小版本）, as at App::new before
            publish_form: String::new(),
            publish_lang: String::new(),
            publish_note: String::new(),
            publish_file: None,
            bindings: Vec::new(),
            bind_from: String::new(),
            bind_to: String::new(),
            bind_from_slot: None,
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
    FocusCommit { branch: String, id: String },
    OpenGraphTarget { index: String, branch: String, commit: Option<String> },
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
    filter: usize, // 0 all | 1 index | 2 unit | 3 showroom
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
                                        sz: st
                                            .get("sz")
                                            .and_then(|x| x.as_f64())
                                            .map(|f| f as f32),
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
        has_xfrm: v
            .get("has_xfrm")
            .and_then(|x| x.as_bool())
            .unwrap_or(false),
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
        if self.elems.iter().any(|(page, list)| self.elems_base.get(page) != Some(list)) {
            return true;
        }
        self.work.iter().any(|(p, runs)| {
            self.base
                .get(p)
                .map(|b| b != runs)
                .unwrap_or(false)
        })
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
        let mut pages: Vec<usize> = self.elems.keys().copied()
            .filter(|&page| only_page.is_none_or(|only| only == page)).collect();
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
                    out.push(json!({"kind":"line_color", "page":p, "el":ei, "color":we.line_color}));
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
fn mm_field(
    ui: &mut egui::Ui,
    label: &str,
    emu: i64,
    want_snapshot: &mut bool,
) -> Option<i64> {
    let mut v = emu_mm(emu);
    let mut out = None;
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new(label).size(11.5).color(palette_color(TEXT_FAINT)));
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
        if ui.selectable_label(pe.selection.contains(&idx), format!("{indent}{label}")).clicked() {
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
                let label = SHAPE_PRESETS.iter().find(|(key, _)| key == preset).map(|(_, label)| tr(label)).unwrap_or(preset);
                egui::ComboBox::from_id_salt(("shape-preset", sel))
                    .selected_text(label.to_owned())
                    .show_ui(ui, |ui| {
                        for (key, label) in SHAPE_PRESETS {
                            if ui.selectable_value(preset, (*key).into(), tr(label)).changed() {
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
                let mut color = elw.line_color.as_deref().map(hex_color32).unwrap_or(egui::Color32::DARK_GRAY);
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
    AllVersions { entry_pos: usize },
    Present(PresentState),
    PageEdit(PageEditState),
    PickTemplate(PickTemplateState),
    Confirm(ConfirmState),
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
            Some(slots) => slots
                .iter()
                .filter_map(|slot| slot.as_str()?.parse::<usize>().ok())
                .filter(|slot| *slot < total)
                .collect::<Vec<_>>(),
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

/// Events from the AI-translation worker thread: progress ticks
/// while the server job runs, then exactly one terminal result (the
/// same payload the synchronous translate endpoint returns).
enum TranslateEvent {
    Progress(usize, usize),
    Done(Result<Value, String>),
}

struct App {
    server: String,
    username: String,
    password: String,
    token: Option<String>,
    user: Option<Value>,
    conns: Vec<Conn>,
    active: Option<usize>,
    add_url: String,
    add_user: String,
    add_pass: String,
    discovered: Vec<Value>,
    disc_rx: Option<std::sync::mpsc::Receiver<Vec<Value>>>,
    disc_scanning: bool,
    cache_dir_input: String,
    pick: Option<PickState>,
    /// Cross-page deep-jump source (nav ruler 3): set when Home
    /// opens a detail on another page; that page's root then shows
    /// a back row returning here. Manual tab/menu switches clear it.
    deep_source: Option<Page>,
    folder_edit: Option<FolderEdit>,
    folder_edit_focus: bool,
    folder_delete_confirm: Option<(String, String)>,
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
    showroom_pending: Vec<Value>,
    showroom: ShowroomState,
    my_assemblies: Vec<Value>,
    inbox: Vec<Value>,
    group_spaces: Vec<Value>,
    my_teams: Vec<Value>,
    library_refs: Vec<Value>,
    my_id: String,
    new_team_name: String,
    team_member_pick: String,
    cache_entries: Vec<Value>,
    cache_policy: Value,
    index_covers: std::collections::HashMap<String, String>,
    index_tags: std::collections::HashMap<String, String>,
    index_search_tags: BTreeMap<String, Vec<String>>,
    content: ContentState,
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
        usize,
        Option<std::sync::Arc<Vec<u8>>>,
        Result<PdfImg, String>,
    )>,
    pdf_rx: std::sync::mpsc::Receiver<(
        String,
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
}

enum ThumbState {
    Loading,
    Ready(egui::TextureHandle),
    Failed,
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
    let latest = craft_update::parse_github_release_list(&text, "lanline136/qimu", false)
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
    let locale = std::env::var("QIMU_LOCALE")
        .ok()
        .unwrap_or_else(|| s(&settings, "locale"));
    ENGLISH_UI.set(locale.to_ascii_lowercase().starts_with("en"));
    LIGHT_THEME.set(settings["theme"] == "light");
}

/// Translate a UI string through the embedded catalogs.
fn tr(source: &str) -> &str {
    let Some(i18n) = I18N.get() else {
        return source;
    };
    let code = if ENGLISH_UI.get() { "en-us" } else { "zh-hans" };
    i18n.from_code(code).map_or(source, |lang| i18n.t(lang, source))
}

fn icon_png(b64: &str) -> &'static [u8] {
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(b64.trim())
        .expect("embedded icon base64 must decode");
    Box::leak(bytes.into_boxed_slice())
}

/// The brand logo (blue Q ribbon), extracted from the product
/// prototype's top bar and stored as base64 text
/// (assets/logo.png.b64). Drives the shell / login / about image,
/// the runtime window icon (app_icon) and — via build.rs — the
/// executable's embedded icon.
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
                username: v.get("username").and_then(|u| u.as_str()).unwrap_or("").to_string(),
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
    let _ = std::fs::write(
        dir.join("servers.json"),
        serde_json::to_string_pretty(&Value::Array(arr)).unwrap_or_default(),
    );
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

/// One HTTP call against an explicit server/token (App::call delegates
/// here with the active connection). `short_timeout` is for session
/// validation probes, where a dead server must not stall startup.
fn raw_call(
    url: &str,
    token: Option<&str>,
    method: &str,
    path: &str,
    body: Option<Value>,
    short_timeout: bool,
) -> Result<Value, String> {
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
        match method {
            "GET" => ureq::get(&full),
            "PUT" => ureq::put(&full),
            "PATCH" => ureq::patch(&full),
            "DELETE" => ureq::delete(&full),
            _ => ureq::post(&full),
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
        Ok(r) => r.into_json::<Value>().map_err(|e| e.to_string()),
        Err(ureq::Error::Status(_, r)) => {
            let text = r.into_string().unwrap_or_default();
            Err(serde_json::from_str::<Value>(&text)
                .ok()
                .and_then(|v| v.get("error").and_then(|e| e.as_str()).map(|s| s.to_string()))
                .unwrap_or(text))
        }
        Err(e) => Err(format!("请求失败: {e}")),
    }
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

fn tcp_reachable(port: u16) -> bool {
    let addr = std::net::SocketAddr::from(([127, 0, 0, 1], port));
    std::net::TcpStream::connect_timeout(&addr, std::time::Duration::from_millis(300)).is_ok()
}

fn find_server_exe() -> Option<std::path::PathBuf> {
    if let Ok(p) = std::env::var("QIMU_SERVER_EXE") {
        let pb = std::path::PathBuf::from(p);
        if pb.exists() {
            return Some(pb);
        }
    }
    let name = if cfg!(target_os = "windows") {
        "qimu-server.exe"
    } else {
        "qimu-server"
    };
    let cand = exe_dir().join(name);
    if cand.exists() {
        Some(cand)
    } else {
        None
    }
}

/// Best-effort: mark the bundled server's data directory as a
/// standalone installation (a standalone.on marker the server
/// honors when QIMU_STANDALONE is unset — the installer-autostarted
/// instance never sees the env the client would have spawned it
/// with). Only ever touches the data dir beside this executable;
/// an organization server's directory sits on another machine and
/// is unreachable from here.
fn mark_local_standalone() {
    if let Some(exe) = find_server_exe() {
        if let Some(dir) = exe.parent() {
            let data = dir.join("qimu-data");
            if std::fs::create_dir_all(&data).is_ok() {
                let _ = std::fs::write(data.join("standalone.on"), "standalone\n");
            }
        }
    }
}

/// Start the bundled server next to this executable, detached, with its
/// data in a `qimu-data` folder beside it (the server also resolves that
/// itself; the env makes it explicit).
fn spawn_local_server(exe: &std::path::Path) -> bool {
    let dir = exe.parent().unwrap_or_else(|| std::path::Path::new("."));
    let data = dir.join("qimu-data");
    let _ = std::fs::create_dir_all(&data);
    let mut cmd = std::process::Command::new(exe);
    // The bundled server is this machine's own data plane: it runs
    // in standalone mode, where tokenless loopback requests act as
    // the owner (org-model standalone gap). An organization server
    // is started by hand and never sees this env.
    cmd.env("QIMU_DATA_DIR", &data)
        .env("QIMU_STANDALONE", "1")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    cmd.spawn().is_ok()
}

impl App {
    fn new(cc: &eframe::CreationContext<'_>) -> Self {
        egui_extras::install_image_loaders(&cc.egui_ctx);
        install_system_fonts(&cc.egui_ctx);
        init_i18n();
        apply_modern_style(&cc.egui_ctx);
        migrate_cache_dir_once();
        let (thumb_tx, thumb_rx) = std::sync::mpsc::channel();
        let (pdf_tx, pdf_rx) = std::sync::mpsc::channel();
        let (upd_tx, upd_rx) = std::sync::mpsc::channel();
        let (translate_tx, translate_rx) = std::sync::mpsc::channel();
        let (dc_tx, dc_rx) = std::sync::mpsc::channel();
        let mut app = App {
            server: std::env::var("QIMU_SERVER").unwrap_or_else(|_| "http://127.0.0.1:8730".into()),
            username: std::env::var("QIMU_USERNAME").unwrap_or_else(|_| "admin".into()),
            password: std::env::var("QIMU_PASSWORD").unwrap_or_default(),
            token: None,
            user: None,
            conns: vec![],
            active: None,
            add_url: String::new(),
            add_user: String::new(),
            add_pass: String::new(),
            discovered: vec![],
            disc_rx: None,
            disc_scanning: false,
            cache_dir_input: cache_dir().to_string_lossy().into_owned(),
            pick: None,
            deep_source: None,
            folder_edit: None,
            folder_edit_focus: false,
            folder_delete_confirm: None,
            page: match std::env::var("QIMU_PAGE").as_deref() {
                Ok("home") => Page::Home,
                Ok("content") => Page::Content,
                Ok("index") => Page::Index,
                Ok("trash") => Page::Trash,
                Ok("settings") => Page::Settings,
                _ => Page::Showroom,
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
            showroom_pending: vec![],
            showroom: ShowroomState::default(),
            my_assemblies: vec![],
            inbox: vec![],
            group_spaces: vec![],
            my_teams: vec![],
            library_refs: vec![],
            my_id: String::new(),
            new_team_name: String::new(),
            team_member_pick: String::new(),
            cache_entries: load_cache_index(),
            cache_policy: json!({ "allowed": true, "max_mb": 500 }),
            index_covers: std::collections::HashMap::new(),
            index_tags: std::collections::HashMap::new(),
            index_search_tags: BTreeMap::new(),
            content: ContentState::default(),
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
            login_open: false,
            about_open: false,
        };
        if std::env::var("QIMU_AUTOLOGIN").is_ok() && !app.password.is_empty() {
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
            // Normal start: restore saved connections (validating each
            // token); with none saved, make sure a local server exists
            // so the user never has to start one by hand.
            app.restore_connections();
            if std::env::var("QIMU_DISCOVER").is_ok() {
                // Screenshot/test hook: run one LAN scan up front.
                app.discovered = discover_servers();
            }
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
                                app.cache_deck(
                                    &format!("sr_{item_id}"),
                                    &title,
                                    &bytes,
                                    pages,
                                );
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
        raw_call(&self.server, self.token.as_deref(), method, path, body, false)
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
                self.token = v.get("token").and_then(|t| t.as_str()).map(|s| s.to_string());
                self.user = v.get("user").cloned();
                self.msg = "登录成功".into();
                self.login_open = false;
                // Persist this server as a saved connection so the next
                // start goes straight in.
                let url = self.server.clone();
                let idx = match self
                    .conns
                    .iter()
                    .position(|c| c.url == url && c.username == self.username)
                {
                    Some(i) => {
                        self.conns[i].token = self.token.clone();
                        self.conns[i].status = ConnStatus::Connected;
                        i
                    }
                    None => {
                        self.conns.push(Conn {
                            name: conn_display_name(&url),
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
        if let Some(i) = self.active {
            if let Some(c) = self.conns.get(i) {
                self.server = c.url.clone();
                self.username = c.username.clone();
                // A named session mirrors its user token; an
                // anonymous connection mirrors its guest token
                // (org-model G1), so guest reads carry the
                // session. Identity decisions never key on this
                // field — the connection's anon flag is the key.
                self.token = if c.status == ConnStatus::Connected
                    || c.status == ConnStatus::Anonymous
                {
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

    /// Make conn i the active server: validate, mirror fields, reload.
    fn activate(&mut self, i: usize) {
        if i >= self.conns.len() {
            return;
        }
        if self.active != Some(i) {
            self.clear_account_state();
        }
        self.active = Some(i);
        if let Some(user) = self.validate_conn(i) {
            self.user = Some(user);
        }
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
        }
    }

    /// Standalone local form (org-model standalone gap): when the
    /// bundled server runs in standalone mode it answers a
    /// tokenless /v1/me as the machine's owner — adopt that
    /// identity so the local library loads in full, with no
    /// account anywhere in sight. Any other answer (an organization
    /// server, or standalone mode off) leaves the client on the
    /// guest surface, exactly as before.
    fn try_adopt_local_owner(&mut self) -> bool {
        match raw_call(&self.server, None, "GET", "/v1/me", None, true) {
            Ok(v) => match v.get("user").cloned() {
                Some(u) => {
                    self.user = Some(u);
                    self.refresh_all();
                    true
                }
                None => false,
            },
            Err(_) => false,
        }
    }

    /// Startup path when no env-driven login was requested: load saved
    /// connections, validate them, enter through the remembered active
    /// one (or the first that still answers). With nothing saved, make
    /// sure a local server is up so the login screen is ready to use.
    fn restore_connections(&mut self) {
        self.conns = load_conns();
        if self.conns.is_empty() {
            self.autostart_local_server();
            if !self.try_adopt_local_owner() {
                self.refresh_guest();
            }
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
                self.conns
                    .iter()
                    .position(|c| {
                        c.url == u
                            && (c.status == ConnStatus::Connected
                                || c.status == ConnStatus::Anonymous)
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
    }

    /// "The server is not something the user starts": when nothing is
    /// listening on 127.0.0.1:8730, launch the server bundled beside
    /// this executable and wait for its port.
    fn autostart_local_server(&mut self) {
        mark_local_standalone();
        if tcp_reachable(8730) {
            self.server = "http://127.0.0.1:8730".into();
            return;
        }
        if let Some(exe) = find_server_exe() {
            if spawn_local_server(&exe) {
                for _ in 0..80 {
                    if tcp_reachable(8730) {
                        break;
                    }
                    std::thread::sleep(std::time::Duration::from_millis(100));
                }
                if tcp_reachable(8730) {
                    self.server = "http://127.0.0.1:8730".into();
                    self.msg = "已自动启动本机服务器".into();
                }
            }
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
                let token = v.get("token").and_then(|t| t.as_str()).map(|s| s.to_string());
                let idx = match self
                    .conns
                    .iter()
                    .position(|c| c.url == url && c.username == username)
                {
                    Some(i) => {
                        self.conns[i].token = token;
                        self.conns[i].status = ConnStatus::Connected;
                        i
                    }
                    None => {
                        self.conns.push(Conn {
                            name: conn_display_name(&url),
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
        let url = normalize_url(url);
        if url.is_empty() {
            self.msg = "请先填写服务器地址".into();
            return false;
        }
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
                self.conns[i].status = ConnStatus::Anonymous;
                if issued.is_some() {
                    self.conns[i].token = issued.clone();
                }
                i
            }
            None => {
                self.conns.push(Conn {
                    name: conn_display_name(&url),
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
        self.activate_anonymous(&url);
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

    fn active_server_label(&self) -> String {
        if let Some(i) = self.active {
            if let Some(c) = self.conns.get(i) {
                return c.name.clone();
            }
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
            if let Some(c) = self.conns.get_mut(i) {
                c.token = None;
                c.status = if c.anon {
                    ConnStatus::Anonymous
                } else {
                    ConnStatus::LoggedOut
                };
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
        if let Ok(v) = self.call("GET", "/v1/showroom?state=live", None) {
            self.showroom_live = v
                .get("items")
                .and_then(|f| f.as_array())
                .cloned()
                .unwrap_or_default();
            self.resolve_showroom_covers();
        }
        self.cache_entries = load_cache_index();
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

    /// Drop every server-scoped list tied to the previous identity.
    fn clear_account_state(&mut self) {
        self.folders.clear();
        self.groups.clear();
        self.units.clear();
        self.indexes.clear();
        self.index.stack = NavStack::new(IndexFrame::Overview(OverviewState::default()));
        self.index.overview_folder = None;
        self.workspace_controls = WorkspaceControls::default();
        self.org = json!({});
        self.showroom_live.clear();
        self.showroom_pending.clear();
        self.my_assemblies.clear();
        self.inbox.clear();
        self.group_spaces.clear();
        self.my_teams.clear();
        self.library_refs.clear();
        self.index_covers.clear();
        self.index_tags.clear();
        self.index_search_tags.clear();
        self.showroom.stack.pop_to(0);
        self.my_id.clear();
    }

    fn refresh_all(&mut self) {
        if let Ok(v) = self.call("GET", "/v1/folders", None) {
            self.folders = v.get("folders").and_then(|f| f.as_array()).cloned().unwrap_or_default();
        }
        if let Ok(v) = self.call("GET", "/v1/groups", None) {
            self.groups = v.get("groups").and_then(|f| f.as_array()).cloned().unwrap_or_default();
        }
        if let Ok(v) = self.call("GET", "/v1/units", None) {
            self.units = v.get("units").and_then(|f| f.as_array()).cloned().unwrap_or_default();
        }
        if let Ok(v) = self.call("GET", "/v1/indexes", None) {
            if let IndexFrame::Overview(overview) = self.index.stack.root_mut() {
                overview.graphs = None;
            }
            self.indexes = v.get("indexes").and_then(|f| f.as_array()).cloned().unwrap_or_default();
        }
        if let Ok(v) = self.call("GET", "/v1/org", None) {
            self.org = v;
        }
        if let Ok(v) = self.call("GET", "/v1/showroom?state=live", None) {
            self.showroom_live = v.get("items").and_then(|f| f.as_array()).cloned().unwrap_or_default();
            // A guest pass earlier in this session may have parked
            // empty placeholders for covers it could not resolve;
            // a named session re-resolves instead of inheriting
            // the blanks.
            self.index_covers.retain(|_, v| !v.is_empty());
            self.resolve_showroom_covers();
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
        if let Ok(v) = self.call("GET", "/v1/showroom?state=pending", None) {
            self.showroom_pending = v.get("items").and_then(|f| f.as_array()).cloned().unwrap_or_default();
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
            self.inbox = v.get("shares").and_then(|f| f.as_array()).cloned().unwrap_or_default();
        }
        if let Ok(v) = self.call("GET", "/v1/group-space", None) {
            self.group_spaces = v.get("spaces").and_then(|f| f.as_array()).cloned().unwrap_or_default();
        }
        if let Ok(v) = self.call("GET", "/v1/custom-groups", None) {
            self.my_teams = v.get("groups").and_then(|f| f.as_array()).cloned().unwrap_or_default();
        }
        if let Ok(v) = self.call("GET", "/v1/library", None) {
            self.library_refs = v.get("entries").and_then(|f| f.as_array()).cloned().unwrap_or_default();
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
                self.content.import_group =
                    g.get("id").and_then(|x| x.as_str()).unwrap_or("").to_string();
            }
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
        // The wall ring follows the open block, as selected_unit did.
        if let ContentFrame::Wall(w) = self.content.stack.root_mut() {
            w.selected = Some(unit_id.to_string());
        }
        match self.call("GET", &format!("/v1/units/{unit_id}"), None) {
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
                    .call("GET", &format!("/v1/units/{unit_id}/page-bindings"), None)
                    .ok()
                    .and_then(|b| {
                        b.get("bindings").and_then(|x| x.as_array()).cloned()
                    })
                    .unwrap_or_default();
                // Opening replaces the drill-down, as the single
                // detail slot did: reset to the wall, then push.
                self.content.stack.pop_to(0);
                self.content.stack.push(ContentFrame::Unit(st));
            }
            Err(e) => self.msg = format!("打开失败：{e}"),
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
        if let Ok(v) = self.call(
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
        self.pdf_view = Some(PdfView {
            att_id: att_id.clone(),
            filename,
            bytes: None,
            page: 0,
            pages: 0,
            tex: None,
            note: "正在加载…".into(),
        });
        let (server, token, tx) = (self.server.clone(), self.token.clone(), self.pdf_tx.clone());
        std::thread::spawn(move || {
            let out = raw_call(
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
            .and_then(|bytes| render_pdf_page(&bytes, 0).map(|img| (bytes, img)));
            match out {
                Ok((bytes, img)) => {
                    let _ = tx.send((att_id, 0, Some(bytes), Ok(img)));
                }
                Err(e) => {
                    let _ = tx.send((att_id, 0, None, Err(e)));
                }
            }
        });
    }

    fn pdf_goto(&mut self, page: usize) {
        let Some(v) = &self.pdf_view else { return };
        let Some(bytes) = v.bytes.clone() else { return };
        let att_id = v.att_id.clone();
        if let Some(vv) = &mut self.pdf_view {
            vv.note = "正在加载…".into();
        }
        let tx = self.pdf_tx.clone();
        std::thread::spawn(move || {
            let res = render_pdf_page(&bytes, page);
            let _ = tx.send((att_id, page, None, res));
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
            let h = if size.x > 0.0 { size.y * (w / size.x) } else { 0.0 };
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
            ui.label(egui::RichText::new("附件").strong().color(palette_color(TEXT_MAIN)));
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
                            match self.call("POST", "/v1/attachments", Some(body)) {
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
        if atts.is_empty() {
            ui.label(egui::RichText::new("暂无附件").color(palette_color(TEXT_FAINT)));
        }
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
                    match self.call(
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
                                    if let Some(path) = rfd::FileDialog::new()
                                        .set_file_name(&filename)
                                        .save_file()
                                    {
                                        match craft_atomic::atomic_write(&path, &bytes) {
                                            Ok(()) => self.msg = "附件已保存".into(),
                                            Err(e) => {
                                                self.msg = format!("保存失败：{e}")
                                            }
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
            if self.pdf_view.as_ref().map(|v| v.att_id.as_str()) == Some(s(att, "id").as_str())
            {
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
        match self.call("GET", &format!("/v1/indexes/{index_id}?branch={}", query_component(&b)), None) {
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
        match self.call("GET", &format!("/v1/indexes/{id}?branch={}", query_component(&branch)), None) {
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
        match self.call("GET", &format!("/v1/indexes/{id}?branch={}", query_component(&branch)), None) {
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
            if let Ok(u) = self.call("GET", &format!("/v1/units/{uid}"), None) {
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
            .call(
                "GET",
                &format!("/v1/indexes/{id}/commits?branch={}", query_component(&branch)),
                None,
            )
            .ok();
        let exports = self
            .call("GET", &format!("/v1/exports?index_id={id}"), None)
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
        let r = self.call(
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
    /// with the entry's block opened (§3.10 scenario 44).
    fn start_pick_pages(&mut self, pos: usize, re: &Value, sel: Vec<bool>) {
        let entry = re.get("entry").cloned().unwrap_or(json!({}));
        let resolved = re.get("resolved").cloned().unwrap_or(Value::Null);
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

    fn cancel_pick(&mut self) {
        self.pick = None;
        self.close_unit_detail();
        self.page = Page::Index;
    }

    fn confirm_pick_pages(&mut self) {
        let Some(PickState::Pages { entry_pos, sel, .. }) = self.pick.clone() else {
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
            es[entry_pos]["excerpt_slots"] =
                json!(chosen.iter().map(|n| n.to_string()).collect::<Vec<_>>());
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
        let (rect, resp) = ui.allocate_exact_size(
            egui::vec2(card_w, card_h),
            egui::Sense::click(),
        );
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
                ui.label(egui::RichText::new("+").size(34.0).color(palette_color(TEXT_FAINT)));
                ui.label(egui::RichText::new("新建内容块").size(13.0).color(palette_color(TEXT_WEAK)));
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
        if !self.need_identity("新建内容块") {
            return;
        }
        let r = self.call(
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
                self.refresh_all();
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
        if !self.need_identity("粘贴新建") {
            return;
        }
        let frag: Option<std::path::PathBuf> = std::env::var("QIMU_PASTE_FRAGMENT")
            .ok()
            .map(std::path::PathBuf::from)
            .filter(|p| p.exists())
            .or_else(|| match qimu_bridge::platform_bridge().capture_copied_slides() {
                Ok(p) => p,
                Err(e) => {
                    self.msg = format!("无法读取 PowerPoint 剪贴板：{e}");
                    None
                }
            });
        let Some(path) = frag else {
            self.msg = "先在 PowerPoint 里复制页面，再点「粘贴 PPT 页新建」".into();
            return;
        };
        let Ok(bytes) = std::fs::read(&path) else {
            self.msg = "读取粘贴片段失败".into();
            return;
        };
        let created = self.call(
            "POST",
            "/v1/units",
            Some(json!({
                "name": "未命名内容块",
                "group_id": self.content.import_group,
                "folder_id": self.content.folder_filter,
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
                return;
            }
        };
        if uid.is_empty() {
            self.msg = "新建失败：服务器未返回内容块".into();
            return;
        }
        let r = self.call(
            "POST",
            &format!("/v1/units/{uid}/publish"),
            Some(json!({
                "data_base64": b64e(&bytes),
                "form": "标准",
                "lang": "zh",
                "bump": "minor",
                "note": "剪贴板粘贴新建",
            })),
        );
        match r {
            Ok(_) => {
                self.msg = "已粘贴新建内容块".into();
                self.refresh_all();
                self.open_unit_detail(&uid);
            }
            Err(e) => self.msg = format!("粘贴发布失败：{e}"),
        }
    }
}

// ---------------- UI ----------------

fn s(v: &Value, key: &str) -> String {
    v.get(key).and_then(|x| x.as_str()).unwrap_or("").to_string()
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
    ui.painter().vline(rect.center().x, rect.y_range(), egui::Stroke::new(1.0, palette_color(LINE_34)));
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
    let columns = ((available + 12.0) / (minimum_width + 12.0)).floor().max(1.0) as usize;
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
        egui::Button::new(egui::RichText::new(label).size(12.5).color(palette_color(TEXT_MAIN)))
            .fill(palette_color(BTN_BG))
            .corner_radius(6)
            .min_size(egui::vec2(0.0, 28.0)),
    )
}

fn btn_danger(ui: &mut egui::Ui, label: &str) -> egui::Response {
    ui.add(
        egui::Button::new(egui::RichText::new(label).size(12.5).color(egui::Color32::WHITE))
            .fill(DANGER_BG)
            .corner_radius(6)
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
                                egui::RichText::new(tr(*label))
                                    .size(12.5)
                                    .color(if active { palette_color(TEXT_MAIN) } else { palette_color(TEXT_WEAK) }),
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
    let mut frame = card_frame();
    if flush {
        frame = frame.inner_margin(0);
    }
    if selected {
        frame = frame.stroke(egui::Stroke::new(1.5, ACCENT));
    } else if let Some(color) = scope {
        frame = frame.stroke(egui::Stroke::new(1.0, color));
    }
    frame.show(ui, |ui| {
        ui.with_layout(egui::Layout::top_down(egui::Align::LEFT), add).inner
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
    painter.galley(egui::pos2(inner.min.x, inner.min.y), g, palette_color(TEXT_FAINT));
    let title_font = egui::FontId::new(12.0, egui::FontFamily::Proportional);
    let g = painter.layout(
        title.to_string(),
        title_font,
        palette_color(TEXT_MAIN),
        inner.width(),
    );
    painter.galley(egui::pos2(inner.min.x, inner.min.y + 14.0), g, palette_color(TEXT_MAIN));
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
        egui::Button::new(egui::RichText::new(label).size(12.5).color(egui::Color32::WHITE))
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
            let btn = egui::Button::new(
                egui::RichText::new(tr(*label))
                    .size(12.0)
                    .color(if active { egui::Color32::WHITE } else { palette_color(TEXT_WEAK) }),
            )
            .fill(if active { ACCENT } else { palette_color(BTN_BG) })
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
            }
        });
        ui.add_space(8.0);
        match self.showroom.zone.as_str() {
            "library" => {
                ui.label(egui::RichText::new("我的目录").size(12.0).color(palette_color(TEXT_WEAK)));
                for (idx, label) in ["全部", "收藏", "本地缓存", "我的组装"]
                    .into_iter()
                    .enumerate()
                {
                    if tree_row(
                        ui,
                        self.showroom.library_view == idx,
                        0.0,
                        None,
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
                ui.label(egui::RichText::new("分享来源").size(12.0).color(palette_color(TEXT_WEAK)));
                for (idx, label) in ["全部", "个人分享", "部门分享"].into_iter().enumerate()
                {
                    let count = self
                        .inbox
                        .iter()
                        .filter(|item| Self::share_matches_source(item, idx))
                        .count();
                    if tree_row(
                        ui,
                        self.showroom.share_source == idx,
                        0.0,
                        None,
                        &format!("{label} · {count}"),
                        12.5,
                    )
                    .clicked()
                    {
                        self.showroom.share_source = idx;
                        self.showroom.stack.pop_to(0);
                    }
                }
            }
            "groups" => {
                ui.label(egui::RichText::new("群组").size(12.0).color(palette_color(TEXT_WEAK)));
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
                    if tree_row(ui, active.as_ref() == Some(&id), 0.0, None, &label, 12.5).clicked()
                    {
                        self.showroom.group_id = Some(id);
                        self.showroom.stack.pop_to(0);
                    }
                }
            }
            _ => {
                ui.label(egui::RichText::new("公共目录").size(12.0).color(palette_color(TEXT_WEAK)));
                let mut selected = self.showroom.folder.clone();
                self.folder_tree(ui, &mut selected);
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
        let mut selected = self.content.folder_filter.clone();
        self.folder_tree(ui, &mut selected);
        if self.content.folder_filter != selected
            && self.pick.is_none()
            && self.content.stack.depth() > 1
        {
            self.close_unit_detail();
        }
        self.content.folder_filter = selected;
    }

    fn ix_nav_body(&mut self, ui: &mut egui::Ui) {
        let (in_detail, mut selected) = match self.index.stack.top() {
            IndexFrame::Detail(detail) => {
                let folder = self.index_folder_of(&detail.index_id);
                (true, (!folder.is_empty()).then_some(folder))
            }
            _ => (false, self.index.overview_folder.clone()),
        };
        let previous = selected.clone();
        self.folder_tree(ui, &mut selected);
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
        for (i, name) in sections.iter().enumerate() {
            if tree_row(ui, self.settings.section == i, 0.0, None, tr(name), 12.5).clicked() {
                self.settings.section = i;
            }
        }
    }

    fn trash_nav_body(&mut self, ui: &mut egui::Ui) {
        if self.token.is_none() {
            return;
        }
        let active = match self.trash.stack.top() {
            TrashFrame::List(state) => state.filter,
            TrashFrame::Preview(_) => usize::MAX,
        };
        for (filter, (kind, label)) in [
            ("", "全部"),
            ("index", "索引"),
            ("unit", "内容块"),
            ("showroom", "演示成品"),
        ]
        .into_iter()
        .enumerate()
        {
            let count = self
                .trash
                .items
                .iter()
                .filter(|item| kind.is_empty() || s(item, "kind") == kind)
                .count();
            let text = format!("{} {}", tr(label), count);
            if tree_row(ui, active == filter, 0.0, None, &text, 12.5).clicked() {
                self.trash.stack.pop_to(0);
                if let TrashFrame::List(state) = self.trash.stack.top_mut() {
                    state.filter = filter;
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
                "showroom" => tr("演示成品"),
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
        let (rect, resp) =
            ui.allocate_exact_size(egui::vec2(width, 66.0), egui::Sense::click());
        let painter = ui.painter();
        let fill = if resp.hovered() { palette_color(HOVER_BG) } else { palette_color(CARD_BG) };
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
        if self.token.is_none() {
            // Explicit not-connected state (not an error): the trash
            // lives on the server. Local work is unaffected, and the
            // page says exactly what connecting unlocks.
            ui.label(egui::RichText::new(tr("回收站")).size(18.0).color(palette_color(TEXT_MAIN)));
            ui.add_space(16.0);
            card_frame().show(ui, |ui| {
                ui.label(
                    egui::RichText::new(tr("回收站保存的是服务器上的已删条目"))
                        .size(13.0)
                        .color(palette_color(TEXT_MAIN)),
                );
                ui.label(
                    egui::RichText::new(tr(
                        "连接并登录服务器后可查看、恢复或彻底删除；未连接不影响本地的索引、内容与演示功能。",
                    ))
                    .size(11.5)
                    .color(palette_color(TEXT_WEAK)),
                );
                ui.add_space(8.0);
                if btn_primary(ui, tr("登录")).clicked() {
                    self.login_open = true;
                }
            });
            return;
        }
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
        match self.call("GET", "/v1/trash", None) {
            Ok(v) => {
                self.trash.items = v
                    .get("items")
                    .and_then(|x| x.as_array())
                    .cloned()
                    .unwrap_or_default();
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
        match self.call(
            "POST",
            "/v1/trash",
            Some(json!({ "kind": kind, "object_id": object_id })),
        ) {
            Ok(_) => {
                self.msg = "已移入回收站，可随时恢复".into();
                self.trash.loaded = false;
                self.refresh_all();
                true
            }
            Err(e) => {
                self.msg = format!("删除失败：{e}");
                false
            }
        }
    }

    fn trash_restore(&mut self, trash_id: &str) {
        match self.call("POST", &format!("/v1/trash/{trash_id}/restore"), Some(json!({}))) {
            Ok(_) => {
                self.msg = "已恢复".into();
                self.trash.loaded = false;
                self.refresh_all();
            }
            Err(e) => self.msg = format!("恢复失败：{e}"),
        }
    }

    fn trash_purge(&mut self, trash_id: &str) {
        match self.call("POST", &format!("/v1/trash/{trash_id}/purge"), Some(json!({}))) {
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
                self.refresh_all();
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
            .filter(|t| match filter {
                1 => s(t, "kind") == "index",
                2 => s(t, "kind") == "unit",
                3 => s(t, "kind") == "showroom",
                _ => true,
            })
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
                ui.label(egui::RichText::new(tr("回收站")).size(18.0).color(palette_color(TEXT_MAIN)));
                ui.add_space(12.0);
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
                        egui::RichText::new(format!("{} {} {}", tr("已选"), selected.len(), tr("项")))
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
                        egui::RichText::new(format!("{}：{}", tr("回收站加载失败"), self.trash.load_error))
                            .size(12.5)
                            .color(palette_color(TEXT_MAIN)),
                    );
                    if btn_primary(ui, tr("重试")).clicked() {
                        retry = true;
                    }
                });
                ui.add_space(10.0);
            }
            if shown.is_empty() {
                card_frame().show(ui, |ui| {
                    ui.label(
                        egui::RichText::new(if items.is_empty() {
                            tr("回收站是空的——删除的内容块和索引会先放进这里，可随时恢复。")
                        } else {
                            tr("这个板块暂无已删除项。")
                        })
                        .size(12.5)
                        .color(palette_color(TEXT_WEAK)),
                    );
                });
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
                    _ => (tr("演示成品"), BadgeTone::Neutral),
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
                                egui::RichText::new(&title).size(13.0).color(palette_color(TEXT_MAIN)),
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
                                if !cover_vid.is_empty()
                                    && btn_default(ui, tr("预览")).clicked()
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
            self.trash.stack.push(TrashFrame::Preview(TrashPreviewState {
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
                        .call("POST", &format!("/v1/trash/{id}/restore"), Some(json!({})))
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
                self.refresh_all();
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
        let (trash_id, kind, title, version_id, page_count, page) =
            match self.trash.stack.top() {
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
            if let Some(d) = crumb_bar(ui, &[tr("回收站"), &title]) {
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
                    _ => tr("演示成品"),
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
            // standalone before the owner is adopted): only the
            // local sections — connecting a
            // server (and logging in there) plus the on-device
            // offline cache. Identity-bound sections appear after
            // login, per role, as before.
            return vec!["服务器", "离线缓存", "演示偏好"];
        }
        let role = self.role();
        let mut sections: Vec<&str> = vec!["服务器", "个人资料"];
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

    /// The status surface: no status bar — a floating toast where
    /// the prototype puts it: top-right, 14px below the 100px
    /// chrome (top = --chrome-h + 14 = 114), --panel3 fill with a
    /// --line2 outline, 8px radius, 13px text, gone after 3.4
    /// seconds. `msg` remains the client's only message channel;
    /// the toast stamps it when it changes.
    fn toast(&mut self, ui: &mut egui::Ui, _screen: egui::Rect) {
        let now = ui.ctx().input(|i| i.time);
        if self.msg != self.toast_msg {
            self.toast_msg = self.msg.clone();
            self.toast_since = now;
            self.workspace_controls.record_message(&self.msg);
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
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
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
                .unwrap_or(ThumbState::Failed);
            self.thumbs.insert(key, state);
            ctx.request_repaint();
        }
        // Collect finished PDF page renders for the attachment preview.
        while let Ok((att_id, page, bytes, res)) = self.pdf_rx.try_recv() {
            if let Some(v) = &mut self.pdf_view {
                if v.att_id == att_id {
                    if bytes.is_some() {
                        v.bytes = bytes;
                    }
                    match res {
                        Ok(img) => {
                            v.pages = img.pages;
                            v.page = page.min(img.pages.saturating_sub(1));
                            let ci = egui::ColorImage::from_rgba_unmultiplied(
                                [img.w, img.h],
                                &img.rgba,
                            );
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
        // Collect AI-translation progress / completion.
        while let Ok(ev) = self.translate_rx.try_recv() {
            match ev {
                TranslateEvent::Progress(d, t) => {
                    if let Some(tr) = &mut self.translating {
                        tr.1 = d;
                        tr.2 = t;
                    }
                }
                TranslateEvent::Done(res) => {
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
                            self.msg = m;
                            if let Some((uid, _, _)) = finished {
                                self.open_unit_detail(&uid);
                            }
                        }
                        Err(e) => self.msg = format!("翻译失败：{e}"),
                    }
                }
            }
            ctx.request_repaint();
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
    }
}

impl App {
    /// Real slide thumbnail, fetched on a background thread (the server
    /// renders it once via LibreOffice and caches it on disk).
    fn thumb(&mut self, version_id: &str, page: usize) -> Option<egui::TextureHandle> {
        if version_id.is_empty() {
            return None;
        }
        let key = format!("{version_id}:{page}");
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
                let url = format!("{server}/v1/versions/{version_id}/thumb/{page}");
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
                            v.get("image_base64")
                                .and_then(|x| x.as_str())
                                .map(str::to_string)
                        })
                        .and_then(|s| b64d(&s).ok());
                    let _ = tx.send((key, out));
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
    fn switch_entry_version(&mut self, pos: usize, version_id: &str, semver: &str, is_latest: bool) {
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
        let r = self.call(
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
                        let _ = std::fs::write(&path, bytes);
                        self.msg = format!(
                            "已导出 {} 页（{}）",
                            v.get("page_count").and_then(|x| x.as_i64()).unwrap_or(0),
                            if kind == "company" { "公用版" } else { "个人版" }
                        );
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
                save_cache_index(&self.cache_entries);
                self.msg = format!("已缓存到本地（加密）：{title}");
            }
            Err(e) => self.msg = format!("缓存失败：{e}"),
        }
    }

    fn remove_cache(&mut self, key: &str) {
        if let Some(e) = self
            .cache_entries
            .iter()
            .find(|e| s(e, "key") == key)
            .cloned()
        {
            let _ = std::fs::remove_file(cache_dir().join(s(&e, "file")));
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
            let data = match std::fs::read(&stored).ok().and_then(|d| decrypt_bytes(&ck, &d)) {
                Some(d) => d,
                None => {
                    self.msg = "缓存解密失败".into();
                    return;
                }
            };
            let open_dir = cache_dir().join("open");
            let _ = std::fs::create_dir_all(&open_dir);
            let p = open_dir.join(format!("{}.pptx", s(&e, "title")));
            if std::fs::write(&p, data).is_err() {
                self.msg = "写出临时副本失败".into();
                return;
            }
            p
        } else {
            stored
        };
        // Local decks jump into DeckCraft when it is available
        // (offline presenting is a handoff capability gain); the
        // system app stays as the fallback.
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
        if let Ok(rd) = std::fs::read_dir(&dir) {
            for e in rd.flatten() {
                let p = e.path();
                if p != path && p.extension().map(|x| x == "pptx").unwrap_or(false) {
                    let _ = std::fs::remove_file(p);
                }
            }
        }
        Ok((path, page_count))
    }

    /// Jump a showroom item into DeckCraft (present handoff stages
    /// 1–2). When no DeckCraft desktop app is available on this
    /// machine, the user is prompted to install DeckCraft separately
    /// (ruling 2026-11-24) and the jump counts as handled. Export
    /// failures are reported here and count as handled: permission
    /// refusals arrive as export errors, so a refused item never
    /// jumps (contract §2.2).
    fn present_via_deckcraft(
        &mut self,
        item_id: &str,
        title: &str,
        mode: DcMode,
        from: usize,
    ) -> bool {
        if deckcraft_app_path().is_none() {
            // User ruling 2026-11-24: the client installer does not
            // bundle DeckCraft; when a jump finds no DeckCraft on this
            // machine, prompt the user to install it separately
            // instead of silently falling back to the built-in player.
            self.msg = "未检测到 DeckCraft 桌面应用：播放、普通视图和演示者视图由 DeckCraft 提供，请先安装 DeckCraft 后再试".into();
            return true;
        }
        match self.export_for_present(item_id) {
            Ok((path, page_count)) => {
                let from = if page_count > 0 {
                    from.min(page_count as usize - 1)
                } else {
                    0
                };
                self.dc_jump(&path, title, mode, from);
                true
            }
            Err(e) => {
                self.msg = format!("演示失败：{e}");
                true
            }
        }
    }

    /// Jump an already-local deck file into DeckCraft (offline
    /// cached decks). Same availability contract as above.
    fn present_local_via_deckcraft(&mut self, path: &std::path::Path, title: &str) -> bool {
        if deckcraft_app_path().is_none() {
            return false;
        }
        self.dc_jump(path, title, DcMode::Play, 0);
        true
    }

    /// Orchestrate one jump into the managed DeckCraft instance
    /// (design §2.3): reuse the instance when its control channel
    /// answers, otherwise launch it with `--control` on a free port;
    /// if the channel never comes up, degrade to a plain
    /// `deckcraft --show` launch and say so. The jump sequence is
    /// `app.open` → presenter flag for this mode → `show.start` →
    /// `ui.focus`; the presenter flag is set explicitly every jump
    /// because it is a persisted DeckCraft preference.
    fn dc_jump(&mut self, path: &std::path::Path, title: &str, mode: DcMode, from: usize) {
        let Some(bin) = deckcraft_app_path() else {
            self.msg = "未检测到 DeckCraft 桌面应用：播放、普通视图和演示者视图由 DeckCraft 提供，请先安装 DeckCraft 后再试".into();
            return;
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
                                u.get("presenterView")
                                    .or_else(|| u.get("presenter_view"))
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
                return;
            };
            let child = match std::process::Command::new(&bin)
                .arg("--control")
                .arg(port.to_string())
                .spawn()
            {
                Ok(c) => c,
                Err(e) => {
                    self.msg = format!("无法启动 DeckCraft：{e}");
                    return;
                }
            };
            match dc_wait_ready(port) {
                Some((ctl, v)) => {
                    let presenter_before = v
                        .get("ui")
                        // UiState is #[serde(rename_all = "camelCase")]:
                        // the wire key is presenterView; keep the
                        // snake_case spelling as a fallback.
                        .and_then(|u| {
                            u.get("presenterView")
                                .or_else(|| u.get("presenter_view"))
                        })
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
                    match std::process::Command::new(&bin)
                        .arg("--show")
                        .arg(path)
                        .spawn()
                    {
                        Ok(_) => {
                            self.msg =
                                "已以降级模式在 DeckCraft 打开演示（控制通道不可用）".into()
                        }
                        Err(e) => self.msg = format!("无法启动 DeckCraft：{e}"),
                    }
                    return;
                }
            }
        }
        let (mut ctl, _inspect) = live.unwrap();
        let port = self.dc.as_ref().map(|s| s.port).unwrap_or(0);
        let run = (|| -> Result<(), String> {
            ctl.call("app.open", json!({ "path": path.to_string_lossy().into_owned() }))?;
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
        match run {
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
                    let gen = self.dc_gen.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
                    let gen_arc = self.dc_gen.clone();
                    let tx = self.dc_tx.clone();
                    std::thread::spawn(move || dc_watch(port, gen_arc, gen, tx));
                }
            }
            Err(e) => {
                self.msg = format!("DeckCraft 跳转失败：{e}");
            }
        }
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
                DcEvent::ShowEnded => {
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
                DcEvent::Gone => {
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
            self.showroom.stack.push(SrFrame::Deck(DeckState {
                item: json!({
                    "id": item_id, "title": title,
                    "source_index_id": "", "submitted_by": "",
                }),
                pages: Vec::new(),
                selected_page: None,
                view: DeckView::Thumbnails,
                preview_open: false,
                closed: false,
            }));
        }
    }

    /// Open a showroom entry's whole-deck view (H): fetch the source
    /// index so its resolved entries can be paged through.
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
        let src = s(item, "source_index_id");
        if !src.is_empty() {
            if let Ok(v) = self.call("GET", &format!("/v1/indexes/{src}"), None) {
                if let SrFrame::Deck(d) = self.showroom.stack.top_mut() {
                    d.pages = showroom_pages(&v);
                }
            }
        }
    }

    /// Icon-only action button: an IconPark glyph (20px) inside a 30px
    /// hot zone, with a hover tooltip and an accessibility label — an
    /// icon button is never shipped without its name (hard floor).
    fn icon_button(
        &mut self,
        ui: &mut egui::Ui,
        icon: ChromeIcon,
        label: &str,
    ) -> bool {
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
            let ci = egui::ColorImage::from_rgba_unmultiplied(
                [w as usize, h as usize],
                rgba.as_raw(),
            );
            let tex = ui.ctx().load_texture(name, ci, egui::TextureOptions::LINEAR);
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
        let resp = craft_ui::buttons::IconButton::new(label, egui::vec2(30.0, 30.0), &style)
            .show(ui, |ui, rect, icon_tint| {
                ChromeIcons.paint(ui, icon, rect.center(), 20.0, icon_tint);
            });
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
        self.tool_slot_sized(ui, active, label, icon, 26.0).clicked()
    }

    fn tool_slot_sized(
        &mut self,
        ui: &mut egui::Ui,
        active: bool,
        label: &str,
        icon: ChromeIcon,
        height: f32,
    ) -> egui::Response {
        let visuals = |fill: egui::Color32, icon: egui::Color32| {
            craft_ui::buttons::ButtonVisuals {
                fill,
                stroke: egui::Stroke::NONE,
                icon,
            }
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

    /// Panel-footer icon button: 28×28 craft-ui IconButton with a
    /// 15px glyph — the prototype's .sp-foot action scale (28px
    /// rows, 15px glyphs, 6px radius).
    fn small_icon_button(
        &mut self,
        ui: &mut egui::Ui,
        icon: ChromeIcon,
        label: &str,
    ) -> bool {
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
        let resp = craft_ui::buttons::IconButton::new(label, egui::vec2(28.0, 28.0), &style)
            .show(ui, |ui, rect, icon_tint| {
                ChromeIcons.paint(ui, icon, rect.center(), 15.0, icon_tint);
            });
        resp.on_hover_text(label).clicked()
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
                        let _ = std::fs::write(path, bytes);
                        self.msg = "已保存".into();
                    }
                }
            }
            Err(e) => self.msg = format!("导出 PDF 失败：{e}"),
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
        let already_cached = self.cache_entries.iter().any(|e| s(e, "key") == cache_key);
        ui.horizontal(|ui| {
            ui.label(
                egui::RichText::new(s(&item, "title"))
                    .strong()
                    .size(18.0)
                    .color(palette_color(TEXT_MAIN)),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.spacing_mut().item_spacing.x = 4.0;
                let (cache_label, cache_tint) = if already_cached {
                    ("已缓存（点击更新缓存）", palette_color(TEXT_WEAK))
                } else {
                    ("缓存到本地", palette_color(ICON_TINT))
                };
                if self.icon_button_tinted(
                    ui,
                    ChromeIcon::Download,
                    cache_label,
                    cache_tint,
                ) {
                    act = Some("cache");
                }
                if self.icon_button(ui, ChromeIcon::Export, "导出") {
                    act = Some("export");
                }
                if self.icon_button(ui, ChromeIcon::Play, "开始演示") {
                    act = Some("play");
                }
                if ChromeIcons.tool(ui, ChromeIcon::Presenter, "演示者视图", true, false, true).clicked() {
                    act = Some("presenter");
                }
                if ChromeIcons.tool(ui, ChromeIcon::Reading, "普通视图", true, false, true).clicked() {
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
        ui.label(
            egui::RichText::new(format!(
                "{} · {}",
                if s(&item, "source_index_id").is_empty() {
                    "个人组装".to_string()
                } else if src_name.is_empty() {
                    "来源索引".to_string()
                } else {
                    format!("来源索引 {src_name}")
                },
                s(&item, "submitted_by")
            ))
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
            Some("export") => {
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
                                let _ = std::fs::write(path, bytes);
                                self.msg = "已保存".into();
                            }
                        }
                    }
                    Err(e) => self.msg = format!("导出失败：{e}"),
                }
            }
            Some("cache") => self.cache_showroom_deck(&item),
            _ => {}
        }
        if let Some((form, page)) = pdf_act {
            self.export_showroom_pdf(&item, form, page);
        }
        ui.add_space(8.0);
        self.showroom_deck_pages(ui, st, &item);
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
            ui.label(egui::RichText::new(format!("{} 页", st.pages.len())).color(palette_color(TEXT_WEAK)));
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

    fn cache_showroom_deck(&mut self, item: &Value) {
        let source = s(item, "source_index_id");
        if source.is_empty() {
            self.msg = "该条目暂不支持缓存到本地".into();
            return;
        }
        match self.call(
            "POST",
            &format!("/v1/indexes/{source}/export"),
            Some(json!({ "branch": "main", "kind": "personal" })),
        ) {
            Ok(data) => {
                if let Ok(bytes) = b64d(&s(&data, "data_base64")) {
                    self.cache_deck(
                        &format!("sr_{}", s(item, "id")),
                        &s(item, "title"),
                        &bytes,
                        i(&data, "page_count"),
                    );
                }
            }
            Err(e) => self.msg = format!("缓存失败：{e}"),
        }
    }

    fn library_view(&mut self, ui: &mut egui::Ui) {
        let section = self.showroom.library_view;
        egui::ScrollArea::vertical()
            .id_salt("showroom-library")
            .show(ui, |ui| {
                if section == 0 || section == 1 {
                    ui.heading("收藏");
                    self.showroom_cards(
                        ui,
                        &self.library_refs.clone(),
                        "暂无收藏",
                        |app, ui, e, width| {
                            let cover = app.showroom_cover(&s(e, "item_id"));
                            app.showroom_card(ui, width, &s(e, "title"), &cover, false, |app, ui| {
                                let folder = s(&e, "folder");
                                if !folder.is_empty() {
                                    ui.label(egui::RichText::new(folder).small().weak());
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
                                                            let _ = std::fs::write(&path, bytes);
                                                            app.msg = "已导出".into();
                                                        }
                                                    }
                                                }
                                                Err(e2) => app.msg = format!("导出失败：{e2}"),
                                            }
                                        }
                                    }
                                }
                                if ui.small_button("移除").clicked() {
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
                    ui.heading("本地缓存");
                    self.showroom_cards(
                        ui,
                        &self.cache_entries.clone(),
                        "暂无缓存",
                        |app, ui, e, width| {
                            app.showroom_card(ui, width, &s(e, "title"), "", false, |app, ui| {
                                ui.label(format!(
                                    "{} 页 · {}",
                                    i(&e, "page_count"),
                                    fmt_time(i(&e, "cached_ms"))
                                ));
                                let key = s(&e, "key");
                                if ui.button("打开").clicked() {
                                    app.open_cache(&key);
                                }
                                if ui.button("移除缓存").clicked() {
                                    app.remove_cache(&key);
                                }
                            });
                        },
                    );
                }
                if section == 0 || section == 3 {
                    ui.heading("我的组装");
                    self.showroom_cards(
                        ui,
                        &self.my_assemblies.clone(),
                        "暂无组装",
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
                    "这个来源下暂时没有分享。",
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
            ui.horizontal_wrapped(|ui| {
                if item["item_kind"] == "showroom" && ui.small_button("打开详情").clicked() {
                    app.open_deck_for_ref("showroom", &id, &title);
                }
                if saved {
                    ui.label(egui::RichText::new("已在我的库").small().weak());
                } else if ui.small_button("保存到我的库").clicked() {
                    let sid = s(item, "id");
                    match app.call("POST", &format!("/v1/shares/{sid}/accept"), Some(json!({}))) {
                        Ok(_) => {
                            app.msg = "已保存到我的库".into();
                            app.refresh_all();
                        }
                        Err(e) => app.msg = format!("保存失败：{e}"),
                    }
                }
            });
        });
        let response = ui.interact(
            cover_response,
            ui.scope_id().with(("share", s(item, "id"))),
            egui::Sense::click(),
        );
        if response.double_clicked() && item["item_kind"] == "showroom" {
            self.open_deck_for_ref("showroom", &id, &title);
        }
    }

    fn groups_view(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.label("新建群组");
            ui.text_edit_singleline(&mut self.new_team_name);
            if ui
                .add_enabled(
                    !self.new_team_name.trim().is_empty(),
                    egui::Button::new("建组"),
                )
                .clicked()
            {
                let name = self.new_team_name.clone();
                match self.call("POST", "/v1/custom-groups", Some(json!({ "name": name }))) {
                    Ok(_) => {
                        self.new_team_name.clear();
                        self.msg = "群组已创建".into();
                        self.refresh_all();
                    }
                    Err(e) => self.msg = format!("建组失败：{e}"),
                }
            }
        });
        ui.add_space(6.0);
        let users = self
            .org
            .get("users")
            .and_then(|u| u.as_array())
            .cloned()
            .unwrap_or_default();
        let Some(space) = self.active_showroom_group().cloned() else {
            ui.label(egui::RichText::new("还没加入任何群组").weak());
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
                ui.heading("群组共享空间");
                let items = space["items"]
                    .as_array()
                    .map(Vec::as_slice)
                    .unwrap_or_default();
                self.showroom_cards(
                    ui,
                    items,
                    "暂无分享到本组的内容",
                    Self::showroom_shared_card,
                );
            });
    }

    /// Showroom content slot, including the optional review queue.
    fn sr_main_body(&mut self, ui: &mut egui::Ui) {
        if self.showroom.zone == "plaza"
            && matches!(self.showroom.stack.top(), SrFrame::Wall)
            && self.role() == "super"
            && !self.showroom_pending.is_empty()
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
                    for item in self.showroom_pending.clone() {
                        ui.horizontal(|ui| {
                            ui.label(egui::RichText::new(s(&item, "title")).strong());
                            ui.label(format!("提交人 {}", s(&item, "submitted_by")));
                            if ui.button("通过").clicked() && self.need_identity("审批演示厅投稿") {
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
                                if let Some(pos) =
                                    sections.iter().position(|x| *x == "审批队列")
                                {
                                    self.settings.section = pos;
                                }
                                self.review.stack.push(ReviewFrame::Detail(
                                    ReviewDetailState {
                                        item_id: s(&item, "id"),
                                        reject_open: true,
                                        reject_text: String::new(),
                                        deck: None,
                                        deck_loaded: false,
                                    },
                                ));
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
        all.sort_by_key(|x| -i(x, "reviewed_ms"));
        self.review.records = all;
        self.review.records_loaded = true;
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
        if let Some(t) = segmented(
            ui,
            &[tr("待审批"), tr("已处理记录")],
            tab,
            SegStyle::Soft,
        ) {
            new_tab = Some(t);
        }
        ui.add_space(10.0);
        let mut open_detail: Option<(String, bool)> = None;
        let mut approve: Option<String> = None;
        if tab == 0 {
            let pending = self.showroom_pending.clone();
            if pending.is_empty() {
                ui.label(
                    egui::RichText::new(tr("暂无待审批的演示成品。新提交会先进入这里，人工通过后才公开展示。"))
                        .size(12.5)
                        .color(palette_color(TEXT_WEAK)),
                );
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
                        slide_cover(ui, tex, tr("演示成品"), &title, "", 84.0);
                        ui.vertical(|ui| {
                            ui.label(
                                egui::RichText::new(&title).size(13.0).color(palette_color(TEXT_MAIN)),
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
        } else {
            let records = self.review.records.clone();
            if records.is_empty() {
                ui.label(
                    egui::RichText::new(tr("暂无已处理记录。"))
                        .size(12.5)
                        .color(palette_color(TEXT_WEAK)),
                );
            }
            for item in &records {
                let rejected = s(item, "state") == "rejected";
                card_frame().show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.label(
                            egui::RichText::new(s(item, "title"))
                                .size(13.0)
                                .color(palette_color(TEXT_MAIN)),
                        );
                        badge(
                            ui,
                            if rejected { tr("已驳回") } else { tr("已通过") },
                            if rejected { BadgeTone::Bad } else { BadgeTone::Ok },
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
                });
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
            self.review.stack.push(ReviewFrame::Detail(ReviewDetailState {
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
                        egui::RichText::new(s(it, "title")).size(14.0).color(palette_color(TEXT_MAIN)),
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
                    ui.label(egui::RichText::new(intro).size(12.0).color(palette_color(TEXT_WEAK)));
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

    fn showroom_body(&mut self, ui: &mut egui::Ui) {
        // Frame-state take-out, same pattern as content/index.
        // The deck view carries its own header (title + icon
        // actions); no "演示厅" heading above it (James 2026-11-15).
        if matches!(self.showroom.stack.top(), SrFrame::Wall) {
            self.showroom_wall(ui);
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
        let title = match self.showroom.zone.as_str() {
            "library" => "我的",
            "inbox" => "分享",
            "groups" => "群组",
            _ => "公共",
        };
        ui.heading(title);
        ui.add_space(6.0);
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
        let items = self
            .showroom_live
            .iter()
            .filter(|item| {
                self.folder_filter_ok(
                    &self.showroom.folder,
                    &self.index_folder_of(&s(item, "source_index_id")),
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
                    "这个目录下暂时没有符合的演示稿。",
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
            ui.horizontal_wrapped(|ui| {
                if ui.small_button("打开详情").clicked() {
                    app.open_showroom_deck(item);
                }
                if ui.small_button("开始演示").clicked() {
                    // The deck frame stays as the return context;
                    // presenting now jumps into DeckCraft (the
                    // built-in presenter is the fallback when no
                    // DeckCraft desktop app is on this machine).
                    app.open_showroom_deck(item);
                    if !app.present_via_deckcraft(&id, &title, DcMode::Play, 0) {
                        app.start_present_from_deck(ui);
                    }
                }
                if !s(item, "source_index_id").is_empty() {
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
                if app.role() == "super"
                    || app
                        .user
                        .as_ref()
                        .is_some_and(|u| s(u, "display_name") == s(item, "submitted_by"))
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
        if response.clicked() {
            self.showroom.selected_card = Some(id);
        }
        if response.double_clicked() {
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
        empty: &str,
        mut card: impl FnMut(&mut Self, &mut egui::Ui, &Value, f32),
    ) {
        if items.is_empty() {
            ui.label(egui::RichText::new(empty).color(palette_color(TEXT_WEAK)));
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
        self.wall_card(ui, WallCard { width, min_height: 0.0, title, cover, selected, scope: None }, body).inner
    }

    fn wall_card(
        &mut self,
        ui: &mut egui::Ui,
        card: WallCard<'_>,
        body: impl FnOnce(&mut Self, &mut egui::Ui),
    ) -> egui::InnerResponse<egui::Rect> {
        standard_card(ui, card.selected, card.scope, true, |ui| {
            ui.set_width(card.width);
            ui.set_min_height(card.min_height);
            ui.spacing_mut().item_spacing.y = 0.0;
            let cover_rect = ui.allocate_ui(egui::vec2(card.width, card.width * 9.0 / 16.0), |ui| {
                if card.cover.is_empty() {
                    slide_cover(ui, None, "", "", "", card.width);
                } else {
                    self.thumb_ui(ui, card.cover, 0, card.width);
                }
            }).response.rect;
            egui::Frame::NONE.inner_margin(egui::Margin::symmetric(12, 10)).show(ui, |ui| {
                ui.spacing_mut().item_spacing = egui::vec2(8.0, 5.0);
                ui.add(egui::Label::new(egui::RichText::new(card.title).strong().size(13.0)).truncate());
                body(self, ui);
            });
            cover_rect
        })
    }

    fn act_import_split(&mut self) {
        if !self.need_identity("智能分块导入") {
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
                match self.call(
                    "POST",
                    "/v1/import/propose",
                    Some(json!({ "data_base64": b64e(&bytes) })),
                ) {
                    Ok(p) => {
                        let blocks = p.get("blocks").cloned().unwrap_or(json!([]));
                        let names: Vec<String> = blocks
                            .as_array()
                            .map(|arr| arr.iter().map(|b| s(b, "name")).collect())
                            .unwrap_or_default();
                        let r = self.call(
                            "POST",
                            "/v1/import",
                            Some(json!({
                                "filename": name,
                                "data_base64": b64e(&bytes),
                                "folder_id": self.content.folder_filter,
                                "group_id": self.content.import_group,
                                "blocks": blocks,
                            })),
                        );
                        match r {
                            Ok(_) => {
                                self.msg = format!(
                                    "智能分块导入完成：{} 块（{}）",
                                    names.len(),
                                    names.join("、")
                                );
                                self.refresh_all();
                            }
                            Err(e) => self.msg = format!("导入失败：{e}"),
                        }
                    }
                    Err(e) => self.msg = format!("提块失败：{e}"),
                }
            }
        }
    }

    /// 导入 PPT (content page action, also its toolbar slot): import
    /// a whole deck into the current folder/group.
    fn act_import_ppt(&mut self) {
        if !self.need_identity("导入 PPT") {
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
                let r = self.call(
                    "POST",
                    "/v1/import",
                    Some(json!({
                        "filename": name,
                        "data_base64": b64e(&bytes),
                        "folder_id": self.content.folder_filter,
                        "group_id": self.content.import_group,
                    })),
                );
                match r {
                    Ok(v) => {
                        self.msg = format!(
                            "导入完成：{} 页，校验通过",
                            v.get("slide_count").and_then(|x| x.as_i64()).unwrap_or(0)
                        );
                        self.refresh_all();
                    }
                    Err(e) => self.msg = format!("导入失败：{e}"),
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
                body_frame().show(ui, |ui| {
                    if self.pick.is_some() {
                        self.pick_bar(ui);
                    }
                    self.unit_detail(ui, st);
                });
                st.closed
            }
            ContentFrame::Wall(w) => {
                self.content_wall(ui, w);
                false
            }
        };
        if matches!(self.content.stack.top(), ContentFrame::Unit(_)) {
            // Opening from the wall pushed a detail above the placeholder.
            // Restore the wall itself, including its search and selection.
            if let ContentFrame::Wall(mut wall) = frame {
                let root = self.content.stack.root_mut();
                if let ContentFrame::Wall(placeholder) = root {
                    wall.selected = placeholder.selected.take().or(wall.selected);
                }
                *root = ContentFrame::Wall(wall);
            }
        } else if self.content.stack.depth() == depth {
            if closed {
                self.content.stack.pop();
            } else {
                self.content.stack.replace_top(frame);
            }
        }
    }

    /// The maintenance wall (root frame body): search, the import
    /// toolbar and the block card grid.
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
            let resp = ui.add(
                egui::TextEdit::singleline(&mut w.search_text)
                    .hint_text("搜索")
                    .desired_width(300.0)
                    .margin(egui::vec2(14.0, 7.0)),
            );
            if resp.lost_focus() && ui.input(|inp| inp.key_pressed(egui::Key::Enter)) {
                match self.call("GET", &format!("/v1/search?q={}", query_component(&w.search_text)), None) {
                    Ok(v) => w.search_hits = Some(v),
                    Err(e) => self.msg = e,
                }
            }
            if text_action(ui, "智能分块导入", palette_color(TEXT_MAIN), 13.0) {
                self.act_import_split();
            }
            if text_action(ui, "导入 PPT", palette_color(TEXT_MAIN), 13.0) {
                self.act_import_ppt();
            }
            ui.label(
                egui::RichText::new("导入到组")
                    .size(13.0)
                    .color(palette_color(TEXT_WEAK)),
            );
            let cur_group = self.group_name(&self.content.import_group.clone());
            egui::ComboBox::from_id_salt("import_group")
                .width(150.0)
                .selected_text(cur_group)
                .show_ui(ui, |ui| {
                    for g in self.groups.clone() {
                        ui.selectable_value(
                            &mut self.content.import_group,
                            s(&g, "id"),
                            s(&g, "name"),
                        );
                    }
                });
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
        egui::ScrollArea::vertical().show(ui, |ui| {
            ui.spacing_mut().item_spacing = egui::vec2(12.0, 12.0);
            // Card grid (prototype .grid): fluid columns, min
            // 220px wide, 12px gaps.
            let avail = ui.available_width();
            let per_row = ((avail + 12.0) / 232.0).floor().max(1.0);
            let card_w = (avail + 12.0) / per_row - 12.0;
            let cover_h = card_w * 9.0 / 16.0;
            let card_h = cover_h + 89.0;
            ui.horizontal_wrapped(|ui| {
                for item in self.units.clone() {
                    let unit = item.get("unit").cloned().unwrap_or(json!({}));
                    if let Some(f) = &self.content.folder_filter {
                        if unit.get("folder_id").and_then(|x| x.as_str()) != Some(f.as_str()) {
                            continue;
                        }
                    }
                    let uid = s(&unit, "id");
                    let rends = item.get("renditions").and_then(|r| r.as_array()).cloned().unwrap_or_default();
                    let latest = rends
                        .first()
                        .and_then(|r| r.get("latest"))
                        .cloned()
                        .unwrap_or(Value::Null);
                    let title = s(&unit, "name");
                    let cover = s(&latest, "id");
                    let outer = self.wall_card(ui, WallCard {
                        width: card_w, min_height: card_h, title: &title, cover: &cover,
                        selected: w.selected.as_deref() == Some(uid.as_str()), scope: None,
                    }, |app, ui| {
                        ui.horizontal(|ui| {
                            if latest.is_object() {
                                chip(
                                    ui,
                                    &format!("v{}", s(&latest, "semver")),
                                    palette_color(ACCENT_DEEP),
                                    CHIP_BLUE_BG,
                                );
                                ui.label(
                                    egui::RichText::new(format!(
                                        "{} 页",
                                        i(&latest, "page_count")
                                    ))
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
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    ui.label(
                                        egui::RichText::new(format!(
                                            "{} · {}",
                                            app.folder_name(
                                                unit.get("folder_id")
                                                    .and_then(|x| x.as_str())
                                                    .unwrap_or("")
                                            ),
                                            s(&unit, "maintainer")
                                        ))
                                        .small()
                                        .color(palette_color(TEXT_WEAK)),
                                    );
                                },
                            );
                        });
                        if matches!(app.pick, Some(PickState::Insert { .. })) {
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
                                app.insert_pick_unit(&item);
                            }
                        }
                    });
                    // Click handling rides on an explicit interact over
                    // the card rect: allocate_ui's own response carries no
                    // click sense, which is why double-click never fired
                    // (James 2026-11-15). Single click selects, double
                    // click opens the maintenance drill-down — the same
                    // contract as the web prototype.
                    let click = ui.interact(
                        outer.response.rect,
                        egui::Id::unique(("unit_card", &uid)),
                        egui::Sense::click(),
                    );
                    if click.double_clicked() {
                        if self.pick.is_none() {
                            self.open_unit_detail(&uid);
                        }
                    } else if click.clicked() {
                        w.selected = Some(uid.clone());
                    }
                }
                // grid-end add card (I): same footprint as a content
                // card; two entries — blank block / paste PPT pages
                if self.pick.is_none() {
                    self.content_add_card(ui, card_w, card_h);
                }
            });
        });
        });
    }

    fn unit_detail(&mut self, ui: &mut egui::Ui, st: &mut UnitState) {
        let detail = st.detail.clone();
        let unit = detail.get("unit").cloned().unwrap_or(json!({}));
        ui.horizontal(|ui| {
            if ui.button("← 返回").clicked() {
                st.closed = true;
                self.refresh_all();
                return;
            }
            ui.heading(s(&unit, "name"));
            ui.label(format!(
                "内容修订 r{} · 维护人 {}",
                i(&unit, "current_revision"),
                s(&unit, "maintainer")
            ));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if btn_danger(ui, "删除内容块").clicked() {
                    let uid = s(&unit, "id");
                    self.trash_add("unit", &uid);
                    st.closed = true;
                }
            });
        });
        ui.separator();
        egui::ScrollArea::vertical().show(ui, |ui| {
            // pick-pages mode: the entry's currently resolved version,
            // page by page with checkboxes (initial state = the entry's
            // existing excerpt, all pages when there is none)
            let pick_vid = match &self.pick {
                Some(PickState::Pages { unit_id, version_id, .. })
                    if *unit_id == s(&unit, "id") =>
                {
                    Some(version_id.clone())
                }
                _ => None,
            };
            if let Some(vid) = pick_vid {
                let total = detail
                    .get("renditions")
                    .and_then(|r| r.as_array())
                    .and_then(|arr| {
                        arr.iter().find_map(|r| {
                            r.get("versions")
                                .and_then(|v| v.as_array())
                                .and_then(|vs| {
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
            if let Some(rends) = detail.get("renditions").and_then(|r| r.as_array()) {
                for r in rends.clone() {
                    let rend = r.get("rendition").cloned().unwrap_or(json!({}));
                    ui.label(
                        egui::RichText::new(format!(
                            "{} · {}",
                            s(&rend, "form"),
                            s(&rend, "lang")
                        ))
                        .strong(),
                    );
                    if let Some(versions) = r.get("versions").and_then(|v| v.as_array()) {
                        for v in versions {
                            ui.horizontal(|ui| {
                                let vid = s(v, "id");
                                self.thumb_ui(ui, &vid, 0, 96.0);
                                ui.label(format!(
                                    "v{} · {} 页 · {} · {}",
                                    s(v, "semver"),
                                    i(v, "page_count"),
                                    s(v, "author"),
                                    fmt_time(i(v, "created_ms"))
                                ));
                                if s(v, "status") == "recalled" {
                                    ui.label(
                                        egui::RichText::new("已撤回")
                                            .color(palette_color(ROSE)),
                                    );
                                }
                                if !s(v, "note").is_empty() {
                                    ui.label(s(v, "note"));
                                }
                                if ui.button("下载片段").clicked() {
                                    if let Ok(resp) = self.call(
                                        "GET",
                                        &format!("/v1/versions/{}/fragment", s(v, "id")),
                                        None,
                                    ) {
                                        if let Some(data) = resp.get("data_base64").and_then(|x| x.as_str()) {
                                            if let (Ok(bytes), Some(path)) = (
                                                b64d(data),
                                                rfd::FileDialog::new()
                                                    .set_file_name("fragment.pptx")
                                                    .save_file(),
                                            ) {
                                                let _ = std::fs::write(path, bytes);
                                                self.msg = "已保存".into();
                                            }
                                        }
                                    }
                                }
                                if ui.button("讲稿").clicked() {
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
                                    st.scripts_edit = scripts;
                                    st.scripts_for = Some(s(v, "id"));
                                }
                                if s(v, "status") != "recalled" && ui.button("撤回").clicked() {
                                    let _ = self.call(
                                        "POST",
                                        &format!("/v1/versions/{}/recall", s(v, "id")),
                                        None,
                                    );
                                    self.open_unit_detail(&st.unit_id.clone());
                                }
                                if s(v, "status") != "recalled" && ui.button("编辑页面").clicked() {
                                    // Page-level visual editing (overlay):
                                    // text runs + speaker notes of this
                                    // version; saving publishes a new
                                    // version under the §8 bump rule.
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
                                        unit_name: s(&unit, "name"),
                                        semver: s(v, "semver"),
                                        page_count: pages,
                                        original_page_count: pages,
                                        page: 0,
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
                            });
                        }
                    }
                }
            }
            ui.separator();
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new("AI 翻译（生成草稿）").strong());
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
                            ui.selectable_value(
                                &mut st.translate_to,
                                "zh".into(),
                                "中文",
                            );
                            ui.selectable_value(
                                &mut st.translate_to,
                                "en".into(),
                                "英语",
                            );
                            ui.selectable_value(
                                &mut st.translate_to,
                                "ja".into(),
                                "日语",
                            );
                        } else {
                            for (code, label) in self.lang_options.clone() {
                                ui.selectable_value(
                                    &mut st.translate_to,
                                    code,
                                    label,
                                );
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
                if ui.add_enabled(!busy && translate_ready, egui::Button::new(btn_text)).clicked() {
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
                                                    i(&p, "done") as usize,
                                                    i(&p, "total") as usize,
                                                ));
                                                match s(&p, "state").as_str() {
                                                    "succeeded" => {
                                                        let _ = tx.send(TranslateEvent::Done(
                                                            Ok(p.get("result")
                                                                .cloned()
                                                                .unwrap_or(Value::Null)),
                                                        ));
                                                        break;
                                                    }
                                                    "failed" => {
                                                        let _ = tx.send(TranslateEvent::Done(
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
                                                let _ = tx.send(TranslateEvent::Done(Err(e)));
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
                ui.separator();
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new("逐页讲稿（保存不升版本）").strong());
                    if ui.button("保存讲稿").clicked() {
                        let scripts = st.scripts_edit.clone();
                        match self.call(
                            "PUT",
                            &format!("/v1/versions/{vid}/scripts"),
                            Some(json!({ "scripts": scripts })),
                        ) {
                            Ok(_) => {
                                self.msg = "讲稿已保存".into();
                                self.open_unit_detail(&st.unit_id.clone());
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
            ui.separator();
            ui.label(egui::RichText::new("发布新版本").strong());
            ui.horizontal(|ui| {
                if ui.button("选择片段文件").clicked() {
                    st.publish_file = rfd::FileDialog::new()
                        .add_filter("PowerPoint", &["pptx"])
                        .pick_file();
                }
                if let Some(p) = &st.publish_file {
                    ui.label(p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default());
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
                .add_enabled(
                    st.publish_file.is_some(),
                    egui::Button::new("发布"),
                )
                .clicked()
            {
                if let Some(path) = st.publish_file.clone() {
                    let uid = st.unit_id.clone();
                    if let Ok(bytes) = std::fs::read(&path) {
                        let bump = ["major", "minor", "patch"][st.publish_bump];
                        let r = self.call(
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
                                self.open_unit_detail(&uid);
                            }
                            Err(e) => self.msg = format!("发布失败：{e}"),
                        }
                    }
                } else {
                    self.msg = "先选择片段文件".into();
                }
            }
            ui.separator();
            ui.label(egui::RichText::new("语言绑定（逐页）").strong());
            let mut all_versions: Vec<(String, String, i64)> = Vec::new();
            if let Some(rends) = detail.get("renditions").and_then(|r| r.as_array()) {
                for r in rends {
                    let rend = r.get("rendition").cloned().unwrap_or(json!({}));
                    if let Some(vs) = r.get("versions").and_then(|v| v.as_array()) {
                        for v in vs {
                            all_versions.push((
                                s(v, "id"),
                                format!(
                                    "{} · {} · v{}",
                                    s(&rend, "form"),
                                    s(&rend, "lang"),
                                    s(v, "semver")
                                ),
                                i(v, "page_count"),
                            ));
                        }
                    }
                }
            }
            if all_versions.len() >= 2 {
                if st.bind_from.is_empty() {
                    st.bind_from = all_versions[0].0.clone();
                }
                if st.bind_to.is_empty() {
                    st.bind_to = all_versions[1].0.clone();
                }
                ui.horizontal(|ui| {
                    ui.label("基准");
                    let cur = all_versions
                        .iter()
                        .find(|v| v.0 == st.bind_from)
                        .map(|v| v.1.clone())
                        .unwrap_or_default();
                    egui::ComboBox::from_id_salt("bind_from")
                        .selected_text(cur)
                        .show_ui(ui, |ui| {
                            for v in &all_versions {
                                ui.selectable_value(
                                    &mut st.bind_from,
                                    v.0.clone(),
                                    &v.1,
                                );
                            }
                        });
                    ui.label("对应");
                    let cur = all_versions
                        .iter()
                        .find(|v| v.0 == st.bind_to)
                        .map(|v| v.1.clone())
                        .unwrap_or_default();
                    egui::ComboBox::from_id_salt("bind_to")
                        .selected_text(cur)
                        .show_ui(ui, |ui| {
                            for v in &all_versions {
                                ui.selectable_value(&mut st.bind_to, v.0.clone(), &v.1);
                            }
                        });
                });
                let from_pages = all_versions
                    .iter()
                    .find(|v| v.0 == st.bind_from)
                    .map(|v| v.2)
                    .unwrap_or(0);
                let to_pages = all_versions
                    .iter()
                    .find(|v| v.0 == st.bind_to)
                    .map(|v| v.2)
                    .unwrap_or(0);
                ui.label("基准页（点选）：");
                ui.horizontal(|ui| {
                    for p in 0..from_pages {
                        ui.vertical(|ui| {
                            let vid = st.bind_from.clone();
                            self.thumb_ui(ui, &vid, p as usize, 84.0);
                            let selected = st.bind_from_slot == Some(p);
                            if ui
                                .selectable_label(selected, format!("第{}页", p + 1))
                                .clicked()
                            {
                                st.bind_from_slot = Some(p);
                            }
                        });
                    }
                });
                ui.label("对应页（点「绑定/已绑定」切换）：");
                ui.horizontal(|ui| {
                    for p in 0..to_pages {
                        ui.vertical(|ui| {
                            let vid = st.bind_to.clone();
                            self.thumb_ui(ui, &vid, p as usize, 84.0);
                            let existing = st.bindings.iter().find(|b| {
                                s(b, "from_version_id") == st.bind_from
                                    && st.bind_from_slot == Some(i(b, "from_slot"))
                                    && s(b, "to_version_id") == st.bind_to
                                    && i(b, "to_slot") == p
                            }).map(|b| s(b, "id"));
                            match (st.bind_from_slot, existing) {
                                (Some(_), Some(bid)) => {
                                    if ui
                                        .add(
                                            egui::Button::new(
                                                egui::RichText::new("已绑定 ✓")
                                                    .color(palette_color(CHIP_GREEN_FG)),
                                            )
                                            .fill(palette_color(CHIP_GREEN_BG))
                                            .corner_radius(6),
                                        )
                                        .clicked()
                                    {
                                        let uid = st.unit_id.clone();
                                            let _ = self.call(
                                                "POST",
                                                &format!("/v1/units/{uid}/page-bindings/{bid}/delete"),
                                                None,
                                            );
                                            self.open_unit_detail(&uid);
                                    }
                                }
                                (Some(fs), None) => {
                                    if ui.button("绑定").clicked() {
                                        let uid = st.unit_id.clone();
                                            let r = self.call(
                                                "POST",
                                                &format!("/v1/units/{uid}/page-bindings"),
                                                Some(json!({
                                                    "from_version_id": st.bind_from,
                                                    "from_slot": fs,
                                                    "to_version_id": st.bind_to,
                                                    "to_slot": p,
                                                })),
                                            );
                                            match r {
                                                Ok(_) => self.open_unit_detail(&uid),
                                                Err(e) => {
                                                    self.msg = format!("绑定失败：{e}")
                                                }
                                            }
                                    }
                                }
                                _ => {
                                    ui.label("先选基准页");
                                }
                            }
                        });
                    }
                });
                if !st.bindings.is_empty() {
                    ui.label(
                        egui::RichText::new(format!("已有绑定 {} 条", st.bindings.len()))
                            .small()
                            .weak(),
                    );
                }
            } else {
                ui.label(egui::RichText::new("至少两个版本后可做逐页绑定").weak());
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
    fn folder_tree(&mut self, ui: &mut egui::Ui, selected: &mut Option<String>) {
        ui.spacing_mut().item_spacing.y = 2.0;
        enum Act {
            NewChild(String),
            NewSibling(Option<String>),
            Rename(String, String),
            Delete(String, String),
        }
        let mut action: Option<Act> = None;
        let all_icon = ChromeIcon::Settings;
        if tree_row(ui, selected.is_none(), 0.0, Some(all_icon), "全部", 12.5).clicked() {
            *selected = None;
        }
        // an inline editor for a NEW root folder sits right under 全部
        if matches!(&self.folder_edit, Some(e) if e.target.is_none() && e.parent.is_none()) {
            self.folder_edit_row(ui, 0.0);
            ui.add_space(2.0);
        }
        let mut folders = self.folders.clone();
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
        for f in &ordered {
            let fid = s(f, "id");
            let fname = s(f, "name");
            let fparent = s(f, "parent_id");
            let d = depth_of(&folders, f);
            let editing_this =
                matches!(&self.folder_edit, Some(e) if e.target.as_deref() == Some(fid.as_str()));
            if editing_this {
                self.folder_edit_row(ui, d as f32 * 14.0);
            } else {
                let is_sel = selected.as_deref() == Some(fid.as_str());
                let ficon = if is_sel {
                    ChromeIcon::FolderOpen
                } else {
                    ChromeIcon::Folder
                };
                let resp = tree_row(
                    ui,
                    is_sel,
                    d as f32 * 15.0,
                    Some(ficon),
                    &fname,
                    12.5,
                );
                if resp.clicked() {
                    *selected = Some(fid.clone());
                }
                let (a, b, c, dd) = (
                    fid.clone(),
                    if fparent.is_empty() { None } else { Some(fparent.clone()) },
                    fname.clone(),
                    fid.clone(),
                );
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
                    if ui.button("删除").clicked() {
                        action = Some(Act::Delete(dd.clone(), c.clone()));
                        ui.close();
                    }
                });
            }
            // an inline editor for a NEW child of this folder
            if matches!(&self.folder_edit, Some(e) if e.target.is_none() && e.parent.as_deref() == Some(fid.as_str())) {
                self.folder_edit_row(ui, (d + 1) as f32 * 14.0);
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
        match action {
            Some(Act::NewChild(parent)) => {
                self.folder_edit = Some(FolderEdit {
                    target: None,
                    parent: Some(parent),
                    text: "新文件夹".into(),
                });
                self.folder_edit_focus = true;
            }
            Some(Act::NewSibling(parent)) => {
                self.folder_edit = Some(FolderEdit {
                    target: None,
                    parent,
                    text: "新文件夹".into(),
                });
                self.folder_edit_focus = true;
            }
            Some(Act::Rename(id, name)) => {
                self.folder_edit = Some(FolderEdit {
                    target: Some(id),
                    parent: None,
                    text: name,
                });
                self.folder_edit_focus = true;
            }
            Some(Act::Delete(id, name)) => self.folder_try_delete(&id, &name),
            None => {}
        }
        self.folder_delete_window(ui);
    }

    /// One inline folder-name editor row (rename or new folder).
    fn folder_edit_row(&mut self, ui: &mut egui::Ui, indent: f32) {
        let mut commit = false;
        let mut cancel = false;
        ui.horizontal(|ui| {
            ui.add_space(indent + 2.0);
            if let Some(edit) = &mut self.folder_edit {
                let resp = ui.add_sized(
                    [190.0, 30.0],
                    egui::TextEdit::singleline(&mut edit.text),
                );
                if self.folder_edit_focus {
                    resp.request_focus();
                    self.folder_edit_focus = false;
                }
                if resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                    commit = true;
                }
                if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                    cancel = true;
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
        if !self.need_identity("编辑文件夹") {
            return;
        }
        let Some(edit) = self.folder_edit.take() else {
            return;
        };
        let name = edit.text.trim().to_string();
        if name.is_empty() {
            self.msg = "文件夹名不能为空".into();
            return;
        }
        match &edit.target {
            Some(id) => {
                match self.call(
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
                match self.call(
                    "POST",
                    "/v1/folders",
                    Some(json!({ "name": name, "parent_id": edit.parent })),
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

    fn reload_folders(&mut self) {
        if let Ok(v) = self.call("GET", "/v1/folders", None) {
            self.folders = v.get("folders").and_then(|f| f.as_array()).cloned().unwrap_or_default();
        }
    }

    /// Delete is only offered after a local non-empty pre-check; the
    /// server repeats the check and its reason wins on failure.
    fn folder_try_delete(&mut self, id: &str, name: &str) {
        if self.folders.iter().any(|f| s(f, "parent_id") == id) {
            self.msg = "文件夹下还有子文件夹，不能删除".into();
            return;
        }
        if self
            .units
            .iter()
            .any(|u| u.get("unit").map(|x| s(x, "folder_id")).unwrap_or_default() == id)
        {
            self.msg = "文件夹内还有内容块，不能删除".into();
            return;
        }
        if self.indexes.iter().any(|x| s(x, "folder_id") == id) {
            self.msg = "文件夹内还有索引，不能删除".into();
            return;
        }
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
                Some(Overlay::AllVersions { entry_pos: _ }) => {
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
    fn all_versions_ui(&mut self, ui: &mut egui::Ui, entry_pos: usize) -> bool {
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
                                        switch = Some((
                                            vid.clone(),
                                            s(v, "semver"),
                                            vid == latest_id,
                                        ));
                                    }
                                    ui.label(
                                        egui::RichText::new(format!(
                                            "v{} · {} 页",
                                            s(v, "semver"),
                                            i(v, "page_count")
                                        ))
                                        .small()
                                        .color(palette_color(if is_cur {
                                            ACCENT_DEEP
                                        } else {
                                            TEXT_WEAK
                                        })),
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
            Overlay::AllVersions { entry_pos } => {
                if self.all_versions_ui(ui, entry_pos) {
                    Some(Overlay::AllVersions { entry_pos })
                } else {
                    None
                }
            }
            Overlay::PickTemplate(t) => self.pick_template_ui(ui, t),
            Overlay::PageEdit(pe) => self.page_edit_ui(ui, screen, pe),
            Overlay::Present(p) => self.present_ui(ui, screen, p),
        }
    }

    fn confirm_ui(&mut self, ui: &mut egui::Ui, c: ConfirmState) -> Option<Overlay> {
        let mut open = true;
        let mut confirmed = false;
        let mut cancelled = false;
        egui::Window::new(&c.title)
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
            .show(ui.ctx(), |ui| {
                ui.set_width(360.0);
                ui.label(egui::RichText::new(&c.body).size(12.5).color(palette_color(TEXT_MAIN)));
                ui.add_space(10.0);
                ui.horizontal(|ui| {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let b = if c.danger {
                            btn_danger(ui, &c.confirm_label)
                        } else {
                            btn_primary(ui, &c.confirm_label)
                        };
                        if b.clicked() {
                            confirmed = true;
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
        if confirmed {
            match c.action {
                ConfirmAction::PurgeTrash(id) => self.trash_purge(&id),
                ConfirmAction::PurgeTrashMany(ids) => {
                    let mut ok = 0usize;
                    for id in &ids {
                        if self
                            .call(
                                "POST",
                                &format!("/v1/trash/{id}/purge"),
                                Some(json!({})),
                            )
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
                    self.refresh_all();
                }
                ConfirmAction::EmptyTrash => {
                    match self.call("POST", "/v1/trash/purge-all", Some(json!({}))) {
                        Ok(_) => {
                            self.msg = "已清空回收站".into();
                            self.trash.stack = NavStack::new(TrashFrame::List(
                                TrashListState {
                                    filter: 0,
                                    selected: Default::default(),
                                },
                            ));
                            self.trash.loaded = false;
                            self.refresh_all();
                        }
                        Err(e) => self.msg = format!("清空失败：{e}"),
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
            }
            return None;
        }
        Some(Overlay::Confirm(c))
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
                        egui::RichText::new(tr("暂无可用模板：先创建一个索引，之后就能以它为模板新建。"))
                            .size(12.0)
                            .color(palette_color(TEXT_WEAK)),
                    );
                } else {
                    egui::ScrollArea::vertical()
                        .max_height(180.0)
                        .show(ui, |ui| {
                            for idx in self.indexes.clone() {
                                ui.radio_value(
                                    &mut t.template_id,
                                    s(&idx, "id"),
                                    s(&idx, "name"),
                                );
                            }
                        });
                }
                ui.add_space(10.0);
                ui.label(egui::RichText::new(tr("新索引名称")).size(12.5).color(palette_color(TEXT_MAIN)));
                ui.add(
                    egui::TextEdit::singleline(&mut t.new_name)
                        .desired_width(360.0)
                        .margin(egui::vec2(10.0, 6.0)),
                );
                ui.add_space(10.0);
                ui.horizontal(|ui| {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let ready = !t.new_name.trim().is_empty()
                            && !t.template_id.is_empty();
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
                self.call("GET", &format!("/v1/indexes/{}", t.template_id), None)
            {
                if let Some(res) = detail
                    .get("resolved_entries")
                    .and_then(|r| r.as_array())
                {
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
            let r = self.call(
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
                        let _ = self.call(
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
                    self.refresh_all();
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
                let (rect, resp) =
                    ui.allocate_exact_size(screen.size(), egui::Sense::click());
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
                            let scale =
                                (stage.width() / ts.x).min(stage.height() / ts.y);
                            let r = egui::Rect::from_center_size(
                                stage.center(),
                                ts * scale,
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
                    let g = painter.layout_no_wrap(
                        bar_text,
                        font,
                        egui::Color32::from_gray(220),
                    );
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
                    painter.rect_filled(
                        rail,
                        0.0,
                        egui::Color32::from_rgb(0x14, 0x17, 0x16),
                    );
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
                    let script = p
                        .scripts
                        .get(p.cur)
                        .cloned()
                        .unwrap_or_default();
                    let body_font =
                        egui::FontId::new(12.5, egui::FontFamily::Proportional);
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
                    painter.galley(egui::pos2(rail.min.x + 14.0, y), g, egui::Color32::from_gray(225));
                }
                if resp.clicked() && !p.speaker {
                    p.cur = (p.cur + 1).min(p.pages.len() - 1);
                }
                resp.context_menu(|ui| {
                    if ui
                        .button(if p.blackout { tr("黑屏 ✓") } else { tr("黑屏") })
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
            match self.call(
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
                        let list: Vec<ElWork> =
                            read_elements(els);
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
                egui::Frame::NONE.fill(palette_color(PANEL_BG)).show(ui, |ui| {
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
                            .add_enabled(
                                !pe.undo.is_empty(),
                                egui::Button::new(tr("撤销")),
                            )
                            .clicked()
                        {
                            undo_req = true;
                        }
                        if ui
                            .add_enabled(
                                !pe.redo.is_empty(),
                                egui::Button::new(tr("重做")),
                            )
                            .clicked()
                        {
                            redo_req = true;
                        }
                        ui.add_enabled_ui(v2, |ui| {
                            ui.menu_button(tr("添加元素"), |ui| {
                                for (kind, label) in [("text", "文本"), ("shape", "形状"), ("line", "线条"), ("picture", "图片")] {
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
                                if text_action(ui, "✕ 关闭", palette_color(TEXT_MAIN), 13.0) {
                                    close = true;
                                }
                            },
                        );
                    });
                    ui.add_space(8.0);
                    ui.separator();
                    ui.horizontal_top(|ui| {
                        ui.vertical(|ui| {
                            if btn_default_enabled(ui, pe.added_pages() < 100, tr("＋ 加页")).clicked() {
                                add_page = true;
                            }
                            egui::ScrollArea::vertical()
                                .id_salt("editor-page-rail")
                                .max_width(124.0)
                                .max_height((screen.height() - 176.0).max(132.0))
                                .show(ui, |ui| {
                                    for pno in 0..pe.page_count {
                                        let clicked = if pno < pe.original_page_count {
                                            self.thumb_button(ui, &pe.version_id, pno, 104.0, pno == pe.page)
                                        } else {
                                            ui.add_sized([104.0, 58.0], egui::Button::new(format!("{} {}", tr("第"), pno + 1))
                                                .selected(pno == pe.page)).clicked()
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
                                let hint = match pe.sel.and_then(|s| {
                                    pe.elems
                                        .get(&pe.page)
                                        .and_then(|l| l.get(s))
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
                                    egui::RichText::new(tr("文本内容（逐段编辑，保存后生成新版本）"))
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
                                        egui::TextEdit::multiline(
                                            &mut pe.scripts[pe.page],
                                        )
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
            pe.select_element(selection, ui.input(|input| input.modifiers.command || input.modifiers.ctrl || input.modifiers.shift));
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
            let picture = if kind == "picture" { pick_editor_picture() } else { None };
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
            let edits = pe.legacy_edits();
            let ops = pe.page_ops(None);
            let mut new_semver = String::new();
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
                match self.call(
                    "POST",
                    &format!("/v1/versions/{}/edit-pages", pe.version_id),
                    Some(payload),
                ) {
                    Ok(v) => {
                        let ver = v.get("version").cloned().unwrap_or(json!({}));
                        new_semver = s(&ver, "semver");
                    }
                    Err(e) => {
                        self.msg = format!("保存失败：{e}");
                        return Some(Overlay::PageEdit(pe));
                    }
                }
            } else if pe.scripts != pe.scripts_base {
                if let Err(e) = self.call(
                    "PUT",
                    &format!("/v1/versions/{}/scripts", pe.version_id),
                    Some(json!({ "scripts": pe.scripts })),
                ) {
                    self.msg = format!("讲稿保存失败：{e}");
                    return Some(Overlay::PageEdit(pe));
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
                self.open_unit_detail(&uid);
            }
            self.refresh_all();
            return None;
        }
        Some(Overlay::PageEdit(pe))
    }

    fn page_edit_stage(&mut self, ui: &mut egui::Ui, pe: &mut PageEditState) {
        let page = pe.page;
        let width = (ui.ctx().content_rect().width() - 480.0).clamp(300.0, 850.0);
        let height = width * pe.slide_cy.max(1) as f32 / pe.slide_cx.max(1) as f32;
        let (rect, response) = ui.allocate_exact_size(egui::vec2(width, height), egui::Sense::click_and_drag());
        let painter = ui.painter().with_clip_rect(rect.intersect(ui.clip_rect()));
        let picture = pe.preview.texture(page).or_else(|| {
            (page < pe.original_page_count).then(|| self.thumb(&pe.version_id, page)).flatten()
        });
        if let Some(texture) = picture {
            painter.image(texture.id(), rect, egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0)), egui::Color32::WHITE);
        } else {
            painter.rect_filled(rect, 0.0, if page >= pe.original_page_count { egui::Color32::WHITE } else { palette_color(HOVER_BG) });
        }
        let elements = pe.elems.get(&page).map(Vec::as_slice).unwrap_or(&[]);
        let scale = width / pe.slide_cx.max(1) as f32;
        let root = StageTransform {
            origin: rect.min, x_axis: egui::vec2(scale, 0.0), y_axis: egui::vec2(0.0, scale),
        };
        let transforms = stage_transforms(elements, root);
        let bounds: Vec<_> = elements.iter().zip(&transforms)
            .map(|(element, transform)| transform.project(element)).collect();
        let handle = |element: egui::Rect| egui::Rect::from_min_size(
            element.max - egui::vec2(9.0, 9.0), egui::vec2(9.0, 9.0),
        );
        for (index, area) in bounds.iter().copied().enumerate() {
            let selected = pe.selection.contains(&index);
            if elements[index].has_xfrm {
                painter.rect_stroke(
                    area, 0.0, egui::Stroke::new(
                        if selected { 1.8 } else { 0.6 },
                        if selected { ACCENT } else { palette_color(TEXT_FAINT).gamma_multiply(0.55) },
                    ), egui::StrokeKind::Middle,
                );
                if pe.sel == Some(index) && pe.selection.len() == 1 {
                    painter.rect_filled(handle(area), 0.0, ACCENT);
                }
            }
        }
        let Some(pointer) = response.interact_pointer_pos() else {
            if response.drag_stopped() { pe.drag = None; }
            return;
        };
        let hit = || bounds.iter().enumerate().rev().find(|(index, area)| {
            elements[*index].has_xfrm && area.contains(pointer)
        }).map(|(index, _)| index);
        if response.drag_started() {
            let resize = pe.sel.filter(|&index| {
                pe.selection.len() == 1 && bounds.get(index).is_some_and(|area| handle(*area).expand(2.0).contains(pointer))
            });
            if let Some(index) = resize.or_else(hit) {
                let element = &elements[index];
                let drag = ElDrag {
                    el: index,
                    resize: resize.is_some(),
                    start_pos: pointer,
                    transform: transforms[index],
                    x: element.x, y: element.y, cx: element.cx, cy: element.cy,
                };
                pe.select_element(Some(index), false);
                pe.push_undo();
                pe.drag = Some(drag);
            }
        } else if response.clicked() {
            let extend = ui.input(|input| input.modifiers.command || input.modifiers.ctrl || input.modifiers.shift);
            pe.select_element(hit(), extend);
        }
        if response.dragged() {
            if let Some(drag) = pe.drag {
                let delta = drag.transform.world_delta(pointer - drag.start_pos);
                if let Some(element) = pe.elems.get_mut(&page).and_then(|items| items.get_mut(drag.el)) {
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
        let mut do_delete = false;
        let mut cancel = false;
        egui::Window::new("删除文件夹")
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
            .show(ui.ctx(), |ui| {
                ui.label(format!("删除文件夹「{name}」？只能删除空文件夹。"));
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    if ui.button("删除").clicked() {
                        do_delete = true;
                    }
                    if ui.button("取消").clicked() {
                        cancel = true;
                    }
                });
            });
        if do_delete {
            match self.call("DELETE", &format!("/v1/folders/{id}"), None) {
                Ok(_) => {
                    self.msg = "已删除文件夹".into();
                    if self.content.folder_filter.as_deref() == Some(id.as_str()) {
                        self.content.folder_filter = None;
                    }
                    if self.showroom.folder.as_deref() == Some(id.as_str()) {
                        self.showroom.folder = None;
                    }
                    if self.index.overview_folder.as_deref() == Some(id.as_str()) {
                        self.index.overview_folder = None;
                    }
                    self.reload_folders();
                }
                Err(e) => self.msg = format!("删除失败：{e}"),
            }
        }
        if do_delete || cancel || !open {
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

    /// Does an item whose home is `folder_id` pass the folder filter?
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
        if !self.need_identity("新建索引") {
            return;
        }
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
        self.index.stack.push(IndexFrame::Create(CreateState {
            new_group,
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
                Some(IndexDetailAction::OpenGraphTarget { index, branch, commit }) => {
                    self.open_index_detail_as(&index, branch.clone());
                    if let IndexFrame::Detail(st) = self.index.stack.top_mut() {
                        if st.index_id == index && st.branch == branch {
                            st.view = if commit.is_some() { IndexDetailView::Commits } else { IndexDetailView::Lineage };
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
        let title = match &filter {
            None => "全部索引".to_string(),
            Some(fid) => self
                .folders
                .iter()
                .find(|f| s(f, "id") == *fid)
                .map(|f| s(f, "name"))
                .unwrap_or_else(|| "索引".into()),
        };
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
        ui.label(egui::RichText::new(title).size(18.0).strong());
        ui.add_space(14.0);
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
            ui.with_layout(
                egui::Layout::left_to_right(egui::Align::Min).with_main_wrap(true),
                |ui| {
                    for x in items {
                        let xid = s(&x, "id");
                        let selected = ov.selected.as_deref() == Some(xid.as_str());
                        let scope_color = IndexScope::of(&x).color();
                        let title = s(&x, "name");
                        let cover = self.index_covers.get(&xid).cloned().unwrap_or_default();
                        let resp = self.wall_card(ui, WallCard {
                            width: card_w, min_height: card_h, title: &title, cover: &cover, selected,
                            scope: Some(scope_color),
                        }, |app, ui| {
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
                                        s(f, "id") == home
                                            && s(f, "main_index_id") == xid
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
                                    "void" => {
                                        chip(ui, "已作废", palette_color(ROSE), ROSE_SOFT_BG)
                                    }
                                    _ => {}
                                }
                            });
                        });
                        let rect = resp.response.rect;
                        let click = ui
                            .interact(rect, ui.scope_id().with(("idx_card", &xid)), egui::Sense::click())
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
                            ui.painter()
                                .rect_stroke(rect, 6.0, stroke, egui::StrokeKind::Inside);
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
                    let r = self.call(
                        "POST",
                        "/v1/indexes",
                        Some(json!({
                            "name": st.new_name,
                            "group_id": st.new_group,
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
                                format!("{} · {} · {}", s(&unit, "name"), s(&rend, "form"), s(&rend, "lang")),
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
            ui.label(egui::RichText::new("分支").size(11.5).color(palette_color(TEXT_WEAK)));
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
                if !self.need_identity("发布版本") {
                    // Anonymous session: the login popup is open;
                    // the typed tag name stays for after the login.
                } else {
                    let id = st.index_id.clone();
                    let r = self.call(
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
            if text_action_enabled(ui, !st.new_branch.is_empty(), "拉分支", palette_color(TEXT_MAIN), 13.0) {
                let id = st.index_id.clone();
                let r = self.call(
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
                                .clicked()
                            {
                                let mut es = entries.clone();
                                es.remove(pos);
                                *action = Some(IndexDetailAction::Commit(es, "删除条目"));
                                return;
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
                                    for sl in slots {
                                        if let Some(n) = sl
                                            .as_str()
                                            .and_then(|x| x.parse::<usize>().ok())
                                        {
                                            if n < total {
                                                sel[n] = true;
                                            }
                                        }
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
                                            self.overlays.push(Overlay::AllVersions { entry_pos: pos });
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
                match self.call(
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
                ui.label(egui::RichText::new("无冲突，可自动合并").color(palette_color(CHIP_GREEN_FG)));
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
                    let r = self.call(
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
                        ConnStatus::Anonymous => ("匿名连接", egui::Color32::from_rgb(0x2F, 0x6B, 0xF5)),
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
                "disconnect" => {
                    if let Some(c) = self.conns.get_mut(i) {
                        c.token = None;
                        c.status = if c.anon {
                            ConnStatus::Anonymous
                        } else {
                            ConnStatus::LoggedOut
                        };
                    }
                    if self.active == Some(i) {
                        self.token = None;
                        self.user = None;
                    }
                    self.persist_conns();
                    self.msg = "已断开该服务器（保存的地址保留）".into();
                }
                "delete" => {
                    if i < self.conns.len() {
                        self.conns.remove(i);
                        match self.active {
                            Some(a) if a == i => {
                                self.active = (0..self.conns.len())
                                    .find(|&j| self.conns[j].status == ConnStatus::Connected);
                                self.sync_from_active();
                                if matches!(
                                    self.active.and_then(|a| self.conns.get(a)),
                                    Some(c) if c.status == ConnStatus::Connected
                                ) {
                                    self.refresh_all();
                                } else {
                                    self.user = None;
                                    if self.anon_active() {
                                        self.refresh_guest();
                                    }
                                }
                            }
                            Some(a) if a > i => self.active = Some(a - 1),
                            _ => {}
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
                ui.label("账号");
                ui.add_sized([380.0, 30.0], egui::TextEdit::singleline(&mut self.add_user));
                ui.end_row();
                ui.label("密码");
                ui.add_sized(
                    [380.0, 30.0],
                    egui::TextEdit::singleline(&mut self.add_pass).password(true),
                );
                ui.end_row();
            });
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            if ui
                .add_enabled(
                    !self.add_url.trim().is_empty(),
                    egui::Button::new(egui::RichText::new("登录并添加").color(egui::Color32::WHITE))
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
                ui.label(egui::RichText::new("搜索中…").size(11.5).color(palette_color(TEXT_WEAK)));
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
        let mut use_url: Option<String> = None;
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
                            egui::RichText::new(if name.is_empty() { "启幕服务器" } else { &name })
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
                                use_url = Some(url.clone());
                            }
                        });
                    });
                });
            ui.add_space(8.0);
        }
        if let Some(u) = use_url {
            self.add_url = u;
            self.msg = "地址已填入上方表单：输入账号密码点「登录并添加」，或点「匿名添加」浏览公开内容".into();
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
            ui.label(egui::RichText::new(note.clone()).size(11.5).color(palette_color(ACCENT_DEEP)));
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
                total_bytes as f64 / 1_000_000.0
            ))
            .size(11.5)
            .color(palette_color(TEXT_WEAK)),
        );
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
                            save_client_settings(
                                json!({ "cache_dir": p.to_string_lossy() }),
                            );
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
                            if ui.checkbox(&mut self.dc_auto_quit, tr("放映结束自动关闭 DeckCraft")).changed() {
                                save_client_settings(json!({ "deckcraft_auto_quit": self.dc_auto_quit }));
                            }
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
                                        egui::RichText::new("个人绑定预览")
                                            .strong()
                                            .size(16.0),
                                    );
                                    ui.add_space(8.0);
                                    let p = &self.settings.profile;
                                    let get = |k: &str| {
                                        p.get(k).cloned().unwrap_or_default()
                                    };
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
                                    egui::TextEdit::singleline(
                                        &mut self.settings.user_display,
                                    ),
                                );
                                ui.label("密码");
                                ui.add_sized(
                                    [150.0, 26.0],
                                    egui::TextEdit::singleline(
                                        &mut self.settings.user_password,
                                    )
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
                                        ui.add_sized(
                                            [300.0, 26.0],
                                            egui::TextEdit::singleline(v),
                                        );
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
                                    egui::TextEdit::singleline(
                                        &mut self.settings.languages_text,
                                    ),
                                );
                                if ui.button("保存").clicked() {
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
                                    self.settings.ai_has_key = v
                                        .get("has_key")
                                        .and_then(|x| x.as_bool())
                                        .unwrap_or(false);
                                }
                                if let Ok(v) = self.call("GET", "/v1/settings/glossary", None) {
                                    if let Some(pairs) =
                                        v.get("pairs").and_then(|x| x.as_array())
                                    {
                                        self.settings.glossary_text = pairs
                                            .iter()
                                            .map(|pr| {
                                                format!(
                                                    "{}={}",
                                                    s(pr, "src"),
                                                    s(pr, "dst")
                                                )
                                            })
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
                                        egui::TextEdit::singleline(
                                            &mut self.settings.ai_endpoint,
                                        ),
                                    );
                                    ui.end_row();
                                    ui.label("模型");
                                    ui.add_sized(
                                        [300.0, 26.0],
                                        egui::TextEdit::singleline(
                                            &mut self.settings.ai_model,
                                        ),
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
                            if ui.button("保存术语表").clicked() {
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
                                if ui.button("保存策略").clicked() {
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
fn startup_log(msg: &str) {
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
    match eframe::run_native(
        "启幕",
        options,
        Box::new(|cc| Ok(Box::new(App::new(cc)))),
    ) {
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
        let (i18n, _problems) =
            craft_i18n::I18n::new(&I18N_EMBEDDED, &["zh-hans", "en-us"]);
        assert!(i18n.check_all().is_empty(), "catalog problems: {:?}", i18n.check_all());
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
            (false, egui::Theme::Dark, egui::Theme::Light, egui::Color32::WHITE),
            (true, egui::Theme::Light, egui::Theme::Dark, egui::Color32::BLACK),
            (false, egui::Theme::Dark, egui::Theme::Light, egui::Color32::WHITE),
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

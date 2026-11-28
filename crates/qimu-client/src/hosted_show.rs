//! T2 SPIKE — in-process hosting of DeckCraft's slide show
//! (transplant strategy (b): host a `SlideApp`, DeckCraft unchanged).
//!
//! Status: productized (T2). The three presentation entries go
//! through `present_via_deckcraft` / `present_local_via_deckcraft`,
//! which try this shell first on the deck bytes; the external
//! `dc_jump` path (DeckCraft app + control protocol) remains in the
//! tree as the fallback when hosting cannot start — and, since the
//! 2026-11-25 「放映打开方式」 ruling, also as a user-selectable
//! primary engine in 设置 → 演示偏好, so it is NOT retired.
//! Evidence for every claim made here is in
//! `~/workspace/qimu-transplant-t2-spike/SPIKE.md`.
//!
//! The shell mirrors DeckCraft's own desktop host
//! (`apps/deckcraft/src/main.rs`: a 20-line `eframe::App` wrapper
//! forwarding `logic` / `raw_input_hook` / `ui`), with four hosting
//! rules that keep the guest from leaking into qimu:
//!
//! 1. One fresh `SlideApp` per show. `ui()` is forwarded only while
//!    a show is live; on the first frame where `app.show` is `None`
//!    the caller drops the instance and calls `restore_host_ui`.
//!    The guest's editor chrome is therefore unreachable: during a
//!    show `SlideApp::ui` short-circuits to `show::ui`, and outside
//!    a show the guest is never drawn at all.
//! 2. No persistence of any kind: the shell never calls
//!    `enable_update_checks`, never sets `session.recovery_dir`
//!    (stays `None`, so AutoRecover is inert), and never loads or
//!    saves DeckCraft prefs (`ui.json` / `prefs.json` are host-side
//!    concerns in DeckCraft's own app; this shell is not that host).
//! 3. The hosted document is a memory-only copy: it is loaded with
//!    `file.openBytes`, which binds **no path**
//!    (`add_opened(name, None, doc)`), so no save command could ever
//!    write back over a real file. The shell sends no command other
//!    than that one load (guard K5: show-in, nothing out).
//! 4. `SlideApp::ui` owns the egui context's fonts/visuals while it
//!    runs (`ui_setup` installs DeckCraft's interface fonts and
//!    theme). On show end the caller restores qimu's own setup via
//!    `restore_host_ui` (qimu's existing startup functions).

use base64::Engine as _;
use deckcraft_engine::Session;
use deckcraft_ui_egui::{Services, SlideApp, ViewMode};
use serde_json::{Value, json};

/// What the load reported about the deck copy: pages or media that
/// could not be read, fonts the deck names that are unavailable.
/// Surfaced so the caller can tell the user the copy is damaged
/// instead of presenting a silently degraded show (T2 acceptance ⑦).
#[derive(Debug, Default, Clone, Copy)]
pub struct DamageNote {
    pub lost_slides: usize,
    pub damaged_media: usize,
    pub missing_fonts: usize,
    pub import_warnings: usize,
}

impl DamageNote {
    fn from_open_result(v: &Value) -> Self {
        fn count(v: Option<&Value>) -> usize {
            match v {
                Some(Value::Array(a)) => a.len(),
                Some(Value::Number(n)) => n.as_u64().unwrap_or(0) as usize,
                _ => 0,
            }
        }
        DamageNote {
            lost_slides: count(v.get("lostSlides")),
            damaged_media: count(v.get("damagedMedia")),
            missing_fonts: count(v.get("missingFonts")),
            import_warnings: count(v.get("importWarnings")),
        }
    }

    pub fn is_clean(&self) -> bool {
        self.lost_slides == 0 && self.damaged_media == 0 && self.import_warnings == 0
    }
}

fn load_slide_app(
    name: &str,
    bytes: &[u8],
    displays: Option<Box<dyn FnMut() -> Option<usize>>>,
) -> Result<(SlideApp, DamageNote), String> {
    let mut services = Services::default();
    services.displays = displays;
    services.audio_out = std::env::var_os("QIMU_NO_AUDIO")
        .is_none()
        .then(|| Box::new(crate::hosted_audio::CpalOut::default()) as Box<dyn deckcraft_media::AudioOut>);
    let mut app = SlideApp::new(Session::new(), services);
    let data = base64::engine::general_purpose::STANDARD.encode(bytes);
    let opened = app
        .session
        .execute("file.openBytes", &json!({ "name": name, "data": data }))
        .map_err(|e| e.to_string())?;
    Ok((app, DamageNote::from_open_result(&opened)))
}

/// One hosted show: a guest `SlideApp` plus the damage reported
/// when its deck copy was loaded.
pub struct HostedShow {
    app: SlideApp,
    pub damage: DamageNote,
}

impl HostedShow {
    /// Build the guest, load `bytes` as its only document and start
    /// the show for `mode` at slide `from`.
    ///
    /// `displays` is the D1 wiring point (see SPIKE.md Q3):
    /// - `None` — no display-count service: `second_display()` in
    ///   DeckCraft defaults to *allow*, so Presenter View opens even
    ///   on a single screen (this is also what DeckCraft's own
    ///   Windows desktop build effectively runs today).
    /// - `Some(f)` — `f` reports the real connected-display count
    ///   and the upstream gate applies (presenter only when >= 2).
    /// D1 final ruling (user, 2026-11-24 14:24): callers pass
    /// `None` — the shell does not inject a display count, matching
    /// upstream's Windows factory behavior, so Presenter View opens
    /// on a single screen too.
    pub fn start(
        name: &str,
        bytes: &[u8],
        mode: crate::DcMode,
        from: usize,
        displays: Option<Box<dyn FnMut() -> Option<usize>>>,
    ) -> Result<Self, String> {
        if matches!(
            mode,
            crate::DcMode::Normal | crate::DcMode::Thumbnails | crate::DcMode::Notes
        ) {
            return Err("普通视图使用内嵌编辑器；缩略图和备注视图需要 DeckCraft 桌面应用".into());
        }
        let (mut app, damage) = load_slide_app(name, bytes, displays)?;

        // `UiState::default()` has `presenter_view: true`, so set the
        // flag explicitly per mode — the same discipline dc_jump
        // applies to the persisted flag over the control protocol.
        app.ui.presenter_view = matches!(mode, crate::DcMode::Presenter);
        app.start_show(from, false);
        if app.show.is_none() {
            return Err("DeckCraft did not start a show (no active document)".into());
        }
        Ok(HostedShow { app, damage })
    }

    /// Per-frame background half, forwarded exactly as DeckCraft's
    /// own host forwards it. With no control receiver, no inbox and
    /// recovery unset, everything in it is inert except the show's
    /// own clocks.
    pub fn logic(&mut self, ctx: &egui::Context) {
        self.app.logic(ctx);
    }

    /// Input hook, forwarded for parity with the vendor host.
    pub fn raw_input_hook(&mut self, ctx: &egui::Context, raw: &mut egui::RawInput) {
        self.app.raw_input_hook(ctx, raw);
    }

    /// Draw one hosted frame into the root `ui`. Returns `false`
    /// once the show has ended; the caller then drops this instance
    /// and calls [`HostedShow::restore_host_ui`]. The guest sends
    /// the fullscreen viewport commands for the root window itself
    /// (enter on show start, exit in its finish path).
    pub fn ui(&mut self, ui: &mut egui::Ui) -> bool {
        self.app.ui(ui);
        self.app.show.is_some()
    }

    /// Re-apply qimu's own context setup after the guest replaced
    /// the fonts and visuals: qimu's startup functions, unchanged.
    pub fn restore_host_ui(ctx: &egui::Context) {
        crate::install_system_fonts(ctx);
        crate::apply_modern_style(ctx);
    }
}

pub struct EmbeddedEditor {
    app: SlideApp,
    pub damage: DamageNote,
}

impl EmbeddedEditor {
    pub fn start(name: &str, bytes: &[u8], from: usize) -> Result<Self, String> {
        let (mut app, damage) = load_slide_app(name, bytes, None)?;
        app.ui.view = ViewMode::Normal;
        app.ui.presenter_view = false;
        app.ui.notes = false;
        let slides = app.session.active().map_or(0, |doc| doc.doc.slides.len());
        if slides > 0 {
            app.session
                .execute("slide.go", &json!({ "index": from.min(slides - 1) }))
                .map_err(|e| e.to_string())?;
        }
        Ok(Self { app, damage })
    }

    pub fn logic(&mut self, ctx: &egui::Context) {
        self.app.logic(ctx);
    }

    pub fn raw_input_hook(&mut self, ctx: &egui::Context, raw: &mut egui::RawInput) {
        self.app.raw_input_hook(ctx, raw);
    }

    pub fn ui(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        let host_style = ctx.global_style();
        self.app.embedded_normal_ui(ui);
        ctx.set_global_style(host_style);
    }

    pub fn is_dirty(&self) -> bool {
        self.app.session.active().is_some_and(|doc| doc.is_dirty())
    }
}

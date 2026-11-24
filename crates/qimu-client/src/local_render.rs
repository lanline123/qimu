//! In-process DeckCraft rendering (transplant T0/T1, DESIGN §2.1/§2.2/§6):
//! the thumbnail and page-edit stage-preview renders that used to
//! shell out to deckcraft-cli (on the server) now call the very same
//! library functions the CLI wrapped —
//! `deckcraft_engine::cmd::file::open_presentation` +
//! `deckcraft_engine::cmd::file::render_png_with` — inside the
//! client process.
//!
//! Parameter correspondence with the subprocess era (server
//! `render_thumbs_deckcraft` / `render_page_edit_preview`):
//!   batch thumbnails:  `render --all --scale 0.75`
//!     -> render_png_with(doc, page, 0.75, edit=false, transparent=false)
//!   edit stage preview: `render --slide N --scale 1 --edit`
//!     -> render_png_with(doc, page, 1.0, edit=true, transparent=false)
//!   PDF export: `run --export --export-params {…}` (server
//!     `deck_to_pdf` builds the params)
//!     -> pdf_options(params) + deckcraft_pdf::export in-process
//!        (see export_pdf below; T1)
//! The CLI's render subcommand is itself only an argument shell over
//! render_png_with (deckcraft-cli main.rs), so both eras produce the
//! same function's pixels; the dual-track comparison gate in the
//! transplant DESIGN (§6 T0) verifies that on CI artifacts.

use std::path::Path;

/// Warm the shared FontDb once per process: its directory scan is
/// lazy, and kicking it off in the background keeps the first render
/// from paying it on the caller's thread (DESIGN §2.4).
fn warm_fonts() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        deckcraft_fonts::FontDb::global().scan_in_background();
    });
}

/// Open a PPTX / fragment byte string as a DeckCraft presentation.
fn open(bytes: &[u8]) -> Result<deckcraft_model::Presentation, String> {
    open_named("frag.pptx", bytes)
}

fn open_named(name: &str, bytes: &[u8]) -> Result<deckcraft_model::Presentation, String> {
    warm_fonts();
    deckcraft_engine::cmd::file::open_presentation(name, bytes)
        .map_err(|e| e.to_string())
}

/// Render every page of a fragment into `cache_dir` as `<page>.png`
/// (0-based), mirroring the server renderer's publish discipline: a
/// page already in the cache is left alone, and a page is published
/// complete (temp file + rename), never written in place.
pub fn render_frag_thumbs(frag: &[u8], cache_dir: &Path) -> Result<(), String> {
    let doc = open(frag)?;
    std::fs::create_dir_all(cache_dir).map_err(|e| e.to_string())?;
    let pages = doc.slides.len();
    if pages == 0 {
        return Err("DeckCraft 渲染未产出任何页面".into());
    }
    for page in 0..pages {
        let cached = cache_dir.join(format!("{page}.png"));
        if cached.exists() {
            continue;
        }
        let (png, _, _) =
            deckcraft_engine::cmd::file::render_png_with(&doc, page, 0.75, false, false);
        let tmp = cache_dir.join(format!(".{page}.png.tmp"));
        std::fs::write(&tmp, &png).map_err(|e| e.to_string())?;
        if let Err(e) = std::fs::rename(&tmp, &cached) {
            let _ = std::fs::remove_file(&tmp);
            // Another render may have published this page already.
            if !cached.exists() {
                return Err(e.to_string());
            }
        }
    }
    Ok(())
}

/// Render one (already ops-applied) package at stage-preview
/// parameters (scale 1, edit mode) and return the PNG bytes.
pub fn render_edit_preview(pptx_bytes: &[u8], page: usize) -> Result<Vec<u8>, String> {
    let doc = open(pptx_bytes)?;
    if page >= doc.slides.len() {
        return Err("预览页超出范围".into());
    }
    let (png, _, _) =
        deckcraft_engine::cmd::file::render_png_with(&doc, page, 1.0, true, false);
    Ok(png)
}

/// Resolve one thumbnail page for a version in the local library:
/// serve the shared deckcraft-v2 cache, rendering the whole
/// fragment in-process on a miss. Returns None when the version is
/// not in the local library (or the local render failed) — the
/// caller then falls back to the server path, whose renderer and
/// cache layout are the same.
pub fn local_thumb(
    local: &crate::local_store::LocalStore,
    version_id: &str,
    page: usize,
) -> Option<Vec<u8>> {
    let cache_dir = local.thumb_cache_dir(version_id);
    let cache_file = cache_dir.join(format!("{page}.png"));
    if cache_file.exists() {
        return std::fs::read(&cache_file).ok();
    }
    let frag = local.version_fragment(version_id)?;
    render_frag_thumbs(&frag, &cache_dir).ok()?;
    std::fs::read(&cache_file).ok()
}

/// Why an in-process PDF export failed (transplant T1). `TextLoss`
/// is kept structured — count and first lost characters — so the
/// caller can tell the user exactly what would be lost instead of
/// forwarding a subprocess stderr line (DESIGN §2.2: the CJK fix's
/// `deckcraft_pdf::PdfError::TextLoss`, raised by `export()` itself
/// before any bytes are produced).
pub enum PdfFail {
    TextLoss { count: usize, sample: String },
    Other(String),
}

/// Export an assembled deck to PDF in-process: the same call the
/// deckcraft-cli `run --export` wraps
/// (`deckcraft_engine::cmd::file::pdf_bytes` = `pdf_options` +
/// `deckcraft_pdf::export`), minus pdf_bytes' error
/// stringification, so TextLoss survives as data. `form` / `page`
/// are the client's export vocabulary; the params JSON built here
/// is identical to the one the server's `deck_to_pdf` passes as
/// `--export-params` for the same form — `{textLayer: true,
/// layout: slides|notes|handouts, perPage: 6 (handout only),
/// slides: [page] (single page only)}` — with the single deliberate
/// exception of `text_layer = false`, the documented raster-only
/// escape (TextLoss's own remedy for slide text): no text layer is
/// emitted, so no glyph can be lost from it, at the price of text
/// that cannot be searched or copied. Notes text is drawn as real
/// text and stays guarded by TextLoss either way.
pub fn export_pdf(
    deck: &[u8],
    form: &str,
    page: Option<usize>,
    text_layer: bool,
) -> Result<Vec<u8>, PdfFail> {
    let doc = open_named("deck.pptx", deck).map_err(PdfFail::Other)?;
    let mut params = serde_json::json!({ "textLayer": text_layer });
    match form {
        "handout" => {
            params["layout"] = serde_json::json!("handouts");
            params["perPage"] = serde_json::json!(6);
        }
        "notes" => {
            params["layout"] = serde_json::json!("notes");
        }
        _ => {
            params["layout"] = serde_json::json!("slides");
        }
    }
    if let Some(p) = page {
        params["slides"] = serde_json::json!([p]);
    }
    let opts = deckcraft_engine::cmd::file::pdf_options(&params, doc.slides.len())
        .map_err(|e| PdfFail::Other(e.to_string()))?;
    deckcraft_pdf::export(&doc, &opts).map_err(|e| match e {
        deckcraft_pdf::PdfError::TextLoss { count, sample } => {
            PdfFail::TextLoss { count, sample }
        }
        other => PdfFail::Other(other.to_string()),
    })
}

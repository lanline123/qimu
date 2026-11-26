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

/// Thumbnail cache epoch (audit2 E-N4): the cache key is the page
/// alone — it carries no renderer or font-catalog dimension, so a
/// page first rendered while fonts were missing used to pin the
/// substitute-font image forever. All thumbnail files live under
/// this epoch subdirectory; when the renderer or the bundled font
/// set changes materially, bump the epoch and every page re-renders
/// once (the old epoch's files are simply never read again).
const THUMB_EPOCH: &str = "r1";

fn epoch_dir(cache_dir: &Path) -> std::path::PathBuf {
    cache_dir.join(THUMB_EPOCH)
}

/// A cached file only counts as a thumbnail when it actually looks
/// like a PNG (audit2 E-M1: a concurrent write to the old shared
/// fixed-name temp file could publish a truncated image, and the
/// bare `exists()` checks then pinned it forever).
fn valid_png(bytes: &[u8]) -> bool {
    bytes.len() >= 32 && bytes.starts_with(&[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A])
}

/// Read a cached thumbnail, treating a corrupt file as a miss (and
/// removing it, so the render below republishes a good one).
fn read_cached_png(path: &Path) -> Option<Vec<u8>> {
    let bytes = std::fs::read(path).ok()?;
    if valid_png(&bytes) {
        Some(bytes)
    } else {
        let _ = std::fs::remove_file(path);
        None
    }
}

/// Render every page of a fragment into `cache_dir` as `<page>.png`
/// (0-based, under the epoch subdirectory), mirroring the server
/// renderer's publish discipline: a page already in the cache is
/// left alone, and a page is published complete (temp file +
/// rename), never written in place.
pub fn render_frag_thumbs(frag: &[u8], cache_dir: &Path) -> Result<(), String> {
    let doc = open(frag)?;
    let dir = epoch_dir(cache_dir);
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let pages = doc.slides.len();
    if pages == 0 {
        return Err("DeckCraft 渲染未产出任何页面".into());
    }
    for page in 0..pages {
        let cached = dir.join(format!("{page}.png"));
        if read_cached_png(&cached).is_some() {
            continue;
        }
        let (png, _, _) =
            deckcraft_engine::cmd::file::render_png_with(&doc, page, 0.75, false, false);
        if !valid_png(&png) {
            return Err("DeckCraft 渲染产出无效图像".into());
        }
        // Audit2 E-M1: the temp name is unique to this call — the
        // old fixed `.{page}.png.tmp` was shared by every thread
        // rendering the same page (wall, import warm-up, another
        // version of the same page entity), and concurrent writes
        // could truncate each other's image before the rename.
        static TMP_SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let seq = TMP_SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let tmp = dir.join(format!(
            ".{page}.png.tmp-{}-{seq}",
            std::process::id()
        ));
        std::fs::write(&tmp, &png).map_err(|e| e.to_string())?;
        if let Err(e) = std::fs::rename(&tmp, &cached) {
            let _ = std::fs::remove_file(&tmp);
            // Another render may have published this page already.
            if read_cached_png(&cached).is_none() {
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

/// Render ONE slide from `frag_bytes` at full stage pixel resolution
/// (`target_px_w` clamped to `[1280, 2560]`) directly from the PPTX
/// document without reading or writing the `0.75x` thumbnail cache.
/// Returns `(width, height, premultiplied_rgba)` ready for GPU upload.
pub fn render_stage_image(
    frag_bytes: &[u8],
    page: usize,
    target_px_w: u32,
) -> Option<(u32, u32, Vec<u8>)> {
    let doc = open(frag_bytes).ok()?;
    if page >= doc.slides.len() {
        return None;
    }
    let sw = doc.slide_size.width.max(1.0);
    let sh = doc.slide_size.height.max(1.0);
    let w_px = target_px_w.clamp(1280, 2560);
    let h_px = ((w_px as f64 * sh / sw).round() as u32).max(1);
    let scale = w_px as f64 / sw;
    let threads = std::thread::available_parallelism()
        .map(|n| n.get().min(8) as u16)
        .unwrap_or(4);
    let opts = deckcraft_render::RenderOpts {
        scale,
        size: Some((w_px, h_px)),
        threads,
        ..Default::default()
    };
    let img = deckcraft_render::render_slide(&doc, page, &opts);
    let expected = (img.width as usize)
        .checked_mul(img.height as usize)?
        .checked_mul(4)?;
    if img.width == 0 || img.height == 0 || img.pixels.len() != expected {
        return None;
    }
    Some((img.width, img.height, img.pixels))
}

/// Resolve one real high-resolution stage page from the local library
/// (entity package, lazy whole-deck backing, or version fragment) —
/// never returning a low-resolution thumbnail.
pub fn local_stage_image(
    local: &crate::local_store::LocalStore,
    version_id: &str,
    page: usize,
    target_px_w: u32,
) -> Option<(u32, u32, Vec<u8>)> {
    if let Some(page_id) = local.version_page_id(version_id, page) {
        if let Some(bytes) = local.page_package_bytes(&page_id) {
            if let Some(img) = render_stage_image(&bytes, 0, target_px_w) {
                return Some(img);
            }
        }
        if let Some((deck_hash, index)) = local.lazy_backing_for_page(&page_id) {
            if let Some(deck) = local.file_bytes(&deck_hash) {
                if let Some(img) = render_stage_image(&deck, index, target_px_w) {
                    return Some(img);
                }
            }
        }
    }
    let frag = local.version_fragment(version_id)?;
    render_stage_image(&frag, page, target_px_w)
}

/// Resolve one thumbnail page for a version in the local library.
/// Since the staged-import line (⑤) the disk cache is keyed by
/// PAGE ENTITY id, not by version: a page that appears in a new
/// version, another block, or a re-imported deck reuses the one
/// render the library already paid for. Returns None when the
/// version is not in the local library (or the local render
/// failed) — the caller then falls back to the server path, whose
/// renderer and cache layout are the same.
pub fn local_thumb(
    local: &crate::local_store::LocalStore,
    version_id: &str,
    page: usize,
) -> Option<Vec<u8>> {
    if let Some(page_id) = local.version_page_id(version_id, page) {
        if let Some(bytes) = page_thumb(local, &page_id) {
            return Some(bytes);
        }
        // Audit2 E-N5: an entity-backed version whose entity package
        // is missing or fails to render falls through to the legacy
        // fragment path below — the version's own fragment is in the
        // library, so the page must not go blank (and get pinned as
        // failed) over a broken entity record.
    }
    // Legacy fallback: a version whose pages carry no entity ids
    // (the page-entity migration could not split its fragment)
    // keeps the version-keyed whole-fragment render it always had.
    let cache_dir = local.thumb_cache_dir(version_id);
    let cache_file = epoch_dir(&cache_dir).join(format!("{page}.png"));
    if let Some(bytes) = read_cached_png(&cache_file) {
        return Some(bytes);
    }
    let frag = local.version_fragment(version_id)?;
    cached_thumb(&frag, &cache_dir, page)
}

/// Resolve one page entity's thumbnail. The entity is a
/// single-page package, so its cache directory holds exactly one
/// image (`0.png`) under the entity's own key; a miss renders
/// just that page's package, never a whole version fragment.
/// Pages whose thumbnail already exists return from disk — the
/// post-import warm-up walks the imported pages in order through
/// here, so cards light up one by one in page order.
pub fn page_thumb(
    local: &crate::local_store::LocalStore,
    page_id: &str,
) -> Option<Vec<u8>> {
    let cache_dir = local.thumb_cache_dir(page_id);
    let cache_file = epoch_dir(&cache_dir).join("0.png");
    if let Some(bytes) = read_cached_png(&cache_file) {
        return Some(bytes);
    }
    let bytes = match local.page_package_bytes(page_id) {
        Some(b) => b,
        None => {
            // Lazy import (user ruling 2026-11-25): the page's
            // split has not landed, so it has no entity package
            // yet — render its thumbnail straight from the whole
            // deck backing it, so the card shows the real slide
            // from the moment the deck opened. Rendering is not
            // "using" the page: the card stays unusable until the
            // split lands. The render is returned for the session
            // only, deliberately NOT published into the entity's
            // disk cache — when the split lands, `page_thumb`
            // renders (or reuses) the entity package's own image
            // under this key, so no deck-vs-entity pixel question
            // can ever become permanent.
            let (deck_hash, index) = local.lazy_backing_for_page(page_id)?;
            let deck = local.file_bytes(&deck_hash)?;
            return render_deck_page(&deck, index);
        }
    };
    render_frag_thumbs(&bytes, &cache_dir).ok()?;
    read_cached_png(&cache_file)
}

/// Render ONE page of a whole deck in memory (thumbnail scale,
/// the same parameters as the batch path) — the lazy import's
/// pre-split thumbnail source.
fn render_deck_page(deck: &[u8], index: usize) -> Option<Vec<u8>> {
    let doc = open(deck).ok()?;
    if index >= doc.slides.len() {
        return None;
    }
    let (png, _, _) =
        deckcraft_engine::cmd::file::render_png_with(&doc, index, 0.75, false, false);
    Some(png)
}

/// Render one thumbnail page from a fragment byte string into
/// `cache_dir`, with the same publish discipline as `local_thumb`.
/// This is the resolver for fragments that live outside the local
/// library — the offline cache bundle's content blocks (T1 cache
/// payload: 索引 + 内容块（页面）) — so a cached deck's pages
/// render with no server and no local-library copy.
pub fn cached_thumb(frag: &[u8], cache_dir: &Path, page: usize) -> Option<Vec<u8>> {
    let cache_file = epoch_dir(cache_dir).join(format!("{page}.png"));
    if let Some(bytes) = read_cached_png(&cache_file) {
        return Some(bytes);
    }
    render_frag_thumbs(frag, cache_dir).ok()?;
    read_cached_png(&cache_file)
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

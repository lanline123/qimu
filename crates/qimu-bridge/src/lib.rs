//! qimu-bridge — the PowerPoint-side capability (spec §4.3).
//!
//! Phase 1 decision (2026-11-14): NO standalone Office add-in. The Rust
//! client drives the *running* PowerPoint through COM automation instead —
//! no add-in deployment / trust-center burden, and it works across
//! PowerPoint versions. Phase 2 adds real add-ins (PowerPoint Office JS +
//! WPS) behind this same trait.
//!
//! The hard requirement this layer exists for: accepting an update replaces
//! slides **in place in the currently open file** — same presentation, same
//! path, unsaved changes stay in the user's hands. Producing a new updated
//! file is never the acceptance path.
//!
//! Platform status:
//! - Windows: COM automation implementation (`windows_impl`), driving
//!   `PowerPoint.Application` through IDispatch late binding.
//! - Other platforms: `StubBridge` reports the bridge as unavailable;
//!   the file-level flow (CLI `deck apply`) remains fully functional.

use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
pub struct BridgeStatus {
    /// "powerpoint" | "none"
    pub host: String,
    pub running: bool,
    /// e.g. "16.0" when a running instance answered.
    pub version: Option<String>,
    /// Full path of the active presentation, when one is open.
    pub active_path: Option<String>,
    pub detail: String,
}

/// One in-place slide-range replacement inside the open presentation.
///
/// The slides currently occupying `start..start+remove_count` (1-based,
/// matching PowerPoint's own indexing) are deleted and the slides of the
/// fragment file are inserted at `start`. `remove_count == 0` is a pure
/// insertion; an empty `fragment_path` with `remove_count > 0` is a pure
/// deletion.
#[derive(Debug, Clone)]
pub struct SlideReplacement {
    /// 1-based position in the open presentation.
    pub start: usize,
    pub remove_count: usize,
    /// Path to a .pptx fragment file holding the replacement slides.
    pub fragment_path: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ReplaceReport {
    pub path: String,
    pub applied: usize,
    pub slides_before: usize,
    pub slides_after: usize,
}

pub trait PowerPointBridge {
    fn status(&self) -> BridgeStatus;

    /// Replace slides in the presentation that is *currently open* in
    /// PowerPoint. Fails unless the open presentation's path equals
    /// `expected_path` — the bridge never edits a different deck than the
    /// one the update was computed for.
    fn replace_slides_in_place(
        &self,
        expected_path: &str,
        replacements: &[SlideReplacement],
    ) -> Result<ReplaceReport, String>;
}

#[cfg(windows)]
mod windows_impl;
#[cfg(windows)]
pub use windows_impl::WindowsBridge;

#[cfg(not(windows))]
pub struct StubBridge;

#[cfg(not(windows))]
impl PowerPointBridge for StubBridge {
    fn status(&self) -> BridgeStatus {
        BridgeStatus {
            host: "none".into(),
            running: false,
            version: None,
            active_path: None,
            detail: "COM 桥仅在 Windows + PowerPoint 环境可用；当前平台走文件级流程（qimu deck apply）".into(),
        }
    }

    fn replace_slides_in_place(
        &self,
        _expected_path: &str,
        _replacements: &[SlideReplacement],
    ) -> Result<ReplaceReport, String> {
        Err("COM 桥仅在 Windows 上可用".into())
    }
}

/// The bridge for the current platform.
#[cfg(windows)]
pub fn platform_bridge() -> Box<dyn PowerPointBridge> {
    Box::new(WindowsBridge)
}

#[cfg(not(windows))]
pub fn platform_bridge() -> Box<dyn PowerPointBridge> {
    Box::new(StubBridge)
}

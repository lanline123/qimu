//! qimu-core — the shared core library of the 启幕 PPT manager.
//!
//! All product logic lives here exactly once (spec §2/§4.4): the desktop
//! GUI, the CLI and the server are frontends over this library, which is
//! what makes full CLI/API parity — and therefore full AI takeover —
//! possible.

pub mod assemble;
pub mod auth;
pub mod manifest;
pub mod model;
pub mod personalize;
pub mod pptx;
pub mod propose;
pub mod semver;
pub mod store;
pub mod text;
pub mod translate;

pub use model::*;
pub use semver::{Increment, SemVer};

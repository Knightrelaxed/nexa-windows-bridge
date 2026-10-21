//! Automation module for N.E.X.A Windows Bridge
//!
//! Provides desktop input automation (SendInput mouse & keyboard),
//! screen element smart-snapping (Windows UI Automation),
//! and transparent visual overlays.

pub mod input;
pub mod element_finder;
pub mod highlighter;

#[allow(unused_imports)]
pub use input::*;
#[allow(unused_imports)]
pub use element_finder::*;
#[allow(unused_imports)]
pub use highlighter::*;

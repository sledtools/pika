//! Backward-compatible re-export surface.
//!
//! All design-system functionality now lives in [`crate::design`] (tokens,
//! styles, colour shorthands, iced style functions) while pure utility helpers
//! (string truncation, relative-time formatting) live in [`crate::utils`].
//!
//! This module simply re-exports both so that existing `use crate::theme;`
//! imports throughout the view layer continue to compile without changes.

pub use crate::design::*;
pub use crate::utils::*;

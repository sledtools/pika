//! Pika desktop design system.
//!
//! Structured design tokens and widget style methods based on concepts from
//! libcosmic, adapted for a cross-platform chat app. See the design plan in
//! `docs/iced-design-plan.md` for the full architecture rationale.

mod styles;
mod tokens;

pub use styles::BubblePosition;
pub use tokens::*;

/// The dark theme, matching the current production look.
pub const DARK: PikaTheme = PikaTheme::dark_default();

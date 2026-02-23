//! Lucide icon helpers for the Pika desktop app.
//!
//! Uses the Lucide icon font (lucide.ttf) with Unicode codepoints.
//! Icons scale with font size and are inherently DPI-aware.

use iced::widget::text;
use iced::{Color, Element, Font, Theme};

/// The Lucide icon font, loaded via `include_bytes!` in main.rs.
pub const LUCIDE_FONT: Font = Font::with_name("lucide");

/// Geist Medium — for button labels and UI controls.
pub const MEDIUM: Font = Font {
    family: iced::font::Family::Name("Geist"),
    weight: iced::font::Weight::Medium,
    stretch: iced::font::Stretch::Normal,
    style: iced::font::Style::Normal,
};

/// Geist Bold — explicitly named so it never falls back to a wrong font.
pub const BOLD: Font = Font {
    family: iced::font::Family::Name("Geist"),
    weight: iced::font::Weight::Bold,
    stretch: iced::font::Stretch::Normal,
    style: iced::font::Style::Normal,
};

/// Geist Mono — for npubs, hashes, and other machine-readable text.
pub const MONO: Font = Font::with_name("GeistMono");

// ── Icon codepoints ─────────────────────────────────────────────────────────

pub const ARROW_UP: &str = "\u{e04a}";
pub const REPLY: &str = "\u{e22a}";
pub const SMILE_PLUS: &str = "\u{e301}";
pub const X: &str = "\u{e1b2}";
pub const CIRCLE_X: &str = "\u{e084}";
pub const PHONE: &str = "\u{e133}";
pub const PHONE_INCOMING: &str = "\u{e136}";
pub const VIDEO: &str = "\u{e1a5}";
#[allow(dead_code)]
pub const SEND: &str = "\u{e152}";
pub const MESSAGE_SQUARE_PLUS: &str = "\u{e40c}";
pub const USERS: &str = "\u{e1a4}";
pub const USER: &str = "\u{e19f}";
pub const PLUS: &str = "\u{e13d}";
#[allow(dead_code)]
pub const SEARCH: &str = "\u{e151}";
#[allow(dead_code)]
pub const ELLIPSIS: &str = "\u{e0b6}";
pub const CHECK: &str = "\u{e06c}";
pub const CHECK_CHECK: &str = "\u{e38e}";
pub const CLOCK: &str = "\u{e087}";
pub const COPY: &str = "\u{e09e}";
pub const CHEVRON_LEFT: &str = "\u{e06e}";
pub const LOG_OUT: &str = "\u{e10e}";
pub const PEN: &str = "\u{e12f}";
pub const KEY: &str = "\u{e0fd}";
pub const INFO: &str = "\u{e0f9}";
#[allow(dead_code)]
pub const AT_SIGN: &str = "\u{e04e}";

// ── Helper ──────────────────────────────────────────────────────────────────

/// Create a Lucide icon text element with the given codepoint, size, and color.
#[allow(dead_code)]
pub fn icon<'a, M: 'a>(codepoint: &'a str, size: f32, color: Color) -> Element<'a, M, Theme> {
    text(codepoint)
        .font(LUCIDE_FONT)
        .size(size)
        .color(color)
        .center()
        .into()
}

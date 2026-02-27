//! Pika desktop design system — public API.
//!
//! ## Module layout
//!
//! | Module      | Responsibility                                         |
//! |-------------|--------------------------------------------------------|
//! | `tokens`    | Pure data: `PikaTheme`, token structs, theme catalogue |
//! | `styles`    | Widget appearance methods on `PikaTheme`               |
//! | `mod` (here)| Global theme state, colour shorthands, iced style fns  |
//!
//! View code should import from `crate::design` (or the backward-compat
//! re-export `crate::theme`). No view should reach into `tokens` or `styles`
//! directly.

mod styles;
mod tokens;

// Re-export the items views actually need.
pub use styles::BubblePosition;
pub use tokens::*;

use std::sync::atomic::{AtomicPtr, AtomicUsize, Ordering};

use iced::widget::{button, container, rule, scrollable, text_input};
use iced::{Color, Theme};

// ── Global active theme ─────────────────────────────────────────────────────

/// Direct pointer to the currently active `PikaTheme`.
///
/// Initialised to null; the first call to [`set_active`] (or the fallback in
/// [`current`]) resolves it to a concrete `&'static PikaTheme`.
static ACTIVE_THEME: AtomicPtr<PikaTheme> = AtomicPtr::new(std::ptr::null_mut());

/// Stores the index into `ALL_THEMES` so the theme picker can highlight the
/// active entry.  Kept in sync by [`set_active`].
static ACTIVE_THEME_INDEX: AtomicUsize = AtomicUsize::new(0);

/// Set the active theme by index into [`ALL_THEMES`].
///
/// The index is clamped to the valid range.  This updates both the cached
/// pointer *and* the index used by the theme picker.
pub fn set_active(index: usize) {
    let clamped = if index < ALL_THEMES.len() { index } else { 0 };
    let entry = &ALL_THEMES[clamped];
    // SAFETY: `entry.theme` lives inside `ALL_THEMES` which is `&'static`,
    // so the pointer is valid for the lifetime of the programme.
    let ptr = &entry.theme as *const PikaTheme as *mut PikaTheme;
    ACTIVE_THEME.store(ptr, Ordering::Release);
    ACTIVE_THEME_INDEX.store(clamped, Ordering::Relaxed);
}

/// Return the current active theme index (into [`ALL_THEMES`]).
#[allow(dead_code)]
pub fn active_index() -> usize {
    ACTIVE_THEME_INDEX.load(Ordering::Relaxed)
}

/// Return a reference to the currently active [`PikaTheme`].
///
/// This is the primary accessor that all style functions should use.  It is a
/// single atomic pointer load — no index arithmetic, no `unwrap`, no fallback
/// every time.
pub fn current() -> &'static PikaTheme {
    let ptr = ACTIVE_THEME.load(Ordering::Acquire);
    if ptr.is_null() {
        // First access before `set_active` was called — return the default
        // theme and lazily initialise the pointer for future calls.
        let default = &ALL_THEMES[0].theme as *const PikaTheme as *mut PikaTheme;
        // Best-effort CAS; if another thread raced us, their value wins, but
        // both point into the same static array so either is fine.
        let _ = ACTIVE_THEME.compare_exchange(
            std::ptr::null_mut(),
            default,
            Ordering::Release,
            Ordering::Relaxed,
        );
        &ALL_THEMES[0].theme
    } else {
        // SAFETY: `ptr` was derived from `&'static PikaTheme` in `set_active`
        // (or the fallback above) and is never freed.
        unsafe { &*ptr }
    }
}

// ── Colour shorthand accessors ──────────────────────────────────────────────
//
// These read the currently active theme at call time so they can be used
// directly in view code without passing a theme reference around.

pub fn received_bubble() -> Color {
    current().received_bubble.background
}
pub fn text_primary() -> Color {
    current().background.on
}
pub fn text_secondary() -> Color {
    current().background.on_secondary
}
pub fn text_faded() -> Color {
    current().background.on_faded
}
pub fn accent_blue() -> Color {
    current().accent.base
}
pub fn hover_bg() -> Color {
    current().background.component.hover
}
#[allow(dead_code)]
pub fn selected_bg() -> Color {
    current().background.component.selected
}
pub fn input_border() -> Color {
    current().background.divider
}
pub fn danger() -> Color {
    current().danger.base
}

// ── Iced-compatible style functions ─────────────────────────────────────────
//
// Each function has the signature expected by the corresponding iced widget's
// `.style(…)` method, so views can pass them directly (e.g.
// `container.style(design::surface_style)`).

// ── Containers ──────────────────────────────────────────────────────────────

#[allow(dead_code)]
pub fn bubble_sent_style(_theme: &Theme) -> container::Style {
    current().bubble_sent()
}

#[allow(dead_code)]
pub fn bubble_received_style(_theme: &Theme) -> container::Style {
    current().bubble_received()
}

pub fn surface_style(_theme: &Theme) -> container::Style {
    current().surface()
}

pub fn rail_container_style(_theme: &Theme) -> container::Style {
    current().rail()
}

pub fn input_bar_style(_theme: &Theme) -> container::Style {
    current().input_bar()
}

pub fn login_card_style(_theme: &Theme) -> container::Style {
    current().login_card()
}

pub fn avatar_container_style(_theme: &Theme) -> container::Style {
    current().avatar()
}

pub fn badge_container_style(_theme: &Theme) -> container::Style {
    current().badge()
}

pub fn incoming_call_banner_style(_theme: &Theme) -> container::Style {
    current().call_banner()
}

pub fn call_screen_bg_style(_theme: &Theme) -> container::Style {
    current().call_screen_bg()
}

pub fn drop_zone_style(_theme: &Theme) -> container::Style {
    current().drop_zone()
}

pub fn media_chip_style(is_mine: bool) -> impl Fn(&Theme) -> container::Style {
    move |_theme: &Theme| current().media_chip(is_mine)
}

pub fn checkbox_style(is_checked: bool) -> impl Fn(&Theme) -> container::Style {
    move |_theme: &Theme| current().checkbox_indicator(is_checked)
}

// ── Buttons ─────────────────────────────────────────────────────────────────

pub fn primary_button_style(_theme: &Theme, status: button::Status) -> button::Style {
    current().button_primary(status)
}

pub fn secondary_button_style(_theme: &Theme, status: button::Status) -> button::Style {
    current().button_secondary(status)
}

pub fn danger_button_style(_theme: &Theme, status: button::Status) -> button::Style {
    current().button_danger(status)
}

pub fn icon_button_style(is_active: bool) -> impl Fn(&Theme, button::Status) -> button::Style {
    move |_theme: &Theme, status: button::Status| current().button_icon(is_active, status)
}

pub fn chat_item_style(is_selected: bool) -> impl Fn(&Theme, button::Status) -> button::Style {
    move |_theme: &Theme, status: button::Status| current().chat_item(is_selected, status)
}

pub fn call_accept_button_style(_theme: &Theme, status: button::Status) -> button::Style {
    current().button_call_accept(status)
}

pub fn call_muted_button_style(_theme: &Theme, status: button::Status) -> button::Style {
    current().button_call_muted(status)
}

pub fn call_control_button_style(_theme: &Theme, status: button::Status) -> button::Style {
    current().button_call_control(status)
}

// ── Text input ──────────────────────────────────────────────────────────────

pub fn dark_input_style(_theme: &Theme, status: text_input::Status) -> text_input::Style {
    current().text_input(status)
}

// ── Scrollable ──────────────────────────────────────────────────────────────

pub fn scrollable_style() -> impl Fn(&Theme, scrollable::Status) -> scrollable::Style {
    move |_theme: &Theme, status: scrollable::Status| current().scrollable(status)
}

pub fn invisible_scrollable() -> impl Fn(&Theme, scrollable::Status) -> scrollable::Style {
    move |_theme: &Theme, _status: scrollable::Status| {
        use iced::widget::scrollable::{AutoScroll, Rail, Scroller, Style};

        let rail = Rail {
            background: None,
            border: Default::default(),
            scroller: Scroller {
                background: Color::TRANSPARENT.into(),
                border: Default::default(),
            },
        };

        let auto_scroll = AutoScroll {
            background: Color::TRANSPARENT.into(),
            border: Default::default(),
            shadow: Default::default(),
            icon: Color::TRANSPARENT,
        };

        Style {
            container: Default::default(),
            vertical_rail: rail,
            horizontal_rail: rail,
            gap: None,
            auto_scroll,
        }
    }
}

// ── Rule ────────────────────────────────────────────────────────────────────

pub fn subtle_rule_style(_theme: &Theme) -> rule::Style {
    let t = current();
    rule::Style {
        color: t.background.divider,
        radius: 0.0.into(),
        fill_mode: rule::FillMode::Full,
        snap: true,
    }
}

// ── Overlay ─────────────────────────────────────────────────────────────────

/// Container for command palette and theme picker cards.
pub fn overlay_container() -> impl Fn(&Theme) -> container::Style {
    move |_theme: &Theme| {
        let t = current();
        container::Style {
            background: Some(iced::Background::Color(t.primary.base)),
            border: iced::Border {
                color: t.background.divider,
                width: 1.0,
                radius: iced::border::radius(t.radii.l),
            },
            shadow: iced::Shadow {
                color: Color::BLACK.scale_alpha(0.5),
                offset: iced::Vector::new(0.0, 8.0),
                blur_radius: t.radii.l,
            },
            ..Default::default()
        }
    }
}

/// Backdrop that dims the rest of the UI behind an overlay.
pub fn overlay_backdrop_style() -> impl Fn(&Theme) -> container::Style {
    move |_theme: &Theme| container::Style {
        background: Some(iced::Background::Color(Color::BLACK.scale_alpha(0.55))),
        ..Default::default()
    }
}

/// Style for a shortcut chiclet badge in the command palette.
pub fn shortcut_chiclet_style(_theme: &Theme) -> container::Style {
    let t = current();
    container::Style {
        background: Some(iced::Background::Color(Color::TRANSPARENT)),
        border: iced::Border {
            color: t.background.on_faded,
            width: 1.0,
            radius: iced::border::radius(t.radii.s),
        },
        ..Default::default()
    }
}

/// Style for palette / picker item rows (command palette and theme picker).
pub fn palette_item_style(is_selected: bool) -> impl Fn(&Theme, button::Status) -> button::Style {
    move |_theme: &Theme, status: button::Status| {
        let t = current();
        let bg = if is_selected {
            t.background.component.selected
        } else {
            match status {
                button::Status::Hovered => t.background.component.hover,
                _ => Color::TRANSPARENT,
            }
        };
        button::Style {
            background: Some(iced::Background::Color(bg)),
            text_color: t.background.on,
            border: iced::border::rounded(t.radii.m),
            ..Default::default()
        }
    }
}

/// Style for the search text-input inside overlays (command palette / theme picker).
pub fn palette_input_style(_theme: &Theme, _status: text_input::Status) -> text_input::Style {
    let t = current();
    text_input::Style {
        background: iced::Background::Color(Color::TRANSPARENT),
        border: iced::Border {
            color: t.background.divider,
            width: 0.0,
            radius: iced::border::radius(t.radii.l),
        },
        icon: t.background.on_faded,
        placeholder: t.background.on_faded,
        value: t.background.on,
        selection: t.accent.base.scale_alpha(0.3),
    }
}

/// Style for the call banner "Accept" / "Reject" buttons (white-on-dark).
pub fn call_banner_button_style(_theme: &Theme, status: button::Status) -> button::Style {
    let t = current();
    let bg = match status {
        button::Status::Hovered => t.background.on_secondary,
        _ => t.background.on,
    };
    button::Style {
        background: Some(iced::Background::Color(bg)),
        text_color: t.background.base,
        border: iced::border::rounded(t.radii.s),
        ..Default::default()
    }
}

/// Error text colour for call screen diagnostics.
pub fn call_error_color() -> Color {
    current().danger.base.scale_alpha(0.8)
}

//! Widget style methods on [`PikaTheme`].
//!
//! Each method returns an iced widget appearance struct, using the theme's
//! design tokens to derive colors, borders, and backgrounds. These methods
//! are the single source of truth for how Pika widgets look.

use iced::border::{self, Radius};
use iced::widget::{button, container, text_input};
use iced::{Background, Border, Color};

use super::tokens::PikaTheme;

// ── BubblePosition ─────────────────────────────────────────────────────────

/// Position of a message within a consecutive group from the same sender.
///
/// Used to compute per-corner radii for Signal/iOS-style grouped bubbles.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BubblePosition {
    /// Only message in its group.
    Single,
    /// First message in a group (more messages follow from same sender).
    First,
    /// Middle of a group (messages before and after from same sender).
    Middle,
    /// Last message in a group (preceded by messages from same sender).
    Last,
}

// ── PikaTheme style methods ────────────────────────────────────────────────

impl PikaTheme {
    // ── Container styles ───────────────────────────────────────────────

    pub fn surface(&self) -> container::Style {
        container::Style {
            background: Some(Background::Color(self.background.base)),
            ..Default::default()
        }
    }

    pub fn rail(&self) -> container::Style {
        container::Style {
            background: Some(Background::Color(self.primary.base)),
            ..Default::default()
        }
    }

    pub fn input_bar(&self) -> container::Style {
        container::Style {
            background: Some(Background::Color(self.background.base)),
            ..Default::default()
        }
    }

    #[allow(dead_code)]
    pub fn bubble_sent(&self) -> container::Style {
        container::Style {
            text_color: Some(self.sent_bubble.on),
            background: Some(Background::Color(self.sent_bubble.background)),
            border: border::rounded(self.sent_bubble.radius),
            ..Default::default()
        }
    }

    #[allow(dead_code)]
    pub fn bubble_received(&self) -> container::Style {
        container::Style {
            text_color: Some(self.received_bubble.on),
            background: Some(Background::Color(self.received_bubble.background)),
            border: border::rounded(self.received_bubble.radius),
            ..Default::default()
        }
    }

    // ── Grouped bubble styles (Signal-style per-corner radii) ──────────

    /// Sent bubble with per-corner radii based on group position.
    ///
    /// Sent messages are right-aligned, so the "tail" side is the right edge.
    /// Consecutive messages compress the right-side corners.
    pub fn bubble_sent_grouped(&self, position: BubblePosition) -> container::Style {
        let r = self.sent_bubble.radius;
        let small = self.radii.xs;

        let radius = match position {
            BubblePosition::Single => border::radius(r),
            BubblePosition::First => Radius {
                top_left: r,
                top_right: r,
                bottom_right: small,
                bottom_left: r,
            },
            BubblePosition::Middle => Radius {
                top_left: r,
                top_right: small,
                bottom_right: small,
                bottom_left: r,
            },
            BubblePosition::Last => Radius {
                top_left: r,
                top_right: small,
                bottom_right: r,
                bottom_left: r,
            },
        };

        container::Style {
            text_color: Some(self.sent_bubble.on),
            background: Some(Background::Color(self.sent_bubble.background)),
            border: Border {
                radius,
                ..Default::default()
            },
            ..Default::default()
        }
    }

    /// Received bubble with per-corner radii based on group position.
    ///
    /// Received messages are left-aligned, so the "tail" side is the left edge.
    /// Consecutive messages compress the left-side corners.
    pub fn bubble_received_grouped(&self, position: BubblePosition) -> container::Style {
        let r = self.received_bubble.radius;
        let small = self.radii.xs;

        let radius = match position {
            BubblePosition::Single => border::radius(r),
            BubblePosition::First => Radius {
                top_left: r,
                top_right: r,
                bottom_right: r,
                bottom_left: small,
            },
            BubblePosition::Middle => Radius {
                top_left: small,
                top_right: r,
                bottom_right: r,
                bottom_left: small,
            },
            BubblePosition::Last => Radius {
                top_left: small,
                top_right: r,
                bottom_right: r,
                bottom_left: r,
            },
        };

        container::Style {
            text_color: Some(self.received_bubble.on),
            background: Some(Background::Color(self.received_bubble.background)),
            border: Border {
                radius,
                ..Default::default()
            },
            ..Default::default()
        }
    }

    // ── Other container styles ─────────────────────────────────────────

    pub fn avatar(&self) -> container::Style {
        container::Style {
            text_color: Some(Color::WHITE),
            background: Some(Background::Color(self.avatar_bg)),
            border: border::rounded(self.radii.full),
            ..Default::default()
        }
    }

    #[allow(dead_code)]
    pub fn toast(&self) -> container::Style {
        container::Style {
            text_color: Some(self.accent.on),
            background: Some(Background::Color(self.accent.base)),
            border: border::rounded(self.radii.s),
            ..Default::default()
        }
    }

    pub fn login_card(&self) -> container::Style {
        container::Style {
            text_color: Some(self.background.on),
            background: Some(Background::Color(self.primary.base)),
            border: Border {
                color: self.background.divider,
                width: 1.0,
                radius: border::radius(self.radii.l),
            },
            ..Default::default()
        }
    }

    pub fn badge(&self) -> container::Style {
        container::Style {
            text_color: Some(self.accent.on),
            background: Some(Background::Color(self.accent.base)),
            border: border::rounded(self.radii.full),
            ..Default::default()
        }
    }

    pub fn call_banner(&self) -> container::Style {
        container::Style {
            text_color: Some(self.success.on),
            background: Some(Background::Color(self.success.base)),
            ..Default::default()
        }
    }

    pub fn call_screen_bg(&self) -> container::Style {
        container::Style {
            background: Some(Background::Color(self.call_bg)),
            ..Default::default()
        }
    }

    // ── Checkbox indicator ─────────────────────────────────────────────

    pub fn checkbox_indicator(&self, is_checked: bool) -> container::Style {
        if is_checked {
            container::Style {
                text_color: Some(Color::WHITE),
                background: Some(Background::Color(self.accent.base)),
                border: Border {
                    color: self.accent.base,
                    width: 1.5,
                    radius: border::radius(self.radii.xs),
                },
                ..Default::default()
            }
        } else {
            container::Style {
                text_color: Some(Color::TRANSPARENT),
                background: Some(Background::Color(Color::TRANSPARENT)),
                border: Border {
                    color: self.background.on_faded,
                    width: 1.5,
                    radius: border::radius(self.radii.xs),
                },
                ..Default::default()
            }
        }
    }

    // ── Button styles ──────────────────────────────────────────────────

    pub fn button_primary(&self, status: button::Status) -> button::Style {
        let (bg, fg) = match status {
            button::Status::Hovered => (self.accent.hover, self.accent.on),
            button::Status::Disabled => (self.background.component.hover, self.background.on_faded),
            _ => (self.accent.base, self.accent.on),
        };
        button::Style {
            background: Some(Background::Color(bg)),
            text_color: fg,
            border: border::rounded(self.radii.s),
            ..Default::default()
        }
    }

    pub fn button_secondary(&self, status: button::Status) -> button::Style {
        let bg = match status {
            button::Status::Hovered => self.background.component.hover,
            _ => Color::TRANSPARENT,
        };
        button::Style {
            background: Some(Background::Color(bg)),
            text_color: self.background.on_secondary,
            border: border::rounded(self.radii.s),
            ..Default::default()
        }
    }

    /// Ghost icon button: no border, shows neutral hover/selected state.
    /// Use for toolbar icon buttons where accent blue would be too heavy.
    pub fn button_icon(&self, is_active: bool, status: button::Status) -> button::Style {
        let bg = if is_active {
            self.background.component.selected
        } else {
            match status {
                button::Status::Hovered => self.background.component.hover,
                _ => Color::TRANSPARENT,
            }
        };
        button::Style {
            background: Some(Background::Color(bg)),
            text_color: self.background.on,
            border: border::rounded(self.radii.s),
            ..Default::default()
        }
    }

    pub fn button_danger(&self, status: button::Status) -> button::Style {
        let bg = match status {
            button::Status::Hovered => self.danger.hover,
            _ => self.danger.base,
        };
        button::Style {
            background: Some(Background::Color(bg)),
            text_color: self.danger.on,
            border: border::rounded(self.radii.s),
            ..Default::default()
        }
    }

    pub fn button_call_accept(&self, status: button::Status) -> button::Style {
        let bg = match status {
            button::Status::Hovered => self.success.hover,
            _ => self.success.base,
        };
        button::Style {
            background: Some(Background::Color(bg)),
            text_color: self.success.on,
            border: border::rounded(self.radii.s),
            ..Default::default()
        }
    }

    pub fn button_call_muted(&self, status: button::Status) -> button::Style {
        let bg = match status {
            button::Status::Hovered => self.warning.hover,
            _ => self.warning.base,
        };
        button::Style {
            background: Some(Background::Color(bg)),
            text_color: self.warning.on,
            border: border::rounded(self.radii.s),
            ..Default::default()
        }
    }

    pub fn button_call_control(&self, status: button::Status) -> button::Style {
        let bg = match status {
            button::Status::Hovered => self.call_control_hover,
            _ => self.call_control_base,
        };
        button::Style {
            background: Some(Background::Color(bg)),
            text_color: Color::WHITE,
            border: border::rounded(self.radii.s),
            ..Default::default()
        }
    }

    pub fn chat_item(&self, is_selected: bool, status: button::Status) -> button::Style {
        let bg = if is_selected {
            self.background.component.selected
        } else {
            match status {
                button::Status::Hovered => self.background.component.hover,
                _ => Color::TRANSPARENT,
            }
        };
        button::Style {
            background: Some(Background::Color(bg)),
            text_color: self.background.on,
            border: border::rounded(self.radii.m),
            ..Default::default()
        }
    }

    // ── Text input style ───────────────────────────────────────────────

    pub fn text_input(&self, status: text_input::Status) -> text_input::Style {
        let border_color = match status {
            text_input::Status::Focused { .. } => self.accent.base,
            text_input::Status::Hovered => self.background.on_faded,
            _ => self.background.divider,
        };
        text_input::Style {
            background: Background::Color(self.background.input_bg),
            border: Border {
                color: border_color,
                width: 1.0,
                radius: border::radius(self.radii.s),
            },
            icon: self.background.on_faded,
            placeholder: self.background.on_faded,
            value: self.background.on,
            selection: self.accent.base.scale_alpha(0.3),
        }
    }
}

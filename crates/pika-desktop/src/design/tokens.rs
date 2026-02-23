#![allow(dead_code)]
//! Design token definitions for the Pika desktop app.
//!
//! Tokens are the lowest-level design decisions: colors, spacing, radii,
//! and typography scales. A [`PikaTheme`] bundles all tokens into a single
//! value that can be swapped to change the entire look (e.g. dark -> light).

use iced::Color;

// ── Surface ────────────────────────────────────────────────────────────────

/// A surface layer in the UI hierarchy (background -> primary -> secondary).
///
/// Each surface defines the colors for content rendered on top of it:
/// text (`on`, `on_secondary`, `on_faded`), dividers, the input-field
/// background, and an interactive [`Component`] for widgets on this surface.
#[derive(Debug, Clone, Copy)]
pub struct Surface {
    /// Background color of this surface layer.
    pub base: Color,
    /// Primary text color on this surface.
    pub on: Color,
    /// Secondary (muted) text color.
    pub on_secondary: Color,
    /// Faded (tertiary) text color.
    pub on_faded: Color,
    /// Divider / separator color.
    pub divider: Color,
    /// Background color for text inputs sitting on this surface.
    pub input_bg: Color,
    /// Default interactive component colors on this surface.
    pub component: Component,
}

// ── Component ──────────────────────────────────────────────────────────────

/// State-based colors for an interactive element (button, list item, chip).
#[derive(Debug, Clone, Copy)]
pub struct Component {
    pub base: Color,
    pub hover: Color,
    pub pressed: Color,
    pub selected: Color,
    pub disabled: Color,
    /// Foreground (text/icon) color.
    pub on: Color,
    pub border: Color,
    pub focus: Color,
}

// ── BubbleStyle ────────────────────────────────────────────────────────────

/// Colors for a chat message bubble.
#[derive(Debug, Clone, Copy)]
pub struct BubbleStyle {
    pub background: Color,
    /// Primary text color inside the bubble.
    pub on: Color,
    /// Secondary text (timestamps, delivery indicators).
    pub on_secondary: Color,
    /// Inline link color.
    pub link: Color,
    /// Default corner radius (outer corners in grouped bubbles).
    pub radius: f32,
}

// ── Spacing ────────────────────────────────────────────────────────────────

/// Named spacing scale. Values are in logical pixels.
#[derive(Debug, Clone, Copy)]
pub struct Spacing {
    pub none: u16, // 0
    pub xxxs: u16, // 2
    pub xxs: u16,  // 4
    pub xs: u16,   // 6
    pub s: u16,    // 8
    pub sm: u16,   // 10
    pub m: u16,    // 12
    pub l: u16,    // 16
    pub xl: u16,   // 20
    pub xxl: u16,  // 24
    pub xxxl: u16, // 32
    pub huge: u16, // 48
}

// ── Radii ──────────────────────────────────────────────────────────────────

/// Named corner-radius scale.
#[derive(Debug, Clone, Copy)]
pub struct Radii {
    pub none: f32, // 0
    pub xs: f32,   // 4  — grouped bubble corners, small chips
    pub s: f32,    // 8  — buttons, inputs
    pub m: f32,    // 12 — chat list items, cards
    pub l: f32,    // 16 — containers, login card
    pub xl: f32,   // 24 — large cards
    pub full: f32, // 9999 (circles/pills)
}

// ── Typography ─────────────────────────────────────────────────────────────

/// Named font-size scale (in logical pixels).
#[derive(Debug, Clone, Copy)]
pub struct Typography {
    pub display: f32,    // 36  login hero
    pub title_lg: f32,   // 24  large titles (call peer name)
    pub title: f32,      // 22  dialog titles
    pub heading: f32,    // 20  section headings (rail header, timer)
    pub headline: f32,   // 16  sub-headings (conv header)
    pub body: f32,       // 14  primary body text
    pub label: f32,      // 13  small labels
    pub caption: f32,    // 12  secondary info, timestamps
    pub caption_sm: f32, // 11  badges, npub display
    pub small: f32,      // 10  bubble timestamps
    pub icon: f32,       // 18  icon buttons
}

// ── PikaTheme ──────────────────────────────────────────────────────────────

/// The complete design token set for the Pika desktop app.
#[derive(Debug, Clone, Copy)]
pub struct PikaTheme {
    // Surface layers
    pub background: Surface,
    pub primary: Surface,
    pub secondary: Surface,

    // Semantic interactive colors
    pub accent: Component,
    pub success: Component,
    pub danger: Component,
    pub warning: Component,

    // Chat-specific
    pub sent_bubble: BubbleStyle,
    pub received_bubble: BubbleStyle,

    // Standalone colors
    pub avatar_bg: Color,
    pub call_bg: Color,
    pub call_control_base: Color,
    pub call_control_hover: Color,

    // Scales
    pub spacing: Spacing,
    pub radii: Radii,
    pub typography: Typography,
    pub is_dark: bool,
}

impl PikaTheme {
    /// Constructs the default dark theme.
    ///
    /// Neutral charcoal palette inspired by Signal desktop: minimal cool tint,
    /// strong surface contrast, vivid blue accent, generous corner radii.
    pub const fn dark_default() -> Self {
        // ── Palette constants ──────────────────────────────────────────
        //
        // Neutral dark grays with only a subtle cool bias (~10% more blue
        // than pure gray). Signal desktop uses this approach for a modern
        // dark theme that avoids both warm-brown and cold-blue washes.
        const TEXT_PRIMARY: Color = Color::from_rgb(0.910, 0.910, 0.929); // #E8E8ED
        const TEXT_SECONDARY: Color = Color::from_rgb(0.545, 0.545, 0.596); // #8B8B98
        const TEXT_FADED: Color = Color::from_rgb(0.361, 0.361, 0.408); // #5C5C68
        const DIVIDER: Color = Color::from_rgb(0.161, 0.165, 0.192); // #292A31
        const INPUT_BG: Color = Color::from_rgb(0.078, 0.078, 0.094); // #141418
        const HOVER_BG: Color = Color::from_rgb(0.133, 0.137, 0.161); // #222229
        const SELECTED_BG: Color = Color::from_rgb(0.161, 0.165, 0.192); // #292A31
        const ACCENT_BLUE: Color = Color::from_rgb(0.204, 0.471, 0.965); // #3478F6

        const BG_COMPONENT: Component = Component {
            base: Color::TRANSPARENT,
            hover: HOVER_BG,
            pressed: SELECTED_BG,
            selected: SELECTED_BG,
            disabled: Color::TRANSPARENT,
            on: TEXT_PRIMARY,
            border: DIVIDER,
            focus: ACCENT_BLUE,
        };

        Self {
            // Background (canvas) — deepest dark, conversation area
            background: Surface {
                base: Color::from_rgb(0.059, 0.063, 0.078), // #0F1014
                on: TEXT_PRIMARY,
                on_secondary: TEXT_SECONDARY,
                on_faded: TEXT_FADED,
                divider: DIVIDER,
                input_bg: INPUT_BG,
                component: BG_COMPONENT,
            },
            // Primary (rail, headers, input bar) — elevated charcoal
            primary: Surface {
                base: Color::from_rgb(0.106, 0.110, 0.133), // #1B1C22
                on: TEXT_PRIMARY,
                on_secondary: TEXT_SECONDARY,
                on_faded: TEXT_FADED,
                divider: DIVIDER,
                input_bg: INPUT_BG,
                component: BG_COMPONENT,
            },
            // Secondary (popovers, elevated panels)
            secondary: Surface {
                base: Color::from_rgb(0.157, 0.161, 0.192), // #282931
                on: TEXT_PRIMARY,
                on_secondary: TEXT_SECONDARY,
                on_faded: TEXT_FADED,
                divider: DIVIDER,
                input_bg: INPUT_BG,
                component: BG_COMPONENT,
            },

            // Accent — Signal blue
            accent: Component {
                base: ACCENT_BLUE,
                hover: Color::from_rgb(0.267, 0.533, 1.0), // #4488FF
                pressed: ACCENT_BLUE,
                selected: ACCENT_BLUE,
                disabled: Color::from_rgb(0.110, 0.235, 0.478), // #1C3C7A
                on: Color::WHITE,
                border: ACCENT_BLUE,
                focus: ACCENT_BLUE,
            },
            // Success — vibrant green
            success: Component {
                base: Color::from_rgb(0.133, 0.773, 0.369),  // #22c55e
                hover: Color::from_rgb(0.204, 0.820, 0.416), // #34d16a
                pressed: Color::from_rgb(0.133, 0.773, 0.369),
                selected: Color::from_rgb(0.133, 0.773, 0.369),
                disabled: Color::from_rgb(0.100, 0.500, 0.250),
                on: Color::WHITE,
                border: Color::from_rgb(0.133, 0.773, 0.369),
                focus: Color::from_rgb(0.133, 0.773, 0.369),
            },
            // Danger — clean red
            danger: Component {
                base: Color::from_rgb(0.937, 0.267, 0.267),  // #ef4444
                hover: Color::from_rgb(0.863, 0.216, 0.216), // #dc3737
                pressed: Color::from_rgb(0.937, 0.267, 0.267),
                selected: Color::from_rgb(0.937, 0.267, 0.267),
                disabled: Color::from_rgb(0.600, 0.180, 0.180),
                on: Color::WHITE,
                border: Color::from_rgb(0.937, 0.267, 0.267),
                focus: Color::from_rgb(0.937, 0.267, 0.267),
            },
            // Warning — warm amber
            warning: Component {
                base: Color::from_rgb(0.961, 0.620, 0.043),  // #f59e0b
                hover: Color::from_rgb(0.984, 0.690, 0.125), // #fbb020
                pressed: Color::from_rgb(0.961, 0.620, 0.043),
                selected: Color::from_rgb(0.961, 0.620, 0.043),
                disabled: Color::from_rgb(0.600, 0.400, 0.050),
                on: Color::WHITE,
                border: Color::from_rgb(0.961, 0.620, 0.043),
                focus: Color::from_rgb(0.961, 0.620, 0.043),
            },

            // Sent bubble — accent blue (Signal-style)
            sent_bubble: BubbleStyle {
                background: ACCENT_BLUE, // #3478F6
                on: Color::WHITE,
                on_secondary: Color::from_rgba(1.0, 1.0, 1.0, 0.65),
                link: Color::WHITE,
                radius: 18.0,
            },
            // Received bubble — clearly visible against dark canvas
            received_bubble: BubbleStyle {
                background: Color::from_rgb(0.180, 0.184, 0.220), // #2E2F38
                on: TEXT_PRIMARY,
                on_secondary: TEXT_FADED,
                link: ACCENT_BLUE,
                radius: 18.0,
            },

            avatar_bg: Color::from_rgb(0.239, 0.243, 0.271), // #3D3E45 neutral gray
            call_bg: Color::from_rgb(0.039, 0.039, 0.055),   // #0A0A0E
            call_control_base: Color::from_rgb(0.102, 0.106, 0.137), // #1A1B23
            call_control_hover: Color::from_rgb(0.149, 0.153, 0.192), // #262731

            spacing: Spacing {
                none: 0,
                xxxs: 2,
                xxs: 4,
                xs: 6,
                s: 8,
                sm: 10,
                m: 12,
                l: 16,
                xl: 20,
                xxl: 24,
                xxxl: 32,
                huge: 48,
            },
            radii: Radii {
                none: 0.0,
                xs: 4.0,
                s: 8.0,
                m: 12.0,
                l: 16.0,
                xl: 24.0,
                full: 9999.0,
            },
            typography: Typography {
                display: 36.0,
                title_lg: 24.0,
                title: 22.0,
                heading: 20.0,
                headline: 16.0,
                body: 14.0,
                label: 13.0,
                caption: 12.0,
                caption_sm: 11.0,
                small: 10.0,
                icon: 18.0,
            },
            is_dark: true,
        }
    }
}

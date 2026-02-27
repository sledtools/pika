#![allow(dead_code)]
//! Design token definitions for the Pika desktop app.
//!
//! Tokens are the lowest-level design decisions: colors, spacing, radii,
//! and typography scales. A [`PikaTheme`] bundles all tokens into a single
//! value that can be swapped to change the entire look (e.g. dark -> light).

use iced::{Background, Color};

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

    // ── Additional theme constructors ──────────────────────────────────

    /// Deep blue-black theme with purple accent.
    pub const fn midnight() -> Self {
        const TEXT_PRIMARY: Color = Color::from_rgb(0.878, 0.886, 0.941);
        const TEXT_SECONDARY: Color = Color::from_rgb(0.502, 0.518, 0.616);
        const TEXT_FADED: Color = Color::from_rgb(0.337, 0.349, 0.431);
        const DIVIDER: Color = Color::from_rgb(0.133, 0.141, 0.200);
        const INPUT_BG: Color = Color::from_rgb(0.059, 0.063, 0.102);
        const HOVER_BG: Color = Color::from_rgb(0.106, 0.114, 0.169);
        const SELECTED_BG: Color = Color::from_rgb(0.133, 0.141, 0.200);
        const ACCENT: Color = Color::from_rgb(0.553, 0.369, 0.988);

        const COMP: Component = Component {
            base: Color::TRANSPARENT,
            hover: HOVER_BG,
            pressed: SELECTED_BG,
            selected: SELECTED_BG,
            disabled: Color::TRANSPARENT,
            on: TEXT_PRIMARY,
            border: DIVIDER,
            focus: ACCENT,
        };

        Self {
            background: Surface {
                base: Color::from_rgb(0.039, 0.043, 0.075),
                on: TEXT_PRIMARY,
                on_secondary: TEXT_SECONDARY,
                on_faded: TEXT_FADED,
                divider: DIVIDER,
                input_bg: INPUT_BG,
                component: COMP,
            },
            primary: Surface {
                base: Color::from_rgb(0.067, 0.071, 0.118),
                on: TEXT_PRIMARY,
                on_secondary: TEXT_SECONDARY,
                on_faded: TEXT_FADED,
                divider: DIVIDER,
                input_bg: INPUT_BG,
                component: COMP,
            },
            secondary: Surface {
                base: Color::from_rgb(0.106, 0.114, 0.169),
                on: TEXT_PRIMARY,
                on_secondary: TEXT_SECONDARY,
                on_faded: TEXT_FADED,
                divider: DIVIDER,
                input_bg: INPUT_BG,
                component: COMP,
            },
            accent: Component {
                base: ACCENT,
                hover: Color::from_rgb(0.620, 0.443, 1.0),
                pressed: ACCENT,
                selected: ACCENT,
                disabled: Color::from_rgb(0.300, 0.200, 0.530),
                on: Color::WHITE,
                border: ACCENT,
                focus: ACCENT,
            },
            success: Component {
                base: Color::from_rgb(0.133, 0.773, 0.369),
                hover: Color::from_rgb(0.204, 0.820, 0.416),
                pressed: Color::from_rgb(0.133, 0.773, 0.369),
                selected: Color::from_rgb(0.133, 0.773, 0.369),
                disabled: Color::from_rgb(0.100, 0.500, 0.250),
                on: Color::WHITE,
                border: Color::from_rgb(0.133, 0.773, 0.369),
                focus: Color::from_rgb(0.133, 0.773, 0.369),
            },
            danger: Component {
                base: Color::from_rgb(0.937, 0.267, 0.267),
                hover: Color::from_rgb(0.863, 0.216, 0.216),
                pressed: Color::from_rgb(0.937, 0.267, 0.267),
                selected: Color::from_rgb(0.937, 0.267, 0.267),
                disabled: Color::from_rgb(0.600, 0.180, 0.180),
                on: Color::WHITE,
                border: Color::from_rgb(0.937, 0.267, 0.267),
                focus: Color::from_rgb(0.937, 0.267, 0.267),
            },
            warning: Component {
                base: Color::from_rgb(0.961, 0.620, 0.043),
                hover: Color::from_rgb(0.984, 0.690, 0.125),
                pressed: Color::from_rgb(0.961, 0.620, 0.043),
                selected: Color::from_rgb(0.961, 0.620, 0.043),
                disabled: Color::from_rgb(0.600, 0.400, 0.050),
                on: Color::WHITE,
                border: Color::from_rgb(0.961, 0.620, 0.043),
                focus: Color::from_rgb(0.961, 0.620, 0.043),
            },
            sent_bubble: BubbleStyle {
                background: ACCENT,
                on: Color::WHITE,
                on_secondary: Color::from_rgba(1.0, 1.0, 1.0, 0.65),
                link: Color::WHITE,
                radius: 18.0,
            },
            received_bubble: BubbleStyle {
                background: Color::from_rgb(0.133, 0.141, 0.200),
                on: TEXT_PRIMARY,
                on_secondary: TEXT_FADED,
                link: ACCENT,
                radius: 18.0,
            },
            avatar_bg: Color::from_rgb(0.200, 0.208, 0.286),
            call_bg: Color::from_rgb(0.024, 0.027, 0.051),
            call_control_base: Color::from_rgb(0.067, 0.071, 0.118),
            call_control_hover: Color::from_rgb(0.106, 0.114, 0.169),
            spacing: SHARED_SPACING,
            radii: SHARED_RADII,
            typography: SHARED_TYPOGRAPHY,
            is_dark: true,
        }
    }

    /// Nord-inspired cool blue-gray theme.
    pub const fn nord() -> Self {
        const TEXT_PRIMARY: Color = Color::from_rgb(0.925, 0.937, 0.957); // #ECEFF4
        const TEXT_SECONDARY: Color = Color::from_rgb(0.608, 0.639, 0.690); // #9BA3B0
        const TEXT_FADED: Color = Color::from_rgb(0.431, 0.459, 0.514); // #6E7583
        const DIVIDER: Color = Color::from_rgb(0.231, 0.259, 0.322); // #3B4252
        const INPUT_BG: Color = Color::from_rgb(0.165, 0.188, 0.243);
        const HOVER_BG: Color = Color::from_rgb(0.263, 0.298, 0.369); // #434C5E
        const SELECTED_BG: Color = Color::from_rgb(0.298, 0.337, 0.416); // #4C566A
        const ACCENT: Color = Color::from_rgb(0.533, 0.753, 0.816); // #88C0D0

        const COMP: Component = Component {
            base: Color::TRANSPARENT,
            hover: HOVER_BG,
            pressed: SELECTED_BG,
            selected: SELECTED_BG,
            disabled: Color::TRANSPARENT,
            on: TEXT_PRIMARY,
            border: DIVIDER,
            focus: ACCENT,
        };

        Self {
            background: Surface {
                base: Color::from_rgb(0.180, 0.204, 0.251), // #2E3440
                on: TEXT_PRIMARY,
                on_secondary: TEXT_SECONDARY,
                on_faded: TEXT_FADED,
                divider: DIVIDER,
                input_bg: INPUT_BG,
                component: COMP,
            },
            primary: Surface {
                base: Color::from_rgb(0.231, 0.259, 0.322), // #3B4252
                on: TEXT_PRIMARY,
                on_secondary: TEXT_SECONDARY,
                on_faded: TEXT_FADED,
                divider: DIVIDER,
                input_bg: INPUT_BG,
                component: COMP,
            },
            secondary: Surface {
                base: Color::from_rgb(0.263, 0.298, 0.369), // #434C5E
                on: TEXT_PRIMARY,
                on_secondary: TEXT_SECONDARY,
                on_faded: TEXT_FADED,
                divider: DIVIDER,
                input_bg: INPUT_BG,
                component: COMP,
            },
            accent: Component {
                base: ACCENT,
                hover: Color::from_rgb(0.600, 0.808, 0.871),
                pressed: ACCENT,
                selected: ACCENT,
                disabled: Color::from_rgb(0.310, 0.435, 0.471),
                on: Color::from_rgb(0.180, 0.204, 0.251),
                border: ACCENT,
                focus: ACCENT,
            },
            success: Component {
                base: Color::from_rgb(0.639, 0.745, 0.549), // #A3BE8C
                hover: Color::from_rgb(0.698, 0.800, 0.612),
                pressed: Color::from_rgb(0.639, 0.745, 0.549),
                selected: Color::from_rgb(0.639, 0.745, 0.549),
                disabled: Color::from_rgb(0.400, 0.475, 0.349),
                on: Color::from_rgb(0.180, 0.204, 0.251),
                border: Color::from_rgb(0.639, 0.745, 0.549),
                focus: Color::from_rgb(0.639, 0.745, 0.549),
            },
            danger: Component {
                base: Color::from_rgb(0.749, 0.380, 0.416), // #BF616A
                hover: Color::from_rgb(0.812, 0.443, 0.478),
                pressed: Color::from_rgb(0.749, 0.380, 0.416),
                selected: Color::from_rgb(0.749, 0.380, 0.416),
                disabled: Color::from_rgb(0.475, 0.243, 0.267),
                on: Color::WHITE,
                border: Color::from_rgb(0.749, 0.380, 0.416),
                focus: Color::from_rgb(0.749, 0.380, 0.416),
            },
            warning: Component {
                base: Color::from_rgb(0.922, 0.796, 0.545), // #EBCB8B
                hover: Color::from_rgb(0.953, 0.843, 0.612),
                pressed: Color::from_rgb(0.922, 0.796, 0.545),
                selected: Color::from_rgb(0.922, 0.796, 0.545),
                disabled: Color::from_rgb(0.580, 0.502, 0.345),
                on: Color::from_rgb(0.180, 0.204, 0.251),
                border: Color::from_rgb(0.922, 0.796, 0.545),
                focus: Color::from_rgb(0.922, 0.796, 0.545),
            },
            sent_bubble: BubbleStyle {
                background: Color::from_rgb(0.369, 0.506, 0.675), // muted steel blue
                on: Color::WHITE,
                on_secondary: Color::from_rgba(1.0, 1.0, 1.0, 0.65),
                link: Color::WHITE,
                radius: 18.0,
            },
            received_bubble: BubbleStyle {
                background: Color::from_rgb(0.263, 0.298, 0.369),
                on: TEXT_PRIMARY,
                on_secondary: TEXT_FADED,
                link: ACCENT,
                radius: 18.0,
            },
            avatar_bg: Color::from_rgb(0.298, 0.337, 0.416),
            call_bg: Color::from_rgb(0.149, 0.169, 0.212),
            call_control_base: Color::from_rgb(0.231, 0.259, 0.322),
            call_control_hover: Color::from_rgb(0.263, 0.298, 0.369),
            spacing: SHARED_SPACING,
            radii: SHARED_RADII,
            typography: SHARED_TYPOGRAPHY,
            is_dark: true,
        }
    }

    /// Catppuccin Mocha — warm dark theme with lavender accent.
    pub const fn catppuccin_mocha() -> Self {
        const TEXT_PRIMARY: Color = Color::from_rgb(0.804, 0.839, 0.957); // #CDD6F4
        const TEXT_SECONDARY: Color = Color::from_rgb(0.580, 0.612, 0.745);
        const TEXT_FADED: Color = Color::from_rgb(0.427, 0.447, 0.553);
        const DIVIDER: Color = Color::from_rgb(0.184, 0.192, 0.263); // #313244
        const INPUT_BG: Color = Color::from_rgb(0.118, 0.122, 0.188);
        const HOVER_BG: Color = Color::from_rgb(0.216, 0.224, 0.306);
        const SELECTED_BG: Color = Color::from_rgb(0.247, 0.255, 0.341);
        const ACCENT: Color = Color::from_rgb(0.702, 0.671, 0.969); // #B4BEFE lavender

        const COMP: Component = Component {
            base: Color::TRANSPARENT,
            hover: HOVER_BG,
            pressed: SELECTED_BG,
            selected: SELECTED_BG,
            disabled: Color::TRANSPARENT,
            on: TEXT_PRIMARY,
            border: DIVIDER,
            focus: ACCENT,
        };

        Self {
            background: Surface {
                base: Color::from_rgb(0.118, 0.118, 0.180), // #1E1E2E
                on: TEXT_PRIMARY,
                on_secondary: TEXT_SECONDARY,
                on_faded: TEXT_FADED,
                divider: DIVIDER,
                input_bg: INPUT_BG,
                component: COMP,
            },
            primary: Surface {
                base: Color::from_rgb(0.149, 0.153, 0.227),
                on: TEXT_PRIMARY,
                on_secondary: TEXT_SECONDARY,
                on_faded: TEXT_FADED,
                divider: DIVIDER,
                input_bg: INPUT_BG,
                component: COMP,
            },
            secondary: Surface {
                base: Color::from_rgb(0.184, 0.192, 0.263),
                on: TEXT_PRIMARY,
                on_secondary: TEXT_SECONDARY,
                on_faded: TEXT_FADED,
                divider: DIVIDER,
                input_bg: INPUT_BG,
                component: COMP,
            },
            accent: Component {
                base: ACCENT,
                hover: Color::from_rgb(0.769, 0.741, 1.0),
                pressed: ACCENT,
                selected: ACCENT,
                disabled: Color::from_rgb(0.400, 0.380, 0.549),
                on: Color::from_rgb(0.118, 0.118, 0.180),
                border: ACCENT,
                focus: ACCENT,
            },
            success: Component {
                base: Color::from_rgb(0.651, 0.890, 0.631), // #A6E3A1
                hover: Color::from_rgb(0.718, 0.922, 0.698),
                pressed: Color::from_rgb(0.651, 0.890, 0.631),
                selected: Color::from_rgb(0.651, 0.890, 0.631),
                disabled: Color::from_rgb(0.380, 0.530, 0.369),
                on: Color::from_rgb(0.118, 0.118, 0.180),
                border: Color::from_rgb(0.651, 0.890, 0.631),
                focus: Color::from_rgb(0.651, 0.890, 0.631),
            },
            danger: Component {
                base: Color::from_rgb(0.953, 0.545, 0.659), // #F38BA8
                hover: Color::from_rgb(0.976, 0.612, 0.718),
                pressed: Color::from_rgb(0.953, 0.545, 0.659),
                selected: Color::from_rgb(0.953, 0.545, 0.659),
                disabled: Color::from_rgb(0.569, 0.325, 0.396),
                on: Color::from_rgb(0.118, 0.118, 0.180),
                border: Color::from_rgb(0.953, 0.545, 0.659),
                focus: Color::from_rgb(0.953, 0.545, 0.659),
            },
            warning: Component {
                base: Color::from_rgb(0.980, 0.886, 0.686), // #F9E2AF
                hover: Color::from_rgb(0.992, 0.918, 0.749),
                pressed: Color::from_rgb(0.980, 0.886, 0.686),
                selected: Color::from_rgb(0.980, 0.886, 0.686),
                disabled: Color::from_rgb(0.588, 0.529, 0.412),
                on: Color::from_rgb(0.118, 0.118, 0.180),
                border: Color::from_rgb(0.980, 0.886, 0.686),
                focus: Color::from_rgb(0.980, 0.886, 0.686),
            },
            sent_bubble: BubbleStyle {
                background: Color::from_rgb(0.522, 0.498, 0.718), // muted lavender
                on: Color::WHITE,
                on_secondary: Color::from_rgba(1.0, 1.0, 1.0, 0.65),
                link: Color::WHITE,
                radius: 18.0,
            },
            received_bubble: BubbleStyle {
                background: Color::from_rgb(0.184, 0.192, 0.263),
                on: TEXT_PRIMARY,
                on_secondary: TEXT_FADED,
                link: ACCENT,
                radius: 18.0,
            },
            avatar_bg: Color::from_rgb(0.247, 0.255, 0.341),
            call_bg: Color::from_rgb(0.082, 0.082, 0.137),
            call_control_base: Color::from_rgb(0.149, 0.153, 0.227),
            call_control_hover: Color::from_rgb(0.184, 0.192, 0.263),
            spacing: SHARED_SPACING,
            radii: SHARED_RADII,
            typography: SHARED_TYPOGRAPHY,
            is_dark: true,
        }
    }

    /// Rosé Pine — muted warm dark with rose gold accent.
    pub const fn rose_pine() -> Self {
        const TEXT_PRIMARY: Color = Color::from_rgb(0.878, 0.847, 0.863); // #E0D6DC
        const TEXT_SECONDARY: Color = Color::from_rgb(0.576, 0.545, 0.573);
        const TEXT_FADED: Color = Color::from_rgb(0.424, 0.396, 0.424);
        const DIVIDER: Color = Color::from_rgb(0.161, 0.141, 0.176);
        const INPUT_BG: Color = Color::from_rgb(0.098, 0.082, 0.114);
        const HOVER_BG: Color = Color::from_rgb(0.192, 0.173, 0.212);
        const SELECTED_BG: Color = Color::from_rgb(0.224, 0.204, 0.247);
        const ACCENT: Color = Color::from_rgb(0.918, 0.647, 0.588); // #EAA596 rose

        const COMP: Component = Component {
            base: Color::TRANSPARENT,
            hover: HOVER_BG,
            pressed: SELECTED_BG,
            selected: SELECTED_BG,
            disabled: Color::TRANSPARENT,
            on: TEXT_PRIMARY,
            border: DIVIDER,
            focus: ACCENT,
        };

        Self {
            background: Surface {
                base: Color::from_rgb(0.098, 0.082, 0.114), // #191521
                on: TEXT_PRIMARY,
                on_secondary: TEXT_SECONDARY,
                on_faded: TEXT_FADED,
                divider: DIVIDER,
                input_bg: INPUT_BG,
                component: COMP,
            },
            primary: Surface {
                base: Color::from_rgb(0.133, 0.114, 0.149),
                on: TEXT_PRIMARY,
                on_secondary: TEXT_SECONDARY,
                on_faded: TEXT_FADED,
                divider: DIVIDER,
                input_bg: INPUT_BG,
                component: COMP,
            },
            secondary: Surface {
                base: Color::from_rgb(0.173, 0.153, 0.192),
                on: TEXT_PRIMARY,
                on_secondary: TEXT_SECONDARY,
                on_faded: TEXT_FADED,
                divider: DIVIDER,
                input_bg: INPUT_BG,
                component: COMP,
            },
            accent: Component {
                base: ACCENT,
                hover: Color::from_rgb(0.949, 0.718, 0.659),
                pressed: ACCENT,
                selected: ACCENT,
                disabled: Color::from_rgb(0.549, 0.388, 0.353),
                on: Color::from_rgb(0.098, 0.082, 0.114),
                border: ACCENT,
                focus: ACCENT,
            },
            success: Component {
                base: Color::from_rgb(0.565, 0.745, 0.537),
                hover: Color::from_rgb(0.631, 0.800, 0.604),
                pressed: Color::from_rgb(0.565, 0.745, 0.537),
                selected: Color::from_rgb(0.565, 0.745, 0.537),
                disabled: Color::from_rgb(0.345, 0.455, 0.325),
                on: Color::from_rgb(0.098, 0.082, 0.114),
                border: Color::from_rgb(0.565, 0.745, 0.537),
                focus: Color::from_rgb(0.565, 0.745, 0.537),
            },
            danger: Component {
                base: Color::from_rgb(0.922, 0.404, 0.404),
                hover: Color::from_rgb(0.949, 0.478, 0.478),
                pressed: Color::from_rgb(0.922, 0.404, 0.404),
                selected: Color::from_rgb(0.922, 0.404, 0.404),
                disabled: Color::from_rgb(0.553, 0.243, 0.243),
                on: Color::WHITE,
                border: Color::from_rgb(0.922, 0.404, 0.404),
                focus: Color::from_rgb(0.922, 0.404, 0.404),
            },
            warning: Component {
                base: Color::from_rgb(0.957, 0.816, 0.494),
                hover: Color::from_rgb(0.976, 0.859, 0.565),
                pressed: Color::from_rgb(0.957, 0.816, 0.494),
                selected: Color::from_rgb(0.957, 0.816, 0.494),
                disabled: Color::from_rgb(0.573, 0.490, 0.298),
                on: Color::from_rgb(0.098, 0.082, 0.114),
                border: Color::from_rgb(0.957, 0.816, 0.494),
                focus: Color::from_rgb(0.957, 0.816, 0.494),
            },
            sent_bubble: BubbleStyle {
                background: Color::from_rgb(0.667, 0.467, 0.427),
                on: Color::WHITE,
                on_secondary: Color::from_rgba(1.0, 1.0, 1.0, 0.65),
                link: Color::WHITE,
                radius: 18.0,
            },
            received_bubble: BubbleStyle {
                background: Color::from_rgb(0.173, 0.153, 0.192),
                on: TEXT_PRIMARY,
                on_secondary: TEXT_FADED,
                link: ACCENT,
                radius: 18.0,
            },
            avatar_bg: Color::from_rgb(0.224, 0.204, 0.247),
            call_bg: Color::from_rgb(0.067, 0.055, 0.078),
            call_control_base: Color::from_rgb(0.133, 0.114, 0.149),
            call_control_hover: Color::from_rgb(0.173, 0.153, 0.192),
            spacing: SHARED_SPACING,
            radii: SHARED_RADII,
            typography: SHARED_TYPOGRAPHY,
            is_dark: true,
        }
    }

    /// Solarized Dark — Ethan Schoonover's warm dark palette with teal accent.
    pub const fn solarized_dark() -> Self {
        const TEXT_PRIMARY: Color = Color::from_rgb(0.514, 0.580, 0.588); // #839496 base0
        const TEXT_SECONDARY: Color = Color::from_rgb(0.396, 0.482, 0.514);
        const TEXT_FADED: Color = Color::from_rgb(0.345, 0.431, 0.459); // #586e75 base01
        const DIVIDER: Color = Color::from_rgb(0.027, 0.212, 0.259); // #073642 base02
        const INPUT_BG: Color = Color::from_rgb(0.0, 0.145, 0.180);
        const HOVER_BG: Color = Color::from_rgb(0.027, 0.212, 0.259);
        const SELECTED_BG: Color = Color::from_rgb(0.059, 0.259, 0.310);
        const ACCENT: Color = Color::from_rgb(0.149, 0.545, 0.824); // #268BD2 blue

        const COMP: Component = Component {
            base: Color::TRANSPARENT,
            hover: HOVER_BG,
            pressed: SELECTED_BG,
            selected: SELECTED_BG,
            disabled: Color::TRANSPARENT,
            on: TEXT_PRIMARY,
            border: DIVIDER,
            focus: ACCENT,
        };

        Self {
            background: Surface {
                base: Color::from_rgb(0.0, 0.169, 0.212), // #002B36 base03
                on: TEXT_PRIMARY,
                on_secondary: TEXT_SECONDARY,
                on_faded: TEXT_FADED,
                divider: DIVIDER,
                input_bg: INPUT_BG,
                component: COMP,
            },
            primary: Surface {
                base: Color::from_rgb(0.027, 0.212, 0.259), // #073642 base02
                on: TEXT_PRIMARY,
                on_secondary: TEXT_SECONDARY,
                on_faded: TEXT_FADED,
                divider: DIVIDER,
                input_bg: INPUT_BG,
                component: COMP,
            },
            secondary: Surface {
                base: Color::from_rgb(0.059, 0.259, 0.310),
                on: TEXT_PRIMARY,
                on_secondary: TEXT_SECONDARY,
                on_faded: TEXT_FADED,
                divider: DIVIDER,
                input_bg: INPUT_BG,
                component: COMP,
            },
            accent: Component {
                base: ACCENT,
                hover: Color::from_rgb(0.220, 0.616, 0.886),
                pressed: ACCENT,
                selected: ACCENT,
                disabled: Color::from_rgb(0.090, 0.329, 0.498),
                on: Color::from_rgb(0.992, 0.965, 0.890),
                border: ACCENT,
                focus: ACCENT,
            },
            success: Component {
                base: Color::from_rgb(0.522, 0.600, 0.0), // #859900
                hover: Color::from_rgb(0.588, 0.667, 0.067),
                pressed: Color::from_rgb(0.522, 0.600, 0.0),
                selected: Color::from_rgb(0.522, 0.600, 0.0),
                disabled: Color::from_rgb(0.310, 0.357, 0.0),
                on: Color::from_rgb(0.992, 0.965, 0.890),
                border: Color::from_rgb(0.522, 0.600, 0.0),
                focus: Color::from_rgb(0.522, 0.600, 0.0),
            },
            danger: Component {
                base: Color::from_rgb(0.863, 0.196, 0.184), // #DC322F
                hover: Color::from_rgb(0.914, 0.267, 0.255),
                pressed: Color::from_rgb(0.863, 0.196, 0.184),
                selected: Color::from_rgb(0.863, 0.196, 0.184),
                disabled: Color::from_rgb(0.518, 0.118, 0.110),
                on: Color::from_rgb(0.992, 0.965, 0.890),
                border: Color::from_rgb(0.863, 0.196, 0.184),
                focus: Color::from_rgb(0.863, 0.196, 0.184),
            },
            warning: Component {
                base: Color::from_rgb(0.710, 0.537, 0.0), // #B58900
                hover: Color::from_rgb(0.780, 0.604, 0.067),
                pressed: Color::from_rgb(0.710, 0.537, 0.0),
                selected: Color::from_rgb(0.710, 0.537, 0.0),
                disabled: Color::from_rgb(0.427, 0.322, 0.0),
                on: Color::from_rgb(0.992, 0.965, 0.890),
                border: Color::from_rgb(0.710, 0.537, 0.0),
                focus: Color::from_rgb(0.710, 0.537, 0.0),
            },
            sent_bubble: BubbleStyle {
                background: ACCENT,
                on: Color::from_rgb(0.992, 0.965, 0.890),
                on_secondary: Color::from_rgba(0.992, 0.965, 0.890, 0.65),
                link: Color::from_rgb(0.992, 0.965, 0.890),
                radius: 18.0,
            },
            received_bubble: BubbleStyle {
                background: Color::from_rgb(0.027, 0.212, 0.259),
                on: TEXT_PRIMARY,
                on_secondary: TEXT_FADED,
                link: ACCENT,
                radius: 18.0,
            },
            avatar_bg: Color::from_rgb(0.059, 0.259, 0.310),
            call_bg: Color::from_rgb(0.0, 0.118, 0.149),
            call_control_base: Color::from_rgb(0.027, 0.212, 0.259),
            call_control_hover: Color::from_rgb(0.059, 0.259, 0.310),
            spacing: SHARED_SPACING,
            radii: SHARED_RADII,
            typography: SHARED_TYPOGRAPHY,
            is_dark: true,
        }
    }

    /// Moonlight — soft blue-gray with green-teal accent.
    pub const fn moonlight() -> Self {
        const TEXT_PRIMARY: Color = Color::from_rgb(0.780, 0.831, 0.902);
        const TEXT_SECONDARY: Color = Color::from_rgb(0.506, 0.561, 0.643);
        const TEXT_FADED: Color = Color::from_rgb(0.369, 0.412, 0.490);
        const DIVIDER: Color = Color::from_rgb(0.161, 0.184, 0.235);
        const INPUT_BG: Color = Color::from_rgb(0.106, 0.122, 0.165);
        const HOVER_BG: Color = Color::from_rgb(0.192, 0.216, 0.275);
        const SELECTED_BG: Color = Color::from_rgb(0.220, 0.247, 0.310);
        const ACCENT: Color = Color::from_rgb(0.392, 0.808, 0.667); // teal-green

        const COMP: Component = Component {
            base: Color::TRANSPARENT,
            hover: HOVER_BG,
            pressed: SELECTED_BG,
            selected: SELECTED_BG,
            disabled: Color::TRANSPARENT,
            on: TEXT_PRIMARY,
            border: DIVIDER,
            focus: ACCENT,
        };

        Self {
            background: Surface {
                base: Color::from_rgb(0.114, 0.133, 0.173),
                on: TEXT_PRIMARY,
                on_secondary: TEXT_SECONDARY,
                on_faded: TEXT_FADED,
                divider: DIVIDER,
                input_bg: INPUT_BG,
                component: COMP,
            },
            primary: Surface {
                base: Color::from_rgb(0.145, 0.165, 0.212),
                on: TEXT_PRIMARY,
                on_secondary: TEXT_SECONDARY,
                on_faded: TEXT_FADED,
                divider: DIVIDER,
                input_bg: INPUT_BG,
                component: COMP,
            },
            secondary: Surface {
                base: Color::from_rgb(0.192, 0.216, 0.275),
                on: TEXT_PRIMARY,
                on_secondary: TEXT_SECONDARY,
                on_faded: TEXT_FADED,
                divider: DIVIDER,
                input_bg: INPUT_BG,
                component: COMP,
            },
            accent: Component {
                base: ACCENT,
                hover: Color::from_rgb(0.463, 0.859, 0.725),
                pressed: ACCENT,
                selected: ACCENT,
                disabled: Color::from_rgb(0.235, 0.486, 0.400),
                on: Color::from_rgb(0.098, 0.110, 0.149),
                border: ACCENT,
                focus: ACCENT,
            },
            success: Component {
                base: Color::from_rgb(0.392, 0.808, 0.667),
                hover: Color::from_rgb(0.463, 0.859, 0.725),
                pressed: Color::from_rgb(0.392, 0.808, 0.667),
                selected: Color::from_rgb(0.392, 0.808, 0.667),
                disabled: Color::from_rgb(0.235, 0.486, 0.400),
                on: Color::from_rgb(0.098, 0.110, 0.149),
                border: Color::from_rgb(0.392, 0.808, 0.667),
                focus: Color::from_rgb(0.392, 0.808, 0.667),
            },
            danger: Component {
                base: Color::from_rgb(0.937, 0.353, 0.392),
                hover: Color::from_rgb(0.961, 0.427, 0.463),
                pressed: Color::from_rgb(0.937, 0.353, 0.392),
                selected: Color::from_rgb(0.937, 0.353, 0.392),
                disabled: Color::from_rgb(0.561, 0.212, 0.235),
                on: Color::WHITE,
                border: Color::from_rgb(0.937, 0.353, 0.392),
                focus: Color::from_rgb(0.937, 0.353, 0.392),
            },
            warning: Component {
                base: Color::from_rgb(1.0, 0.776, 0.329),
                hover: Color::from_rgb(1.0, 0.824, 0.420),
                pressed: Color::from_rgb(1.0, 0.776, 0.329),
                selected: Color::from_rgb(1.0, 0.776, 0.329),
                disabled: Color::from_rgb(0.600, 0.467, 0.198),
                on: Color::from_rgb(0.098, 0.110, 0.149),
                border: Color::from_rgb(1.0, 0.776, 0.329),
                focus: Color::from_rgb(1.0, 0.776, 0.329),
            },
            sent_bubble: BubbleStyle {
                background: Color::from_rgb(0.286, 0.588, 0.486),
                on: Color::WHITE,
                on_secondary: Color::from_rgba(1.0, 1.0, 1.0, 0.65),
                link: Color::WHITE,
                radius: 18.0,
            },
            received_bubble: BubbleStyle {
                background: Color::from_rgb(0.192, 0.216, 0.275),
                on: TEXT_PRIMARY,
                on_secondary: TEXT_FADED,
                link: ACCENT,
                radius: 18.0,
            },
            avatar_bg: Color::from_rgb(0.220, 0.247, 0.310),
            call_bg: Color::from_rgb(0.078, 0.090, 0.122),
            call_control_base: Color::from_rgb(0.145, 0.165, 0.212),
            call_control_hover: Color::from_rgb(0.192, 0.216, 0.275),
            spacing: SHARED_SPACING,
            radii: SHARED_RADII,
            typography: SHARED_TYPOGRAPHY,
            is_dark: true,
        }
    }

    /// Dawn — warm light theme with soft rose accent.
    pub const fn dawn() -> Self {
        const TEXT_PRIMARY: Color = Color::from_rgb(0.224, 0.204, 0.247);
        const TEXT_SECONDARY: Color = Color::from_rgb(0.447, 0.420, 0.478);
        const TEXT_FADED: Color = Color::from_rgb(0.576, 0.553, 0.608);
        const DIVIDER: Color = Color::from_rgb(0.855, 0.835, 0.871);
        const INPUT_BG: Color = Color::from_rgb(0.949, 0.937, 0.957);
        const HOVER_BG: Color = Color::from_rgb(0.910, 0.894, 0.925);
        const SELECTED_BG: Color = Color::from_rgb(0.878, 0.859, 0.898);
        const ACCENT: Color = Color::from_rgb(0.831, 0.467, 0.455); // dusty rose

        const COMP: Component = Component {
            base: Color::TRANSPARENT,
            hover: HOVER_BG,
            pressed: SELECTED_BG,
            selected: SELECTED_BG,
            disabled: Color::TRANSPARENT,
            on: TEXT_PRIMARY,
            border: DIVIDER,
            focus: ACCENT,
        };

        Self {
            background: Surface {
                base: Color::from_rgb(0.976, 0.965, 0.984),
                on: TEXT_PRIMARY,
                on_secondary: TEXT_SECONDARY,
                on_faded: TEXT_FADED,
                divider: DIVIDER,
                input_bg: INPUT_BG,
                component: COMP,
            },
            primary: Surface {
                base: Color::from_rgb(0.949, 0.937, 0.957),
                on: TEXT_PRIMARY,
                on_secondary: TEXT_SECONDARY,
                on_faded: TEXT_FADED,
                divider: DIVIDER,
                input_bg: INPUT_BG,
                component: COMP,
            },
            secondary: Surface {
                base: Color::from_rgb(0.918, 0.902, 0.933),
                on: TEXT_PRIMARY,
                on_secondary: TEXT_SECONDARY,
                on_faded: TEXT_FADED,
                divider: DIVIDER,
                input_bg: INPUT_BG,
                component: COMP,
            },
            accent: Component {
                base: ACCENT,
                hover: Color::from_rgb(0.878, 0.533, 0.522),
                pressed: ACCENT,
                selected: ACCENT,
                disabled: Color::from_rgb(0.729, 0.600, 0.596),
                on: Color::WHITE,
                border: ACCENT,
                focus: ACCENT,
            },
            success: Component {
                base: Color::from_rgb(0.286, 0.667, 0.404),
                hover: Color::from_rgb(0.349, 0.725, 0.463),
                pressed: Color::from_rgb(0.286, 0.667, 0.404),
                selected: Color::from_rgb(0.286, 0.667, 0.404),
                disabled: Color::from_rgb(0.490, 0.706, 0.549),
                on: Color::WHITE,
                border: Color::from_rgb(0.286, 0.667, 0.404),
                focus: Color::from_rgb(0.286, 0.667, 0.404),
            },
            danger: Component {
                base: Color::from_rgb(0.886, 0.271, 0.271),
                hover: Color::from_rgb(0.918, 0.337, 0.337),
                pressed: Color::from_rgb(0.886, 0.271, 0.271),
                selected: Color::from_rgb(0.886, 0.271, 0.271),
                disabled: Color::from_rgb(0.757, 0.502, 0.502),
                on: Color::WHITE,
                border: Color::from_rgb(0.886, 0.271, 0.271),
                focus: Color::from_rgb(0.886, 0.271, 0.271),
            },
            warning: Component {
                base: Color::from_rgb(0.878, 0.620, 0.184),
                hover: Color::from_rgb(0.914, 0.678, 0.271),
                pressed: Color::from_rgb(0.878, 0.620, 0.184),
                selected: Color::from_rgb(0.878, 0.620, 0.184),
                disabled: Color::from_rgb(0.729, 0.639, 0.455),
                on: Color::WHITE,
                border: Color::from_rgb(0.878, 0.620, 0.184),
                focus: Color::from_rgb(0.878, 0.620, 0.184),
            },
            sent_bubble: BubbleStyle {
                background: ACCENT,
                on: Color::WHITE,
                on_secondary: Color::from_rgba(1.0, 1.0, 1.0, 0.75),
                link: Color::WHITE,
                radius: 18.0,
            },
            received_bubble: BubbleStyle {
                background: Color::from_rgb(0.918, 0.902, 0.933),
                on: TEXT_PRIMARY,
                on_secondary: TEXT_FADED,
                link: ACCENT,
                radius: 18.0,
            },
            avatar_bg: Color::from_rgb(0.808, 0.788, 0.831),
            call_bg: Color::from_rgb(0.937, 0.922, 0.949),
            call_control_base: Color::from_rgb(0.918, 0.902, 0.933),
            call_control_hover: Color::from_rgb(0.878, 0.859, 0.898),
            spacing: SHARED_SPACING,
            radii: SHARED_RADII,
            typography: SHARED_TYPOGRAPHY,
            is_dark: false,
        }
    }

    /// Solarized Light — Ethan Schoonover's warm light palette.
    pub const fn solarized_light() -> Self {
        const TEXT_PRIMARY: Color = Color::from_rgb(0.396, 0.482, 0.514); // #657B83 base00
        const TEXT_SECONDARY: Color = Color::from_rgb(0.514, 0.580, 0.588);
        const TEXT_FADED: Color = Color::from_rgb(0.576, 0.631, 0.631); // #93A1A1 base1
        const DIVIDER: Color = Color::from_rgb(0.933, 0.910, 0.835); // #EEE8D5 base2
        const INPUT_BG: Color = Color::from_rgb(0.933, 0.910, 0.835);
        const HOVER_BG: Color = Color::from_rgb(0.910, 0.886, 0.808);
        const SELECTED_BG: Color = Color::from_rgb(0.886, 0.859, 0.776);
        const ACCENT: Color = Color::from_rgb(0.149, 0.545, 0.824); // #268BD2 blue

        const COMP: Component = Component {
            base: Color::TRANSPARENT,
            hover: HOVER_BG,
            pressed: SELECTED_BG,
            selected: SELECTED_BG,
            disabled: Color::TRANSPARENT,
            on: TEXT_PRIMARY,
            border: DIVIDER,
            focus: ACCENT,
        };

        Self {
            background: Surface {
                base: Color::from_rgb(0.992, 0.965, 0.890), // #FDF6E3 base3
                on: TEXT_PRIMARY,
                on_secondary: TEXT_SECONDARY,
                on_faded: TEXT_FADED,
                divider: DIVIDER,
                input_bg: INPUT_BG,
                component: COMP,
            },
            primary: Surface {
                base: Color::from_rgb(0.933, 0.910, 0.835), // #EEE8D5 base2
                on: TEXT_PRIMARY,
                on_secondary: TEXT_SECONDARY,
                on_faded: TEXT_FADED,
                divider: DIVIDER,
                input_bg: INPUT_BG,
                component: COMP,
            },
            secondary: Surface {
                base: Color::from_rgb(0.910, 0.886, 0.808),
                on: TEXT_PRIMARY,
                on_secondary: TEXT_SECONDARY,
                on_faded: TEXT_FADED,
                divider: DIVIDER,
                input_bg: INPUT_BG,
                component: COMP,
            },
            accent: Component {
                base: ACCENT,
                hover: Color::from_rgb(0.220, 0.616, 0.886),
                pressed: ACCENT,
                selected: ACCENT,
                disabled: Color::from_rgb(0.506, 0.671, 0.808),
                on: Color::from_rgb(0.992, 0.965, 0.890),
                border: ACCENT,
                focus: ACCENT,
            },
            success: Component {
                base: Color::from_rgb(0.522, 0.600, 0.0),
                hover: Color::from_rgb(0.588, 0.667, 0.067),
                pressed: Color::from_rgb(0.522, 0.600, 0.0),
                selected: Color::from_rgb(0.522, 0.600, 0.0),
                disabled: Color::from_rgb(0.659, 0.698, 0.400),
                on: Color::from_rgb(0.992, 0.965, 0.890),
                border: Color::from_rgb(0.522, 0.600, 0.0),
                focus: Color::from_rgb(0.522, 0.600, 0.0),
            },
            danger: Component {
                base: Color::from_rgb(0.863, 0.196, 0.184),
                hover: Color::from_rgb(0.914, 0.267, 0.255),
                pressed: Color::from_rgb(0.863, 0.196, 0.184),
                selected: Color::from_rgb(0.863, 0.196, 0.184),
                disabled: Color::from_rgb(0.808, 0.502, 0.498),
                on: Color::from_rgb(0.992, 0.965, 0.890),
                border: Color::from_rgb(0.863, 0.196, 0.184),
                focus: Color::from_rgb(0.863, 0.196, 0.184),
            },
            warning: Component {
                base: Color::from_rgb(0.710, 0.537, 0.0),
                hover: Color::from_rgb(0.780, 0.604, 0.067),
                pressed: Color::from_rgb(0.710, 0.537, 0.0),
                selected: Color::from_rgb(0.710, 0.537, 0.0),
                disabled: Color::from_rgb(0.757, 0.663, 0.400),
                on: Color::from_rgb(0.992, 0.965, 0.890),
                border: Color::from_rgb(0.710, 0.537, 0.0),
                focus: Color::from_rgb(0.710, 0.537, 0.0),
            },
            sent_bubble: BubbleStyle {
                background: ACCENT,
                on: Color::from_rgb(0.992, 0.965, 0.890),
                on_secondary: Color::from_rgba(0.992, 0.965, 0.890, 0.70),
                link: Color::from_rgb(0.992, 0.965, 0.890),
                radius: 18.0,
            },
            received_bubble: BubbleStyle {
                background: Color::from_rgb(0.933, 0.910, 0.835),
                on: TEXT_PRIMARY,
                on_secondary: TEXT_FADED,
                link: ACCENT,
                radius: 18.0,
            },
            avatar_bg: Color::from_rgb(0.831, 0.808, 0.749),
            call_bg: Color::from_rgb(0.961, 0.937, 0.867),
            call_control_base: Color::from_rgb(0.933, 0.910, 0.835),
            call_control_hover: Color::from_rgb(0.910, 0.886, 0.808),
            spacing: SHARED_SPACING,
            radii: SHARED_RADII,
            typography: SHARED_TYPOGRAPHY,
            is_dark: false,
        }
    }

    /// Ferra — warm dark theme with salmon accent.
    pub const fn ferra() -> Self {
        const TEXT_PRIMARY: Color = Color::from_rgb(0.867, 0.827, 0.808); // #DDD3CE
        const TEXT_SECONDARY: Color = Color::from_rgb(0.620, 0.580, 0.561); // #9E9490
        const TEXT_FADED: Color = Color::from_rgb(0.467, 0.435, 0.420); // #776F6B
        const DIVIDER: Color = Color::from_rgb(0.220, 0.196, 0.188); // #38322F
        const INPUT_BG: Color = Color::from_rgb(0.149, 0.133, 0.125); // #262220
        const HOVER_BG: Color = Color::from_rgb(0.196, 0.176, 0.165); // #322D2A
        const SELECTED_BG: Color = Color::from_rgb(0.243, 0.220, 0.208); // #3E3835
        const ACCENT: Color = Color::from_rgb(0.918, 0.667, 0.569); // #EAA991

        const COMP: Component = Component {
            base: Color::TRANSPARENT,
            hover: HOVER_BG,
            pressed: SELECTED_BG,
            selected: SELECTED_BG,
            disabled: Color::TRANSPARENT,
            on: TEXT_PRIMARY,
            border: DIVIDER,
            focus: ACCENT,
        };

        Self {
            background: Surface {
                base: Color::from_rgb(0.118, 0.106, 0.098), // #211B19
                on: TEXT_PRIMARY,
                on_secondary: TEXT_SECONDARY,
                on_faded: TEXT_FADED,
                divider: DIVIDER,
                input_bg: INPUT_BG,
                component: COMP,
            },
            primary: Surface {
                base: Color::from_rgb(0.149, 0.133, 0.125), // #262220
                on: TEXT_PRIMARY,
                on_secondary: TEXT_SECONDARY,
                on_faded: TEXT_FADED,
                divider: DIVIDER,
                input_bg: INPUT_BG,
                component: COMP,
            },
            secondary: Surface {
                base: Color::from_rgb(0.196, 0.176, 0.165), // #322D2A
                on: TEXT_PRIMARY,
                on_secondary: TEXT_SECONDARY,
                on_faded: TEXT_FADED,
                divider: DIVIDER,
                input_bg: INPUT_BG,
                component: COMP,
            },
            accent: Component {
                base: ACCENT,
                hover: Color::from_rgb(0.941, 0.722, 0.635),
                pressed: ACCENT,
                selected: ACCENT,
                disabled: Color::from_rgb(0.600, 0.450, 0.380),
                on: Color::from_rgb(0.118, 0.106, 0.098),
                border: ACCENT,
                focus: ACCENT,
            },
            success: Component {
                base: Color::from_rgb(0.529, 0.737, 0.553), // #87BC8D
                hover: Color::from_rgb(0.588, 0.780, 0.608),
                pressed: Color::from_rgb(0.529, 0.737, 0.553),
                selected: Color::from_rgb(0.529, 0.737, 0.553),
                disabled: Color::from_rgb(0.350, 0.500, 0.370),
                on: Color::from_rgb(0.118, 0.106, 0.098),
                border: Color::from_rgb(0.529, 0.737, 0.553),
                focus: Color::from_rgb(0.529, 0.737, 0.553),
            },
            danger: Component {
                base: Color::from_rgb(0.882, 0.427, 0.447), // #E16D7A
                hover: Color::from_rgb(0.910, 0.498, 0.514),
                pressed: Color::from_rgb(0.882, 0.427, 0.447),
                selected: Color::from_rgb(0.882, 0.427, 0.447),
                disabled: Color::from_rgb(0.580, 0.300, 0.310),
                on: Color::WHITE,
                border: Color::from_rgb(0.882, 0.427, 0.447),
                focus: Color::from_rgb(0.882, 0.427, 0.447),
            },
            warning: Component {
                base: Color::from_rgb(0.918, 0.757, 0.529), // #EAC187
                hover: Color::from_rgb(0.941, 0.800, 0.596),
                pressed: Color::from_rgb(0.918, 0.757, 0.529),
                selected: Color::from_rgb(0.918, 0.757, 0.529),
                disabled: Color::from_rgb(0.600, 0.500, 0.360),
                on: Color::from_rgb(0.118, 0.106, 0.098),
                border: Color::from_rgb(0.918, 0.757, 0.529),
                focus: Color::from_rgb(0.918, 0.757, 0.529),
            },
            sent_bubble: BubbleStyle {
                background: ACCENT,
                on: Color::from_rgb(0.118, 0.106, 0.098),
                on_secondary: Color::from_rgba(0.118, 0.106, 0.098, 0.65),
                link: Color::from_rgb(0.118, 0.106, 0.098),
                radius: 18.0,
            },
            received_bubble: BubbleStyle {
                background: Color::from_rgb(0.220, 0.196, 0.188),
                on: TEXT_PRIMARY,
                on_secondary: TEXT_FADED,
                link: ACCENT,
                radius: 18.0,
            },
            avatar_bg: Color::from_rgb(0.310, 0.282, 0.267),
            call_bg: Color::from_rgb(0.098, 0.086, 0.078),
            call_control_base: Color::from_rgb(0.149, 0.133, 0.125),
            call_control_hover: Color::from_rgb(0.196, 0.176, 0.165),
            spacing: SHARED_SPACING,
            radii: SHARED_RADII,
            typography: SHARED_TYPOGRAPHY,
            is_dark: true,
        }
    }

    /// Gruvbox — retro warm dark theme with orange accent.
    pub const fn gruvbox() -> Self {
        const TEXT_PRIMARY: Color = Color::from_rgb(0.922, 0.859, 0.698); // #EBDBB2
        const TEXT_SECONDARY: Color = Color::from_rgb(0.659, 0.600, 0.518); // #A89984
        const TEXT_FADED: Color = Color::from_rgb(0.502, 0.467, 0.400); // #807766
        const DIVIDER: Color = Color::from_rgb(0.231, 0.212, 0.176); // #3C3836
        const INPUT_BG: Color = Color::from_rgb(0.180, 0.165, 0.133); // #2E2A22
        const HOVER_BG: Color = Color::from_rgb(0.243, 0.224, 0.188); // #3E3930
        const SELECTED_BG: Color = Color::from_rgb(0.286, 0.263, 0.216); // #494337
        const ACCENT: Color = Color::from_rgb(0.980, 0.741, 0.184); // #FABD2F

        const COMP: Component = Component {
            base: Color::TRANSPARENT,
            hover: HOVER_BG,
            pressed: SELECTED_BG,
            selected: SELECTED_BG,
            disabled: Color::TRANSPARENT,
            on: TEXT_PRIMARY,
            border: DIVIDER,
            focus: ACCENT,
        };

        Self {
            background: Surface {
                base: Color::from_rgb(0.157, 0.145, 0.122), // #282828
                on: TEXT_PRIMARY,
                on_secondary: TEXT_SECONDARY,
                on_faded: TEXT_FADED,
                divider: DIVIDER,
                input_bg: INPUT_BG,
                component: COMP,
            },
            primary: Surface {
                base: Color::from_rgb(0.196, 0.180, 0.153), // #32302F
                on: TEXT_PRIMARY,
                on_secondary: TEXT_SECONDARY,
                on_faded: TEXT_FADED,
                divider: DIVIDER,
                input_bg: INPUT_BG,
                component: COMP,
            },
            secondary: Surface {
                base: Color::from_rgb(0.243, 0.224, 0.188), // #3E3930
                on: TEXT_PRIMARY,
                on_secondary: TEXT_SECONDARY,
                on_faded: TEXT_FADED,
                divider: DIVIDER,
                input_bg: INPUT_BG,
                component: COMP,
            },
            accent: Component {
                base: ACCENT,
                hover: Color::from_rgb(0.996, 0.796, 0.286),
                pressed: ACCENT,
                selected: ACCENT,
                disabled: Color::from_rgb(0.600, 0.470, 0.130),
                on: Color::from_rgb(0.157, 0.145, 0.122),
                border: ACCENT,
                focus: ACCENT,
            },
            success: Component {
                base: Color::from_rgb(0.722, 0.733, 0.149), // #B8BB26
                hover: Color::from_rgb(0.773, 0.784, 0.243),
                pressed: Color::from_rgb(0.722, 0.733, 0.149),
                selected: Color::from_rgb(0.722, 0.733, 0.149),
                disabled: Color::from_rgb(0.480, 0.490, 0.110),
                on: Color::from_rgb(0.157, 0.145, 0.122),
                border: Color::from_rgb(0.722, 0.733, 0.149),
                focus: Color::from_rgb(0.722, 0.733, 0.149),
            },
            danger: Component {
                base: Color::from_rgb(0.984, 0.286, 0.204), // #FB4934
                hover: Color::from_rgb(0.996, 0.380, 0.298),
                pressed: Color::from_rgb(0.984, 0.286, 0.204),
                selected: Color::from_rgb(0.984, 0.286, 0.204),
                disabled: Color::from_rgb(0.600, 0.200, 0.150),
                on: Color::WHITE,
                border: Color::from_rgb(0.984, 0.286, 0.204),
                focus: Color::from_rgb(0.984, 0.286, 0.204),
            },
            warning: Component {
                base: Color::from_rgb(0.980, 0.741, 0.184), // same as accent
                hover: Color::from_rgb(0.996, 0.796, 0.286),
                pressed: Color::from_rgb(0.980, 0.741, 0.184),
                selected: Color::from_rgb(0.980, 0.741, 0.184),
                disabled: Color::from_rgb(0.600, 0.470, 0.130),
                on: Color::from_rgb(0.157, 0.145, 0.122),
                border: Color::from_rgb(0.980, 0.741, 0.184),
                focus: Color::from_rgb(0.980, 0.741, 0.184),
            },
            sent_bubble: BubbleStyle {
                background: ACCENT,
                on: Color::from_rgb(0.157, 0.145, 0.122),
                on_secondary: Color::from_rgba(0.157, 0.145, 0.122, 0.65),
                link: Color::from_rgb(0.157, 0.145, 0.122),
                radius: 18.0,
            },
            received_bubble: BubbleStyle {
                background: Color::from_rgb(0.243, 0.224, 0.188),
                on: TEXT_PRIMARY,
                on_secondary: TEXT_FADED,
                link: ACCENT,
                radius: 18.0,
            },
            avatar_bg: Color::from_rgb(0.353, 0.325, 0.275),
            call_bg: Color::from_rgb(0.118, 0.110, 0.090),
            call_control_base: Color::from_rgb(0.196, 0.180, 0.153),
            call_control_hover: Color::from_rgb(0.243, 0.224, 0.188),
            spacing: SHARED_SPACING,
            radii: SHARED_RADII,
            typography: SHARED_TYPOGRAPHY,
            is_dark: true,
        }
    }

    /// Nord Light — clean arctic light theme based on Nord's Snow Storm palette.
    pub const fn nord_light() -> Self {
        const TEXT_PRIMARY: Color = Color::from_rgb(0.180, 0.204, 0.251); // #2E3440 nord0
        const TEXT_SECONDARY: Color = Color::from_rgb(0.263, 0.298, 0.369); // #434C5E nord2
        const TEXT_FADED: Color = Color::from_rgb(0.369, 0.404, 0.478); // #5E677A
        const DIVIDER: Color = Color::from_rgb(0.808, 0.827, 0.855); // #CED3DA
        const INPUT_BG: Color = Color::from_rgb(0.906, 0.918, 0.937); // #E8EAEF
        const HOVER_BG: Color = Color::from_rgb(0.878, 0.894, 0.918); // #E0E4EA
        const SELECTED_BG: Color = Color::from_rgb(0.847, 0.867, 0.898); // #D8DDE5
        const ACCENT: Color = Color::from_rgb(0.369, 0.506, 0.675); // #5E81AC nord10

        const COMP: Component = Component {
            base: Color::TRANSPARENT,
            hover: HOVER_BG,
            pressed: SELECTED_BG,
            selected: SELECTED_BG,
            disabled: Color::TRANSPARENT,
            on: TEXT_PRIMARY,
            border: DIVIDER,
            focus: ACCENT,
        };

        Self {
            background: Surface {
                base: Color::from_rgb(0.925, 0.937, 0.957), // #ECEFF4 nord6
                on: TEXT_PRIMARY,
                on_secondary: TEXT_SECONDARY,
                on_faded: TEXT_FADED,
                divider: DIVIDER,
                input_bg: INPUT_BG,
                component: COMP,
            },
            primary: Surface {
                base: Color::from_rgb(0.898, 0.914, 0.941), // #E5E9F0 nord5
                on: TEXT_PRIMARY,
                on_secondary: TEXT_SECONDARY,
                on_faded: TEXT_FADED,
                divider: DIVIDER,
                input_bg: INPUT_BG,
                component: COMP,
            },
            secondary: Surface {
                base: Color::from_rgb(0.847, 0.871, 0.914), // #D8DEE9 nord4
                on: TEXT_PRIMARY,
                on_secondary: TEXT_SECONDARY,
                on_faded: TEXT_FADED,
                divider: DIVIDER,
                input_bg: INPUT_BG,
                component: COMP,
            },
            accent: Component {
                base: ACCENT,
                hover: Color::from_rgb(0.420, 0.557, 0.722),
                pressed: ACCENT,
                selected: ACCENT,
                disabled: Color::from_rgb(0.541, 0.635, 0.741),
                on: Color::WHITE,
                border: ACCENT,
                focus: ACCENT,
            },
            success: Component {
                base: Color::from_rgb(0.639, 0.745, 0.549), // #A3BE8C nord14
                hover: Color::from_rgb(0.690, 0.788, 0.604),
                pressed: Color::from_rgb(0.639, 0.745, 0.549),
                selected: Color::from_rgb(0.639, 0.745, 0.549),
                disabled: Color::from_rgb(0.682, 0.765, 0.616),
                on: Color::WHITE,
                border: Color::from_rgb(0.639, 0.745, 0.549),
                focus: Color::from_rgb(0.639, 0.745, 0.549),
            },
            danger: Component {
                base: Color::from_rgb(0.749, 0.380, 0.416), // #BF616A nord11
                hover: Color::from_rgb(0.800, 0.443, 0.478),
                pressed: Color::from_rgb(0.749, 0.380, 0.416),
                selected: Color::from_rgb(0.749, 0.380, 0.416),
                disabled: Color::from_rgb(0.769, 0.553, 0.576),
                on: Color::WHITE,
                border: Color::from_rgb(0.749, 0.380, 0.416),
                focus: Color::from_rgb(0.749, 0.380, 0.416),
            },
            warning: Component {
                base: Color::from_rgb(0.922, 0.796, 0.545), // #EBCB8B nord13
                hover: Color::from_rgb(0.945, 0.839, 0.612),
                pressed: Color::from_rgb(0.922, 0.796, 0.545),
                selected: Color::from_rgb(0.922, 0.796, 0.545),
                disabled: Color::from_rgb(0.835, 0.773, 0.627),
                on: TEXT_PRIMARY,
                border: Color::from_rgb(0.922, 0.796, 0.545),
                focus: Color::from_rgb(0.922, 0.796, 0.545),
            },
            sent_bubble: BubbleStyle {
                background: ACCENT,
                on: Color::WHITE,
                on_secondary: Color::from_rgba(1.0, 1.0, 1.0, 0.75),
                link: Color::WHITE,
                radius: 18.0,
            },
            received_bubble: BubbleStyle {
                background: Color::from_rgb(0.847, 0.871, 0.914), // nord4
                on: TEXT_PRIMARY,
                on_secondary: TEXT_FADED,
                link: ACCENT,
                radius: 18.0,
            },
            avatar_bg: Color::from_rgb(0.753, 0.784, 0.827),
            call_bg: Color::from_rgb(0.898, 0.914, 0.941),
            call_control_base: Color::from_rgb(0.847, 0.871, 0.914),
            call_control_hover: Color::from_rgb(0.808, 0.839, 0.886),
            spacing: SHARED_SPACING,
            radii: SHARED_RADII,
            typography: SHARED_TYPOGRAPHY,
            is_dark: false,
        }
    }

    /// Windows 95 — nostalgic retro theme with teal window backgrounds and
    /// the classic silver-gray UI chrome.
    pub const fn windows95() -> Self {
        const TEXT_PRIMARY: Color = Color::from_rgb(0.0, 0.0, 0.0);
        const TEXT_SECONDARY: Color = Color::from_rgb(0.314, 0.314, 0.314); // #505050
        const TEXT_FADED: Color = Color::from_rgb(0.502, 0.502, 0.502); // #808080
        const DIVIDER: Color = Color::from_rgb(0.627, 0.627, 0.627); // #A0A0A0
        const INPUT_BG: Color = Color::WHITE;
        const HOVER_BG: Color = Color::from_rgb(0.820, 0.820, 0.820); // #D1D1D1
        const SELECTED_BG: Color = Color::from_rgb(0.0, 0.0, 0.502); // #000080 navy
        const ACCENT: Color = Color::from_rgb(0.0, 0.0, 0.502); // #000080 navy

        const COMP: Component = Component {
            base: Color::TRANSPARENT,
            hover: HOVER_BG,
            pressed: Color::from_rgb(0.749, 0.749, 0.749),
            selected: SELECTED_BG,
            disabled: Color::TRANSPARENT,
            on: TEXT_PRIMARY,
            border: DIVIDER,
            focus: ACCENT,
        };

        Self {
            background: Surface {
                base: Color::from_rgb(0.753, 0.753, 0.753), // #C0C0C0 silver
                on: TEXT_PRIMARY,
                on_secondary: TEXT_SECONDARY,
                on_faded: TEXT_FADED,
                divider: DIVIDER,
                input_bg: INPUT_BG,
                component: COMP,
            },
            primary: Surface {
                base: Color::from_rgb(0.753, 0.753, 0.753), // #C0C0C0
                on: TEXT_PRIMARY,
                on_secondary: TEXT_SECONDARY,
                on_faded: TEXT_FADED,
                divider: DIVIDER,
                input_bg: INPUT_BG,
                component: COMP,
            },
            secondary: Surface {
                base: Color::from_rgb(0.820, 0.820, 0.820), // #D1D1D1
                on: TEXT_PRIMARY,
                on_secondary: TEXT_SECONDARY,
                on_faded: TEXT_FADED,
                divider: DIVIDER,
                input_bg: INPUT_BG,
                component: COMP,
            },
            accent: Component {
                base: ACCENT,
                hover: Color::from_rgb(0.0, 0.0, 0.627), // lighter navy
                pressed: ACCENT,
                selected: ACCENT,
                disabled: Color::from_rgb(0.376, 0.376, 0.502),
                on: Color::WHITE,
                border: ACCENT,
                focus: ACCENT,
            },
            success: Component {
                base: Color::from_rgb(0.0, 0.502, 0.0), // #008000
                hover: Color::from_rgb(0.0, 0.600, 0.0),
                pressed: Color::from_rgb(0.0, 0.502, 0.0),
                selected: Color::from_rgb(0.0, 0.502, 0.0),
                disabled: Color::from_rgb(0.376, 0.502, 0.376),
                on: Color::WHITE,
                border: Color::from_rgb(0.0, 0.502, 0.0),
                focus: Color::from_rgb(0.0, 0.502, 0.0),
            },
            danger: Component {
                base: Color::from_rgb(0.753, 0.0, 0.0), // #C00000
                hover: Color::from_rgb(0.878, 0.0, 0.0),
                pressed: Color::from_rgb(0.753, 0.0, 0.0),
                selected: Color::from_rgb(0.753, 0.0, 0.0),
                disabled: Color::from_rgb(0.502, 0.314, 0.314),
                on: Color::WHITE,
                border: Color::from_rgb(0.753, 0.0, 0.0),
                focus: Color::from_rgb(0.753, 0.0, 0.0),
            },
            warning: Component {
                base: Color::from_rgb(0.808, 0.808, 0.0), // #CECE00
                hover: Color::from_rgb(0.878, 0.878, 0.0),
                pressed: Color::from_rgb(0.808, 0.808, 0.0),
                selected: Color::from_rgb(0.808, 0.808, 0.0),
                disabled: Color::from_rgb(0.502, 0.502, 0.314),
                on: TEXT_PRIMARY,
                border: Color::from_rgb(0.808, 0.808, 0.0),
                focus: Color::from_rgb(0.808, 0.808, 0.0),
            },
            sent_bubble: BubbleStyle {
                background: ACCENT,
                on: Color::WHITE,
                on_secondary: Color::from_rgba(1.0, 1.0, 1.0, 0.75),
                link: Color::from_rgb(0.753, 0.753, 1.0),
                radius: 4.0,
            },
            received_bubble: BubbleStyle {
                background: Color::WHITE,
                on: TEXT_PRIMARY,
                on_secondary: TEXT_FADED,
                link: Color::from_rgb(0.0, 0.0, 0.753),
                radius: 4.0,
            },
            avatar_bg: Color::from_rgb(0.0, 0.502, 0.502), // #008080 teal
            call_bg: Color::from_rgb(0.753, 0.753, 0.753),
            call_control_base: Color::from_rgb(0.753, 0.753, 0.753),
            call_control_hover: Color::from_rgb(0.820, 0.820, 0.820),
            spacing: SHARED_SPACING,
            radii: SHARED_RADII,
            typography: SHARED_TYPOGRAPHY,
            is_dark: false,
        }
    }

    /// Scrollbar style for a theme
    pub fn scrollable(
        &self,
        status: iced::widget::scrollable::Status,
    ) -> iced::widget::scrollable::Style {
        use iced::widget::scrollable::{AutoScroll, Rail, Scroller, Style};
        use iced::{border, Shadow, Vector};

        let rail = Rail {
            background: Some(Background::Color(self.background.component.disabled)),
            border: Default::default(),
            scroller: Scroller {
                background: Background::Color(self.background.component.selected),
                border: border::rounded(2),
            },
        };

        let auto_scroll = AutoScroll {
            background: self.background.base.scale_alpha(0.9).into(),
            border: border::rounded(u32::MAX)
                .width(1)
                .color(self.background.base.scale_alpha(0.8)),
            shadow: Shadow {
                color: Color::BLACK.scale_alpha(0.7),
                offset: Vector::ZERO,
                blur_radius: 2.0,
            },
            icon: self.background.on.scale_alpha(0.8),
        };

        let mut style = Style {
            container: Default::default(),
            vertical_rail: rail,
            horizontal_rail: rail,
            gap: None,
            auto_scroll,
        };

        match status {
            iced::widget::scrollable::Status::Active { .. } => style,
            iced::widget::scrollable::Status::Hovered {
                is_horizontal_scrollbar_hovered,
                is_vertical_scrollbar_hovered,
                ..
            } => {
                let hovered = Scroller {
                    background: Background::Color(self.background.component.hover),
                    ..rail.scroller
                };

                if is_horizontal_scrollbar_hovered {
                    style.horizontal_rail.scroller = hovered;
                }
                if is_vertical_scrollbar_hovered {
                    style.vertical_rail.scroller = hovered;
                }

                style
            }
            iced::widget::scrollable::Status::Dragged {
                is_horizontal_scrollbar_dragged,
                is_vertical_scrollbar_dragged,
                ..
            } => {
                let dragged = Scroller {
                    background: Background::Color(self.background.component.pressed),
                    ..rail.scroller
                };

                if is_horizontal_scrollbar_dragged {
                    style.horizontal_rail.scroller = dragged;
                }
                if is_vertical_scrollbar_dragged {
                    style.vertical_rail.scroller = dragged;
                }

                style
            }
        }
    }
}

// ── Shared scales ──────────────────────────────────────────────────────────
//
// These are the same across all themes. Extracted as consts so each theme
// constructor can reference them without duplicating the values.

const SHARED_SPACING: Spacing = Spacing {
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
};

const SHARED_RADII: Radii = Radii {
    none: 0.0,
    xs: 4.0,
    s: 8.0,
    m: 12.0,
    l: 16.0,
    xl: 24.0,
    full: 9999.0,
};

const SHARED_TYPOGRAPHY: Typography = Typography {
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
};

// ── Theme catalogue ────────────────────────────────────────────────────────

/// A named theme entry for use in the theme picker.
pub struct ThemeEntry {
    pub name: &'static str,
    pub theme: PikaTheme,
    /// When this theme corresponds to a built-in iced theme variant, store it
    /// here so [`From<&ThemeEntry>`] returns the canonical variant instead of
    /// `Theme::custom(…)`.
    pub iced_builtin: Option<iced::Theme>,
}

/// All available themes, sorted from darkest background to lightest.
///
/// Sort key: background.base luminance (darkest first), then foreground
/// text luminance, then accent luminance to break ties.
pub static ALL_THEMES: &[ThemeEntry] = &[
    // ── Dark themes (darkest → lightest background) ────────────────────
    ThemeEntry {
        name: "Dark",
        theme: PikaTheme::dark_default(),
        iced_builtin: Some(iced::Theme::Dark),
    },
    ThemeEntry {
        name: "Midnight",
        theme: PikaTheme::midnight(),
        iced_builtin: None,
    },
    ThemeEntry {
        name: "Rosé Pine",
        theme: PikaTheme::rose_pine(),
        iced_builtin: None,
    },
    ThemeEntry {
        name: "Moonlight",
        theme: PikaTheme::moonlight(),
        iced_builtin: None,
    },
    ThemeEntry {
        name: "Ferra",
        theme: PikaTheme::ferra(),
        iced_builtin: Some(iced::Theme::Ferra),
    },
    ThemeEntry {
        name: "Gruvbox Dark",
        theme: PikaTheme::gruvbox(),
        iced_builtin: Some(iced::Theme::GruvboxDark),
    },
    ThemeEntry {
        name: "Catppuccin Mocha",
        theme: PikaTheme::catppuccin_mocha(),
        iced_builtin: Some(iced::Theme::CatppuccinMocha),
    },
    ThemeEntry {
        name: "Nord",
        theme: PikaTheme::nord(),
        iced_builtin: Some(iced::Theme::Nord),
    },
    ThemeEntry {
        name: "Solarized Dark",
        theme: PikaTheme::solarized_dark(),
        iced_builtin: Some(iced::Theme::SolarizedDark),
    },
    // ── Light themes ───────────────────────────────────────────────────
    ThemeEntry {
        name: "Dawn",
        theme: PikaTheme::dawn(),
        iced_builtin: None,
    },
    ThemeEntry {
        name: "Nord Light",
        theme: PikaTheme::nord_light(),
        iced_builtin: None,
    },
    ThemeEntry {
        name: "Solarized Light",
        theme: PikaTheme::solarized_light(),
        iced_builtin: Some(iced::Theme::SolarizedLight),
    },
    // ── Novelty ────────────────────────────────────────────────────────
    ThemeEntry {
        name: "Windows 95",
        theme: PikaTheme::windows95(),
        iced_builtin: None,
    },
];

// ── Compatibility ───────────────────────────────────────────────────────

impl From<&ThemeEntry> for iced::Theme {
    fn from(entry: &ThemeEntry) -> Self {
        // Prefer the canonical built-in variant when available so iced's
        // own widget styles align with the Pika token set.
        if let Some(ref builtin) = entry.iced_builtin {
            return builtin.clone();
        }

        // Custom Pika themes that have no built-in equivalent.
        let palette = iced::theme::Palette {
            background: entry.theme.background.base,
            text: entry.theme.background.on,
            primary: entry.theme.accent.base,
            success: entry.theme.success.base,
            warning: entry.theme.warning.base,
            danger: entry.theme.danger.base,
        };

        iced::Theme::custom(entry.name, palette)
    }
}

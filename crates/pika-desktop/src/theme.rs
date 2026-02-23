use iced::widget::{button, container, rule, text_input};
use iced::{Color, Theme};

use crate::design;

// ── Color aliases (from design tokens) ────────────────────────────────────

pub const RECEIVED_BUBBLE: Color = design::DARK.received_bubble.background;
pub const TEXT_PRIMARY: Color = design::DARK.background.on;
pub const TEXT_SECONDARY: Color = design::DARK.background.on_secondary;
pub const TEXT_FADED: Color = design::DARK.background.on_faded;
pub const ACCENT_BLUE: Color = design::DARK.accent.base;
pub const HOVER_BG: Color = design::DARK.background.component.hover;
pub const INPUT_BORDER: Color = design::DARK.background.divider;
pub const DANGER: Color = design::DARK.danger.base;

// ── Reusable style functions (delegate to design system) ──────────────────

#[allow(dead_code)]
pub fn bubble_sent_style(_theme: &Theme) -> container::Style {
    design::DARK.bubble_sent()
}

#[allow(dead_code)]
pub fn bubble_received_style(_theme: &Theme) -> container::Style {
    design::DARK.bubble_received()
}

pub fn chat_item_style(is_selected: bool) -> impl Fn(&Theme, button::Status) -> button::Style {
    move |_theme: &Theme, status: button::Status| design::DARK.chat_item(is_selected, status)
}

pub fn primary_button_style(_theme: &Theme, status: button::Status) -> button::Style {
    design::DARK.button_primary(status)
}

pub fn secondary_button_style(_theme: &Theme, status: button::Status) -> button::Style {
    design::DARK.button_secondary(status)
}

pub fn danger_button_style(_theme: &Theme, status: button::Status) -> button::Style {
    design::DARK.button_danger(status)
}

pub fn icon_button_style(is_active: bool) -> impl Fn(&Theme, button::Status) -> button::Style {
    move |_theme: &Theme, status: button::Status| design::DARK.button_icon(is_active, status)
}

pub fn dark_input_style(_theme: &Theme, status: text_input::Status) -> text_input::Style {
    design::DARK.text_input(status)
}

pub fn avatar_container_style(_theme: &Theme) -> container::Style {
    design::DARK.avatar()
}

pub fn rail_container_style(_theme: &Theme) -> container::Style {
    design::DARK.rail()
}

pub fn surface_style(_theme: &Theme) -> container::Style {
    design::DARK.surface()
}

pub fn login_card_style(_theme: &Theme) -> container::Style {
    design::DARK.login_card()
}

pub fn input_bar_style(_theme: &Theme) -> container::Style {
    design::DARK.input_bar()
}

pub fn badge_container_style(_theme: &Theme) -> container::Style {
    design::DARK.badge()
}

pub fn subtle_rule_style(_theme: &Theme) -> rule::Style {
    rule::Style {
        color: Color::from_rgb(0.125, 0.129, 0.153),
        radius: 0.0.into(),
        fill_mode: rule::FillMode::Full,
        snap: true,
    }
}

pub fn checkbox_style(is_checked: bool) -> impl Fn(&Theme) -> container::Style {
    move |_theme: &Theme| design::DARK.checkbox_indicator(is_checked)
}

// ── Call screen ─────────────────────────────────────────────────────────────

pub fn incoming_call_banner_style(_theme: &Theme) -> container::Style {
    design::DARK.call_banner()
}

pub fn call_screen_bg_style(_theme: &Theme) -> container::Style {
    design::DARK.call_screen_bg()
}

pub fn call_accept_button_style(_theme: &Theme, status: button::Status) -> button::Style {
    design::DARK.button_call_accept(status)
}

pub fn call_muted_button_style(_theme: &Theme, status: button::Status) -> button::Style {
    design::DARK.button_call_muted(status)
}

pub fn call_control_button_style(_theme: &Theme, status: button::Status) -> button::Style {
    design::DARK.button_call_control(status)
}

// ── Helpers ─────────────────────────────────────────────────────────────────

pub fn relative_time(unix_secs: i64) -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64;
    let diff = now - unix_secs;

    if diff < 60 {
        "now".to_string()
    } else if diff < 3600 {
        format!("{}m", diff / 60)
    } else if diff < 86400 {
        format!("{}h", diff / 3600)
    } else if diff < 604800 {
        format!("{}d", diff / 86400)
    } else {
        // Show abbreviated month + day for older
        let secs = unix_secs as u64;
        // Simple month/day from unix timestamp
        let days_since_epoch = secs / 86400;
        let (year, month, day) = days_to_ymd(days_since_epoch);
        let _ = year;
        let month_name = match month {
            1 => "Jan",
            2 => "Feb",
            3 => "Mar",
            4 => "Apr",
            5 => "May",
            6 => "Jun",
            7 => "Jul",
            8 => "Aug",
            9 => "Sep",
            10 => "Oct",
            11 => "Nov",
            _ => "Dec",
        };
        format!("{month_name} {day}")
    }
}

fn days_to_ymd(days: u64) -> (u64, u64, u64) {
    // Civil days algorithm (simplified)
    let z = days + 719468;
    let era = z / 146097;
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    (y, m, d)
}

pub fn truncate(s: &str, max_chars: usize) -> String {
    if s.chars().count() <= max_chars {
        s.to_string()
    } else {
        let truncated: String = s.chars().take(max_chars.saturating_sub(1)).collect();
        format!("{truncated}\u{2026}")
    }
}

pub fn truncated_npub(npub: &str) -> String {
    if npub.len() <= 20 {
        return npub.to_string();
    }
    format!("{}\u{2026}{}", &npub[..12], &npub[npub.len() - 4..])
}

pub fn truncated_npub_long(npub: &str) -> String {
    if npub.len() <= 30 {
        return npub.to_string();
    }
    format!("{}\u{2026}{}", &npub[..16], &npub[npub.len() - 8..])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncate_short_unchanged() {
        assert_eq!(truncate("hello", 10), "hello");
    }

    #[test]
    fn truncate_long_adds_ellipsis() {
        assert_eq!(truncate("hello world", 6), "hello\u{2026}");
    }

    #[test]
    fn truncated_npub_short_unchanged() {
        assert_eq!(truncated_npub("npub1abcd"), "npub1abcd");
    }

    #[test]
    fn truncated_npub_long_is_compact() {
        assert_eq!(
            truncated_npub("npub1abcdefghijklmnopqrstu"),
            "npub1abcdefg\u{2026}rstu"
        );
    }

    #[test]
    fn truncated_npub_long_variant_is_compact() {
        assert_eq!(
            truncated_npub_long("npub1abcdefghijklmnopqrstuvwxyz123456"),
            "npub1abcdefghijk\u{2026}yz123456"
        );
    }

    #[test]
    fn relative_time_recent() {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;
        assert_eq!(relative_time(now), "now");
        assert_eq!(relative_time(now - 120), "2m");
        assert_eq!(relative_time(now - 7200), "2h");
        assert_eq!(relative_time(now - 172800), "2d");
    }
}

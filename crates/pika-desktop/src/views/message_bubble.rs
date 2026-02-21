use iced::widget::{button, column, container, mouse_area, row, text, Space};
use iced::{border, Background, Color, Element, Fill, Font, Theme};
use pika_core::{ChatMessage, MessageDeliveryState};

use crate::theme;
use crate::Message;

/// Font used for emoji rendering. Falls back through system fonts (Noto Color
/// Emoji on Linux, Apple Color Emoji on macOS, Segoe UI Emoji on Windows).
const EMOJI_FONT: Font = Font::with_name("Noto Color Emoji");

/// Common emoji choices for the quick picker.
const EMOJI_CHOICES: &[&str] = &[
    "\u{2764}\u{FE0F}", // ❤️
    "\u{1F44D}",        // 👍
    "\u{1F602}",        // 😂
    "\u{1F62E}",        // 😮
    "\u{1F622}",        // 😢
    "\u{1F64F}",        // 🙏
    "\u{1F525}",        // 🔥
    "\u{1F389}",        // 🎉
];

/// Renders a single message as a styled bubble with reactions.
///
/// Layout mirrors Signal desktop: small action icons sit beside the bubble
/// (to the left for sent messages, to the right for received messages).
/// Icons only appear on hover. Existing reaction chips render below the bubble.
pub fn message_bubble<'a>(
    msg: &'a ChatMessage,
    is_group: bool,
    emoji_picker_open: bool,
    hovered: bool,
) -> Element<'a, Message, Theme> {
    let timestamp = theme::relative_time(msg.timestamp);

    let delivery_indicator = match &msg.delivery {
        MessageDeliveryState::Pending => " \u{231B}",
        MessageDeliveryState::Sent => "",
        MessageDeliveryState::Failed { .. } => " \u{26A0}",
    };

    let time_text = format!("{timestamp}{delivery_indicator}");
    let msg_id = msg.id.clone();

    // Show action icons when hovered or picker is open
    let show_icons = hovered || emoji_picker_open;

    // ── Reaction chips below the bubble ─────────────────────────
    let chips_row = reaction_chips_row(msg, &msg_id);

    // ── Emoji picker (appears below bubble when open) ───────────
    let picker = if emoji_picker_open {
        Some(emoji_picker_bar(&msg_id))
    } else {
        None
    };

    let content: Element<'a, Message, Theme> = if msg.is_mine {
        // ── Sent: right-aligned ─────────────────────────────────
        // Signal layout: [spacer] [icons] [bubble]
        let bubble = container(
            column![
                text(&msg.display_content).size(14).color(Color::WHITE),
                text(time_text)
                    .size(10)
                    .color(Color::WHITE.scale_alpha(0.6)),
            ]
            .spacing(2),
        )
        .padding([8, 12])
        .max_width(500)
        .style(theme::bubble_sent_style);

        let mut bubble_row = row![Space::new().width(Fill)]
            .spacing(6)
            .align_y(iced::Alignment::Center)
            .width(Fill);

        if show_icons {
            bubble_row = bubble_row.push(message_action_icons(&msg_id, emoji_picker_open));
        }
        bubble_row = bubble_row.push(bubble);

        let mut col = column![bubble_row].spacing(2);
        if let Some(chips) = chips_row {
            col = col.push(row![Space::new().width(Fill), chips]);
        }
        if let Some(p) = picker {
            col = col.push(row![Space::new().width(Fill), p]);
        }
        col.into()
    } else {
        // ── Received: left-aligned ──────────────────────────────
        // Signal layout: [bubble] [icons] [spacer]
        let sender_name = if is_group {
            msg.sender_name.as_deref().unwrap_or("Unknown")
        } else {
            ""
        };

        let mut bubble_content = column![].spacing(2);
        if !sender_name.is_empty() {
            bubble_content =
                bubble_content.push(text(sender_name).size(12).color(theme::ACCENT_BLUE));
        }
        bubble_content = bubble_content.push(
            text(&msg.display_content)
                .size(14)
                .color(theme::TEXT_PRIMARY),
        );
        bubble_content = bubble_content.push(text(time_text).size(10).color(theme::TEXT_FADED));

        let bubble = container(bubble_content)
            .padding([8, 12])
            .max_width(500)
            .style(theme::bubble_received_style);

        let mut bubble_row = row![bubble]
            .spacing(6)
            .align_y(iced::Alignment::Center)
            .width(Fill);

        if show_icons {
            bubble_row = bubble_row.push(message_action_icons(&msg_id, emoji_picker_open));
        }
        bubble_row = bubble_row.push(Space::new().width(Fill));

        let mut col = column![bubble_row].spacing(2);
        if let Some(chips) = chips_row {
            col = col.push(chips);
        }
        if let Some(p) = picker {
            col = col.push(p);
        }
        col.into()
    };

    // Wrap in mouse_area for hover detection
    mouse_area(content)
        .on_enter(Message::HoverMessage(msg_id.clone()))
        .on_exit(Message::UnhoverMessage)
        .into()
}

/// Small action icons beside the bubble (Signal-style).
/// Currently just a react button; could add reply / more later.
fn message_action_icons<'a>(msg_id: &str, picker_open: bool) -> Element<'a, Message, Theme> {
    let mid = msg_id.to_string();

    // React icon: ✕ when picker is open, + otherwise
    let icon = if picker_open { "\u{2715}" } else { "+" };

    let react_btn = button(text(icon).size(14).center())
        .padding([4, 4])
        .on_press(Message::ToggleEmojiPicker(mid))
        .style(|_theme: &Theme, status: button::Status| {
            let text_color = match status {
                button::Status::Hovered => theme::TEXT_PRIMARY,
                _ => theme::TEXT_FADED,
            };
            button::Style {
                background: Some(Background::Color(Color::TRANSPARENT)),
                text_color,
                border: border::rounded(4),
                ..Default::default()
            }
        });

    row![react_btn].align_y(iced::Alignment::Center).into()
}

/// Existing reaction chips displayed below the bubble.
fn reaction_chips_row<'a>(
    msg: &'a ChatMessage,
    msg_id: &str,
) -> Option<Element<'a, Message, Theme>> {
    if msg.reactions.is_empty() {
        return None;
    }

    let mut chips = row![].spacing(4).align_y(iced::Alignment::Center);

    for reaction in &msg.reactions {
        let label = if reaction.count > 1 {
            format!("{} {}", reaction.emoji, reaction.count)
        } else {
            reaction.emoji.clone()
        };

        let emoji = reaction.emoji.clone();
        let mid = msg_id.to_string();
        let reacted = reaction.reacted_by_me;

        let chip = button(text(label).size(13).font(EMOJI_FONT).center())
            .padding([2, 6])
            .on_press(Message::ReactToMessage {
                message_id: mid,
                emoji,
            })
            .style(move |theme: &Theme, status: button::Status| {
                reaction_chip_style(theme, status, reacted)
            });
        chips = chips.push(chip);
    }

    Some(chips.into())
}

/// Inline emoji picker bar (appears below bubble when react icon is clicked).
fn emoji_picker_bar<'a>(msg_id: &str) -> Element<'a, Message, Theme> {
    let mut picker_row = row![].spacing(2);
    for &emoji in EMOJI_CHOICES {
        let mid = msg_id.to_string();
        let e = emoji.to_string();
        let emoji_btn = button(text(emoji).size(18).font(EMOJI_FONT).center())
            .padding([4, 6])
            .on_press(Message::ReactToMessage {
                message_id: mid,
                emoji: e,
            })
            .style(|_theme: &Theme, status: button::Status| {
                let bg = match status {
                    button::Status::Hovered => theme::HOVER_BG,
                    _ => Color::TRANSPARENT,
                };
                button::Style {
                    background: Some(Background::Color(bg)),
                    text_color: theme::TEXT_PRIMARY,
                    border: border::rounded(6),
                    ..Default::default()
                }
            });
        picker_row = picker_row.push(emoji_btn);
    }

    container(picker_row)
        .padding([4, 8])
        .style(|_theme: &Theme| container::Style {
            background: Some(Background::Color(theme::RECEIVED_BUBBLE)),
            border: iced::Border {
                color: theme::INPUT_BORDER,
                width: 1.0,
                radius: border::radius(8),
            },
            ..Default::default()
        })
        .into()
}

fn reaction_chip_style(
    _theme: &Theme,
    status: button::Status,
    reacted_by_me: bool,
) -> button::Style {
    let bg = if reacted_by_me {
        match status {
            button::Status::Hovered => theme::ACCENT_BLUE.scale_alpha(0.4),
            _ => theme::ACCENT_BLUE.scale_alpha(0.25),
        }
    } else {
        match status {
            button::Status::Hovered => theme::HOVER_BG,
            _ => theme::RECEIVED_BUBBLE,
        }
    };

    let border_color = if reacted_by_me {
        theme::ACCENT_BLUE.scale_alpha(0.6)
    } else {
        theme::INPUT_BORDER
    };

    button::Style {
        background: Some(Background::Color(bg)),
        text_color: theme::TEXT_PRIMARY,
        border: iced::Border {
            color: border_color,
            width: 1.0,
            radius: border::radius(10),
        },
        ..Default::default()
    }
}

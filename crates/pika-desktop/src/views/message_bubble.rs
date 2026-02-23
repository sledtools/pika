use iced::widget::{button, column, container, mouse_area, row, text, Space};
use iced::{border, Alignment, Background, Color, Element, Fill, Font, Theme};
use pika_core::{ChatMessage, MessageDeliveryState};

use super::conversation::Message;
use crate::design::{self, BubblePosition};
use crate::icons;
use crate::theme;

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
    reply_target: Option<&'a ChatMessage>,
    emoji_picker_open: bool,
    hovered: bool,
    position: BubblePosition,
) -> Element<'a, Message, Theme> {
    let timestamp = theme::relative_time(msg.timestamp);
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
    let make_reply_preview = || {
        msg.reply_to_message_id.as_ref().map(|reply_to_id| {
            let sender = match reply_target {
                Some(target) if target.is_mine => "You".to_string(),
                Some(target) => target
                    .sender_name
                    .clone()
                    .filter(|name| !name.trim().is_empty())
                    .unwrap_or_else(|| target.sender_pubkey.chars().take(8).collect()),
                None => "Original message".to_string(),
            };
            let snippet = match reply_target {
                Some(target) => {
                    let head = target.display_content.lines().next().unwrap_or("").trim();
                    if head.is_empty() {
                        "(empty message)".to_string()
                    } else if head.chars().count() > 80 {
                        format!("{}…", head.chars().take(80).collect::<String>())
                    } else {
                        head.to_string()
                    }
                }
                None => "Original message not loaded".to_string(),
            };
            let preview: Element<'a, Message, Theme> = container(
                column![
                    text(sender).size(12).color(theme::TEXT_SECONDARY),
                    text(snippet).size(12).color(theme::TEXT_FADED),
                ]
                .spacing(2),
            )
            .padding([6, 8])
            .into();
            if reply_target.is_some() {
                button(preview)
                    .on_press(Message::JumpToMessage(reply_to_id.clone()))
                    .style(theme::secondary_button_style)
                    .into()
            } else {
                preview
            }
        })
    };

    let content: Element<'a, Message, Theme> = if msg.is_mine {
        // ── Sent: right-aligned ─────────────────────────────────
        // Signal layout: [spacer] [icons] [bubble]
        let mut bubble_content = column![].spacing(2);
        if let Some(preview) = make_reply_preview() {
            bubble_content = bubble_content.push(preview);
        }
        bubble_content =
            bubble_content.push(text(&msg.display_content).size(15).color(Color::WHITE));
        bubble_content = bubble_content.push(timestamp_row(timestamp, &msg.delivery, true));
        let bubble = container(bubble_content)
            .padding([10, 14])
            .max_width(500)
            .style(move |_theme: &Theme| design::DARK.bubble_sent_grouped(position));

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
        // Only show sender on first/single message in a group
        let sender_name =
            if is_group && !matches!(position, BubblePosition::Middle | BubblePosition::Last) {
                msg.sender_name.as_deref().unwrap_or("Unknown")
            } else {
                ""
            };

        let mut bubble_content = column![].spacing(2);
        if !sender_name.is_empty() {
            let sender_pk = msg.sender_pubkey.clone();
            bubble_content = bubble_content.push(
                button(text(sender_name).size(13).color(theme::ACCENT_BLUE))
                    .on_press(Message::OpenPeerProfile(sender_pk))
                    .padding(0)
                    .style(|_: &Theme, _| button::Style {
                        background: Some(Background::Color(Color::TRANSPARENT)),
                        text_color: theme::ACCENT_BLUE,
                        ..Default::default()
                    }),
            );
        }
        if let Some(preview) = make_reply_preview() {
            bubble_content = bubble_content.push(preview);
        }
        bubble_content = bubble_content.push(
            text(&msg.display_content)
                .size(15)
                .color(theme::TEXT_PRIMARY),
        );
        bubble_content = bubble_content.push(timestamp_row(timestamp, &msg.delivery, false));

        let bubble = container(bubble_content)
            .padding([10, 14])
            .max_width(500)
            .style(move |_theme: &Theme| design::DARK.bubble_received_grouped(position));

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
fn message_action_icons<'a>(msg_id: &str, picker_open: bool) -> Element<'a, Message, Theme> {
    let mid = msg_id.to_string();
    let reply_mid = msg_id.to_string();

    // React icon: ✕ when picker is open, smiley-plus otherwise
    let react_icon = if picker_open {
        icons::CIRCLE_X
    } else {
        icons::SMILE_PLUS
    };

    let reply_btn = button(
        text(icons::REPLY)
            .font(icons::LUCIDE_FONT)
            .size(18)
            .center(),
    )
    .padding([6, 6])
    .width(32.0)
    .height(32.0)
    .on_press(Message::SetReplyTarget(reply_mid))
    .style(|_theme: &Theme, status: button::Status| {
        let (bg, text_color) = match status {
            button::Status::Hovered => (theme::HOVER_BG, theme::TEXT_PRIMARY),
            _ => (Color::TRANSPARENT, theme::TEXT_FADED),
        };
        button::Style {
            background: Some(Background::Color(bg)),
            text_color,
            border: border::rounded(8),
            ..Default::default()
        }
    });

    let react_btn = button(text(react_icon).font(icons::LUCIDE_FONT).size(18).center())
        .padding([6, 6])
        .width(32.0)
        .height(32.0)
        .on_press(Message::ToggleEmojiPicker(mid))
        .style(|_theme: &Theme, status: button::Status| {
            let (bg, text_color) = match status {
                button::Status::Hovered => (theme::HOVER_BG, theme::TEXT_PRIMARY),
                _ => (Color::TRANSPARENT, theme::TEXT_FADED),
            };
            button::Style {
                background: Some(Background::Color(bg)),
                text_color,
                border: border::rounded(8),
                ..Default::default()
            }
        });

    row![reply_btn, react_btn]
        .spacing(4)
        .align_y(iced::Alignment::Center)
        .into()
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

/// Timestamp + delivery state row for a message bubble.
/// Sent messages get a Lucide checkmark icon; received just show the timestamp.
fn timestamp_row<'a>(
    timestamp: String,
    delivery: &MessageDeliveryState,
    is_mine: bool,
) -> Element<'a, Message, Theme> {
    let text_color = if is_mine {
        Color::WHITE.scale_alpha(0.6)
    } else {
        theme::TEXT_FADED
    };

    if !is_mine {
        return text(timestamp).size(11).color(text_color).into();
    }

    let (icon_cp, icon_color) = match delivery {
        MessageDeliveryState::Pending => (icons::CLOCK, text_color),
        MessageDeliveryState::Sent => (icons::CHECK_CHECK, text_color),
        MessageDeliveryState::Failed { .. } => (icons::X, theme::DANGER),
    };

    row![
        text(timestamp).size(11).color(text_color),
        text(icon_cp)
            .font(icons::LUCIDE_FONT)
            .size(11)
            .color(icon_color),
    ]
    .spacing(3)
    .align_y(Alignment::Center)
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

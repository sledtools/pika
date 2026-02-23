use iced::widget::{button, column, container, row, scrollable, text, Space};
use iced::{Alignment, Element, Fill, Length, Padding, Theme};
use pika_core::ChatSummary;

use crate::icons;
use crate::theme;
use crate::views::avatar::avatar_circle;

#[derive(Debug, Clone)]
pub enum Message {
    ClickMyProfile,
    ClickNewChat,
    ClickNewGroup,
    OpenChat(String),
}

/// Left sidebar containing the chat list and action buttons.
pub fn view<'a>(
    chat_list: &[ChatSummary],
    selected_id: Option<&str>,
    show_new_chat_form: bool,
    show_new_group_form: bool,
    show_my_profile: bool,
    my_picture_url: Option<&str>,
    avatar_cache: &mut super::avatar::AvatarCache,
) -> Element<'a, Message, Theme> {
    // ── Header ──────────────────────────────────────────────────────
    let header = row![
        text("Chats")
            .size(22)
            .font(icons::BOLD)
            .color(theme::TEXT_PRIMARY),
        Space::new().width(Fill),
        button(
            text(icons::MESSAGE_SQUARE_PLUS)
                .font(icons::LUCIDE_FONT)
                .size(20)
                .color(theme::TEXT_PRIMARY)
                .center(),
        )
        .on_press(Message::ClickNewChat)
        .padding([6, 8])
        .style(theme::icon_button_style(show_new_chat_form)),
        button(
            text(icons::USERS)
                .font(icons::LUCIDE_FONT)
                .size(20)
                .color(theme::TEXT_PRIMARY)
                .center(),
        )
        .on_press(Message::ClickNewGroup)
        .padding([6, 8])
        .style(theme::icon_button_style(show_new_group_form)),
    ]
    .spacing(6)
    .align_y(Alignment::Center)
    .padding([0, 4]);

    // ── Chat list ───────────────────────────────────────────────────
    let chat_items = chat_list.iter().fold(column![].spacing(4), |col, chat| {
        col.push(chat_item(chat, selected_id, avatar_cache))
    });

    // ── Assemble rail ───────────────────────────────────────────────
    let mut rail = column![header].spacing(10).padding(Padding {
        top: 44.0,
        right: 16.0,
        bottom: 16.0,
        left: 16.0,
    });

    rail = rail.push(scrollable(chat_items).height(Fill));

    // Profile button at bottom
    let profile_btn_content = row![
        avatar_circle(Some("Me"), my_picture_url, 28.0, avatar_cache),
        text("My Profile").size(14).color(theme::TEXT_PRIMARY),
    ]
    .spacing(8)
    .align_y(Alignment::Center);

    rail = rail.push(
        button(profile_btn_content)
            .on_press(Message::ClickMyProfile)
            .width(Fill)
            .padding([6, 8])
            .style(theme::icon_button_style(show_my_profile)),
    );

    container(rail)
        .width(Length::Fixed(320.0))
        .height(Fill)
        .style(theme::rail_container_style)
        .into()
}

/// A single chat list item row.
fn chat_item<'a>(
    chat: &ChatSummary,
    selected_id: Option<&str>,
    avatar_cache: &mut super::avatar::AvatarCache,
) -> Element<'a, Message, Theme> {
    let is_selected = selected_id == Some(chat.chat_id.as_str());

    let name = chat_display_name(chat);
    let preview = chat.last_message.as_deref().unwrap_or("No messages yet");

    let timestamp_text: Element<'a, Message, Theme> = if let Some(ts) = chat.last_message_at {
        text(theme::relative_time(ts))
            .size(12)
            .color(theme::TEXT_FADED)
            .into()
    } else {
        Space::new().width(0).into()
    };

    let picture_url = chat
        .members
        .iter()
        .find(|m| !m.npub.is_empty())
        .and_then(|m| m.picture_url.as_deref());

    let avatar: Element<'a, Message, Theme> =
        avatar_circle(Some(&name), picture_url, 48.0, avatar_cache);

    // Name + timestamp row
    let top_row = row![
        text(theme::truncate(&name, 20))
            .size(15)
            .font(icons::BOLD)
            .color(theme::TEXT_PRIMARY),
        Space::new().width(Fill),
        timestamp_text,
    ]
    .align_y(Alignment::Center);

    // Preview + optional badge
    let mut bottom_row = row![text(theme::truncate(preview, 30))
        .size(13)
        .color(theme::TEXT_SECONDARY)
        .wrapping(text::Wrapping::None)]
    .align_y(Alignment::Center);

    if chat.unread_count > 0 {
        bottom_row = bottom_row.push(Space::new().width(Fill));
        bottom_row = bottom_row.push(
            container(
                text(chat.unread_count.to_string())
                    .size(11)
                    .color(iced::Color::WHITE)
                    .center(),
            )
            .width(Length::Fixed(20.0))
            .height(Length::Fixed(20.0))
            .align_x(Alignment::Center)
            .align_y(Alignment::Center)
            .style(theme::badge_container_style),
        );
    }

    let content = row![avatar, column![top_row, bottom_row].spacing(3).width(Fill),]
        .spacing(12)
        .align_y(Alignment::Center);

    button(content)
        .on_press(Message::OpenChat(chat.chat_id.clone()))
        .width(Fill)
        .padding([10, 12])
        .style(theme::chat_item_style(is_selected))
        .into()
}

/// Derive a display name for a chat.
fn chat_display_name(chat: &ChatSummary) -> String {
    if chat.is_group {
        if let Some(name) = &chat.group_name {
            if !name.trim().is_empty() {
                return name.clone();
            }
        }
        return "Group".to_string();
    }

    if let Some(member) = chat.members.iter().find(|m| !m.npub.is_empty()) {
        return member
            .name
            .clone()
            .unwrap_or_else(|| short_npub(&member.npub));
    }

    "Direct chat".to_string()
}

fn short_npub(npub: &str) -> String {
    const TAIL: usize = 8;
    if npub.len() <= TAIL {
        return npub.to_string();
    }
    format!("\u{2026}{}", &npub[npub.len() - TAIL..])
}

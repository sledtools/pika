use iced::widget::{button, column, container, row, text, Space};
use iced::{Alignment, Element, Fill, Theme};
use pika_core::PeerProfileState;

use crate::theme;
use crate::views::avatar::avatar_circle;

#[derive(Debug, Clone)]
pub enum Message {
    Close,
    CopyNpub,
    Follow,
    Unfollow,
    StartChat(String),
}

pub enum Event {
    Close,
    CopyNpub,
    Follow,
    Unfollow,
    StartChat { peer_npub: String },
}

pub fn update(message: Message) -> Option<Event> {
    match message {
        Message::Close => Some(Event::Close),
        Message::CopyNpub => Some(Event::CopyNpub),
        Message::Follow => Some(Event::Follow),
        Message::Unfollow => Some(Event::Unfollow),
        Message::StartChat(npub) => Some(Event::StartChat { peer_npub: npub }),
    }
}

pub fn peer_profile_view<'a>(
    profile: &'a PeerProfileState,
    avatar_cache: &mut super::avatar::AvatarCache,
) -> Element<'a, Message, Theme> {
    let mut content = column![].spacing(16).padding([32, 32]).width(Fill);

    // ── Close button row ────────────────────────────────────────────
    content = content.push(
        row![
            Space::new().width(Fill),
            button(text("Close").size(14))
                .on_press(Message::Close)
                .padding([6, 16])
                .style(theme::secondary_button_style),
        ]
        .width(Fill),
    );

    // ── Avatar ──────────────────────────────────────────────────────
    content = content.push(
        container(avatar_circle(
            profile.name.as_deref(),
            profile.picture_url.as_deref(),
            96.0,
            avatar_cache,
        ))
        .width(Fill)
        .center_x(Fill),
    );

    // ── Name / About ────────────────────────────────────────────────
    if let Some(name) = &profile.name {
        content = content.push(
            container(text(name).size(20).color(theme::TEXT_PRIMARY))
                .width(Fill)
                .center_x(Fill),
        );
    }

    if let Some(about) = &profile.about {
        if !about.trim().is_empty() {
            content = content.push(
                container(text(about).size(14).color(theme::TEXT_SECONDARY))
                    .width(Fill)
                    .center_x(Fill),
            );
        }
    }

    // ── npub ────────────────────────────────────────────────────────
    let npub_display = theme::truncated_npub_long(&profile.npub);
    content = content.push(
        row![
            text(npub_display)
                .size(12)
                .color(theme::TEXT_FADED)
                .width(Fill),
            button(text("Copy").size(12))
                .on_press(Message::CopyNpub)
                .padding([4, 10])
                .style(theme::secondary_button_style),
        ]
        .spacing(8)
        .align_y(Alignment::Center),
    );

    // ── Follow / Unfollow ───────────────────────────────────────────
    let follow_btn = if profile.is_followed {
        button(text("Unfollow").size(14).width(Fill).center())
            .on_press(Message::Unfollow)
            .width(Fill)
            .padding([10, 0])
            .style(theme::danger_button_style)
    } else {
        button(text("Follow").size(14).width(Fill).center())
            .on_press(Message::Follow)
            .width(Fill)
            .padding([10, 0])
            .style(theme::primary_button_style)
    };

    content = content.push(follow_btn);

    // ── Message button ──────────────────────────────────────────────
    content = content.push(
        button(text("Message").size(14).width(Fill).center())
            .on_press(Message::StartChat(profile.npub.clone()))
            .width(Fill)
            .padding([10, 0])
            .style(theme::secondary_button_style),
    );

    container(content)
        .width(Fill)
        .height(Fill)
        .style(theme::surface_style)
        .into()
}

use iced::widget::{button, column, container, row, text};
use iced::{Alignment, Element, Fill, Length, Theme};
use pika_core::PeerProfileState;

use crate::icons;
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
    let mut content = column![].spacing(20).padding([24, 32]).width(Fill);

    // ── Back button (top-left) ───────────────────────────────────────
    content = content.push(
        button(
            row![
                text(icons::CHEVRON_LEFT)
                    .font(icons::LUCIDE_FONT)
                    .size(18)
                    .color(theme::TEXT_SECONDARY),
                text("Back").size(14).color(theme::TEXT_SECONDARY),
            ]
            .spacing(4)
            .align_y(Alignment::Center),
        )
        .on_press(Message::Close)
        .padding([6, 12])
        .style(theme::icon_button_style(false)),
    );

    // ── Avatar (centered) ────────────────────────────────────────────
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

    // ── Name ─────────────────────────────────────────────────────────
    if let Some(name) = &profile.name {
        content = content.push(
            container(
                text(name)
                    .size(22)
                    .font(icons::BOLD)
                    .color(theme::TEXT_PRIMARY),
            )
            .width(Fill)
            .center_x(Fill),
        );
    }

    // ── About ────────────────────────────────────────────────────────
    if let Some(about) = &profile.about {
        if !about.trim().is_empty() {
            content = content.push(
                container(text(about).size(14).color(theme::TEXT_SECONDARY))
                    .width(Fill)
                    .center_x(Fill),
            );
        }
    }

    // ── npub (monospace, with copy icon) ─────────────────────────────
    let npub_display = theme::truncated_npub_long(&profile.npub);
    content = content.push(
        container(
            row![
                text(npub_display)
                    .size(14)
                    .font(icons::MONO)
                    .color(theme::TEXT_SECONDARY),
                button(
                    text(icons::COPY)
                        .font(icons::LUCIDE_FONT)
                        .size(16)
                        .color(theme::TEXT_SECONDARY)
                        .center(),
                )
                .on_press(Message::CopyNpub)
                .padding([4, 6])
                .style(theme::icon_button_style(false)),
            ]
            .spacing(8)
            .align_y(Alignment::Center),
        )
        .width(Fill)
        .center_x(Fill),
    );

    // ── Action buttons (centered, compact) ───────────────────────────
    let follow_btn = if profile.is_followed {
        button(text("Unfollow").size(14).font(icons::MEDIUM).center())
            .on_press(Message::Unfollow)
            .width(Length::Fixed(160.0))
            .padding([10, 24])
            .style(theme::danger_button_style)
    } else {
        button(text("Follow").size(14).font(icons::MEDIUM).center())
            .on_press(Message::Follow)
            .width(Length::Fixed(160.0))
            .padding([10, 24])
            .style(theme::primary_button_style)
    };

    let message_btn = button(text("Message").size(14).font(icons::MEDIUM).center())
        .on_press(Message::StartChat(profile.npub.clone()))
        .width(Length::Fixed(160.0))
        .padding([10, 24])
        .style(theme::icon_button_style(false));

    content = content.push(
        container(
            row![follow_btn, message_btn]
                .spacing(12)
                .align_y(Alignment::Center),
        )
        .width(Fill)
        .center_x(Fill),
    );

    container(content)
        .width(Fill)
        .height(Fill)
        .style(theme::surface_style)
        .into()
}

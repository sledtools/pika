use iced::widget::{button, column, container, row, rule, scrollable, text, text_input, Space};
use iced::{Alignment, Element, Fill, Theme};
use pika_core::ChatViewState;

use crate::theme;
use crate::views::avatar::avatar_circle;
use crate::Message;

/// Group Info screen shown in the center pane.
pub fn group_info_view<'a>(
    chat: &'a ChatViewState,
    name_draft: &str,
    npub_input: &str,
    my_pubkey: &str,
    avatar_cache: &mut super::avatar::AvatarCache,
) -> Element<'a, Message, Theme> {
    let mut content = column![].spacing(16).padding([24, 32]).width(Fill);

    // ── Group name + edit ───────────────────────────────────────────
    let name_row = row![
        text_input("Group name\u{2026}", name_draft)
            .on_input(Message::GroupInfoNameChanged)
            .on_submit(Message::RenameGroup)
            .padding(10)
            .width(Fill)
            .style(theme::dark_input_style),
        button(text("Rename").size(14).center())
            .on_press(Message::RenameGroup)
            .padding([10, 20])
            .style(theme::primary_button_style),
    ]
    .spacing(8)
    .align_y(Alignment::Center);

    content = content.push(name_row);

    content = content.push(rule::horizontal(1));

    // ── Members header ──────────────────────────────────────────────
    content = content.push(
        text(format!("Members ({})", chat.members.len()))
            .size(16)
            .color(theme::TEXT_PRIMARY),
    );

    // ── Member list ─────────────────────────────────────────────────
    let is_admin = chat.is_admin;
    let member_list = chat
        .members
        .iter()
        .fold(column![].spacing(4), |col, member| {
            let is_me = member.pubkey == my_pubkey;
            col.push(member_row(member, is_me, is_admin, avatar_cache))
        });

    content = content.push(scrollable(member_list).height(Fill).width(Fill));

    // ── Add member ──────────────────────────────────────────────────
    if is_admin {
        let add_row = row![
            text_input("npub1\u{2026}", npub_input)
                .on_input(Message::GroupInfoNpubChanged)
                .on_submit(Message::AddGroupMember)
                .padding(10)
                .width(Fill)
                .style(theme::dark_input_style),
            button(text("Add").size(14).center())
                .on_press_maybe(if npub_input.trim().is_empty() {
                    None
                } else {
                    Some(Message::AddGroupMember)
                })
                .padding([10, 20])
                .style(theme::primary_button_style),
        ]
        .spacing(8)
        .align_y(Alignment::Center);

        content = content.push(add_row);
    }

    // ── Leave group ─────────────────────────────────────────────────
    content = content.push(
        container(
            button(text("Leave Group").size(14).center())
                .on_press(Message::LeaveGroup)
                .padding([10, 24])
                .style(theme::danger_button_style),
        )
        .width(Fill)
        .align_x(Alignment::Center)
        .padding([8, 0]),
    );

    // ── Close button ────────────────────────────────────────────────
    content = content.push(
        container(
            button(text("Close").size(13).color(theme::TEXT_SECONDARY).center())
                .on_press(Message::CloseGroupInfo)
                .padding([8, 20])
                .style(theme::secondary_button_style),
        )
        .width(Fill)
        .align_x(Alignment::Center),
    );

    container(content)
        .width(Fill)
        .height(Fill)
        .style(theme::surface_style)
        .into()
}

/// A single member row.
fn member_row<'a>(
    member: &'a pika_core::MemberInfo,
    is_me: bool,
    is_admin: bool,
    avatar_cache: &mut super::avatar::AvatarCache,
) -> Element<'a, Message, Theme> {
    let name = member.name.as_deref().unwrap_or("");
    let display_name = if name.is_empty() {
        theme::truncated_npub(&member.npub)
    } else {
        name.to_string()
    };

    let avatar: Element<'_, Message, Theme> = avatar_circle(
        Some(&display_name),
        member.picture_url.as_deref(),
        36.0,
        avatar_cache,
    );

    let label = if is_me {
        format!("{display_name} (You)")
    } else {
        display_name.clone()
    };

    let pubkey_for_profile = member.pubkey.clone();
    let profile_btn = button(
        row![avatar, text(label).size(14).color(theme::TEXT_PRIMARY),]
            .spacing(10)
            .align_y(Alignment::Center),
    )
    .on_press_maybe(if is_me {
        None
    } else {
        Some(Message::OpenPeerProfile(pubkey_for_profile))
    })
    .padding(0)
    .style(|_: &Theme, status: button::Status| {
        let bg = match status {
            button::Status::Hovered => theme::HOVER_BG,
            _ => iced::Color::TRANSPARENT,
        };
        button::Style {
            background: Some(iced::Background::Color(bg)),
            text_color: theme::TEXT_PRIMARY,
            border: iced::border::rounded(6),
            ..Default::default()
        }
    });

    let mut row_content = row![profile_btn, Space::new().width(Fill),]
        .spacing(10)
        .align_y(Alignment::Center);

    if member.is_admin {
        row_content = row_content.push(text("Admin").size(12).color(theme::TEXT_FADED));
    }
    if !is_me && is_admin {
        let pubkey = member.pubkey.clone();
        row_content = row_content.push(
            button(text("Remove").size(12).color(theme::DANGER).center())
                .on_press(Message::RemoveGroupMember(pubkey))
                .padding([4, 10])
                .style(|_: &Theme, status: button::Status| {
                    let bg = match status {
                        button::Status::Hovered => theme::HOVER_BG,
                        _ => iced::Color::TRANSPARENT,
                    };
                    button::Style {
                        background: Some(iced::Background::Color(bg)),
                        text_color: theme::DANGER,
                        border: iced::border::rounded(6),
                        ..Default::default()
                    }
                }),
        );
    }

    container(row_content).width(Fill).padding([8, 12]).into()
}

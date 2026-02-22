use iced::widget::{button, column, container, row, rule, scrollable, text, text_input, Space};
use iced::{Alignment, Element, Fill, Theme};
use pika_core::ChatViewState;

use crate::theme;
use crate::views::avatar::avatar_circle;

// ── State ───────────────────────────────────────────────────────────────────

#[derive(Debug)]
pub struct State {
    pub name_draft: String,
    pub npub_input: String,
}

// ── Messages ────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub enum Message {
    NameChanged(String),
    RenameGroup,
    NpubChanged(String),
    AddMember,
    RemoveMember(String),
    LeaveGroup,
    Close,
    OpenPeerProfile(String),
}

// ── Events ──────────────────────────────────────────────────────────────────

pub enum Event {
    RenameGroup { name: String },
    AddMember { npub: String },
    RemoveMember { pubkey: String },
    LeaveGroup,
    Close,
    OpenPeerProfile { pubkey: String },
}

// ── Implementation ──────────────────────────────────────────────────────────

impl State {
    pub fn new(group_name: Option<&str>) -> Self {
        Self {
            name_draft: group_name.unwrap_or_default().to_string(),
            npub_input: String::new(),
        }
    }

    pub fn update(&mut self, message: Message) -> Option<Event> {
        match message {
            Message::NameChanged(value) => {
                self.name_draft = value;
                None
            }
            Message::RenameGroup => Some(Event::RenameGroup {
                name: self.name_draft.clone(),
            }),
            Message::NpubChanged(value) => {
                self.npub_input = value;
                None
            }
            Message::AddMember => {
                let npub = self.npub_input.trim().to_string();
                if npub.is_empty() {
                    return None;
                }
                self.npub_input.clear();
                Some(Event::AddMember { npub })
            }
            Message::RemoveMember(pubkey) => Some(Event::RemoveMember { pubkey }),
            Message::LeaveGroup => Some(Event::LeaveGroup),
            Message::Close => Some(Event::Close),
            Message::OpenPeerProfile(pubkey) => Some(Event::OpenPeerProfile { pubkey }),
        }
    }

    /// Group Info screen shown in the center pane.
    pub fn view<'a>(
        &'a self,
        chat: &'a ChatViewState,
        my_pubkey: &str,
        avatar_cache: &mut super::avatar::AvatarCache,
    ) -> Element<'a, Message, Theme> {
        let mut content = column![].spacing(16).padding([24, 32]).width(Fill);

        // ── Group name + edit ───────────────────────────────────────────
        let name_row = row![
            text_input("Group name\u{2026}", &self.name_draft)
                .on_input(Message::NameChanged)
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
                col.push(member_row(
                    member,
                    is_me(member, my_pubkey),
                    is_admin,
                    avatar_cache,
                ))
            });

        content = content.push(scrollable(member_list).height(Fill).width(Fill));

        // ── Add member ──────────────────────────────────────────────────
        if is_admin {
            let add_row = row![
                text_input("npub1\u{2026}", &self.npub_input)
                    .on_input(Message::NpubChanged)
                    .on_submit(Message::AddMember)
                    .padding(10)
                    .width(Fill)
                    .style(theme::dark_input_style),
                button(text("Add").size(14).center())
                    .on_press_maybe(if self.npub_input.trim().is_empty() {
                        None
                    } else {
                        Some(Message::AddMember)
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
                    .on_press(Message::Close)
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
}

// ── Private helpers ─────────────────────────────────────────────────────────

fn is_me(member: &pika_core::MemberInfo, my_pubkey: &str) -> bool {
    member.pubkey == my_pubkey
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
                .on_press(Message::RemoveMember(pubkey))
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

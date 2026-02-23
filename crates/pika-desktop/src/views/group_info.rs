use iced::widget::{button, column, container, row, rule, scrollable, text, text_input, Space};
use iced::{Alignment, Element, Fill, Theme};
use pika_core::ChatViewState;

use crate::icons;
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

    /// Group Info screen — Signal-style layout.
    pub fn view<'a>(
        &'a self,
        chat: &'a ChatViewState,
        my_pubkey: &str,
        avatar_cache: &mut super::avatar::AvatarCache,
    ) -> Element<'a, Message, Theme> {
        let mut content = column![].spacing(4).width(Fill);

        // ── Back button ──────────────────────────────────────────────
        content = content.push(
            container(
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
                .padding([8, 12])
                .style(theme::icon_button_style(false)),
            )
            .padding([12, 16]),
        );

        // ── Group name edit ──────────────────────────────────────────
        let name_row = row![
            text_input("Group name\u{2026}", &self.name_draft)
                .on_input(Message::NameChanged)
                .on_submit(Message::RenameGroup)
                .padding(10)
                .width(Fill)
                .style(theme::dark_input_style),
            button(text("Rename").size(14).font(icons::MEDIUM).center())
                .on_press(Message::RenameGroup)
                .padding([10, 20])
                .style(theme::primary_button_style),
        ]
        .spacing(8)
        .align_y(Alignment::Center);

        content = content.push(container(name_row).padding([8, 24]));

        content = content.push(container(rule::horizontal(1)).padding([8, 24]));

        // ── Members section ──────────────────────────────────────────
        content = content.push(
            container(
                text(format!("{} members", chat.members.len()))
                    .size(14)
                    .font(icons::BOLD)
                    .color(theme::TEXT_PRIMARY),
            )
            .padding([8, 24]),
        );

        // Add member row (if admin)
        if chat.is_admin {
            content = content.push(action_row_with_input(
                icons::PLUS,
                "npub1\u{2026}",
                &self.npub_input,
                self.npub_input.trim().is_empty(),
            ));
        }

        // Member list
        let is_admin = chat.is_admin;
        let member_list = chat
            .members
            .iter()
            .fold(column![].spacing(0), |col, member| {
                col.push(member_row(
                    member,
                    is_me(member, my_pubkey),
                    is_admin,
                    avatar_cache,
                ))
            });

        content = content.push(scrollable(member_list).height(Fill).width(Fill));

        content = content.push(container(rule::horizontal(1)).padding([4, 24]));

        // ── Leave group (danger action row) ──────────────────────────
        content = content.push(danger_action_row(
            icons::X,
            "Leave Group",
            Message::LeaveGroup,
        ));

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

/// Signal-style full-width action row: [icon] [label] — hover shows bg.
fn danger_action_row<'a>(
    icon_cp: &'a str,
    label: &'a str,
    on_press: Message,
) -> Element<'a, Message, Theme> {
    button(
        row![
            text(icon_cp)
                .font(icons::LUCIDE_FONT)
                .size(18)
                .color(theme::DANGER),
            text(label).size(14).color(theme::DANGER),
        ]
        .spacing(12)
        .align_y(Alignment::Center),
    )
    .on_press(on_press)
    .width(Fill)
    .padding([12, 24])
    .style(|_: &Theme, status: button::Status| {
        let bg = match status {
            button::Status::Hovered => theme::HOVER_BG,
            _ => iced::Color::TRANSPARENT,
        };
        button::Style {
            background: Some(iced::Background::Color(bg)),
            text_color: theme::DANGER,
            ..Default::default()
        }
    })
    .into()
}

/// Add member input row with icon prefix.
fn action_row_with_input<'a>(
    icon_cp: &'a str,
    placeholder: &'a str,
    value: &'a str,
    add_disabled: bool,
) -> Element<'a, Message, Theme> {
    let add_btn = button(text("Add").size(14).font(icons::MEDIUM).center())
        .on_press_maybe(if add_disabled {
            None
        } else {
            Some(Message::AddMember)
        })
        .padding([8, 16])
        .style(theme::primary_button_style);

    container(
        row![
            text(icon_cp)
                .font(icons::LUCIDE_FONT)
                .size(18)
                .color(theme::TEXT_SECONDARY),
            text_input(placeholder, value)
                .on_input(Message::NpubChanged)
                .on_submit(Message::AddMember)
                .padding(8)
                .width(Fill)
                .style(theme::dark_input_style),
            add_btn,
        ]
        .spacing(10)
        .align_y(Alignment::Center),
    )
    .padding([4, 24])
    .into()
}

/// A single member row — Signal style: full-width, hover bg, avatar + name.
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
        "You".to_string()
    } else {
        display_name
    };

    let mut row_content = row![avatar, text(label).size(14).color(theme::TEXT_PRIMARY)]
        .spacing(12)
        .align_y(Alignment::Center);

    row_content = row_content.push(Space::new().width(Fill));

    if member.is_admin {
        row_content = row_content.push(text("Admin").size(12).color(theme::TEXT_FADED));
    }

    if !is_me && is_admin {
        let pubkey = member.pubkey.clone();
        row_content = row_content.push(
            button(
                text(icons::X)
                    .font(icons::LUCIDE_FONT)
                    .size(14)
                    .color(theme::DANGER),
            )
            .on_press(Message::RemoveMember(pubkey))
            .padding([4, 6])
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

    let pubkey_for_profile = member.pubkey.clone();
    button(row_content)
        .on_press_maybe(if is_me {
            None
        } else {
            Some(Message::OpenPeerProfile(pubkey_for_profile))
        })
        .width(Fill)
        .padding([10, 24])
        .style(|_: &Theme, status: button::Status| {
            let bg = match status {
                button::Status::Hovered => theme::HOVER_BG,
                _ => iced::Color::TRANSPARENT,
            };
            button::Style {
                background: Some(iced::Background::Color(bg)),
                text_color: theme::TEXT_PRIMARY,
                ..Default::default()
            }
        })
        .into()
}

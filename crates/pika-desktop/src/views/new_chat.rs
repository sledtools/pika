use iced::widget::{button, column, container, row, scrollable, text, text_input, Space};
use iced::{Alignment, Element, Fill, Theme};
use pika_core::FollowListEntry;

use crate::icons;
use crate::theme;
use crate::views::avatar::avatar_circle;

// ── State ───────────────────────────────────────────────────────────────────

#[derive(Debug)]
pub struct State {
    pub input: String,
    pub search: String,
    pub filtered_follows: Vec<FollowListEntry>,
}

// ── Messages ────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub enum Message {
    InputChanged(String),
    SearchChanged(String),
    StartChat,
    StartChatWith(String),
}

// ── Events ──────────────────────────────────────────────────────────────────

pub enum Event {
    CreateChat { peer_npub: String },
}

// ── Implementation ──────────────────────────────────────────────────────────

impl State {
    pub fn new(follows: &[FollowListEntry]) -> Self {
        Self {
            input: String::new(),
            search: String::new(),
            filtered_follows: follows.to_vec(),
        }
    }

    pub fn update(&mut self, message: Message, follows: &[FollowListEntry]) -> Option<Event> {
        match message {
            Message::InputChanged(value) => {
                self.input = value;
                None
            }
            Message::SearchChanged(value) => {
                self.search = value;
                self.filtered_follows = refilter_follows(follows, &self.search);
                None
            }
            Message::StartChat => {
                let peer_npub = self.input.trim().to_string();
                if peer_npub.is_empty() {
                    return None;
                }
                Some(Event::CreateChat { peer_npub })
            }
            Message::StartChatWith(peer_npub) => Some(Event::CreateChat { peer_npub }),
        }
    }

    /// Re-filter the follows list (e.g. after the source data changes).
    pub fn update_follows(&mut self, follows: &[FollowListEntry]) {
        self.filtered_follows = refilter_follows(follows, &self.search);
    }

    /// "New Chat" screen shown in the center pane.
    pub fn view<'a>(
        &'a self,
        creating_chat: bool,
        fetching_follow_list: bool,
        avatar_cache: &mut super::avatar::AvatarCache,
    ) -> Element<'a, Message, Theme> {
        let mut content = column![].spacing(16).padding([24, 32]).width(Fill);

        // ── Header ──────────────────────────────────────────────────────
        content = content.push(text("New Chat").size(22).color(theme::TEXT_PRIMARY));

        // ── Manual entry ────────────────────────────────────────────────
        let input_row = row![
            text_input("npub1\u{2026} or hex pubkey", &self.input)
                .on_input(Message::InputChanged)
                .on_submit(Message::StartChat)
                .padding(10)
                .width(Fill)
                .style(theme::dark_input_style),
            if creating_chat {
                button(
                    text("Creating\u{2026}")
                        .size(14)
                        .font(icons::MEDIUM)
                        .color(theme::TEXT_FADED)
                        .center(),
                )
                .padding([10, 20])
                .style(theme::secondary_button_style)
            } else {
                button(text("Start Chat").size(14).font(icons::MEDIUM).center())
                    .on_press_maybe(if self.input.trim().is_empty() {
                        None
                    } else {
                        Some(Message::StartChat)
                    })
                    .padding([10, 20])
                    .style(theme::primary_button_style)
            },
        ]
        .spacing(8)
        .align_y(Alignment::Center);

        content = content.push(input_row);

        // ── Search bar for follows ──────────────────────────────────────
        content = content.push(
            text_input("Search follows\u{2026}", &self.search)
                .on_input(Message::SearchChanged)
                .padding(8)
                .style(theme::dark_input_style),
        );

        // ── Follow list ─────────────────────────────────────────────────
        let header_row = row![
            text("Follows").size(14).color(theme::TEXT_SECONDARY),
            Space::new().width(Fill),
            if fetching_follow_list {
                text("Loading\u{2026}").size(12).color(theme::TEXT_FADED)
            } else {
                text(format!("{}", self.filtered_follows.len()))
                    .size(12)
                    .color(theme::TEXT_FADED)
            },
        ]
        .align_y(Alignment::Center);
        content = content.push(header_row);

        if self.filtered_follows.is_empty() && !fetching_follow_list {
            content = content.push(
                container(
                    text("No follows found")
                        .size(14)
                        .color(theme::TEXT_FADED)
                        .center(),
                )
                .width(Fill)
                .padding([20, 0]),
            );
        } else {
            let list = self
                .filtered_follows
                .iter()
                .fold(column![].spacing(2), |col, entry| {
                    col.push(follow_row(entry, creating_chat, avatar_cache))
                });

            content = content.push(scrollable(list).height(Fill).width(Fill));
        }

        container(content)
            .width(Fill)
            .height(Fill)
            .style(theme::surface_style)
            .into()
    }
}

// ── Helpers ─────────────────────────────────────────────────────────────────

pub fn refilter_follows(source: &[FollowListEntry], search: &str) -> Vec<FollowListEntry> {
    if search.is_empty() {
        source.to_vec()
    } else {
        let q = search.to_lowercase();
        source
            .iter()
            .filter(|e| {
                e.name.as_deref().unwrap_or("").to_lowercase().contains(&q)
                    || e.npub.to_lowercase().contains(&q)
            })
            .cloned()
            .collect()
    }
}

/// A single follow list row — tap to start a chat.
fn follow_row<'a>(
    entry: &'a FollowListEntry,
    disabled: bool,
    avatar_cache: &mut super::avatar::AvatarCache,
) -> Element<'a, Message, Theme> {
    let name = entry.name.as_deref().unwrap_or("");
    let display_name = if name.is_empty() {
        theme::truncated_npub(&entry.npub)
    } else {
        name.to_string()
    };

    let avatar: Element<'_, Message, Theme> = avatar_circle(
        Some(&display_name),
        entry.picture_url.as_deref(),
        40.0,
        avatar_cache,
    );

    let mut info = column![text(theme::truncate(&display_name, 30))
        .size(14)
        .color(theme::TEXT_PRIMARY),]
    .spacing(2);

    if !name.is_empty() {
        info = info.push(
            text(theme::truncated_npub(&entry.npub))
                .size(11)
                .color(theme::TEXT_FADED),
        );
    }

    let row_content = row![avatar, info].spacing(12).align_y(Alignment::Center);

    let npub = entry.npub.clone();
    let mut btn = button(row_content).width(Fill).padding([8, 12]).style(
        |_: &Theme, status: button::Status| {
            let bg = match status {
                button::Status::Hovered => theme::HOVER_BG,
                _ => iced::Color::TRANSPARENT,
            };
            button::Style {
                background: Some(iced::Background::Color(bg)),
                text_color: theme::TEXT_PRIMARY,
                border: iced::border::rounded(8),
                ..Default::default()
            }
        },
    );

    if !disabled {
        btn = btn.on_press(Message::StartChatWith(npub));
    }

    btn.into()
}

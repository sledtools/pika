use iced::widget::{button, column, container, row, scrollable, text, text_input, Space};
use iced::{Alignment, Element, Fill, Length, Theme};
use pika_core::FollowListEntry;

use crate::icons;
use crate::theme;
use crate::views::avatar::avatar_circle;
use crate::views::new_chat::refilter_follows;

// ── State ───────────────────────────────────────────────────────────────────

#[derive(Debug)]
pub struct State {
    pub input: String,
    pub search: String,
    pub filtered_follows: Vec<FollowListEntry>,
    pub group_name: String,
    pub selected_members: Vec<String>,
}

// ── Messages ────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub enum Message {
    InputChanged(String),
    SearchChanged(String),
    GroupNameChanged(String),
    ToggleGroupMember(String),
    AddManualMember,
    CreateGroup,
}

// ── Events ──────────────────────────────────────────────────────────────────

pub enum Event {
    CreateGroup {
        peer_npubs: Vec<String>,
        group_name: String,
    },
}

// ── Implementation ──────────────────────────────────────────────────────────

impl State {
    pub fn new(follows: &[FollowListEntry]) -> Self {
        Self {
            input: String::new(),
            search: String::new(),
            filtered_follows: follows.to_vec(),
            group_name: String::new(),
            selected_members: Vec::new(),
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
            Message::GroupNameChanged(value) => {
                self.group_name = value;
                None
            }
            Message::ToggleGroupMember(npub) => {
                if let Some(pos) = self.selected_members.iter().position(|n| n == &npub) {
                    self.selected_members.remove(pos);
                } else {
                    self.selected_members.push(npub);
                }
                None
            }
            Message::AddManualMember => {
                let npub = self.input.trim().to_string();
                if !npub.is_empty() && !self.selected_members.contains(&npub) {
                    self.selected_members.push(npub);
                    self.input.clear();
                }
                None
            }
            Message::CreateGroup => {
                if self.selected_members.is_empty() {
                    return None;
                }
                Some(Event::CreateGroup {
                    peer_npubs: self.selected_members.clone(),
                    group_name: self.group_name.clone(),
                })
            }
        }
    }

    /// Re-filter the follows list (e.g. after the source data changes).
    pub fn update_follows(&mut self, follows: &[FollowListEntry]) {
        self.filtered_follows = refilter_follows(follows, &self.search);
    }

    /// "New Group" screen shown in the center pane.
    pub fn view<'a>(
        &'a self,
        creating_chat: bool,
        fetching_follow_list: bool,
        avatar_cache: &mut super::avatar::AvatarCache,
    ) -> Element<'a, Message, Theme> {
        let mut content = column![].spacing(16).padding([24, 32]).width(Fill);

        // ── Header ──────────────────────────────────────────────────────
        content = content.push(text("New Group").size(22).color(theme::TEXT_PRIMARY));

        // ── Group name ──────────────────────────────────────────────────
        let mut group_name_field = text_input("Group name\u{2026}", &self.group_name)
            .padding(10)
            .style(theme::dark_input_style);
        if !creating_chat {
            group_name_field = group_name_field.on_input(Message::GroupNameChanged);
        }
        content = content.push(group_name_field);

        // ── Selected members chips ──────────────────────────────────────
        if !self.selected_members.is_empty() {
            let mut chips_row = row![text("Selected:").size(13).color(theme::TEXT_SECONDARY),]
                .spacing(6)
                .align_y(Alignment::Center);

            for npub in &self.selected_members {
                let label = self
                    .filtered_follows
                    .iter()
                    .find(|e| e.npub == *npub)
                    .and_then(|e| e.name.clone())
                    .unwrap_or_else(|| theme::truncated_npub(npub));
                let npub_clone = npub.clone();
                let mut chip = button(
                    text(format!("{label} \u{00d7}"))
                        .size(13)
                        .font(icons::MEDIUM)
                        .color(theme::TEXT_PRIMARY),
                )
                .padding([6, 12])
                .style(theme::secondary_button_style);
                if !creating_chat {
                    chip = chip.on_press(Message::ToggleGroupMember(npub_clone));
                }
                chips_row = chips_row.push(chip);
            }

            content = content.push(chips_row);
        }

        // ── Manual npub entry ───────────────────────────────────────────
        let mut npub_field = text_input("npub1\u{2026} or hex pubkey", &self.input)
            .padding(10)
            .width(Fill)
            .style(theme::dark_input_style);
        if !creating_chat {
            npub_field = npub_field
                .on_input(Message::InputChanged)
                .on_submit(Message::AddManualMember);
        }

        let add_btn = button(text("Add").size(14).font(icons::MEDIUM).center())
            .on_press_maybe(if creating_chat || self.input.trim().is_empty() {
                None
            } else {
                Some(Message::AddManualMember)
            })
            .padding([10, 20])
            .style(theme::secondary_button_style);

        let input_row = row![npub_field, add_btn]
            .spacing(8)
            .align_y(Alignment::Center);

        content = content.push(input_row);

        // ── Search bar ──────────────────────────────────────────────────
        let mut search_field = text_input("Search follows\u{2026}", &self.search)
            .padding(8)
            .style(theme::dark_input_style);
        if !creating_chat {
            search_field = search_field.on_input(Message::SearchChanged);
        }
        content = content.push(search_field);

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
                    let is_selected = self.selected_members.contains(&entry.npub);
                    col.push(follow_row_selectable(
                        entry,
                        is_selected,
                        creating_chat,
                        avatar_cache,
                    ))
                });

            content = content.push(scrollable(list).height(Fill).width(Fill));
        }

        // ── Create Group button ─────────────────────────────────────────
        let can_create = !self.selected_members.is_empty() && !creating_chat;

        let create_btn = if creating_chat {
            button(
                text("Creating\u{2026}")
                    .size(14)
                    .font(icons::MEDIUM)
                    .color(theme::TEXT_FADED)
                    .width(Fill)
                    .center(),
            )
            .width(Fill)
            .padding([12, 0])
            .style(theme::secondary_button_style)
        } else {
            let mut btn = button(
                text("Create Group")
                    .size(14)
                    .font(icons::MEDIUM)
                    .width(Fill)
                    .center(),
            )
            .width(Fill)
            .padding([12, 0])
            .style(theme::primary_button_style);
            if can_create {
                btn = btn.on_press(Message::CreateGroup);
            }
            btn
        };

        content = content.push(create_btn);

        container(content)
            .width(Fill)
            .height(Fill)
            .style(theme::surface_style)
            .into()
    }
}

// ── Private helpers ─────────────────────────────────────────────────────────

/// A follow row with a checkbox indicator for multi-select.
fn follow_row_selectable<'a>(
    entry: &'a FollowListEntry,
    is_selected: bool,
    disabled: bool,
    avatar_cache: &mut super::avatar::AvatarCache,
) -> Element<'a, Message, Theme> {
    let name = entry.name.as_deref().unwrap_or("");
    let display_name = if name.is_empty() {
        theme::truncated_npub(&entry.npub)
    } else {
        name.to_string()
    };

    let checkbox: Element<'_, Message, Theme> = container(
        text(icons::CHECK)
            .font(icons::LUCIDE_FONT)
            .size(12)
            .center(),
    )
    .width(Length::Fixed(20.0))
    .height(Length::Fixed(20.0))
    .align_x(Alignment::Center)
    .align_y(Alignment::Center)
    .style(theme::checkbox_style(is_selected))
    .into();

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

    let row_content = row![checkbox, avatar, info,]
        .spacing(10)
        .align_y(Alignment::Center);

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
        btn = btn.on_press(Message::ToggleGroupMember(npub));
    }

    btn.into()
}

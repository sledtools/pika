//! Command palette overlay — "go to anything" search + action list.
//!
//! Activated with Cmd+K. Provides filtered access to chats, contacts,
//! and app actions. Each result is a two-line rich item with optional
//! keyboard shortcut chiclet.
//!
//! ## Adaptive context
//!
//! The palette adjusts suggestions based on the calling context:
//! - When in a group chat, "Add member to group" appears at the top
//! - When in any chat, "Jump to date" is available
//! - Message content within the current conversation is searchable

use iced::widget::{
    button, column, container, mouse_area, operation, row, scrollable, text, text_input, Id, Space,
};
use iced::{Alignment, Element, Fill, Length, Padding, Task, Theme};
use pika_core::{ChatMessage, ChatSummary, ChatViewState};

use crate::theme;
use crate::{design, icons};

// ── Scrollable ID ───────────────────────────────────────────────────────────

const PALETTE_SCROLL_ID: &str = "command-palette-scroll";

// ── Palette action ──────────────────────────────────────────────────────────

/// The action a palette item will trigger when selected.
#[derive(Debug, Clone)]
pub enum PaletteAction {
    /// Navigate to an existing chat.
    OpenChat { chat_id: String },
    /// Open the "New Chat" form.
    StartNewChat,
    /// Open the "New Group" form.
    StartNewGroup,
    /// Open the user's own profile.
    OpenMyProfile,
    /// Switch to the theme picker overlay.
    OpenThemePicker,
    /// Jump to a specific message in the current conversation.
    JumpToMessage { message_id: String },
}

// ── Palette item ────────────────────────────────────────────────────────────

/// A single entry in the command palette results list.
#[derive(Debug, Clone)]
pub struct PaletteItem {
    pub title: String,
    pub subtitle: String,
    /// Optional keyboard shortcut label to show in a chiclet.
    pub shortcut: Option<String>,
    pub action: PaletteAction,
}

// ── Chat context (adaptive suggestions) ─────────────────────────────────────

/// Optional context about the currently viewed chat, used to tailor palette
/// suggestions (Phase 12: adaptive context).
pub struct ChatContext<'a> {
    pub chat: &'a ChatViewState,
}

// ── State ───────────────────────────────────────────────────────────────────

pub struct State {
    pub query: String,
    pub selected_index: usize,
    pub results: Vec<PaletteItem>,
    pub input_id: Id,
}

// ── Messages ────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub enum Message {
    QueryChanged(String),
    ArrowUp,
    ArrowDown,
    Confirm,
    Dismiss,
    ClickItem(usize),
}

// ── Events (bubbled up to home screen) ──────────────────────────────────────

pub enum Event {
    OpenChat { chat_id: String },
    StartNewChat,
    StartNewGroup,
    OpenMyProfile,
    OpenThemePicker,
    JumpToMessage { message_id: String },
    Dismissed,
}

// ── Implementation ──────────────────────────────────────────────────────────

impl State {
    /// Create a new command palette state, pre-populated with default results.
    ///
    /// `context` provides optional information about the currently viewed chat
    /// for adaptive suggestions.
    pub fn new(chat_list: &[ChatSummary], context: Option<ChatContext<'_>>) -> Self {
        let input_id = Id::unique();
        let results = build_default_results(chat_list, context.as_ref());
        Self {
            query: String::new(),
            selected_index: 0,
            results,
            input_id,
        }
    }

    /// Handle a message and optionally return an event for the parent.
    ///
    /// Also returns an iced `Task` (may be `Task::none()`) — currently used
    /// to scroll the results list so the selected item stays visible.
    pub fn update(
        &mut self,
        message: Message,
        chat_list: &[ChatSummary],
        context: Option<ChatContext<'_>>,
    ) -> (Option<Event>, Task<Message>) {
        match message {
            Message::QueryChanged(value) => {
                self.query = value;
                self.results = if self.query.trim().is_empty() {
                    build_default_results(chat_list, context.as_ref())
                } else {
                    build_filtered_results(chat_list, &self.query, context.as_ref())
                };
                // Reset selection but clamp to results length.
                self.selected_index = 0;
                (None, scroll_to_index(0, self.results.len()))
            }
            Message::ArrowUp => {
                if !self.results.is_empty() {
                    if self.selected_index == 0 {
                        self.selected_index = self.results.len() - 1;
                    } else {
                        self.selected_index -= 1;
                    }
                }
                (
                    None,
                    scroll_to_index(self.selected_index, self.results.len()),
                )
            }
            Message::ArrowDown => {
                if !self.results.is_empty() {
                    self.selected_index = (self.selected_index + 1) % self.results.len();
                }
                (
                    None,
                    scroll_to_index(self.selected_index, self.results.len()),
                )
            }
            Message::Confirm => {
                if let Some(item) = self.results.get(self.selected_index) {
                    (Some(event_for_action(&item.action)), Task::none())
                } else {
                    (None, Task::none())
                }
            }
            Message::ClickItem(index) => {
                if let Some(item) = self.results.get(index) {
                    (Some(event_for_action(&item.action)), Task::none())
                } else {
                    (None, Task::none())
                }
            }
            Message::Dismiss => (Some(Event::Dismissed), Task::none()),
        }
    }

    /// Render the command palette overlay.
    pub fn view(&self) -> Element<'_, Message, Theme> {
        // ── Backdrop (click to dismiss) ─────────────────────────────
        let backdrop = mouse_area(
            container(Space::new())
                .width(Fill)
                .height(Fill)
                .style(theme::overlay_backdrop_style()),
        )
        .on_press(Message::Dismiss);

        // ── Search input ────────────────────────────────────────────
        let search_input: Element<'_, Message, Theme> =
            text_input("Type a command or search\u{2026}", &self.query)
                .id(self.input_id.clone())
                .on_input(Message::QueryChanged)
                .on_submit(Message::Confirm)
                .padding(14)
                .size(16)
                .style(design::palette_input_style)
                .into();

        // ── Results list ────────────────────────────────────────────
        let results_col = self
            .results
            .iter()
            .enumerate()
            .fold(column![].spacing(2), |col, (i, item)| {
                col.push(palette_item_row(item, i == self.selected_index, i))
            });

        let results_scroll = scrollable(results_col)
            .id(PALETTE_SCROLL_ID)
            .height(Length::Shrink)
            .width(Fill)
            .style(design::scrollable_style());

        // ── Palette card ────────────────────────────────────────────
        let card = container(
            column![search_input, results_scroll]
                .spacing(4)
                .padding(Padding {
                    top: 0.0,
                    right: 0.0,
                    bottom: 8.0,
                    left: 0.0,
                }),
        )
        .max_width(800)
        .width(Fill)
        .max_height(480)
        .style(theme::overlay_container());

        // ── Centered layout with margins ────────────────────────────
        let centered = container(card)
            .width(Fill)
            .padding(Padding {
                top: 80.0,
                right: 32.0,
                bottom: 32.0,
                left: 32.0,
            })
            .align_x(Alignment::Center);

        // Stack: backdrop behind the centered card.
        iced::widget::Stack::new()
            .push(backdrop)
            .push(centered)
            .width(Fill)
            .height(Fill)
            .into()
    }

    /// Return a task that focuses the palette text input.
    pub fn focus_input(&self) -> iced::Task<Message> {
        iced::widget::operation::focus(self.input_id.clone())
    }
}

// ── Scroll helper ───────────────────────────────────────────────────────────

/// Produce a `snap_to` task that keeps the selected item visible.
fn scroll_to_index<T>(index: usize, total: usize) -> Task<T> {
    if total <= 1 {
        return Task::none();
    }
    let y = (index as f32 / (total - 1).max(1) as f32).clamp(0.0, 1.0);
    operation::snap_to(PALETTE_SCROLL_ID, operation::RelativeOffset { x: 0.0, y })
}

// ── Result builders ─────────────────────────────────────────────────────────

/// Default results when the search box is empty.
fn build_default_results(
    chat_list: &[ChatSummary],
    context: Option<&ChatContext<'_>>,
) -> Vec<PaletteItem> {
    let mut items = Vec::with_capacity(chat_list.len() + 6);

    // ── Context-aware items at the top ──────────────────────────────
    if let Some(ctx) = context {
        // If in a group chat, surface "Add member" action.
        if ctx.chat.is_group {
            let group_name = ctx.chat.group_name.as_deref().unwrap_or("this group");
            items.push(PaletteItem {
                title: "Add Member to Group".into(),
                subtitle: format!("Add a user to {group_name}"),
                shortcut: None,
                // Re-uses OpenChat to navigate back; the actual "add member"
                // flow is in group info, so we open the chat and the user
                // can proceed from there. In the future this can be a
                // dedicated action.
                action: PaletteAction::OpenChat {
                    chat_id: ctx.chat.chat_id.clone(),
                },
            });
        }

        // "Jump to date" when in any chat.
        items.push(PaletteItem {
            title: "Jump to Date".into(),
            subtitle: "Jump to a specific date in this conversation".into(),
            shortcut: None,
            // Navigate back to the current chat (the actual date picker
            // would need further UI work; this surfaces the intent).
            action: PaletteAction::OpenChat {
                chat_id: ctx.chat.chat_id.clone(),
            },
        });
    }

    // ── Standard action items ───────────────────────────────────────
    items.push(PaletteItem {
        title: "New Chat".into(),
        subtitle: "Start a direct message".into(),
        shortcut: None,
        action: PaletteAction::StartNewChat,
    });
    items.push(PaletteItem {
        title: "New Group".into(),
        subtitle: "Create a group conversation".into(),
        shortcut: None,
        action: PaletteAction::StartNewGroup,
    });
    items.push(PaletteItem {
        title: "Switch Theme".into(),
        subtitle: "Change the app color theme".into(),
        shortcut: Some("\u{2318}T".into()),
        action: PaletteAction::OpenThemePicker,
    });
    items.push(PaletteItem {
        title: "My Profile".into(),
        subtitle: "View and edit your profile".into(),
        shortcut: None,
        action: PaletteAction::OpenMyProfile,
    });

    // Chats sorted by most recently active.
    let mut chats: Vec<&ChatSummary> = chat_list.iter().collect();
    chats.sort_by(|a, b| {
        let a_ts = a.last_message_at.unwrap_or(0);
        let b_ts = b.last_message_at.unwrap_or(0);
        b_ts.cmp(&a_ts)
    });

    for chat in chats {
        let name = chat_display_name(chat);
        let subtitle = chat
            .last_message
            .as_deref()
            .unwrap_or("No messages yet")
            .to_string();
        items.push(PaletteItem {
            title: name,
            subtitle: theme::truncate(&subtitle, 80),
            shortcut: None,
            action: PaletteAction::OpenChat {
                chat_id: chat.chat_id.clone(),
            },
        });
    }

    items
}

/// Filtered results when the user has typed a query.
fn build_filtered_results(
    chat_list: &[ChatSummary],
    query: &str,
    context: Option<&ChatContext<'_>>,
) -> Vec<PaletteItem> {
    let q = query.to_lowercase();
    let mut items = Vec::new();

    // 1. DMs matching username.
    for chat in chat_list.iter().filter(|c| !c.is_group) {
        let name = chat_display_name(chat);
        if name.to_lowercase().contains(&q) {
            let subtitle = chat
                .last_message
                .as_deref()
                .unwrap_or("No messages yet")
                .to_string();
            items.push(PaletteItem {
                title: name,
                subtitle: theme::truncate(&subtitle, 80),
                shortcut: None,
                action: PaletteAction::OpenChat {
                    chat_id: chat.chat_id.clone(),
                },
            });
        }
    }

    // 2. Groups matching group name.
    for chat in chat_list.iter().filter(|c| c.is_group) {
        let name = chat_display_name(chat);
        if name.to_lowercase().contains(&q) {
            let subtitle = chat
                .last_message
                .as_deref()
                .unwrap_or("No messages yet")
                .to_string();
            items.push(PaletteItem {
                title: name,
                subtitle: theme::truncate(&subtitle, 80),
                shortcut: None,
                action: PaletteAction::OpenChat {
                    chat_id: chat.chat_id.clone(),
                },
            });
        }
    }

    // 3. Groups matching any member username (skip already-matched groups).
    let matched_group_ids: Vec<String> = items
        .iter()
        .filter_map(|item| match &item.action {
            PaletteAction::OpenChat { chat_id } => Some(chat_id.clone()),
            _ => None,
        })
        .collect();

    for chat in chat_list
        .iter()
        .filter(|c| c.is_group && !matched_group_ids.iter().any(|id| id == &c.chat_id))
    {
        let member_match = chat
            .members
            .iter()
            .any(|m| m.name.as_deref().unwrap_or("").to_lowercase().contains(&q));
        if member_match {
            let name = chat_display_name(chat);
            let subtitle = chat
                .last_message
                .as_deref()
                .unwrap_or("No messages yet")
                .to_string();
            items.push(PaletteItem {
                title: name,
                subtitle: theme::truncate(&subtitle, 80),
                shortcut: None,
                action: PaletteAction::OpenChat {
                    chat_id: chat.chat_id.clone(),
                },
            });
        }
    }

    // 4. Message search within current conversation (Phase 12).
    if let Some(ctx) = context {
        let message_matches = search_messages(&ctx.chat.messages, &q);
        for (msg, snippet) in message_matches {
            let chat_name = ctx.chat.group_name.as_deref().unwrap_or("this chat");
            items.push(PaletteItem {
                title: format!("Message in {chat_name}"),
                subtitle: snippet,
                shortcut: None,
                action: PaletteAction::JumpToMessage {
                    message_id: msg.id.clone(),
                },
            });
        }
    }

    // 5. Context-aware action items matching the query.
    if let Some(ctx) = context {
        if ctx.chat.is_group {
            let group_name = ctx.chat.group_name.as_deref().unwrap_or("this group");
            let add_member = PaletteItem {
                title: "Add Member to Group".into(),
                subtitle: format!("Add a user to {group_name}"),
                shortcut: None,
                action: PaletteAction::OpenChat {
                    chat_id: ctx.chat.chat_id.clone(),
                },
            };
            if add_member.title.to_lowercase().contains(&q)
                || add_member.subtitle.to_lowercase().contains(&q)
            {
                items.push(add_member);
            }
        }

        let jump_to_date = PaletteItem {
            title: "Jump to Date".into(),
            subtitle: "Jump to a specific date in this conversation".into(),
            shortcut: None,
            action: PaletteAction::OpenChat {
                chat_id: ctx.chat.chat_id.clone(),
            },
        };
        if jump_to_date.title.to_lowercase().contains(&q)
            || jump_to_date.subtitle.to_lowercase().contains(&q)
        {
            items.push(jump_to_date);
        }
    }

    // 6. Standard action items matching the query.
    let action_items = [
        PaletteItem {
            title: "New Chat".into(),
            subtitle: "Start a direct message".into(),
            shortcut: None,
            action: PaletteAction::StartNewChat,
        },
        PaletteItem {
            title: "New Group".into(),
            subtitle: "Create a group conversation".into(),
            shortcut: None,
            action: PaletteAction::StartNewGroup,
        },
        PaletteItem {
            title: "Switch Theme".into(),
            subtitle: "Change the app color theme".into(),
            shortcut: Some("\u{2318}T".into()),
            action: PaletteAction::OpenThemePicker,
        },
        PaletteItem {
            title: "My Profile".into(),
            subtitle: "View and edit your profile".into(),
            shortcut: None,
            action: PaletteAction::OpenMyProfile,
        },
    ];

    for action in action_items {
        if action.title.to_lowercase().contains(&q) || action.subtitle.to_lowercase().contains(&q) {
            items.push(action);
        }
    }

    items
}

// ── Message search ──────────────────────────────────────────────────────────

/// Search message content within the current conversation.
///
/// Returns up to 5 matching messages with a snippet of the matching text.
fn search_messages<'a>(messages: &'a [ChatMessage], query: &str) -> Vec<(&'a ChatMessage, String)> {
    let mut results = Vec::new();
    let q = query.to_lowercase();

    for msg in messages.iter().rev() {
        let content_lower = msg.content.to_lowercase();
        if content_lower.contains(&q) {
            // Build a snippet: find the match position and show surrounding context.
            let snippet = build_snippet(&msg.content, &q, 60);
            results.push((msg, snippet));
            if results.len() >= 5 {
                break;
            }
        }
    }

    results
}

/// Build a short snippet around the first occurrence of `query` in `text`,
/// with a maximum length of `max_chars`.
fn build_snippet(text: &str, query_lower: &str, max_chars: usize) -> String {
    let text_lower = text.to_lowercase();
    let Some(pos) = text_lower.find(query_lower) else {
        return theme::truncate(text, max_chars);
    };

    // Try to show some context before and after the match.
    let half = max_chars / 2;
    let start = pos.saturating_sub(half);
    // Walk back to a char boundary.
    let start = (0..=start)
        .rev()
        .find(|&i| text.is_char_boundary(i))
        .unwrap_or(0);
    let end = (pos + query_lower.len() + half).min(text.len());
    let end = (end..=text.len())
        .find(|&i| text.is_char_boundary(i))
        .unwrap_or(text.len());

    let mut snippet = String::new();
    if start > 0 {
        snippet.push('\u{2026}');
    }
    snippet.push_str(&text[start..end]);
    if end < text.len() {
        snippet.push('\u{2026}');
    }
    snippet
}

fn event_for_action(action: &PaletteAction) -> Event {
    match action {
        PaletteAction::OpenChat { chat_id } => Event::OpenChat {
            chat_id: chat_id.clone(),
        },
        PaletteAction::StartNewChat => Event::StartNewChat,
        PaletteAction::StartNewGroup => Event::StartNewGroup,
        PaletteAction::OpenMyProfile => Event::OpenMyProfile,
        PaletteAction::OpenThemePicker => Event::OpenThemePicker,
        PaletteAction::JumpToMessage { message_id } => Event::JumpToMessage {
            message_id: message_id.clone(),
        },
    }
}

/// Derive a display name for a chat (mirrors chat_rail logic).
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
            .unwrap_or_else(|| theme::truncated_npub(&member.npub));
    }

    "Direct chat".to_string()
}

// ── Styles ──────────────────────────────────────────────────────────────────

/// Render a single palette result row.
fn palette_item_row(
    item: &PaletteItem,
    is_selected: bool,
    index: usize,
) -> Element<'_, Message, Theme> {
    let mut row_content = row![column![
        text(&item.title)
            .size(14)
            .font(icons::BOLD)
            .color(design::text_primary()),
        text(theme::truncate(&item.subtitle, 60))
            .size(12)
            .color(design::text_secondary()),
    ]
    .spacing(2)]
    .align_y(Alignment::Center)
    .spacing(8)
    .width(Fill);

    // Push spacer + shortcut chiclet if present.
    if let Some(ref shortcut) = item.shortcut {
        row_content = row_content.push(Space::new().width(Fill));
        row_content = row_content.push(
            container(
                text(shortcut)
                    .size(11)
                    .font(icons::MONO)
                    .color(design::text_secondary())
                    .center(),
            )
            .padding([3, 8])
            .style(design::shortcut_chiclet_style),
        );
    }

    let padded = container(row_content).padding([8, 16]);

    button(padded)
        .on_press(Message::ClickItem(index))
        .width(Fill)
        .padding(0)
        .style(design::palette_item_style(is_selected))
        .into()
}

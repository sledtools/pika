//! Theme picker overlay — browse and live-preview color themes.
//!
//! Activated with Cmd+T (or via the command palette "Switch Theme" action).
//! Each row shows the theme name styled with that theme's own colors.
//! Arrow keys navigate and trigger a live preview; Enter confirms; Esc reverts.
//!
//! The picker card itself is styled using the currently previewed theme
//! (via `design::current()`), so it updates in real time as the user
//! navigates the list.

use iced::widget::{
    button, column, container, mouse_area, operation, row, scrollable, text, text_input, Id, Space,
};
use iced::{Alignment, Element, Fill, Length, Padding, Task, Theme};

use crate::design::{self, ThemeEntry, ALL_THEMES};
use crate::icons;

// ── Scrollable ID ───────────────────────────────────────────────────────────

const PICKER_SCROLL_ID: &str = "theme-picker-scroll";

// ── State ───────────────────────────────────────────────────────────────────

pub struct State {
    pub query: String,
    pub selected_index: usize,
    /// Index into `ALL_THEMES` of the theme that was active when the picker
    /// opened. Used to revert on dismiss/Esc.
    pub original_theme_index: usize,
    pub filtered_indices: Vec<usize>,
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
    /// Temporarily preview the theme at the given `ALL_THEMES` index.
    PreviewTheme { index: usize },
    /// Commit the theme at the given `ALL_THEMES` index as the active theme.
    SelectTheme { index: usize },
    /// Picker was dismissed — revert to the original theme.
    Dismissed { original_index: usize },
}

// ── Implementation ──────────────────────────────────────────────────────────

impl State {
    /// Create a new theme picker state.
    ///
    /// `active_theme_index` is the index into `ALL_THEMES` of the currently
    /// active theme so we can highlight and revert to it.
    pub fn new(active_theme_index: usize) -> Self {
        let input_id = Id::unique();
        let filtered_indices: Vec<usize> = (0..ALL_THEMES.len()).collect();

        // Find the position of the active theme within the (unfiltered) list
        // so the cursor starts there.
        let selected_index = filtered_indices
            .iter()
            .position(|&i| i == active_theme_index)
            .unwrap_or(0);

        Self {
            query: String::new(),
            selected_index,
            original_theme_index: active_theme_index,
            filtered_indices,
            input_id,
        }
    }

    /// Handle a message and optionally return an event for the parent.
    ///
    /// Also returns an iced `Task` (may be `Task::none()`) — currently used
    /// to scroll the results list so the selected item stays visible.
    pub fn update(&mut self, message: Message) -> (Option<Event>, Task<Message>) {
        match message {
            Message::QueryChanged(value) => {
                self.query = value;
                self.refilter();
                self.selected_index = 0;
                // Preview the first match.
                if let Some(&theme_idx) = self.filtered_indices.first() {
                    return (
                        Some(Event::PreviewTheme { index: theme_idx }),
                        scroll_to_index(0, self.filtered_indices.len()),
                    );
                }
                (None, Task::none())
            }
            Message::ArrowUp => {
                if !self.filtered_indices.is_empty() {
                    if self.selected_index == 0 {
                        self.selected_index = self.filtered_indices.len() - 1;
                    } else {
                        self.selected_index -= 1;
                    }
                    let theme_idx = self.filtered_indices[self.selected_index];
                    return (
                        Some(Event::PreviewTheme { index: theme_idx }),
                        scroll_to_index(self.selected_index, self.filtered_indices.len()),
                    );
                }
                (None, Task::none())
            }
            Message::ArrowDown => {
                if !self.filtered_indices.is_empty() {
                    self.selected_index = (self.selected_index + 1) % self.filtered_indices.len();
                    let theme_idx = self.filtered_indices[self.selected_index];
                    return (
                        Some(Event::PreviewTheme { index: theme_idx }),
                        scroll_to_index(self.selected_index, self.filtered_indices.len()),
                    );
                }
                (None, Task::none())
            }
            Message::Confirm => {
                if let Some(&theme_idx) = self.filtered_indices.get(self.selected_index) {
                    (Some(Event::SelectTheme { index: theme_idx }), Task::none())
                } else {
                    (
                        Some(Event::Dismissed {
                            original_index: self.original_theme_index,
                        }),
                        Task::none(),
                    )
                }
            }
            Message::ClickItem(filtered_pos) => {
                if let Some(&theme_idx) = self.filtered_indices.get(filtered_pos) {
                    (Some(Event::SelectTheme { index: theme_idx }), Task::none())
                } else {
                    (None, Task::none())
                }
            }
            Message::Dismiss => (
                Some(Event::Dismissed {
                    original_index: self.original_theme_index,
                }),
                Task::none(),
            ),
        }
    }

    /// Render the theme picker overlay.
    pub fn view(&self) -> Element<'_, Message, Theme> {
        // ── Backdrop (click to dismiss) ─────────────────────────────
        let backdrop = mouse_area(
            container(Space::new())
                .width(Fill)
                .height(Fill)
                .style(design::overlay_backdrop_style()),
        )
        .on_press(Message::Dismiss);

        // ── Search input ────────────────────────────────────────────
        let search_input: Element<'_, Message, Theme> =
            text_input("Filter themes\u{2026}", &self.query)
                .id(self.input_id.clone())
                .on_input(Message::QueryChanged)
                .on_submit(Message::Confirm)
                .padding(Padding::from(14).bottom(8))
                .size(16)
                .style(design::palette_input_style)
                .into();

        // ── Theme list ──────────────────────────────────────────────
        let list_col =
            self.filtered_indices
                .iter()
                .enumerate()
                .fold(column![], |col, (pos, &theme_idx)| {
                    let entry = &ALL_THEMES[theme_idx];
                    let is_selected = pos == self.selected_index;
                    let is_active = theme_idx == self.original_theme_index;
                    col.push(theme_row(entry, is_selected, is_active, pos))
                });

        let list_scroll = scrollable(list_col)
            .id(PICKER_SCROLL_ID)
            .height(Length::Shrink)
            .width(Fill)
            .style(design::invisible_scrollable());

        // ── Picker card ─────────────────────────────────────────────
        let card = container(
            column![search_input, list_scroll]
                .spacing(4)
                .padding(Padding {
                    top: 0.0,
                    right: 0.0,
                    bottom: 16.0,
                    left: 0.0,
                }),
        )
        .max_width(480)
        .width(Fill)
        .max_height(520)
        .style(design::overlay_container());

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

        iced::widget::Stack::new()
            .push(backdrop)
            .push(centered)
            .width(Fill)
            .height(Fill)
            .into()
    }

    /// Return a task that focuses the picker text input.
    pub fn focus_input(&self) -> iced::Task<Message> {
        iced::widget::operation::focus(self.input_id.clone())
    }

    // ── Private ─────────────────────────────────────────────────────

    fn refilter(&mut self) {
        let q = self.query.to_lowercase();
        if q.is_empty() {
            self.filtered_indices = (0..ALL_THEMES.len()).collect();
        } else {
            self.filtered_indices = (0..ALL_THEMES.len())
                .filter(|&i| ALL_THEMES[i].name.to_lowercase().contains(&q))
                .collect();
        }
    }
}

// ── Scroll helper ───────────────────────────────────────────────────────────

/// Produce a `snap_to` task that keeps the selected item visible.
fn scroll_to_index<T>(index: usize, total: usize) -> Task<T> {
    if total <= 1 {
        return Task::none();
    }
    let y = (index as f32 / (total - 1).max(1) as f32).clamp(0.0, 1.0);
    operation::snap_to(PICKER_SCROLL_ID, operation::RelativeOffset { x: 0.0, y })
}

// ── Row rendering ───────────────────────────────────────────────────────────

/// Render a single theme row, styled with the theme's own colors.
fn theme_row<'a>(
    entry: &'a ThemeEntry,
    is_selected: bool,
    is_active: bool,
    filtered_pos: usize,
) -> Element<'a, Message, Theme> {
    let t = &entry.theme;

    // Small color swatch circle showing the theme's accent.
    let swatch = container(Space::new())
        .width(Length::Fixed(14.0))
        .height(Length::Fixed(14.0))
        .style(move |_: &Theme| container::Style {
            background: Some(iced::Background::Color(t.accent.base)),
            border: iced::border::rounded(7.0),
            ..Default::default()
        });

    // Theme name — use higher contrast for the selected row.
    let name_text = text(entry.name)
        .size(14)
        .font(icons::MEDIUM)
        .color(t.background.on);

    // Row content: swatch + name (+ optional "current" / "previewing" tag).
    let mut row_content = row![swatch, name_text]
        .spacing(12)
        .align_y(Alignment::Center);

    // Push a right-aligned tag for special rows.
    if is_active {
        row_content = row_content.push(Space::new().width(Fill));
        row_content = row_content.push(text("current").size(11).color(t.background.on_faded));
    }

    let bg_color = t.background.base;
    let hover_color = t.background.component.hover;
    let selected_bg = t.background.component.selected;
    let accent_color = t.accent.base;

    let padded = container(row_content).padding([10, 12]).width(Fill);

    button(padded)
        .on_press(Message::ClickItem(filtered_pos))
        .width(Fill)
        .padding(0)
        .style(move |_: &Theme, status: button::Status| {
            if is_selected {
                // ── Selected (previewing) row ────────────────────
                // Strong visual: accent left border + highlighted bg.
                button::Style {
                    background: Some(iced::Background::Color(selected_bg)),
                    text_color: iced::Color::WHITE,
                    border: iced::border::width(2).color(accent_color),
                    ..Default::default()
                }
            } else {
                // ── Normal row ───────────────────────────────────
                let bg = match status {
                    button::Status::Hovered => hover_color,
                    _ => bg_color,
                };
                button::Style {
                    background: Some(iced::Background::Color(bg)),
                    text_color: iced::Color::WHITE,
                    border: iced::border::rounded(0),
                    ..Default::default()
                }
            }
        })
        .into()
}

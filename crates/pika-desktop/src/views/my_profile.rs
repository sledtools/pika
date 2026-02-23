use iced::widget::{button, column, container, row, rule, text, text_input, Space};
use iced::{Alignment, Element, Fill, Theme};
use pika_core::{AppAction, MyProfileState};

use crate::icons;
use crate::theme;
use crate::views::avatar::avatar_circle;

#[derive(Debug)]
pub struct State {
    about: String,
    name: String,
}

#[derive(Debug, Clone)]
pub enum Message {
    AboutChanged(String),
    CopyAppVersion,
    CopyNpub,
    Logout,
    NameChanged(String),
    Save,
}

pub enum Event {
    AppAction(AppAction),
    CopyNpub,
    CopyAppVersion,
    Logout,
}

impl State {
    pub fn new(my_profile_state: &MyProfileState) -> State {
        State {
            about: my_profile_state.about.clone(),
            name: my_profile_state.name.clone(),
        }
    }

    pub fn update(&mut self, message: Message) -> Option<Event> {
        match message {
            Message::AboutChanged(about) => {
                self.about = about;
            }
            Message::CopyAppVersion => return Some(Event::CopyAppVersion),
            Message::CopyNpub => return Some(Event::CopyNpub),
            Message::Logout => return Some(Event::Logout),
            Message::NameChanged(name) => {
                self.name = name;
            }
            Message::Save => {
                return Some(Event::AppAction(AppAction::SaveMyProfile {
                    name: self.name.clone(),
                    about: self.about.clone(),
                }));
            }
        }

        None
    }

    /// Update drafts when the core profile state changes.
    pub fn sync_profile(&mut self, profile: &MyProfileState) {
        self.name = profile.name.clone();
        self.about = profile.about.clone();
    }

    pub fn view<'a>(
        &'a self,
        npub: &'a str,
        app_version: &'a str,
        picture_url: Option<&'a str>,
        avatar_cache: &mut super::avatar::AvatarCache,
    ) -> Element<'a, Message, Theme> {
        let mut content = column![].spacing(4).width(Fill);

        // ── Header ───────────────────────────────────────────────────
        content = content.push(
            container(
                text("Profile")
                    .size(16)
                    .font(icons::BOLD)
                    .color(theme::TEXT_PRIMARY),
            )
            .width(Fill)
            .center_x(Fill)
            .padding([16, 0]),
        );

        // ── Avatar (centered) ────────────────────────────────────────
        let display_name = if self.name.is_empty() {
            "Me"
        } else {
            self.name.as_str()
        };
        content = content.push(
            container(avatar_circle(
                Some(display_name),
                picture_url,
                80.0,
                avatar_cache,
            ))
            .width(Fill)
            .center_x(Fill)
            .padding([8, 0]),
        );

        // ── Name field (icon + input row) ────────────────────────────
        content = content.push(icon_input_row(
            icons::USER,
            "Display name\u{2026}",
            self.name.as_str(),
            Message::NameChanged,
        ));

        // ── About field (icon + input row) ───────────────────────────
        content = content.push(icon_input_row(
            icons::PEN,
            "About\u{2026}",
            self.about.as_str(),
            Message::AboutChanged,
        ));

        // ── Save button ──────────────────────────────────────────────
        content = content.push(
            container(
                button(text("Save Changes").size(14).font(icons::MEDIUM).center())
                    .on_press(Message::Save)
                    .padding([10, 24])
                    .style(theme::primary_button_style),
            )
            .width(Fill)
            .center_x(Fill)
            .padding([8, 24]),
        );

        content = content.push(container(rule::horizontal(1)).padding([8, 24]));

        // ── npub row (icon + monospace + copy) ───────────────────────
        content = content.push(
            container(
                button(
                    row![
                        text(icons::KEY)
                            .font(icons::LUCIDE_FONT)
                            .size(18)
                            .color(theme::TEXT_SECONDARY),
                        text(theme::truncated_npub_long(npub))
                            .size(14)
                            .font(icons::MONO)
                            .color(theme::TEXT_SECONDARY),
                        Space::new().width(Fill),
                        text(icons::COPY)
                            .font(icons::LUCIDE_FONT)
                            .size(16)
                            .color(theme::TEXT_FADED),
                    ]
                    .spacing(12)
                    .align_y(Alignment::Center),
                )
                .on_press(Message::CopyNpub)
                .width(Fill)
                .padding([12, 24])
                .style(ghost_row_style),
            )
            .width(Fill),
        );

        // ── Version row (icon + mono + copy) ─────────────────────────
        content = content.push(
            container(
                button(
                    row![
                        text(icons::INFO)
                            .font(icons::LUCIDE_FONT)
                            .size(18)
                            .color(theme::TEXT_SECONDARY),
                        text(format!("Version {app_version}"))
                            .size(14)
                            .font(icons::MONO)
                            .color(theme::TEXT_SECONDARY),
                        Space::new().width(Fill),
                        text(icons::COPY)
                            .font(icons::LUCIDE_FONT)
                            .size(16)
                            .color(theme::TEXT_FADED),
                    ]
                    .spacing(12)
                    .align_y(Alignment::Center),
                )
                .on_press(Message::CopyAppVersion)
                .width(Fill)
                .padding([12, 24])
                .style(ghost_row_style),
            )
            .width(Fill),
        );

        content = content.push(container(rule::horizontal(1)).padding([8, 24]));

        // ── Logout (danger row) ──────────────────────────────────────
        content = content.push(Space::new().height(Fill));

        content = content.push(
            button(
                row![
                    text(icons::LOG_OUT)
                        .font(icons::LUCIDE_FONT)
                        .size(18)
                        .color(theme::DANGER),
                    text("Logout").size(14).color(theme::DANGER),
                ]
                .spacing(12)
                .align_y(Alignment::Center),
            )
            .on_press(Message::Logout)
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
            }),
        );

        container(content)
            .width(Fill)
            .height(Fill)
            .style(theme::surface_style)
            .into()
    }
}

// ── Helpers ─────────────────────────────────────────────────────────────────

/// Signal-style ghost row: transparent by default, subtle hover bg.
fn ghost_row_style(_theme: &Theme, status: button::Status) -> button::Style {
    let bg = match status {
        button::Status::Hovered => theme::HOVER_BG,
        _ => iced::Color::TRANSPARENT,
    };
    button::Style {
        background: Some(iced::Background::Color(bg)),
        text_color: theme::TEXT_PRIMARY,
        ..Default::default()
    }
}

/// Icon + text_input row for editable fields.
fn icon_input_row<'a>(
    icon_cp: &'a str,
    placeholder: &'a str,
    value: &'a str,
    on_input: impl 'a + Fn(String) -> Message,
) -> Element<'a, Message, Theme> {
    container(
        row![
            text(icon_cp)
                .font(icons::LUCIDE_FONT)
                .size(18)
                .color(theme::TEXT_SECONDARY),
            text_input(placeholder, value)
                .on_input(on_input)
                .padding(10)
                .width(Fill)
                .style(theme::dark_input_style),
        ]
        .spacing(12)
        .align_y(Alignment::Center),
    )
    .padding([4, 24])
    .into()
}

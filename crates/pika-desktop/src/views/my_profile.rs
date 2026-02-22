use iced::widget::{button, column, container, row, text, text_input, Space};
use iced::{Alignment, Element, Fill, Theme};
use pika_core::{AppAction, MyProfileState};

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
        &self,
        npub: &str,
        app_version: &'a str,
        picture_url: Option<&str>,
        avatar_cache: &mut super::avatar::AvatarCache,
    ) -> Element<'a, Message, Theme> {
        let mut content = column![]
            .spacing(20)
            .padding([32, 48])
            .width(Fill)
            .align_x(Alignment::Center);

        // ── Avatar ──────────────────────────────────────────────────────
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
            .align_x(Alignment::Center)
            .width(Fill),
        );

        // ── Name field ──────────────────────────────────────────────────
        let name_row = row![
            text("Name").size(14).color(theme::TEXT_SECONDARY).width(60),
            text_input("Display name\u{2026}", self.name.as_str())
                .on_input(Message::NameChanged)
                .padding(10)
                .width(Fill)
                .style(theme::dark_input_style),
        ]
        .spacing(12)
        .align_y(Alignment::Center);

        content = content.push(name_row);

        // ── About field ─────────────────────────────────────────────────
        let about_row = row![
            text("About")
                .size(14)
                .color(theme::TEXT_SECONDARY)
                .width(60),
            text_input("About\u{2026}", self.about.as_str())
                .on_input(Message::AboutChanged)
                .padding(10)
                .width(Fill)
                .style(theme::dark_input_style),
        ]
        .spacing(12)
        .align_y(Alignment::Center);

        content = content.push(about_row);

        // ── Save button ─────────────────────────────────────────────────
        content = content.push(
            container(
                button(text("Save Changes").size(14).center())
                    .on_press(Message::Save)
                    .padding([10, 24])
                    .style(theme::primary_button_style),
            )
            .width(Fill)
            .align_x(Alignment::Center),
        );

        // ── npub display ────────────────────────────────────────────────
        let npub_row = row![
            text(theme::truncated_npub_long(npub))
                .size(12)
                .color(theme::TEXT_FADED),
            Space::new().width(Fill),
            button(text("Copy").size(12).color(theme::TEXT_SECONDARY).center())
                .on_press(Message::CopyNpub)
                .padding([6, 12])
                .style(theme::secondary_button_style),
        ]
        .align_y(Alignment::Center);

        content = content.push(container(npub_row).width(Fill));

        // ── app version / build ────────────────────────────────────────
        let version_row = row![
            text("Version").size(12).color(theme::TEXT_SECONDARY),
            button(text(app_version).size(12).color(theme::TEXT_FADED))
                .on_press(Message::CopyAppVersion)
                .padding(0)
                .style(|_theme: &Theme, _status: button::Status| button::Style {
                    background: None,
                    text_color: theme::TEXT_FADED,
                    ..Default::default()
                }),
            Space::new().width(Fill),
            button(text("Copy").size(12).color(theme::TEXT_SECONDARY).center())
                .on_press(Message::CopyAppVersion)
                .padding([6, 12])
                .style(theme::secondary_button_style),
        ]
        .align_y(Alignment::Center)
        .spacing(8);

        content = content.push(container(version_row).width(Fill));

        // ── Logout ──────────────────────────────────────────────────────
        content = content.push(Space::new().height(Fill));

        content = content.push(
            container(
                button(text("Logout").size(14).center())
                    .on_press(Message::Logout)
                    .padding([10, 24])
                    .style(theme::danger_button_style),
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

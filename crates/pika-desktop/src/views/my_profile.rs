use iced::widget::{button, column, container, row, text, text_input, Space};
use iced::{Alignment, Element, Fill, Theme};

use crate::theme;
use crate::views::avatar::avatar_circle;
use crate::Message;

/// My Profile screen shown in the center pane.
pub fn my_profile_view<'a>(
    name_draft: &str,
    about_draft: &str,
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
    let display_name = if name_draft.is_empty() {
        "Me"
    } else {
        name_draft
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
        text_input("Display name\u{2026}", name_draft)
            .on_input(Message::ProfileNameChanged)
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
        text_input("About\u{2026}", about_draft)
            .on_input(Message::ProfileAboutChanged)
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
                .on_press(Message::SaveProfile)
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

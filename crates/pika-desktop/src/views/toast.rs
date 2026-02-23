use iced::widget::{button, container, row, text};
use iced::{Alignment, Element, Fill, Theme};

use crate::icons;
use crate::theme;

#[derive(Debug, Clone)]
pub enum Message {
    ClearToast,
    ResetRelayConfig,
}

/// Floating toast notification — rendered as a right-aligned pill overlay.
/// The parent should place this inside a `Stack` so it floats above content.
pub fn toast_bar(message: &str, show_relay_reset: bool) -> Element<'_, Message, Theme> {
    let mut row = row![text(message).size(13).color(iced::Color::WHITE)]
        .spacing(10)
        .align_y(Alignment::Center);

    if show_relay_reset {
        row = row.push(
            button(
                text("Reset Relay Config")
                    .color(iced::Color::WHITE)
                    .size(12),
            )
            .on_press(Message::ResetRelayConfig)
            .padding([4, 8])
            .style(|_theme: &Theme, _status: button::Status| button::Style {
                background: Some(iced::Background::Color(crate::theme::DANGER)),
                text_color: iced::Color::WHITE,
                border: iced::border::rounded(6),
                ..Default::default()
            }),
        );
    }

    row = row.push(
        button(
            text(icons::X)
                .font(icons::LUCIDE_FONT)
                .size(14)
                .color(iced::Color::WHITE)
                .center(),
        )
        .on_press(Message::ClearToast)
        .padding([4, 6])
        .style(|_theme: &Theme, status: button::Status| {
            let bg = match status {
                button::Status::Hovered => iced::Color::WHITE.scale_alpha(0.15),
                _ => iced::Color::TRANSPARENT,
            };
            button::Style {
                background: Some(iced::Background::Color(bg)),
                text_color: iced::Color::WHITE,
                border: iced::border::rounded(6),
                ..Default::default()
            }
        }),
    );

    // Right-aligned floating pill
    container(
        container(row)
            .padding([8, 16])
            .style(|_theme: &Theme| container::Style {
                background: Some(iced::Background::Color(theme::ACCENT_BLUE)),
                border: iced::border::rounded(8),
                ..Default::default()
            }),
    )
    .width(Fill)
    .align_x(iced::Alignment::End)
    .padding([12, 16])
    .into()
}

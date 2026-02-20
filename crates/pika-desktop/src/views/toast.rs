use iced::widget::{button, container, row, text};
use iced::{Element, Fill, Theme};

use crate::theme;
use crate::Message;

/// Full-width toast notification bar.
pub fn toast_bar(message: &str, show_relay_reset: bool) -> Element<'_, Message, Theme> {
    let mut row = row![text(message).color(iced::Color::WHITE).width(Fill)]
        .spacing(8)
        .align_y(iced::Alignment::Center);
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
                ..Default::default()
            }),
        );
    }
    row = row.push(
        button(text("\u{2715}").color(iced::Color::WHITE).size(14))
            .on_press(Message::ClearToast)
            .padding([4, 8])
            .style(|_theme: &Theme, _status: button::Status| button::Style {
                background: None,
                text_color: iced::Color::WHITE,
                ..Default::default()
            }),
    );

    container(row)
        .padding([8, 16])
        .width(Fill)
        .style(theme::toast_container_style)
        .into()
}

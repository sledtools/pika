use iced::widget::{button, column, container, row, text, text_input, Space};
use iced::{Center, Element, Fill, Length, Theme};

use crate::theme;
use crate::Message;

/// Styled login screen with centered dark card.
pub fn login_view<'a>(
    nsec_input: &str,
    busy: bool,
    is_restoring: bool,
    toast: Option<&'a str>,
    show_recovery_controls: bool,
    show_relay_reset: bool,
) -> Element<'a, Message, Theme> {
    let heading = text("Pika").size(36).color(theme::TEXT_PRIMARY).center();

    let subtitle = text("Secure messaging over Nostr + MLS")
        .size(14)
        .color(theme::TEXT_SECONDARY)
        .center();

    let nsec_field = text_input("nsec1\u{2026}", nsec_input)
        .on_input(Message::NsecChanged)
        .on_submit(Message::Login)
        .secure(true)
        .padding(10)
        .style(theme::dark_input_style);

    let mut buttons = row![].spacing(10);

    if busy {
        buttons = buttons.push(
            button(text("Creating\u{2026}").color(theme::TEXT_FADED).center())
                .width(Length::Fill)
                .padding([10, 20])
                .style(theme::secondary_button_style),
        );
    } else {
        buttons = buttons.push(
            button(text("Create Account").center())
                .on_press(Message::CreateAccount)
                .width(Length::Fill)
                .padding([10, 20])
                .style(theme::secondary_button_style),
        );
        buttons = buttons.push(
            button(text("Login").center())
                .on_press(Message::Login)
                .width(Length::Fill)
                .padding([10, 20])
                .style(theme::primary_button_style),
        );
    }

    let mut card = column![
        Space::new().height(8),
        heading,
        subtitle,
        Space::new().height(8),
        nsec_field,
        buttons,
    ]
    .spacing(12)
    .padding(32)
    .max_width(420)
    .align_x(Center);

    if let Some(msg) = toast {
        card = card.push(
            row![
                text(msg).size(13).color(theme::DANGER),
                button(text("\u{2715}").size(12).color(theme::TEXT_FADED))
                    .on_press(Message::ClearToast)
                    .padding([2, 6])
                    .style(|_: &Theme, _: button::Status| button::Style {
                        background: None,
                        text_color: theme::TEXT_FADED,
                        ..Default::default()
                    }),
            ]
            .spacing(8)
            .align_y(iced::Alignment::Center),
        );
    } else if is_restoring {
        card = card.push(
            text("Restoring previous session\u{2026}")
                .size(13)
                .color(theme::TEXT_FADED)
                .center(),
        );
    }

    if show_recovery_controls {
        card = card.push(
            button(text("Reset Local Session Data").center())
                .on_press(Message::ResetLocalSessionData)
                .width(Length::Fill)
                .padding([10, 20])
                .style(theme::danger_button_style),
        );
    }

    if show_relay_reset {
        card = card.push(
            button(text("Reset Relay Config").center())
                .on_press(Message::ResetRelayConfig)
                .width(Length::Fill)
                .padding([10, 20])
                .style(theme::secondary_button_style),
        );
    }

    container(
        container(card)
            .style(theme::login_card_style)
            .max_width(420),
    )
    .center_x(Fill)
    .center_y(Fill)
    .width(Fill)
    .height(Fill)
    .style(theme::surface_style)
    .into()
}

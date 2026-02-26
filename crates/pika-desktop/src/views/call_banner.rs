use iced::widget::{button, container, row, text};
use iced::{Element, Fill, Theme};

use crate::{design, theme};

#[derive(Debug, Clone)]
pub enum Message {
    Accept,
    Reject,
}

/// Full-width incoming-call banner shown at the top of the window.
pub fn view(peer_name: &str, is_video_call: bool) -> Element<'_, Message, Theme> {
    let call_type = if is_video_call { "video call" } else { "call" };
    let label = format!("\u{260e} Incoming {call_type} from {peer_name}");

    let row = row![
        text(label).color(iced::Color::WHITE).width(Fill),
        button(text("Decline").size(13).color(iced::Color::WHITE).center())
            .on_press(Message::Reject)
            .padding([6, 16])
            .style(theme::danger_button_style),
        button(text("Accept").size(13).center())
            .on_press(Message::Accept)
            .padding([6, 16])
            .style(design::call_banner_button_style),
    ]
    .spacing(8)
    .align_y(iced::Alignment::Center);

    container(row)
        .padding([8, 16])
        .width(Fill)
        .style(theme::incoming_call_banner_style)
        .into()
}

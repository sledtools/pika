use iced::widget::{column, container, text};
use iced::{Element, Fill, Theme};

use crate::theme;

/// Centered placeholder shown when no conversation is selected.
pub fn view<'a, M: 'a>() -> Element<'a, M, Theme> {
    container(
        column![
            text("Select a conversation")
                .size(22)
                .color(theme::text_secondary()),
            text("Choose a chat from the sidebar to start messaging")
                .size(14)
                .color(theme::text_faded()),
        ]
        .spacing(8)
        .align_x(iced::Alignment::Center),
    )
    .center_x(Fill)
    .center_y(Fill)
    .width(Fill)
    .height(Fill)
    .into()
}

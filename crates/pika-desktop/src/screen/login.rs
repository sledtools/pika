use iced::widget::{button, column, container, row, text, text_input, Space};
use iced::{Center, Element, Fill, Length, Theme};
use pika_core::{AppAction, AppState};

use crate::app_manager::AppManager;

use crate::theme;

#[derive(Default)]
pub struct State {
    nsec_input: String,
}

#[derive(Debug, Clone)]
pub enum Message {
    NsecChanged(String),
    Login,
    CreateAccount,
    ClearToast,
    ResetLocalSessionData,
    ResetRelayConfig,
}

pub enum Event {
    AppAction(AppAction),
    Login { nsec: String },
    ResetLocalSessionData,
    ResetRelayConfig,
}

fn should_offer_recovery_controls(state: &AppState, manager: &AppManager) -> bool {
    let from_toast = state
        .toast
        .as_deref()
        .map(|toast| {
            toast.contains("Login failed")
                || toast.contains("open encrypted mdk sqlite db")
                || toast.contains("keyring")
        })
        .unwrap_or(false);
    let restoring = manager.is_restoring_session();
    from_toast || restoring
}

pub fn should_offer_relay_reset(state: &AppState) -> bool {
    state
        .toast
        .as_deref()
        .map(|toast| {
            toast.contains("relay")
                || toast.contains("no relays")
                || toast.contains("not connected")
        })
        .unwrap_or(false)
}

impl State {
    pub fn new() -> Self {
        State::default()
    }

    pub fn update(&mut self, message: Message) -> Option<Event> {
        match message {
            Message::NsecChanged(nsec) => {
                self.nsec_input = nsec;
                None
            }
            Message::Login => Some(Event::Login {
                nsec: self.nsec_input.trim().to_string(),
            }),
            Message::CreateAccount => Some(Event::AppAction(AppAction::CreateAccount)),
            Message::ClearToast => Some(Event::AppAction(AppAction::ClearToast)),
            Message::ResetLocalSessionData => Some(Event::ResetLocalSessionData),
            Message::ResetRelayConfig => Some(Event::ResetRelayConfig),
        }
    }

    pub fn view<'a>(
        &'a self,
        state: &'a AppState,
        manager: &AppManager,
    ) -> Element<'a, Message, Theme> {
        let is_restoring = manager.is_restoring_session();
        let show_recovery = should_offer_recovery_controls(state, manager);
        let show_relay_reset = should_offer_relay_reset(state);

        let heading = text("Pika").size(36).color(theme::text_primary()).center();

        let subtitle = text("Secure messaging over Nostr + MLS")
            .size(14)
            .color(theme::text_secondary())
            .center();

        let nsec_field = text_input("nsec1\u{2026}", self.nsec_input.as_str())
            .on_input(Message::NsecChanged)
            .on_submit(Message::Login)
            .secure(true)
            .padding(10)
            .style(theme::dark_input_style);

        let mut buttons = row![].spacing(10);

        if state.busy.creating_account {
            buttons = buttons.push(
                button(text("Creating\u{2026}").color(theme::text_faded()).center())
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

        if let Some(msg) = state.toast.as_ref() {
            card = card.push(
                row![
                    text(msg).size(13).color(theme::danger()),
                    button(text("\u{2715}").size(12).color(theme::text_faded()))
                        .on_press(Message::ClearToast)
                        .padding([2, 6])
                        .style(|_: &Theme, _: button::Status| button::Style {
                            background: None,
                            text_color: theme::text_faded(),
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
                    .color(theme::text_faded())
                    .center(),
            );
        }

        if show_recovery {
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
}

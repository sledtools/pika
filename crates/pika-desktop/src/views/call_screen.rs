use iced::widget::{button, center, column, container, row, shader, stack, text, Space};
use iced::{Alignment, Element, Fill, Theme};
use pika_core::{CallState, CallStatus};

use crate::theme;
use crate::video::DesktopVideoPipeline;

#[derive(Debug, Clone)]
pub enum Message {
    DismissCallScreen,
    AcceptCall,
    RejectCall,
    StartCall,
    StartVideoCall,
    EndCall,
    ToggleMute,
    ToggleCamera,
}

/// Full-screen call overlay (matches the iOS CallScreenView layout).
pub fn call_screen_view<'a>(
    call: &'a CallState,
    peer_name: &'a str,
    video_pipeline: &DesktopVideoPipeline,
) -> Element<'a, Message, Theme> {
    let status_text = match &call.status {
        CallStatus::Offering => "Calling\u{2026}",
        CallStatus::Ringing => "Incoming call\u{2026}",
        CallStatus::Connecting => "Connecting\u{2026}",
        CallStatus::Active => "Active",
        CallStatus::Ended { .. } => "Call ended",
    };

    // For video calls, use a stacked layout (video background + controls overlay)
    if call.is_video_call {
        let has_video = video_pipeline.has_video();
        let camera_err = video_pipeline.camera_error();
        let program = video_pipeline.shader_program();
        return build_video_call_layout(
            call,
            peer_name,
            status_text,
            has_video,
            program,
            camera_err,
        );
    }

    // Audio call (or video call without a frame yet): standard layout
    build_audio_call_layout(call, peer_name, status_text)
}

fn build_audio_call_layout<'a>(
    call: &'a CallState,
    peer_name: &'a str,
    status_text: &'a str,
) -> Element<'a, Message, Theme> {
    let initial = peer_name
        .chars()
        .next()
        .unwrap_or('?')
        .to_uppercase()
        .to_string();
    let avatar = container(text(initial).size(42).color(iced::Color::WHITE).center())
        .width(112)
        .height(112)
        .center_x(112)
        .center_y(112)
        .style(|_: &Theme| container::Style {
            background: Some(iced::Background::Color(iced::Color::from_rgba(
                1.0, 1.0, 1.0, 0.18,
            ))),
            border: iced::border::rounded(56),
            ..Default::default()
        });

    let mut content = column![
        dismiss_button_row(),
        Space::new().height(40),
        center(avatar).width(Fill),
        text(peer_name)
            .size(24)
            .color(iced::Color::WHITE)
            .center()
            .width(Fill),
        text(status_text)
            .size(16)
            .color(iced::Color::from_rgba(1.0, 1.0, 1.0, 0.7))
            .center()
            .width(Fill),
    ]
    .spacing(8)
    .width(Fill);

    content = push_duration_and_debug(content, call);
    content = content.push(Space::new().height(Fill));
    content = content.push(center(build_controls(call)).width(Fill));
    content = content.push(Space::new().height(20));

    container(content)
        .width(Fill)
        .height(Fill)
        .padding([20, 24])
        .style(theme::call_screen_bg_style)
        .into()
}

fn build_video_call_layout<'a>(
    call: &'a CallState,
    peer_name: &'a str,
    status_text: &'a str,
    has_video: bool,
    program: crate::video_shader::VideoShaderProgram,
    camera_error: Option<String>,
) -> Element<'a, Message, Theme> {
    // Video background: shader widget renders directly to a persistent GPU texture
    // (no flicker from Handle::from_rgba texture recreation).
    let video_bg: Element<'a, Message, Theme> = if has_video {
        shader(program).width(Fill).into()
    } else {
        // No remote frame yet — show waiting message on black background
        container(
            column![
                text(peer_name)
                    .size(24)
                    .color(iced::Color::WHITE)
                    .center()
                    .width(Fill),
                text(status_text)
                    .size(16)
                    .color(iced::Color::from_rgba(1.0, 1.0, 1.0, 0.6))
                    .center()
                    .width(Fill),
            ]
            .spacing(8)
            .align_x(Alignment::Center),
        )
        .width(Fill)
        .height(Fill)
        .center_x(Fill)
        .center_y(Fill)
        .style(|_: &Theme| container::Style {
            background: Some(iced::Background::Color(iced::Color::BLACK)),
            ..Default::default()
        })
        .into()
    };

    // Controls overlay on top of video
    let mut overlay = column![].width(Fill).height(Fill);

    // Top row: dismiss + status
    overlay = overlay.push(
        row![
            button(text("\u{2304}").size(20).color(iced::Color::WHITE).center())
                .on_press(Message::DismissCallScreen)
                .padding([4, 12])
                .style(theme::call_control_button_style),
            Space::new().width(Fill),
            container(
                text(format!("{peer_name} \u{2022} {status_text}"))
                    .size(14)
                    .color(iced::Color::WHITE),
            )
            .padding([4, 10])
            .style(|_: &Theme| container::Style {
                background: Some(iced::Background::Color(iced::Color::from_rgba(
                    0.0, 0.0, 0.0, 0.5,
                ))),
                border: iced::border::rounded(6),
                ..Default::default()
            }),
        ]
        .padding([12, 16])
        .align_y(Alignment::Center),
    );

    // Camera error banner
    if let Some(err) = camera_error {
        overlay = overlay.push(
            container(
                text(err)
                    .size(12)
                    .color(iced::Color::from_rgb(1.0, 0.6, 0.6)),
            )
            .padding([4, 12])
            .center_x(Fill)
            .style(|_: &Theme| container::Style {
                background: Some(iced::Background::Color(iced::Color::from_rgba(
                    0.0, 0.0, 0.0, 0.7,
                ))),
                border: iced::border::rounded(6),
                ..Default::default()
            }),
        );
    }

    // Duration and debug stats
    overlay = push_duration_and_debug(overlay, call);

    overlay = overlay.push(Space::new().height(Fill));

    // Bottom controls with semi-transparent background
    overlay = overlay.push(
        container(build_controls(call))
            .center_x(Fill)
            .padding([12, 24])
            .style(|_: &Theme| container::Style {
                background: Some(iced::Background::Color(iced::Color::from_rgba(
                    0.0, 0.0, 0.0, 0.5,
                ))),
                ..Default::default()
            }),
    );

    // Stack: video behind, controls on top
    stack![video_bg, overlay].width(Fill).height(Fill).into()
}

fn dismiss_button_row<'a>() -> Element<'a, Message, Theme> {
    row![
        button(text("\u{2304}").size(20).color(iced::Color::WHITE).center())
            .on_press(Message::DismissCallScreen)
            .padding([4, 12])
            .style(theme::call_control_button_style),
        Space::new().width(Fill),
    ]
    .into()
}

fn push_duration_and_debug<'a>(
    mut content: iced::widget::Column<'a, Message, Theme>,
    call: &'a CallState,
) -> iced::widget::Column<'a, Message, Theme> {
    if matches!(call.status, CallStatus::Active) {
        if let Some(started_at) = call.started_at {
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs() as i64;
            let elapsed = (now - started_at).max(0);
            let mins = elapsed / 60;
            let secs = elapsed % 60;
            content = content.push(
                text(format!("{mins:02}:{secs:02}"))
                    .size(20)
                    .color(iced::Color::from_rgba(1.0, 1.0, 1.0, 0.9))
                    .center()
                    .width(Fill),
            );
        }
    }

    if let Some(debug) = &call.debug {
        let video_stats = if debug.video_tx > 0 || debug.video_rx > 0 {
            let mut s = format!(" vtx:{} vrx:{}", debug.video_tx, debug.video_rx);
            if debug.video_rx_decrypt_fail > 0 {
                s += &format!(" vfail:{}", debug.video_rx_decrypt_fail);
            }
            s
        } else {
            String::new()
        };
        let stats = format!(
            "TX:{} RX:{} drop:{} jitter:{}ms{}{}",
            debug.tx_frames,
            debug.rx_frames,
            debug.rx_dropped,
            debug.jitter_buffer_ms,
            debug
                .last_rtt_ms
                .map(|rtt| format!(" rtt:{rtt}ms"))
                .unwrap_or_default(),
            video_stats,
        );
        content = content.push(
            container(
                text(stats)
                    .size(11)
                    .color(iced::Color::from_rgba(1.0, 1.0, 1.0, 0.6)),
            )
            .center_x(Fill)
            .padding([4, 12]),
        );
    }

    content
}

fn build_controls<'a>(call: &'a CallState) -> Element<'a, Message, Theme> {
    match &call.status {
        CallStatus::Ringing => row![
            button(text("Decline").size(14).color(iced::Color::WHITE).center())
                .on_press(Message::RejectCall)
                .padding([12, 24])
                .style(theme::danger_button_style),
            Space::new().width(48),
            button(text("Accept").size(14).color(iced::Color::WHITE).center())
                .on_press(Message::AcceptCall)
                .padding([12, 24])
                .style(theme::call_accept_button_style),
        ]
        .align_y(Alignment::Center)
        .into(),
        CallStatus::Offering | CallStatus::Connecting | CallStatus::Active => {
            let mute_label = if call.is_muted { "Unmute" } else { "Mute" };
            let mute_style: fn(&Theme, button::Status) -> button::Style = if call.is_muted {
                theme::call_muted_button_style
            } else {
                theme::call_control_button_style
            };
            let mut controls =
                row![
                    button(text(mute_label).size(14).color(iced::Color::WHITE).center())
                        .on_press(Message::ToggleMute)
                        .padding([12, 24])
                        .style(mute_style),
                ]
                .align_y(Alignment::Center);

            if call.is_video_call {
                let cam_label = if call.is_camera_enabled {
                    "Cam Off"
                } else {
                    "Cam On"
                };
                let cam_style: fn(&Theme, button::Status) -> button::Style =
                    if !call.is_camera_enabled {
                        theme::call_muted_button_style
                    } else {
                        theme::call_control_button_style
                    };
                controls = controls.push(Space::new().width(24)).push(
                    button(text(cam_label).size(14).color(iced::Color::WHITE).center())
                        .on_press(Message::ToggleCamera)
                        .padding([12, 24])
                        .style(cam_style),
                );
            }

            controls = controls.push(Space::new().width(48)).push(
                button(text("End").size(14).color(iced::Color::WHITE).center())
                    .on_press(Message::EndCall)
                    .padding([12, 24])
                    .style(theme::danger_button_style),
            );
            controls.into()
        }
        CallStatus::Ended { reason } => {
            let reason_text = if reason.is_empty() {
                "Call ended".to_string()
            } else {
                reason.clone()
            };
            column![
                text(reason_text)
                    .size(14)
                    .color(iced::Color::from_rgba(1.0, 1.0, 1.0, 0.7))
                    .center()
                    .width(Fill),
                row![
                    button(text("Done").size(14).color(iced::Color::WHITE).center())
                        .on_press(Message::DismissCallScreen)
                        .padding([10, 20])
                        .style(theme::secondary_button_style),
                    Space::new().width(24),
                    button(
                        text("Start Again")
                            .size(14)
                            .color(iced::Color::WHITE)
                            .center()
                    )
                    .on_press(if call.is_video_call {
                        Message::StartVideoCall
                    } else {
                        Message::StartCall
                    })
                    .padding([10, 20])
                    .style(theme::call_accept_button_style),
                ]
                .align_y(Alignment::Center),
            ]
            .spacing(12)
            .align_x(Alignment::Center)
            .width(Fill)
            .into()
        }
    }
}

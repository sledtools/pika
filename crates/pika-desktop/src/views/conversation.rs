use base64::Engine as _;
use iced::widget::{
    button, column, container, operation, row, scrollable, text, text_input, Space, Stack,
};
use iced::{Alignment, Element, Fill, Task, Theme};
use pika_core::{CallState, CallStatus, ChatMessage, ChatViewState};
use std::collections::HashMap;
use std::path::PathBuf;

use crate::design::BubblePosition;
use crate::icons;
use crate::theme;
use crate::views::avatar::avatar_circle;
use crate::views::message_bubble::message_bubble;

const CONVERSATION_SCROLL_ID: &str = "conversation-scroll";

// ── State ───────────────────────────────────────────────────────────────────

pub struct State {
    pub message_input: String,
    pub reply_to_message_id: Option<String>,
    pub emoji_picker_message_id: Option<String>,
    pub hovered_message_id: Option<String>,
    /// True while a file is being dragged over the conversation area.
    pub file_hover: bool,
}

// ── Messages ────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub enum Message {
    MessageChanged(String),
    SendMessage,
    SetReplyTarget(String),
    CancelReplyTarget,
    JumpToMessage(String),
    ReactToMessage {
        message_id: String,
        emoji: String,
    },
    ToggleEmojiPicker(String),
    CloseEmojiPicker,
    HoverMessage(String),
    UnhoverMessage,
    // File upload
    AttachFileClicked,
    FilesPicked(Vec<PathBuf>),
    FilesDropped(Vec<PathBuf>),
    FileHovered,
    FileHoverLeft,
    DownloadMedia {
        message_id: String,
        original_hash_hex: String,
    },
    // These originate from the conversation header but bubble up as events
    ShowGroupInfo,
    StartCall,
    StartVideoCall,
    OpenCallScreen,
    OpenPeerProfile(String),
}

// ── Events ──────────────────────────────────────────────────────────────────

pub enum Event {
    /// The user typed a non-empty message (parent should send typing indicator)
    TypingStarted,
    /// The user pressed Send
    SendMessage {
        content: String,
        reply_to_message_id: Option<String>,
    },
    /// Send a media file (already read + encoded by the conversation module)
    SendMedia {
        data_base64: String,
        mime_type: String,
        filename: String,
    },
    /// Download a received media attachment
    DownloadMedia {
        message_id: String,
        original_hash_hex: String,
    },
    /// Scroll to a specific message (returns a Task for the parent)
    JumpToMessage(String),
    /// A reaction was sent
    ReactToMessage { message_id: String, emoji: String },
    /// The conversation header's group-info button was pressed
    ShowGroupInfo,
    /// The conversation header's call button was pressed
    StartCall,
    /// The conversation header's video call button was pressed
    StartVideoCall,
    /// The conversation header's active-call button was pressed
    OpenCallScreen,
    /// The user clicked a peer's name/avatar to view their profile
    OpenPeerProfile(String),
}

// ── Implementation ──────────────────────────────────────────────────────────

impl State {
    pub fn new() -> Self {
        Self {
            message_input: String::new(),
            reply_to_message_id: None,
            emoji_picker_message_id: None,
            hovered_message_id: None,
            file_hover: false,
        }
    }

    /// Returns `(optional_event, optional_iced_task)`.
    ///
    /// Most messages produce at most an event; the file-picker dialog is the
    /// only case that produces an `iced::Task` the caller must execute.
    pub fn update(&mut self, message: Message) -> (Option<Event>, Option<Task<Message>>) {
        match message {
            Message::MessageChanged(value) => {
                let was_empty = self.message_input.trim().is_empty();
                self.message_input = value;
                let is_empty = self.message_input.trim().is_empty();
                if was_empty && !is_empty {
                    return (Some(Event::TypingStarted), None);
                }
                if !is_empty {
                    return (Some(Event::TypingStarted), None);
                }
                (None, None)
            }
            Message::SendMessage => {
                let content = self.message_input.trim().to_string();
                if content.is_empty() {
                    return (None, None);
                }
                let reply_to = self.reply_to_message_id.take();
                self.message_input.clear();
                self.emoji_picker_message_id = None;
                (
                    Some(Event::SendMessage {
                        content,
                        reply_to_message_id: reply_to,
                    }),
                    None,
                )
            }
            Message::SetReplyTarget(message_id) => {
                self.reply_to_message_id = Some(message_id);
                (None, None)
            }
            Message::CancelReplyTarget => {
                self.reply_to_message_id = None;
                (None, None)
            }
            Message::JumpToMessage(message_id) => (Some(Event::JumpToMessage(message_id)), None),
            Message::ReactToMessage { message_id, emoji } => {
                self.emoji_picker_message_id = None;
                (Some(Event::ReactToMessage { message_id, emoji }), None)
            }
            Message::ToggleEmojiPicker(message_id) => {
                if self.emoji_picker_message_id.as_deref() == Some(&message_id) {
                    self.emoji_picker_message_id = None;
                } else {
                    self.emoji_picker_message_id = Some(message_id);
                }
                (None, None)
            }
            Message::CloseEmojiPicker => {
                self.emoji_picker_message_id = None;
                (None, None)
            }
            Message::HoverMessage(id) => {
                self.hovered_message_id = Some(id);
                (None, None)
            }
            Message::UnhoverMessage => {
                self.hovered_message_id = None;
                (None, None)
            }
            // ── File upload ─────────────────────────────────────────────
            Message::AttachFileClicked => {
                let task = Task::perform(
                    async {
                        let handle = rfd::AsyncFileDialog::new()
                            .set_title("Attach file")
                            .pick_files()
                            .await;
                        match handle {
                            Some(handles) => handles
                                .into_iter()
                                .map(|h| h.path().to_path_buf())
                                .collect(),
                            None => vec![],
                        }
                    },
                    Message::FilesPicked,
                );
                (None, Some(task))
            }
            Message::FilesPicked(paths) => (read_and_send_first_file(&paths), None),
            Message::FilesDropped(paths) => {
                self.file_hover = false;
                (read_and_send_first_file(&paths), None)
            }
            Message::FileHovered => {
                self.file_hover = true;
                (None, None)
            }
            Message::FileHoverLeft => {
                self.file_hover = false;
                (None, None)
            }
            Message::DownloadMedia {
                message_id,
                original_hash_hex,
            } => (
                Some(Event::DownloadMedia {
                    message_id,
                    original_hash_hex,
                }),
                None,
            ),
            Message::ShowGroupInfo => (Some(Event::ShowGroupInfo), None),
            Message::StartCall => (Some(Event::StartCall), None),
            Message::StartVideoCall => (Some(Event::StartVideoCall), None),
            Message::OpenCallScreen => (Some(Event::OpenCallScreen), None),
            Message::OpenPeerProfile(pubkey) => (Some(Event::OpenPeerProfile(pubkey)), None),
        }
    }

    /// Clean up reply target if the referenced message disappeared.
    pub fn clean_reply_target(&mut self, chat: Option<&ChatViewState>) {
        if let Some(reply_id) = self.reply_to_message_id.as_ref() {
            let still_present = chat
                .map(|c| c.messages.iter().any(|msg| &msg.id == reply_id))
                .unwrap_or(false);
            if !still_present {
                self.reply_to_message_id = None;
            }
        }
    }

    /// Center pane: conversation header + message list + input bar.
    pub fn view<'a>(
        &'a self,
        chat: &'a ChatViewState,
        active_call: Option<&'a CallState>,
        avatar_cache: &mut super::avatar::AvatarCache,
    ) -> Element<'a, Message, Theme> {
        // ── Header bar ──────────────────────────────────────────────────
        let title = chat_title(chat);
        let subtitle = if chat.is_group {
            format!("{} members", chat.members.len())
        } else {
            String::new()
        };

        let mut header_info = column![text(title.clone())
            .size(17)
            .font(icons::BOLD)
            .color(theme::TEXT_PRIMARY),];
        if !subtitle.is_empty() {
            header_info = header_info.push(text(subtitle).size(13).color(theme::TEXT_SECONDARY));
        }

        let picture_url = chat.members.first().and_then(|m| m.picture_url.as_deref());

        // Call buttons for 1:1 chats
        let (call_button, video_call_button): (
            Option<Element<'a, Message, Theme>>,
            Option<Element<'a, Message, Theme>>,
        ) = if !chat.is_group {
            let has_live_call_for_chat = active_call
                .as_ref()
                .map(|c| c.chat_id == chat.chat_id && !matches!(c.status, CallStatus::Ended { .. }))
                .unwrap_or(false);
            let has_live_call_elsewhere = active_call
                .as_ref()
                .map(|c| c.chat_id != chat.chat_id && !matches!(c.status, CallStatus::Ended { .. }))
                .unwrap_or(false);

            let phone_icon = if has_live_call_for_chat {
                icons::PHONE_INCOMING
            } else {
                icons::PHONE
            };

            let btn = button(
                text(phone_icon)
                    .font(icons::LUCIDE_FONT)
                    .size(20)
                    .color(theme::TEXT_PRIMARY)
                    .center(),
            )
            .padding([8, 10])
            .style(theme::icon_button_style(false));

            let audio_btn = if has_live_call_elsewhere {
                Some(btn.into())
            } else if has_live_call_for_chat {
                Some(btn.on_press(Message::OpenCallScreen).into())
            } else {
                Some(btn.on_press(Message::StartCall).into())
            };

            // Video call button (camera icon)
            let video_btn = if !has_live_call_for_chat {
                let vbtn = button(
                    text(icons::VIDEO)
                        .font(icons::LUCIDE_FONT)
                        .size(20)
                        .color(theme::TEXT_PRIMARY)
                        .center(),
                )
                .padding([8, 10])
                .style(theme::icon_button_style(false));
                if has_live_call_elsewhere {
                    Some(vbtn.into())
                } else {
                    Some(vbtn.on_press(Message::StartVideoCall).into())
                }
            } else {
                None
            };

            (audio_btn, video_btn)
        } else {
            (None, None)
        };

        // Profile-clickable area (avatar + name) — hover only on this part
        let profile_content = row![
            avatar_circle(Some(&*title), picture_url, 36.0, avatar_cache),
            header_info,
        ]
        .spacing(10)
        .align_y(Alignment::Center);

        let profile_area: Element<'a, Message, Theme> = if chat.is_group {
            button(profile_content)
                .on_press(Message::ShowGroupInfo)
                .padding([8, 12])
                .style(theme::icon_button_style(false))
                .into()
        } else if let Some(peer) = chat.members.first() {
            let peer_pubkey = peer.pubkey.clone();
            button(profile_content)
                .on_press(Message::OpenPeerProfile(peer_pubkey))
                .padding([8, 12])
                .style(theme::icon_button_style(false))
                .into()
        } else {
            container(profile_content).padding([8, 12]).into()
        };

        // Full header row: [profile area] [spacer] [call buttons]
        let mut header_row =
            row![profile_area, Space::new().width(Fill)].align_y(Alignment::Center);

        if let Some(btn) = video_call_button {
            header_row = header_row.push(btn);
        }
        if let Some(btn) = call_button {
            header_row = header_row.push(btn);
        }

        let header = container(header_row.padding([4, 4])).width(Fill);

        // ── Messages ────────────────────────────────────────────────────
        let is_group = chat.is_group;
        let messages_by_id: HashMap<&str, &ChatMessage> =
            chat.messages.iter().map(|m| (m.id.as_str(), m)).collect();
        let messages = {
            let mut col = column![].padding([8, 16]);
            let msgs = &chat.messages;
            for i in 0..msgs.len() {
                let msg = &msgs[i];

                // Determine grouping: consecutive messages from same sender
                let same_as_prev = i > 0
                    && msgs[i - 1].is_mine == msg.is_mine
                    && msgs[i - 1].sender_pubkey == msg.sender_pubkey;
                let same_as_next = i + 1 < msgs.len()
                    && msgs[i + 1].is_mine == msg.is_mine
                    && msgs[i + 1].sender_pubkey == msg.sender_pubkey;

                let position = match (same_as_prev, same_as_next) {
                    (false, false) => BubblePosition::Single,
                    (false, true) => BubblePosition::First,
                    (true, true) => BubblePosition::Middle,
                    (true, false) => BubblePosition::Last,
                };

                // Variable spacing: tight within groups, looser between
                if i > 0 {
                    let gap = if same_as_prev { 2 } else { 12 };
                    col = col.push(Space::new().height(gap));
                }

                let reply_target = msg
                    .reply_to_message_id
                    .as_deref()
                    .and_then(|id| messages_by_id.get(id).copied());
                let picker_open = self.emoji_picker_message_id.as_deref() == Some(msg.id.as_str());
                let hovered = self.hovered_message_id.as_deref() == Some(msg.id.as_str());
                col = col.push(message_bubble(
                    msg,
                    is_group,
                    reply_target,
                    picker_open,
                    hovered,
                    position,
                ));
            }
            col
        };

        let message_scroll = scrollable(messages)
            .id(CONVERSATION_SCROLL_ID)
            .anchor_bottom()
            .height(Fill)
            .width(Fill);

        // ── Input bar ───────────────────────────────────────────────────
        let send_enabled = !self.message_input.trim().is_empty();

        let attach_button = button(
            text(icons::PAPERCLIP)
                .font(icons::LUCIDE_FONT)
                .size(20)
                .center(),
        )
        .on_press(Message::AttachFileClicked)
        .width(36.0)
        .height(36.0)
        .style(|_: &Theme, status: button::Status| {
            let (bg, fg) = match status {
                button::Status::Hovered => (theme::HOVER_BG, theme::TEXT_PRIMARY),
                _ => (iced::Color::TRANSPARENT, theme::TEXT_FADED),
            };
            button::Style {
                background: Some(iced::Background::Color(bg)),
                text_color: fg,
                border: iced::border::rounded(9999),
                ..Default::default()
            }
        });

        let send_button = if send_enabled {
            button(
                text(icons::ARROW_UP)
                    .font(icons::LUCIDE_FONT)
                    .size(20)
                    .center(),
            )
            .on_press(Message::SendMessage)
            .width(36.0)
            .height(36.0)
            .style(|_: &Theme, status: button::Status| {
                let bg = match status {
                    button::Status::Hovered => theme::ACCENT_BLUE.scale_alpha(0.85),
                    _ => theme::ACCENT_BLUE,
                };
                button::Style {
                    background: Some(iced::Background::Color(bg)),
                    text_color: iced::Color::WHITE,
                    border: iced::border::rounded(9999),
                    ..Default::default()
                }
            })
        } else {
            button(
                text(icons::ARROW_UP)
                    .font(icons::LUCIDE_FONT)
                    .size(20)
                    .center(),
            )
            .width(36.0)
            .height(36.0)
            .style(|_: &Theme, _status: button::Status| button::Style {
                background: Some(iced::Background::Color(theme::HOVER_BG)),
                text_color: theme::TEXT_FADED,
                border: iced::border::rounded(9999),
                ..Default::default()
            })
        };

        let composer = row![
            attach_button,
            text_input("Message\u{2026}", &self.message_input)
                .on_input(Message::MessageChanged)
                .on_submit(Message::SendMessage)
                .padding(10)
                .width(Fill)
                .style(theme::dark_input_style),
            send_button,
        ]
        .spacing(8)
        .align_y(Alignment::Center)
        .padding([10, 16]);

        let mut input_column = column![].spacing(6);
        let replying_to = self
            .reply_to_message_id
            .as_ref()
            .and_then(|reply_id| chat.messages.iter().find(|message| message.id == *reply_id));
        if let Some(replying) = replying_to {
            let sender = if replying.is_mine {
                "You".to_string()
            } else {
                replying
                    .sender_name
                    .clone()
                    .filter(|name| !name.trim().is_empty())
                    .unwrap_or_else(|| replying.sender_pubkey.chars().take(8).collect())
            };
            let snippet = replying
                .display_content
                .lines()
                .next()
                .unwrap_or("")
                .trim()
                .to_string();
            let snippet = if snippet.is_empty() {
                "(empty message)".to_string()
            } else if snippet.chars().count() > 80 {
                format!("{}…", snippet.chars().take(80).collect::<String>())
            } else {
                snippet
            };
            let reply_row = row![
                column![
                    text(format!("Replying to {sender}"))
                        .size(13)
                        .color(theme::TEXT_SECONDARY),
                    text(snippet).size(13).color(theme::TEXT_FADED),
                ]
                .spacing(2)
                .width(Fill),
                button(text("Cancel").size(12))
                    .on_press(Message::CancelReplyTarget)
                    .style(theme::secondary_button_style),
            ]
            .spacing(8)
            .align_y(Alignment::Center)
            .padding([6, 16]);
            input_column = input_column.push(reply_row);
        }
        input_column = input_column.push(composer);

        let input_bar = container(input_column)
            .width(Fill)
            .style(theme::input_bar_style);

        // ── Typing indicator ─────────────────────────────────────────────
        let typing_indicator: Option<Element<'a, Message, Theme>> =
            if !chat.typing_members.is_empty() {
                let label = match chat.typing_members.len() {
                    1 => {
                        let name = chat.typing_members[0].name.as_deref().unwrap_or("Someone");
                        format!("{name} is typing\u{2026}")
                    }
                    2 => {
                        let a = chat.typing_members[0].name.as_deref().unwrap_or("Someone");
                        let b = chat.typing_members[1].name.as_deref().unwrap_or("Someone");
                        format!("{a} and {b} are typing\u{2026}")
                    }
                    n => {
                        let first = chat.typing_members[0].name.as_deref().unwrap_or("Someone");
                        format!("{first} and {} others are typing\u{2026}", n - 1)
                    }
                };
                Some(
                    container(text(label).size(13).color(theme::TEXT_SECONDARY))
                        .padding([4, 16])
                        .into(),
                )
            } else {
                None
            };

        // ── Compose ─────────────────────────────────────────────────────
        let mut layout = column![header, message_scroll,].width(Fill).height(Fill);

        if let Some(indicator) = typing_indicator {
            layout = layout.push(indicator);
        }

        layout = layout.push(input_bar);

        // ── Drop zone overlay ────────────────────────────────────────────
        if self.file_hover {
            let drop_overlay = container(
                container(
                    column![
                        text(icons::PAPERCLIP)
                            .font(icons::LUCIDE_FONT)
                            .size(32)
                            .color(theme::ACCENT_BLUE),
                        text("Drop file to send")
                            .size(16)
                            .color(theme::TEXT_PRIMARY),
                    ]
                    .spacing(8)
                    .align_x(Alignment::Center),
                )
                .center_x(Fill)
                .center_y(Fill)
                .padding(24)
                .style(theme::drop_zone_style),
            )
            .width(Fill)
            .height(Fill)
            .padding(16);

            Stack::new()
                .push(layout)
                .push(drop_overlay)
                .width(Fill)
                .height(Fill)
                .into()
        } else {
            layout.into()
        }
    }
}

// ── Helpers ─────────────────────────────────────────────────────────────────

pub fn jump_to_message_task(chat: &ChatViewState, message_id: &str) -> Option<Task<Message>> {
    if chat.messages.is_empty() {
        return None;
    }
    let Some(index) = chat.messages.iter().position(|m| m.id == message_id) else {
        return None;
    };
    let denom = chat.messages.len().saturating_sub(1) as f32;
    let y = if denom <= 0.0 {
        1.0
    } else {
        (index as f32 / denom).clamp(0.0, 1.0)
    };
    Some(operation::snap_to(
        CONVERSATION_SCROLL_ID,
        operation::RelativeOffset { x: 0.0, y },
    ))
}

/// Read the first file from a list of paths, base64-encode it, and return a
/// [`Event::SendMedia`] event ready for dispatch to the core.
///
/// MIME type is sent as empty string so the Rust core can infer it from the
/// filename extension (matching the iOS app's behaviour).
fn read_and_send_first_file(paths: &[PathBuf]) -> Option<Event> {
    let path = paths.first()?;
    let data = std::fs::read(path).ok()?;
    let filename = path
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .to_string();
    let data_base64 = base64::engine::general_purpose::STANDARD.encode(&data);
    Some(Event::SendMedia {
        data_base64,
        // Empty string → Rust core infers MIME from filename extension.
        mime_type: String::new(),
        filename,
    })
}

fn chat_title(chat: &ChatViewState) -> String {
    if let Some(name) = &chat.group_name {
        if !name.trim().is_empty() {
            return name.clone();
        }
    }
    chat.members
        .first()
        .and_then(|m| m.name.clone())
        .unwrap_or_else(|| "Conversation".to_string())
}

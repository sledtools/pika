use iced::widget::{column, container, mouse_area, row, rule, Stack};
use iced::{Element, Fill, Padding, Task, Theme};
use pika_core::{
    project_desktop, AppAction, AppState, AuthState, CallStatus, DesktopDetailPane,
    DesktopShellMode, FollowListEntry, Screen,
};

use crate::app_manager::AppManager;
use crate::screen::login::should_offer_relay_reset;
use crate::theme;
use crate::video;
use crate::views;

/// The main display area: chats, group creation, etc
#[derive(Debug)]
pub enum Pane {
    Empty,
    NewChat(views::new_chat::State),
    NewGroup(views::new_group_chat::State),
    MyProfile(views::my_profile::State),
}

// ── State ───────────────────────────────────────────────────────────────────

pub struct State {
    /// Activity in the main application pane
    pane: Pane,
    my_npub: String,
    conversation: views::conversation::State,
    group_info: Option<views::group_info::State>,
    optimistic_selected_chat_id: Option<String>,
    pub show_call_screen: bool,
    profile_toast: Option<String>,
    video_pipeline: video::DesktopVideoPipeline,
}

// ── Messages ────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub enum Message {
    // Delegated to child view modules
    Toast(views::toast::Message),
    CallBanner(views::call_banner::Message),
    CallScreen(views::call_screen::Message),
    ChatRail(views::chat_rail::Message),
    Conversation(views::conversation::Message),
    GroupInfo(views::group_info::Message),
    NewChat(views::new_chat::Message),
    NewGroup(views::new_group_chat::Message),
    MyProfile(views::my_profile::Message),
    PeerProfile(views::peer_profile::Message),
    // Window drag (macOS transparent titlebar)
    DragWindow,
    // Call timer (no-op, triggers re-render)
    CallTimerTick,
    // Video frame tick (triggers re-render for video calls)
    VideoFrameTick,
}

pub enum Event {
    /// Instruction to the core manager
    AppAction(AppAction),
    /// Destroy the session
    Logout,
    /// Perform an Iced task
    Task(Task<Message>),
}

// ── Helpers ─────────────────────────────────────────────────────────────────

pub fn effective_selected_chat_id<'a>(
    route_selected_chat_id: Option<&'a str>,
    optimistic_selected_chat_id: Option<&'a str>,
) -> Option<&'a str> {
    optimistic_selected_chat_id.or(route_selected_chat_id)
}

fn follow_source<'a>(
    state: &'a AppState,
    cached_profiles: &'a [FollowListEntry],
) -> &'a [FollowListEntry] {
    if state.follow_list.is_empty() {
        cached_profiles
    } else {
        &state.follow_list
    }
}

// ── Implementation ──────────────────────────────────────────────────────────

impl State {
    pub fn new(state: &AppState) -> Self {
        let my_npub = match &state.auth {
            AuthState::LoggedIn { npub, .. } => npub.clone(),
            AuthState::LoggedOut => String::new(),
        };
        Self {
            pane: Pane::Empty,
            my_npub,
            conversation: views::conversation::State::new(),
            group_info: None,
            optimistic_selected_chat_id: None,
            show_call_screen: false,
            profile_toast: None,
            video_pipeline: video::DesktopVideoPipeline::new(),
        }
    }

    /// Returns true when the overlay is NewChat or NewGroup (i.e. needs follow
    /// list data).
    pub fn needs_follow_list(&self) -> bool {
        matches!(self.pane, Pane::NewChat(_) | Pane::NewGroup(_))
    }

    // ── Sync ────────────────────────────────────────────────────────────────

    /// Called from the top-level `sync_from_manager` when a new core state
    /// revision arrives. `old_state` is the *previous* AppState and
    /// `new_state` is the freshly-fetched one (not yet stored on DesktopApp).
    pub fn sync_from_update(
        &mut self,
        old_state: &AppState,
        new_state: &AppState,
        manager: &AppManager,
        cached_profiles: &[FollowListEntry],
    ) {
        // Close new-chat form once creating_chat finishes.
        if old_state.busy.creating_chat && !new_state.busy.creating_chat {
            if matches!(self.pane, Pane::NewChat(_)) {
                self.pane = Pane::Empty;
            }
            if matches!(self.pane, Pane::NewGroup(_)) {
                self.pane = Pane::Empty;
            }
        }

        // Sync my_profile drafts when profile state updates.
        if let Pane::MyProfile(ref mut profile) = self.pane {
            if old_state.my_profile.name != new_state.my_profile.name {
                profile.sync_profile(&new_state.my_profile);
            }
        }

        // Reconcile optimistic selection.
        let latest_route = project_desktop(new_state);
        if let Some(optimistic_chat_id) = self.optimistic_selected_chat_id.as_deref() {
            let authoritative_selection = latest_route.selected_chat_id.as_deref();
            if authoritative_selection == Some(optimistic_chat_id)
                || authoritative_selection.is_some()
                || matches!(latest_route.shell_mode, DesktopShellMode::Login)
            {
                self.optimistic_selected_chat_id = None;
            }
        }

        // Auto-dismiss call screen when call ends.
        let had_call = old_state.active_call.is_some();
        let has_call = new_state.active_call.is_some();
        if had_call && !has_call {
            self.show_call_screen = false;
        }

        // Sync video pipeline with call state.
        self.video_pipeline
            .sync_with_call(new_state.active_call.as_ref(), manager);

        // Clean up reply target if the referenced message disappeared.
        self.conversation
            .clean_reply_target(new_state.current_chat.as_ref());

        // Refilter follows if an overlay needs them.
        let source: Vec<_> = follow_source(new_state, cached_profiles)
            .iter()
            .filter(|entry| self.my_npub != entry.npub)
            .cloned()
            .collect();
        match &mut self.pane {
            Pane::NewChat(ref mut s) => {
                s.update_follows(source.as_slice());
            }
            Pane::NewGroup(ref mut s) => {
                s.update_follows(source.as_slice());
            }
            _ => {}
        }

        // Retry follow list fetch if the overlay needs it.
        let needs_follows = self.needs_follow_list();
        if needs_follows && new_state.follow_list.is_empty() && !new_state.busy.fetching_follow_list
        {
            manager.dispatch(AppAction::RefreshFollowList);
        }
    }

    pub fn update(
        &mut self,
        message: Message,
        state: &AppState,
        manager: &AppManager,
        cached_profiles: &[FollowListEntry],
    ) -> Option<Event> {
        match message {
            // ── Window drag ───────────────────────────────────────────
            Message::DragWindow => {
                return Some(Event::Task(iced::window::oldest().then(|id| {
                    if let Some(id) = id {
                        iced::window::drag(id)
                    } else {
                        Task::none()
                    }
                })));
            }

            // ── Toast ─────────────────────────────────────────────────
            Message::Toast(msg) => match msg {
                views::toast::Message::ClearToast => {
                    if self.profile_toast.is_some() {
                        self.profile_toast = None;
                    } else {
                        manager.dispatch(AppAction::ClearToast);
                    }
                }
                views::toast::Message::ResetRelayConfig => {
                    manager.reset_relay_config_to_defaults();
                }
            },

            // ── Call banner ───────────────────────────────────────────
            Message::CallBanner(msg) => match msg {
                views::call_banner::Message::Accept => {
                    if let Some(call) = &state.active_call {
                        manager.dispatch(AppAction::AcceptCall {
                            chat_id: call.chat_id.clone(),
                        });
                    }
                    self.show_call_screen = true;
                }
                views::call_banner::Message::Reject => {
                    if let Some(call) = &state.active_call {
                        manager.dispatch(AppAction::RejectCall {
                            chat_id: call.chat_id.clone(),
                        });
                    }
                    self.show_call_screen = false;
                }
            },

            // ── Call screen ───────────────────────────────────────────
            Message::CallScreen(msg) => match msg {
                views::call_screen::Message::StartCall => {
                    if let Some(chat) = &state.current_chat {
                        manager.dispatch(AppAction::StartCall {
                            chat_id: chat.chat_id.clone(),
                        });
                    }
                    self.show_call_screen = true;
                }
                views::call_screen::Message::StartVideoCall => {
                    if let Some(chat) = &state.current_chat {
                        manager.dispatch(AppAction::StartVideoCall {
                            chat_id: chat.chat_id.clone(),
                        });
                    }
                    self.show_call_screen = true;
                }
                views::call_screen::Message::AcceptCall => {
                    if let Some(call) = &state.active_call {
                        manager.dispatch(AppAction::AcceptCall {
                            chat_id: call.chat_id.clone(),
                        });
                    }
                    self.show_call_screen = true;
                }
                views::call_screen::Message::RejectCall => {
                    if let Some(call) = &state.active_call {
                        manager.dispatch(AppAction::RejectCall {
                            chat_id: call.chat_id.clone(),
                        });
                    }
                    self.show_call_screen = false;
                }
                views::call_screen::Message::EndCall => {
                    manager.dispatch(AppAction::EndCall);
                }
                views::call_screen::Message::ToggleMute => {
                    manager.dispatch(AppAction::ToggleMute);
                }
                views::call_screen::Message::ToggleCamera => {
                    manager.dispatch(AppAction::ToggleCamera);
                }
                views::call_screen::Message::DismissCallScreen => {
                    self.show_call_screen = false;
                }
            },

            // ── Chat rail ─────────────────────────────────────────────
            Message::ChatRail(msg) => match msg {
                views::chat_rail::Message::OpenChat(chat_id) => {
                    self.optimistic_selected_chat_id = Some(chat_id.clone());
                    self.conversation.emoji_picker_message_id = None;
                    self.clear_pane();
                    manager.dispatch(AppAction::OpenChat { chat_id });
                }
                views::chat_rail::Message::ClickNewChat => {
                    let opening = !matches!(self.pane, Pane::NewChat(_));
                    self.clear_pane();
                    if opening {
                        let source = follow_source(state, cached_profiles);
                        self.pane = Pane::NewChat(views::new_chat::State::new(source));
                        manager.dispatch(AppAction::RefreshFollowList);
                    }
                }
                views::chat_rail::Message::ClickNewGroup => {
                    let opening = !matches!(self.pane, Pane::NewGroup(_));
                    self.clear_pane();
                    if opening {
                        let source = follow_source(state, cached_profiles);
                        self.pane = Pane::NewGroup(views::new_group_chat::State::new(source));
                        manager.dispatch(AppAction::RefreshFollowList);
                    }
                }
                views::chat_rail::Message::ClickMyProfile => {
                    if matches!(self.pane, Pane::MyProfile(_)) {
                        self.clear_pane();
                    } else {
                        self.pane =
                            Pane::MyProfile(views::my_profile::State::new(&state.my_profile));
                        manager.dispatch(AppAction::RefreshMyProfile);
                    }
                }
            },

            // ── Conversation ──────────────────────────────────────────
            Message::Conversation(msg) => {
                if let Some(event) = self.conversation.update(msg) {
                    match event {
                        views::conversation::Event::TypingStarted => {
                            if let Some(chat) = &state.current_chat {
                                manager.dispatch(AppAction::TypingStarted {
                                    chat_id: chat.chat_id.clone(),
                                });
                            }
                        }
                        views::conversation::Event::SendMessage {
                            content,
                            reply_to_message_id,
                        } => {
                            if let Some(chat) = &state.current_chat {
                                manager.dispatch(AppAction::SendMessage {
                                    chat_id: chat.chat_id.clone(),
                                    content,
                                    kind: None,
                                    reply_to_message_id,
                                });
                            }
                        }
                        views::conversation::Event::JumpToMessage(message_id) => {
                            if let Some(chat) = &state.current_chat {
                                if let Some(task) =
                                    views::conversation::jump_to_message_task(chat, &message_id)
                                {
                                    return Some(Event::Task(task.map(Message::Conversation)));
                                }
                            }
                        }
                        views::conversation::Event::ReactToMessage { message_id, emoji } => {
                            if let Some(chat) = &state.current_chat {
                                manager.dispatch(AppAction::ReactToMessage {
                                    chat_id: chat.chat_id.clone(),
                                    message_id,
                                    emoji,
                                });
                            }
                        }
                        views::conversation::Event::ShowGroupInfo => {
                            self.clear_pane();
                            if let Some(chat) = &state.current_chat {
                                self.group_info =
                                    Some(views::group_info::State::new(chat.group_name.as_deref()));

                                manager.dispatch(AppAction::PushScreen {
                                    screen: Screen::GroupInfo {
                                        chat_id: chat.chat_id.clone(),
                                    },
                                });
                            }
                        }
                        views::conversation::Event::StartCall => {
                            if let Some(chat) = &state.current_chat {
                                manager.dispatch(AppAction::StartCall {
                                    chat_id: chat.chat_id.clone(),
                                });
                            }
                            self.show_call_screen = true;
                        }
                        views::conversation::Event::StartVideoCall => {
                            if let Some(chat) = &state.current_chat {
                                manager.dispatch(AppAction::StartVideoCall {
                                    chat_id: chat.chat_id.clone(),
                                });
                            }
                            self.show_call_screen = true;
                        }
                        views::conversation::Event::OpenCallScreen => {
                            self.show_call_screen = true;
                        }
                        views::conversation::Event::OpenPeerProfile(pubkey) => {
                            manager.dispatch(AppAction::OpenPeerProfile { pubkey });
                        }
                    }
                }
            }

            // ── Group info ────────────────────────────────────────────
            Message::GroupInfo(msg) => {
                if let Some(ref mut gi_state) = self.group_info {
                    if let Some(event) = gi_state.update(msg) {
                        match event {
                            views::group_info::Event::RenameGroup { name } => {
                                if let Some(chat) = &state.current_chat {
                                    manager.dispatch(AppAction::RenameGroup {
                                        chat_id: chat.chat_id.clone(),
                                        name,
                                    });
                                }
                            }
                            views::group_info::Event::AddMember { npub } => {
                                if let Some(chat) = &state.current_chat {
                                    manager.dispatch(AppAction::AddGroupMembers {
                                        chat_id: chat.chat_id.clone(),
                                        peer_npubs: vec![npub],
                                    });
                                }
                            }
                            views::group_info::Event::RemoveMember { pubkey } => {
                                if let Some(chat) = &state.current_chat {
                                    manager.dispatch(AppAction::RemoveGroupMembers {
                                        chat_id: chat.chat_id.clone(),
                                        member_pubkeys: vec![pubkey],
                                    });
                                }
                            }
                            views::group_info::Event::LeaveGroup => {
                                if let Some(chat) = &state.current_chat {
                                    manager.dispatch(AppAction::LeaveGroup {
                                        chat_id: chat.chat_id.clone(),
                                    });
                                }
                                self.group_info = None;
                                self.clear_pane();
                            }
                            views::group_info::Event::Close => {
                                self.group_info = None;
                                let mut stack = state.router.screen_stack.clone();
                                if matches!(stack.last(), Some(Screen::GroupInfo { .. })) {
                                    stack.pop();
                                    manager.dispatch(AppAction::UpdateScreenStack { stack });
                                }
                            }
                            views::group_info::Event::OpenPeerProfile { pubkey } => {
                                manager.dispatch(AppAction::OpenPeerProfile { pubkey });
                            }
                        }
                    }
                }
            }

            // ── New chat ──────────────────────────────────────────────
            Message::NewChat(msg) => {
                if let Pane::NewChat(ref mut nc_state) = self.pane {
                    let source = follow_source(state, cached_profiles);
                    if let Some(event) = nc_state.update(msg, source) {
                        match event {
                            views::new_chat::Event::CreateChat { peer_npub } => {
                                manager.dispatch(AppAction::CreateChat { peer_npub });
                            }
                        }
                    }
                }
            }

            // ── New group ─────────────────────────────────────────────
            Message::NewGroup(msg) => {
                if let Pane::NewGroup(ref mut ng_state) = self.pane {
                    let source = follow_source(state, cached_profiles);
                    if let Some(event) = ng_state.update(msg, source) {
                        match event {
                            views::new_group_chat::Event::CreateGroup {
                                peer_npubs,
                                group_name,
                            } => {
                                {
                                    manager.dispatch(AppAction::CreateGroupChat {
                                        peer_npubs,
                                        group_name,
                                    });
                                }
                                self.clear_pane();
                                self.optimistic_selected_chat_id = None;
                            }
                        }
                    }
                }
            }

            // ── My profile ────────────────────────────────────────────
            Message::MyProfile(msg) => {
                if let Pane::MyProfile(ref mut profile_state) = self.pane {
                    if let Some(event) = profile_state.update(msg) {
                        match event {
                            views::my_profile::Event::AppAction(action) => {
                                return Some(Event::AppAction(action));
                            }
                            views::my_profile::Event::CopyNpub => {
                                if let AuthState::LoggedIn { ref npub, .. } = state.auth {
                                    self.profile_toast = Some("Copied npub".to_string());
                                    return Some(Event::Task(
                                        iced::clipboard::write(npub.clone())
                                            .map(|_: ()| Message::CallTimerTick),
                                    ));
                                }
                            }
                            views::my_profile::Event::CopyAppVersion => {
                                let version = crate::app_version_display();
                                self.profile_toast = Some("Copied app version".to_string());
                                return Some(Event::Task(
                                    iced::clipboard::write(version)
                                        .map(|_: ()| Message::CallTimerTick),
                                ));
                            }
                            views::my_profile::Event::Logout => {
                                return Some(Event::Logout);
                            }
                        }
                    }
                }
            }

            // ── Peer profile ──────────────────────────────────────────
            Message::PeerProfile(msg) => {
                if let Some(event) = views::peer_profile::update(msg) {
                    match event {
                        views::peer_profile::Event::Close => {
                            manager.dispatch(AppAction::ClosePeerProfile);
                        }
                        views::peer_profile::Event::CopyNpub => {
                            if let Some(profile) = &state.peer_profile {
                                self.profile_toast = Some("Copied npub".to_string());
                                return Some(Event::Task(
                                    iced::clipboard::write(profile.npub.clone())
                                        .map(|_: ()| Message::CallTimerTick),
                                ));
                            }
                        }
                        views::peer_profile::Event::Follow => {
                            if let Some(profile) = &state.peer_profile {
                                manager.dispatch(AppAction::FollowUser {
                                    pubkey: profile.pubkey.clone(),
                                });
                            }
                        }
                        views::peer_profile::Event::Unfollow => {
                            if let Some(profile) = &state.peer_profile {
                                manager.dispatch(AppAction::UnfollowUser {
                                    pubkey: profile.pubkey.clone(),
                                });
                            }
                        }
                        views::peer_profile::Event::StartChat { peer_npub } => {
                            manager.dispatch(AppAction::CreateChat { peer_npub });
                        }
                    }
                }
            }

            // ── Call timer tick ────────────────────────────────────────
            Message::CallTimerTick => {
                // No-op: triggers a re-render so the duration display updates.
            }

            // ── Video frame tick ──────────────────────────────────────
            Message::VideoFrameTick => {
                // Check for stale remote video and trigger re-render for new frames.
                self.video_pipeline.check_staleness();
            }
        }

        None
    }

    // ── View ────────────────────────────────────────────────────────────────

    pub fn view<'a>(
        &'a self,
        state: &'a AppState,
        avatar_cache: &'a std::cell::RefCell<views::avatar::AvatarCache>,
        app_version_display: &'a str,
    ) -> Element<'a, Message, Theme> {
        let route = project_desktop(state);

        // ── Toast (floating overlay — added to Stack at the end) ───
        let toast_overlay: Option<Element<'_, Message>> = if let Some(toast_msg) =
            self.profile_toast.as_deref().or(state.toast.as_deref())
        {
            let show_relay_reset = self.profile_toast.is_none() && should_offer_relay_reset(state);
            Some(views::toast::toast_bar(toast_msg, show_relay_reset).map(Message::Toast))
        } else {
            None
        };

        let mut main_column = column![];

        // ── Incoming call banner ────────────────────────────────────
        if let Some(call) = &state.active_call {
            if matches!(call.status, CallStatus::Ringing) {
                let peer_name = state
                    .chat_list
                    .iter()
                    .find(|c| c.chat_id == call.chat_id)
                    .and_then(|c| c.members.first())
                    .and_then(|m| m.name.as_deref())
                    .unwrap_or("Unknown");
                main_column = main_column.push(
                    views::call_banner::view(peer_name, call.is_video_call)
                        .map(Message::CallBanner),
                );
            }
        }

        let cache = &mut *avatar_cache.borrow_mut();
        cache.reset_budget();

        // ── Chat rail (left sidebar) ────────────────────────────────
        let my_profile_pic = state.my_profile.picture_url.as_deref();
        let selected_chat_id = effective_selected_chat_id(
            route.selected_chat_id.as_deref(),
            self.optimistic_selected_chat_id.as_deref(),
        );
        let rail = views::chat_rail::view(
            &state.chat_list,
            selected_chat_id,
            matches!(self.pane, Pane::NewChat(_)),
            matches!(self.pane, Pane::NewGroup(_)),
            matches!(self.pane, Pane::MyProfile(_)),
            my_profile_pic,
            cache,
        )
        .map(Message::ChatRail);

        // ── Center pane routing ─────────────────────────────────────
        let center_pane: Element<'_, Message> = if let Pane::MyProfile(ref profile) = self.pane {
            profile
                .view(
                    &self.my_npub,
                    app_version_display,
                    state.my_profile.picture_url.as_deref(),
                    cache,
                )
                .map(Message::MyProfile)
        } else if matches!(route.detail_pane, DesktopDetailPane::GroupInfo { .. }) {
            if let (Some(ref gi_state), Some(chat)) = (&self.group_info, &state.current_chat) {
                let my_pubkey = match &state.auth {
                    AuthState::LoggedIn { pubkey, .. } => pubkey.as_str(),
                    _ => "",
                };
                gi_state
                    .view(chat, my_pubkey, cache)
                    .map(Message::GroupInfo)
            } else {
                views::empty_state::view()
            }
        } else if let Pane::NewGroup(ref group) = self.pane {
            group
                .view(
                    state.busy.creating_chat,
                    state.busy.fetching_follow_list,
                    cache,
                )
                .map(Message::NewGroup)
        } else if let Pane::NewChat(ref new_chat) = self.pane {
            new_chat
                .view(
                    state.busy.creating_chat,
                    state.busy.fetching_follow_list,
                    cache,
                )
                .map(Message::NewChat)
        } else if let Some(call) = state.active_call.as_ref().filter(|_| self.show_call_screen) {
            let peer_name = state
                .current_chat
                .as_ref()
                .and_then(|c| c.members.first())
                .and_then(|m| m.name.as_deref())
                .unwrap_or("Unknown");
            views::call_screen::call_screen_view(call, peer_name, &self.video_pipeline)
                .map(Message::CallScreen)
        } else if matches!(route.detail_pane, DesktopDetailPane::PeerProfile { .. }) {
            if let Some(profile) = &state.peer_profile {
                views::peer_profile::peer_profile_view(profile, cache).map(Message::PeerProfile)
            } else {
                views::empty_state::view()
            }
        } else if route.selected_chat_id.is_some() {
            if let Some(chat) = &state.current_chat {
                self.conversation
                    .view(chat, state.active_call.as_ref(), cache)
                    .map(Message::Conversation)
            } else {
                views::empty_state::view()
            }
        } else {
            views::empty_state::view()
        };

        // Wrap center pane with top inset for macOS titlebar
        let center_pane_inset = container(center_pane)
            .padding(Padding {
                top: 28.0,
                right: 0.0,
                bottom: 0.0,
                left: 0.0,
            })
            .width(Fill)
            .height(Fill);

        let content = row![
            rail,
            rule::vertical(1).style(theme::subtle_rule_style),
            center_pane_inset
        ]
        .height(Fill);
        main_column = main_column.push(content);

        let base = container(main_column)
            .width(Fill)
            .height(Fill)
            .style(theme::surface_style);

        // Invisible drag target in the titlebar zone (28px tall, full width)
        let drag_target =
            mouse_area(container(column![]).width(Fill).height(28)).on_press(Message::DragWindow);

        // Stack: [content] [drag overlay at top] [toast overlay]
        let mut stack = Stack::new().push(base).push(drag_target);

        if let Some(toast) = toast_overlay {
            stack = stack.push(toast);
        }

        stack.width(Fill).height(Fill).into()
    }

    // ── Private helpers ─────────────────────────────────────────────────────

    fn clear_pane(&mut self) {
        self.pane = Pane::Empty;
    }
}

#[cfg(test)]
mod tests {
    use super::effective_selected_chat_id;

    #[test]
    fn effective_selected_chat_prefers_optimistic_selection() {
        let selected = effective_selected_chat_id(Some("route-chat"), Some("optimistic-chat"));
        assert_eq!(selected, Some("optimistic-chat"));
    }

    #[test]
    fn effective_selected_chat_falls_back_to_projected_selection() {
        let selected = effective_selected_chat_id(Some("route-chat"), None);
        assert_eq!(selected, Some("route-chat"));
    }
}

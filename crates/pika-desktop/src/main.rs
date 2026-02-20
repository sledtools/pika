mod app_manager;
mod theme;
mod views;

use app_manager::AppManager;
use iced::widget::{column, container, row, rule, text};
use iced::{Element, Fill, Font, Size, Subscription, Task, Theme};
use pika_core::{
    project_desktop, AppAction, AppState, AuthState, DesktopDetailPane, DesktopShellMode, Screen,
};
use std::time::Duration;

fn app_version_display() -> String {
    let version = env!("CARGO_PKG_VERSION");
    if let Some(build) = option_env!("PIKA_BUILD_NUMBER") {
        format!("v{version} ({build})")
    } else {
        format!("v{version}")
    }
}

pub fn main() -> iced::Result {
    iced::application(DesktopApp::new, DesktopApp::update, DesktopApp::view)
        .title("Pika Desktop")
        .subscription(DesktopApp::subscription)
        .theme(dark_theme)
        .window_size(Size::new(1024.0, 720.0))
        .font(include_bytes!("../fonts/UbuntuSansMono.ttf").as_slice())
        .default_font(Font::with_name("Ubuntu Sans Mono"))
        .run()
}

fn dark_theme(_state: &DesktopApp) -> Theme {
    Theme::Dark
}

fn manager_update_stream(manager: &AppManager) -> impl iced::futures::Stream<Item = ()> {
    let rx = manager.subscribe_updates();
    iced::futures::stream::unfold(rx, |rx| async move {
        match rx.recv_async().await {
            Ok(()) => Some(((), rx)),
            Err(_) => None,
        }
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum UiOverlay {
    None,
    NewChat,
    NewGroup,
    MyProfile,
}

struct DesktopApp {
    manager: Option<AppManager>,
    boot_error: Option<String>,
    state: AppState,
    avatar_cache: std::cell::RefCell<views::avatar::AvatarCache>,
    cached_profiles: Vec<pika_core::FollowListEntry>,
    nsec_input: String,
    new_chat_input: String,
    new_chat_search: String,
    filtered_follows: Vec<pika_core::FollowListEntry>,
    message_input: String,
    optimistic_selected_chat_id: Option<String>,
    overlay: UiOverlay,
    // Group creation
    group_name_input: String,
    selected_group_members: Vec<String>,
    // My profile
    profile_name_draft: String,
    profile_about_draft: String,
    app_version_display: String,
    profile_toast: Option<String>,
    // Group info
    group_info_name_draft: String,
    group_info_npub_input: String,
}

#[derive(Debug, Clone)]
pub enum Message {
    CoreUpdated,
    RelativeTimeTick,
    NsecChanged(String),
    Login,
    CreateAccount,
    Logout,
    ResetLocalSessionData,
    ResetRelayConfig,
    NewChatChanged(String),
    NewChatSearchChanged(String),
    StartChat,
    StartChatWith(String),
    ToggleNewChatForm,
    OpenChat(String),
    MessageChanged(String),
    SendMessage,
    ClearToast,
    // Group creation
    ToggleNewGroupForm,
    GroupNameChanged(String),
    ToggleGroupMember(String),
    AddManualGroupMember,
    CreateGroup,
    // My profile
    ToggleMyProfile,
    ProfileNameChanged(String),
    ProfileAboutChanged(String),
    SaveProfile,
    CopyNpub,
    CopyAppVersion,
    // Group info
    ShowGroupInfo,
    CloseGroupInfo,
    GroupInfoNameChanged(String),
    RenameGroup,
    AddGroupMember,
    RemoveGroupMember(String),
    LeaveGroup,
    GroupInfoNpubChanged(String),
}

impl DesktopApp {
    fn new() -> (Self, Task<Message>) {
        let data_dir = app_manager::resolve_data_dir()
            .unwrap_or_else(|_| std::path::PathBuf::from(".pika"))
            .to_string_lossy()
            .to_string();
        let cached_profiles = pika_core::load_cached_profiles(&data_dir);

        let (manager, boot_error, state) = match AppManager::new() {
            Ok(manager) => {
                let state = manager.state();
                (Some(manager), None, state)
            }
            Err(error) => (
                None,
                Some(format!("failed to start desktop manager: {error}")),
                AppState::empty(),
            ),
        };

        (
            Self::from_boot_state(cached_profiles, manager, boot_error, state),
            Task::none(),
        )
    }

    fn subscription(&self) -> Subscription<Message> {
        if let Some(manager) = &self.manager {
            let core_updates = Subscription::run_with(manager.clone(), manager_update_stream)
                .map(|_| Message::CoreUpdated);
            let relative_time_ticks =
                iced::time::every(Duration::from_secs(30)).map(|_| Message::RelativeTimeTick);

            Subscription::batch([core_updates, relative_time_ticks])
        } else {
            Subscription::none()
        }
    }

    fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::CoreUpdated => self.sync_from_manager(),
            Message::RelativeTimeTick => self.retry_follow_list_if_needed(),
            Message::NsecChanged(nsec) => self.nsec_input = nsec,
            Message::Login => {
                if let Some(manager) = &self.manager {
                    manager.login_with_nsec(self.nsec_input.trim().to_string());
                }
            }
            Message::CreateAccount => {
                if let Some(manager) = &self.manager {
                    manager.dispatch(AppAction::CreateAccount);
                }
            }
            Message::Logout => {
                if let Some(manager) = &self.manager {
                    manager.logout();
                }
                self.avatar_cache.borrow_mut().clear();
                self.optimistic_selected_chat_id = None;
                self.profile_toast = None;
                self.clear_all_overlays();
            }
            Message::ResetLocalSessionData => {
                if let Some(manager) = &self.manager {
                    manager.clear_local_session_for_recovery();
                    manager.dispatch(AppAction::ClearToast);
                }
                self.optimistic_selected_chat_id = None;
                self.clear_all_overlays();
            }
            Message::ResetRelayConfig => {
                if let Some(manager) = &self.manager {
                    manager.reset_relay_config_to_defaults();
                }
            }
            Message::NewChatChanged(value) => self.new_chat_input = value,
            Message::NewChatSearchChanged(value) => {
                self.new_chat_search = value;
                self.refilter_follows();
            }
            Message::ToggleNewChatForm => {
                let opening = self.overlay != UiOverlay::NewChat;
                self.clear_all_overlays();
                if opening {
                    self.overlay = UiOverlay::NewChat;
                    self.refilter_follows();
                    if let Some(manager) = &self.manager {
                        manager.dispatch(AppAction::RefreshFollowList);
                    }
                }
            }
            Message::StartChat => {
                let peer_npub = self.new_chat_input.trim().to_string();
                if peer_npub.is_empty() {
                    return Task::none();
                }
                if let Some(manager) = &self.manager {
                    manager.dispatch(AppAction::CreateChat { peer_npub });
                }
            }
            Message::StartChatWith(peer_npub) => {
                if let Some(manager) = &self.manager {
                    manager.dispatch(AppAction::CreateChat { peer_npub });
                }
                // Keep form open — it will show a loading state via busy.creating_chat.
                // Form closes when the next core state update reports completion.
            }
            Message::OpenChat(chat_id) => {
                self.optimistic_selected_chat_id = Some(chat_id.clone());
                self.clear_all_overlays();
                if let Some(manager) = &self.manager {
                    manager.dispatch(AppAction::OpenChat { chat_id });
                }
            }
            Message::MessageChanged(value) => {
                if !value.trim().is_empty() {
                    if let Some(chat) = &self.state.current_chat {
                        if let Some(manager) = &self.manager {
                            manager.dispatch(AppAction::TypingStarted {
                                chat_id: chat.chat_id.clone(),
                            });
                        }
                    }
                }
                self.message_input = value;
            }
            Message::SendMessage => {
                let content = self.message_input.trim().to_string();
                if content.is_empty() {
                    return Task::none();
                }
                if let Some(chat) = &self.state.current_chat {
                    if let Some(manager) = &self.manager {
                        manager.dispatch(AppAction::SendMessage {
                            chat_id: chat.chat_id.clone(),
                            content,
                        });
                    }
                    self.message_input.clear();
                }
            }
            Message::ClearToast => {
                if self.profile_toast.is_some() {
                    self.profile_toast = None;
                } else if let Some(manager) = &self.manager {
                    manager.dispatch(AppAction::ClearToast);
                }
            }

            // ── Group creation ────────────────────────────────────────
            Message::ToggleNewGroupForm => {
                let opening = self.overlay != UiOverlay::NewGroup;
                self.clear_all_overlays();
                if opening {
                    self.overlay = UiOverlay::NewGroup;
                    self.refilter_follows();
                    if let Some(manager) = &self.manager {
                        manager.dispatch(AppAction::RefreshFollowList);
                    }
                }
            }
            Message::GroupNameChanged(value) => self.group_name_input = value,
            Message::ToggleGroupMember(npub) => {
                if let Some(pos) = self.selected_group_members.iter().position(|n| n == &npub) {
                    self.selected_group_members.remove(pos);
                } else {
                    self.selected_group_members.push(npub);
                }
            }
            Message::AddManualGroupMember => {
                let npub = self.new_chat_input.trim().to_string();
                if !npub.is_empty() && !self.selected_group_members.contains(&npub) {
                    self.selected_group_members.push(npub);
                    self.new_chat_input.clear();
                }
            }
            Message::CreateGroup => {
                if self.selected_group_members.is_empty() {
                    return Task::none();
                }
                if let Some(manager) = &self.manager {
                    manager.dispatch(AppAction::CreateGroupChat {
                        peer_npubs: self.selected_group_members.clone(),
                        group_name: self.group_name_input.clone(),
                    });
                }
                self.clear_all_overlays();
                self.optimistic_selected_chat_id = None;
            }

            // ── My profile ────────────────────────────────────────────
            Message::ToggleMyProfile => {
                let opening = self.overlay != UiOverlay::MyProfile;
                self.clear_all_overlays();
                if opening {
                    self.overlay = UiOverlay::MyProfile;
                    self.profile_name_draft = self.state.my_profile.name.clone();
                    self.profile_about_draft = self.state.my_profile.about.clone();
                    if let Some(manager) = &self.manager {
                        manager.dispatch(AppAction::RefreshMyProfile);
                    }
                }
            }
            Message::ProfileNameChanged(value) => self.profile_name_draft = value,
            Message::ProfileAboutChanged(value) => self.profile_about_draft = value,
            Message::SaveProfile => {
                if let Some(manager) = &self.manager {
                    manager.dispatch(AppAction::SaveMyProfile {
                        name: self.profile_name_draft.clone(),
                        about: self.profile_about_draft.clone(),
                    });
                }
            }
            Message::CopyNpub => {
                if let AuthState::LoggedIn { ref npub, .. } = self.state.auth {
                    self.profile_toast = Some("Copied npub".to_string());
                    return iced::clipboard::write(npub.clone());
                }
            }
            Message::CopyAppVersion => {
                self.profile_toast = Some("Copied app version".to_string());
                return iced::clipboard::write(self.app_version_display.clone());
            }

            // ── Group info ────────────────────────────────────────────
            Message::ShowGroupInfo => {
                self.clear_all_overlays();
                if let Some(chat) = &self.state.current_chat {
                    self.group_info_name_draft = chat.group_name.clone().unwrap_or_default();
                    if let Some(manager) = &self.manager {
                        manager.dispatch(AppAction::PushScreen {
                            screen: Screen::GroupInfo {
                                chat_id: chat.chat_id.clone(),
                            },
                        });
                    }
                }
            }
            Message::CloseGroupInfo => {
                self.group_info_npub_input.clear();
                if let Some(manager) = &self.manager {
                    let mut stack = self.state.router.screen_stack.clone();
                    if matches!(stack.last(), Some(Screen::GroupInfo { .. })) {
                        stack.pop();
                        manager.dispatch(AppAction::UpdateScreenStack { stack });
                    }
                }
            }
            Message::GroupInfoNameChanged(value) => self.group_info_name_draft = value,
            Message::RenameGroup => {
                if let Some(chat) = &self.state.current_chat {
                    if let Some(manager) = &self.manager {
                        manager.dispatch(AppAction::RenameGroup {
                            chat_id: chat.chat_id.clone(),
                            name: self.group_info_name_draft.clone(),
                        });
                    }
                }
            }
            Message::GroupInfoNpubChanged(value) => self.group_info_npub_input = value,
            Message::AddGroupMember => {
                let npub = self.group_info_npub_input.trim().to_string();
                if npub.is_empty() {
                    return Task::none();
                }
                if let Some(chat) = &self.state.current_chat {
                    if let Some(manager) = &self.manager {
                        manager.dispatch(AppAction::AddGroupMembers {
                            chat_id: chat.chat_id.clone(),
                            peer_npubs: vec![npub],
                        });
                    }
                }
                self.group_info_npub_input.clear();
            }
            Message::RemoveGroupMember(pubkey) => {
                if let Some(chat) = &self.state.current_chat {
                    if let Some(manager) = &self.manager {
                        manager.dispatch(AppAction::RemoveGroupMembers {
                            chat_id: chat.chat_id.clone(),
                            member_pubkeys: vec![pubkey],
                        });
                    }
                }
            }
            Message::LeaveGroup => {
                if let Some(chat) = &self.state.current_chat {
                    if let Some(manager) = &self.manager {
                        manager.dispatch(AppAction::LeaveGroup {
                            chat_id: chat.chat_id.clone(),
                        });
                    }
                }
                self.clear_all_overlays();
            }
        }

        Task::none()
    }

    fn view(&self) -> Element<'_, Message> {
        let route = project_desktop(&self.state);

        // ── Boot error ──────────────────────────────────────────────
        if let Some(error) = &self.boot_error {
            return container(
                column![
                    text("Pika Desktop").size(24).color(theme::TEXT_PRIMARY),
                    text(error).color(theme::DANGER),
                ]
                .spacing(12),
            )
            .center_x(Fill)
            .center_y(Fill)
            .style(theme::surface_style)
            .into();
        }

        // ── Login screen ────────────────────────────────────────────
        if matches!(route.shell_mode, DesktopShellMode::Login) {
            let is_restoring = self
                .manager
                .as_ref()
                .map_or(false, |m| m.is_restoring_session());
            let show_recovery = self.should_offer_recovery_controls();

            return views::login::login_view(
                &self.nsec_input,
                self.state.busy.creating_account,
                is_restoring,
                self.state.toast.as_deref(),
                show_recovery,
                self.should_offer_relay_reset(),
            );
        }

        // ── Main 3-pane layout ──────────────────────────────────────

        // Toast bar (optional)
        let mut main_column = column![];
        if let Some(toast_msg) = self
            .profile_toast
            .as_deref()
            .or(self.state.toast.as_deref())
        {
            let show_relay_reset = self.profile_toast.is_none() && self.should_offer_relay_reset();
            main_column = main_column.push(views::toast::toast_bar(toast_msg, show_relay_reset));
        }

        let cache = &mut *self.avatar_cache.borrow_mut();
        cache.reset_budget();

        // Chat rail (left sidebar)
        let my_profile_pic = self.state.my_profile.picture_url.as_deref();
        let selected_chat_id = effective_selected_chat_id(
            route.selected_chat_id.as_deref(),
            self.optimistic_selected_chat_id.as_deref(),
        );
        let rail = views::chat_rail::chat_rail_view(
            &self.state.chat_list,
            selected_chat_id,
            self.overlay == UiOverlay::NewChat,
            self.overlay == UiOverlay::NewGroup,
            self.overlay == UiOverlay::MyProfile,
            my_profile_pic,
            cache,
        );

        // Center pane routing (mutually exclusive overlays)
        let center_pane: Element<'_, Message> = if self.overlay == UiOverlay::MyProfile {
            let npub = match &self.state.auth {
                AuthState::LoggedIn { npub, .. } => npub.as_str(),
                _ => "",
            };
            views::my_profile::my_profile_view(
                &self.profile_name_draft,
                &self.profile_about_draft,
                npub,
                &self.app_version_display,
                self.state.my_profile.picture_url.as_deref(),
                cache,
            )
        } else if matches!(route.detail_pane, DesktopDetailPane::GroupInfo { .. }) {
            if let Some(chat) = &self.state.current_chat {
                let my_pubkey = match &self.state.auth {
                    AuthState::LoggedIn { pubkey, .. } => pubkey.as_str(),
                    _ => "",
                };
                views::group_info::group_info_view(
                    chat,
                    &self.group_info_name_draft,
                    &self.group_info_npub_input,
                    my_pubkey,
                    cache,
                )
            } else {
                views::empty_state::empty_state_view()
            }
        } else if self.overlay == UiOverlay::NewGroup {
            views::new_group_chat::new_group_chat_view(
                &self.filtered_follows,
                &self.group_name_input,
                &self.new_chat_input,
                &self.selected_group_members,
                self.state.busy.creating_chat,
                self.state.busy.fetching_follow_list,
                &self.new_chat_search,
                cache,
            )
        } else if self.overlay == UiOverlay::NewChat {
            views::new_chat::new_chat_view(
                &self.filtered_follows,
                &self.new_chat_input,
                self.state.busy.creating_chat,
                self.state.busy.fetching_follow_list,
                &self.new_chat_search,
                cache,
            )
        } else if route.selected_chat_id.is_some() {
            if let Some(chat) = &self.state.current_chat {
                views::conversation::conversation_view(chat, &self.message_input, cache)
            } else {
                views::empty_state::empty_state_view()
            }
        } else if matches!(route.detail_pane, DesktopDetailPane::PeerProfile { .. }) {
            views::empty_state::empty_state_view()
        } else {
            views::empty_state::empty_state_view()
        };

        let content = row![rail, rule::vertical(1), center_pane].height(Fill);

        main_column = main_column.push(content);

        container(main_column)
            .width(Fill)
            .height(Fill)
            .style(theme::surface_style)
            .into()
    }

    fn from_boot_state(
        cached_profiles: Vec<pika_core::FollowListEntry>,
        manager: Option<AppManager>,
        boot_error: Option<String>,
        state: AppState,
    ) -> Self {
        Self {
            manager,
            boot_error,
            state,
            avatar_cache: std::cell::RefCell::new(views::avatar::AvatarCache::new()),
            cached_profiles,
            nsec_input: String::new(),
            new_chat_input: String::new(),
            new_chat_search: String::new(),
            filtered_follows: Vec::new(),
            message_input: String::new(),
            optimistic_selected_chat_id: None,
            overlay: UiOverlay::None,
            group_name_input: String::new(),
            selected_group_members: Vec::new(),
            profile_name_draft: String::new(),
            profile_about_draft: String::new(),
            app_version_display: app_version_display(),
            profile_toast: None,
            group_info_name_draft: String::new(),
            group_info_npub_input: String::new(),
        }
    }

    fn sync_from_manager(&mut self) {
        let Some(manager) = &self.manager else {
            return;
        };

        let latest = manager.state();
        if latest.rev != self.state.rev {
            // On login transition, dispatch Foregrounded to load profiles.
            let was_logged_out = matches!(self.state.auth, AuthState::LoggedOut);
            let now_logged_in = matches!(latest.auth, AuthState::LoggedIn { .. });
            if was_logged_out && now_logged_in {
                manager.dispatch(AppAction::Foregrounded);
            }

            // Close new-chat form once creating_chat finishes.
            if self.state.busy.creating_chat
                && !latest.busy.creating_chat
                && self.overlay == UiOverlay::NewChat
            {
                self.overlay = UiOverlay::None;
                self.new_chat_input.clear();
            }

            // Close new-group form once creating_chat finishes.
            if self.state.busy.creating_chat
                && !latest.busy.creating_chat
                && self.overlay == UiOverlay::NewGroup
            {
                self.clear_all_overlays();
            }

            // Sync my_profile drafts when profile state updates.
            if self.overlay == UiOverlay::MyProfile
                && self.state.my_profile.name != latest.my_profile.name
            {
                self.profile_name_draft = latest.my_profile.name.clone();
                self.profile_about_draft = latest.my_profile.about.clone();
            }

            let latest_route = project_desktop(&latest);
            if let Some(optimistic_chat_id) = self.optimistic_selected_chat_id.as_deref() {
                let authoritative_selection = latest_route.selected_chat_id.as_deref();
                if authoritative_selection == Some(optimistic_chat_id)
                    || authoritative_selection.is_some()
                    || matches!(latest_route.shell_mode, DesktopShellMode::Login)
                {
                    self.optimistic_selected_chat_id = None;
                }
            }

            self.state = latest;
            if matches!(self.overlay, UiOverlay::NewChat | UiOverlay::NewGroup) {
                self.refilter_follows();
            }
        }

        self.retry_follow_list_if_needed();
    }

    fn retry_follow_list_if_needed(&self) {
        let needs_follows = matches!(self.overlay, UiOverlay::NewChat | UiOverlay::NewGroup);
        if needs_follows
            && self.state.follow_list.is_empty()
            && !self.state.busy.fetching_follow_list
        {
            if let Some(manager) = &self.manager {
                manager.dispatch(AppAction::RefreshFollowList);
            }
        }
    }

    fn clear_all_overlays(&mut self) {
        self.overlay = UiOverlay::None;
        self.new_chat_input.clear();
        self.new_chat_search.clear();
        self.group_name_input.clear();
        self.selected_group_members.clear();
        self.group_info_npub_input.clear();
    }

    fn should_offer_recovery_controls(&self) -> bool {
        let from_toast = self
            .state
            .toast
            .as_deref()
            .map(|toast| {
                toast.contains("Login failed")
                    || toast.contains("open encrypted mdk sqlite db")
                    || toast.contains("keyring")
            })
            .unwrap_or(false);
        let restoring = self
            .manager
            .as_ref()
            .map(|m| m.is_restoring_session())
            .unwrap_or(false);
        from_toast || restoring
    }

    fn should_offer_relay_reset(&self) -> bool {
        self.state
            .toast
            .as_deref()
            .map(|toast| {
                toast.contains("relay")
                    || toast.contains("no relays")
                    || toast.contains("not connected")
            })
            .unwrap_or(false)
    }

    fn refilter_follows(&mut self) {
        // Use the relay follow list if available, otherwise fall back to
        // all cached profiles from the on-disk database.
        let source = if self.state.follow_list.is_empty() {
            &self.cached_profiles
        } else {
            &self.state.follow_list
        };

        if self.new_chat_search.is_empty() {
            self.filtered_follows = source.clone();
        } else {
            let q = self.new_chat_search.to_lowercase();
            self.filtered_follows = source
                .iter()
                .filter(|e| {
                    e.name.as_deref().unwrap_or("").to_lowercase().contains(&q)
                        || e.npub.to_lowercase().contains(&q)
                })
                .cloned()
                .collect();
        }
    }
}

fn effective_selected_chat_id<'a>(
    route_selected_chat_id: Option<&'a str>,
    optimistic_selected_chat_id: Option<&'a str>,
) -> Option<&'a str> {
    optimistic_selected_chat_id.or(route_selected_chat_id)
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

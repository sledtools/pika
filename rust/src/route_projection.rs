use crate::{AppState, AuthState, CallStatus, Router, Screen};

#[derive(Clone, Debug, PartialEq)]
pub struct MobileRouteState {
    pub root_screen: Screen,
    pub stack: Vec<Screen>,
    pub active_screen: Screen,
    pub can_pop: bool,
}

/// Maps core router semantics to the navigation model shared by iOS and Android.
pub fn project_mobile(state: &AppState) -> MobileRouteState {
    if matches!(state.router.default_screen, Screen::Login) {
        return MobileRouteState {
            root_screen: Screen::Login,
            stack: vec![],
            active_screen: Screen::Login,
            can_pop: false,
        };
    }

    let stack = state.router.screen_stack.clone();
    let active_screen = stack
        .last()
        .cloned()
        .unwrap_or_else(|| state.router.default_screen.clone());
    MobileRouteState {
        root_screen: state.router.default_screen.clone(),
        can_pop: !stack.is_empty(),
        stack,
        active_screen,
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum DesktopShellMode {
    Login,
    Main,
}

#[derive(Clone, Debug, PartialEq)]
pub enum DesktopDetailPane {
    None,
    GroupInfo { chat_id: String },
    PeerProfile { pubkey: String },
}

#[derive(Clone, Debug, PartialEq)]
pub enum DesktopModal {
    None,
    ActiveCall { chat_id: String, call_id: String },
}

#[derive(Clone, Debug, PartialEq)]
pub struct DesktopRouteState {
    pub shell_mode: DesktopShellMode,
    pub active_screen: Screen,
    pub selected_chat_id: Option<String>,
    pub detail_pane: DesktopDetailPane,
    pub modal: DesktopModal,
    pub can_pop: bool,
}

/// Projects core state into desktop shell concerns (mode, selection, detail pane, modal).
pub fn project_desktop(state: &AppState) -> DesktopRouteState {
    let force_login_shell = matches!(state.router.default_screen, Screen::Login)
        || matches!(state.auth, AuthState::LoggedOut);
    if force_login_shell {
        return DesktopRouteState {
            shell_mode: DesktopShellMode::Login,
            active_screen: Screen::Login,
            selected_chat_id: None,
            detail_pane: DesktopDetailPane::None,
            modal: DesktopModal::None,
            can_pop: false,
        };
    }

    let active_screen = active_screen(&state.router);
    let selected_chat_id = match &active_screen {
        Screen::Chat { chat_id } | Screen::GroupInfo { chat_id } => Some(chat_id.clone()),
        _ => state.current_chat.as_ref().map(|chat| chat.chat_id.clone()),
    };

    let detail_pane = if let Some(profile) = &state.peer_profile {
        DesktopDetailPane::PeerProfile {
            pubkey: profile.pubkey.clone(),
        }
    } else if let Screen::GroupInfo { chat_id } = &active_screen {
        DesktopDetailPane::GroupInfo {
            chat_id: chat_id.clone(),
        }
    } else {
        DesktopDetailPane::None
    };

    let modal = if let Some(call) = &state.active_call {
        if is_live_call_status(&call.status) {
            DesktopModal::ActiveCall {
                chat_id: call.chat_id.clone(),
                call_id: call.call_id.clone(),
            }
        } else {
            DesktopModal::None
        }
    } else {
        DesktopModal::None
    };

    DesktopRouteState {
        shell_mode: DesktopShellMode::Main,
        active_screen,
        selected_chat_id,
        detail_pane,
        modal,
        can_pop: !state.router.screen_stack.is_empty(),
    }
}

fn active_screen(router: &Router) -> Screen {
    router
        .screen_stack
        .last()
        .cloned()
        .unwrap_or_else(|| router.default_screen.clone())
}

fn is_live_call_status(status: &CallStatus) -> bool {
    matches!(
        status,
        CallStatus::Offering | CallStatus::Ringing | CallStatus::Connecting | CallStatus::Active
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{AuthState, CallState, ChatViewState};

    fn state_with_router(default_screen: Screen, stack: Vec<Screen>) -> AppState {
        let mut state = AppState::empty();
        state.router.default_screen = default_screen;
        state.router.screen_stack = stack;
        state
    }

    fn logged_in(mut state: AppState) -> AppState {
        state.auth = AuthState::LoggedIn {
            npub: "npub1test".into(),
            pubkey: "pubkey".into(),
        };
        state
    }

    #[test]
    fn mobile_login_projection_ignores_stack() {
        let state = state_with_router(
            Screen::Login,
            vec![Screen::Chat {
                chat_id: "c1".into(),
            }],
        );

        let route = project_mobile(&state);
        assert_eq!(route.root_screen, Screen::Login);
        assert_eq!(route.active_screen, Screen::Login);
        assert!(route.stack.is_empty());
        assert!(!route.can_pop);
    }

    #[test]
    fn mobile_projection_uses_stack_top_as_active() {
        let state = state_with_router(
            Screen::ChatList,
            vec![
                Screen::NewChat,
                Screen::Chat {
                    chat_id: "c2".into(),
                },
            ],
        );

        let route = project_mobile(&state);
        assert_eq!(route.root_screen, Screen::ChatList);
        assert_eq!(
            route.active_screen,
            Screen::Chat {
                chat_id: "c2".into()
            }
        );
        assert!(route.can_pop);
        assert_eq!(route.stack.len(), 2);
    }

    #[test]
    fn desktop_projection_uses_login_shell_when_logged_out() {
        let state = state_with_router(Screen::ChatList, vec![]);
        let route = project_desktop(&state);
        assert_eq!(route.shell_mode, DesktopShellMode::Login);
        assert_eq!(route.active_screen, Screen::Login);
        assert_eq!(route.selected_chat_id, None);
        assert_eq!(route.detail_pane, DesktopDetailPane::None);
    }

    #[test]
    fn desktop_group_info_projection_selects_chat_and_detail_panel() {
        let state = logged_in(state_with_router(
            Screen::ChatList,
            vec![Screen::GroupInfo {
                chat_id: "g1".into(),
            }],
        ));
        let route = project_desktop(&state);
        assert_eq!(route.shell_mode, DesktopShellMode::Main);
        assert_eq!(
            route.active_screen,
            Screen::GroupInfo {
                chat_id: "g1".into()
            }
        );
        assert_eq!(route.selected_chat_id, Some("g1".into()));
        assert_eq!(
            route.detail_pane,
            DesktopDetailPane::GroupInfo {
                chat_id: "g1".into()
            }
        );
        assert!(route.can_pop);
    }

    #[test]
    fn desktop_projection_prefers_peer_profile_detail_panel() {
        let mut state = logged_in(state_with_router(
            Screen::ChatList,
            vec![Screen::GroupInfo {
                chat_id: "g2".into(),
            }],
        ));
        state.peer_profile = Some(crate::PeerProfileState {
            pubkey: "peer_pk".into(),
            npub: "npub1peer".into(),
            name: None,
            about: None,
            picture_url: None,
            is_followed: false,
        });
        let route = project_desktop(&state);
        assert_eq!(
            route.detail_pane,
            DesktopDetailPane::PeerProfile {
                pubkey: "peer_pk".into()
            }
        );
    }

    #[test]
    fn desktop_projection_uses_current_chat_when_stack_empty() {
        let mut state = logged_in(state_with_router(Screen::ChatList, vec![]));
        state.current_chat = Some(ChatViewState {
            chat_id: "c9".into(),
            is_group: false,
            group_name: None,
            members: vec![],
            is_admin: false,
            messages: vec![],
            can_load_older: false,
            typing_members: vec![],
        });
        let route = project_desktop(&state);
        assert_eq!(route.selected_chat_id, Some("c9".into()));
        assert_eq!(route.detail_pane, DesktopDetailPane::None);
    }

    #[test]
    fn desktop_projection_emits_active_call_modal_for_live_call() {
        let mut state = logged_in(state_with_router(Screen::ChatList, vec![]));
        state.active_call = Some(CallState {
            call_id: "call1".into(),
            chat_id: "chat1".into(),
            peer_npub: "npub1peer".into(),
            status: CallStatus::Active,
            started_at: None,
            is_muted: false,
            debug: None,
        });
        let route = project_desktop(&state);
        assert_eq!(
            route.modal,
            DesktopModal::ActiveCall {
                chat_id: "chat1".into(),
                call_id: "call1".into()
            }
        );
    }
}

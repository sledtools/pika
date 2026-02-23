mod app_manager;
mod design;
mod icons;
mod screen;
mod theme;
mod video;
mod video_shader;
mod views;

use app_manager::AppManager;
use iced::widget::{column, container, text};
use iced::{Element, Fill, Font, Size, Subscription, Task, Theme};
use pika_core::{project_desktop, AppAction, AppState, AuthState, CallStatus, DesktopShellMode};
use std::time::Duration;

pub fn app_version_display() -> String {
    let version = env!("CARGO_PKG_VERSION");
    if let Some(build) = option_env!("PIKA_BUILD_NUMBER") {
        format!("v{version} ({build})")
    } else {
        format!("v{version}")
    }
}

pub fn main() -> iced::Result {
    let window_settings = iced::window::Settings {
        size: Size::new(1024.0, 720.0),
        #[cfg(target_os = "macos")]
        platform_specific: iced::window::settings::PlatformSpecific {
            title_hidden: true,
            titlebar_transparent: true,
            fullsize_content_view: true,
        },
        ..Default::default()
    };

    iced::application(DesktopApp::new, DesktopApp::update, DesktopApp::view)
        .title("Pika Desktop")
        .subscription(DesktopApp::subscription)
        .theme(dark_theme)
        .window(window_settings)
        .default_font(Font::with_name("Geist"))
        .font(include_bytes!("../fonts/Geist-Regular.ttf").as_slice())
        .font(include_bytes!("../fonts/Geist-Medium.ttf").as_slice())
        .font(include_bytes!("../fonts/Geist-Bold.ttf").as_slice())
        .font(include_bytes!("../fonts/GeistMono-Regular.ttf").as_slice())
        .font(include_bytes!("../fonts/NotoColorEmoji.ttf").as_slice())
        .font(include_bytes!("../fonts/lucide.ttf").as_slice())
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

enum Screen {
    Home(Box<screen::home::State>),
    Login(screen::login::State),
}

#[derive(Debug, Clone)]
pub enum Message {
    CoreUpdated,
    Home(screen::home::Message),
    Login(screen::login::Message),
    RelativeTimeTick,
}

enum DesktopApp {
    BootError {
        error: String,
    },
    Loaded {
        app_version_display: String,
        avatar_cache: std::cell::RefCell<views::avatar::AvatarCache>,
        cached_profiles: Vec<pika_core::FollowListEntry>,
        manager: AppManager,
        screen: Screen,
        state: AppState,
    },
}

impl DesktopApp {
    fn new() -> (Self, Task<Message>) {
        let data_dir = app_manager::resolve_data_dir()
            .unwrap_or_else(|_| std::path::PathBuf::from(".pika"))
            .to_string_lossy()
            .to_string();
        let cached_profiles = pika_core::load_cached_profiles(&data_dir);

        let app = match AppManager::new() {
            Ok(manager) => {
                let state = manager.state();
                let route = project_desktop(&state);
                let screen = if matches!(route.shell_mode, DesktopShellMode::Login) {
                    Screen::Login(screen::login::State::new())
                } else {
                    Screen::Home(Box::new(screen::home::State::new(&state)))
                };

                Self::Loaded {
                    app_version_display: app_version_display(),
                    avatar_cache: std::cell::RefCell::new(views::avatar::AvatarCache::new()),
                    cached_profiles,
                    manager,
                    screen,
                    state,
                }
            }
            Err(error) => Self::BootError {
                error: format!("failed to start desktop manager: {error}"),
            },
        };

        (app, Task::none())
    }

    fn subscription(&self) -> Subscription<Message> {
        match self {
            DesktopApp::BootError { .. } => Subscription::none(),
            DesktopApp::Loaded {
                manager,
                screen,
                state,
                ..
            } => {
                let core_updates = Subscription::run_with(manager.clone(), manager_update_stream)
                    .map(|_| Message::CoreUpdated);
                let relative_time_ticks =
                    iced::time::every(Duration::from_secs(30)).map(|_| Message::RelativeTimeTick);

                let mut subs = vec![core_updates, relative_time_ticks];

                if let Screen::Home(ref home) = screen {
                    if home.show_call_screen
                        && state
                            .active_call
                            .as_ref()
                            .is_some_and(|c| matches!(c.status, CallStatus::Active))
                    {
                        subs.push(
                            iced::time::every(Duration::from_secs(1))
                                .map(|_| Message::Home(screen::home::Message::CallTimerTick)),
                        );
                    }

                    // Poll for new video frames at ~30fps during video calls.
                    let is_video_call = state.active_call.as_ref().is_some_and(|c| c.is_video_call);
                    let is_active_call = state
                        .active_call
                        .as_ref()
                        .is_some_and(|c| matches!(c.status, CallStatus::Active));
                    if is_video_call && is_active_call {
                        subs.push(
                            iced::time::every(Duration::from_millis(33))
                                .map(|_| Message::Home(screen::home::Message::VideoFrameTick)),
                        );
                    }
                }

                Subscription::batch(subs)
            }
        }
    }

    fn update(&mut self, message: Message) -> Task<Message> {
        match self {
            DesktopApp::BootError { .. } => {}
            DesktopApp::Loaded {
                avatar_cache,
                cached_profiles,
                manager,
                screen,
                state,
                ..
            } => match message {
                Message::CoreUpdated => {
                    self.sync_from_manager();
                }
                Message::Home(message) => {
                    if let Screen::Home(ref mut home_state) = screen {
                        if let Some(event) =
                            home_state.update(message, state, manager, &cached_profiles)
                        {
                            match event {
                                screen::home::Event::AppAction(action) => {
                                    manager.dispatch(action);
                                }
                                screen::home::Event::Logout => {
                                    manager.logout();
                                    avatar_cache.borrow_mut().clear();
                                    *screen = Screen::Login(screen::login::State::new());
                                }
                                screen::home::Event::Task(task) => {
                                    return task.map(Message::Home);
                                }
                            }
                        }
                    }
                }
                Message::Login(message) => {
                    if let Screen::Login(ref mut login_state) = screen {
                        if let Some(event) = login_state.update(message) {
                            match event {
                                screen::login::Event::AppAction(action) => {
                                    manager.dispatch(action);
                                }
                                screen::login::Event::Login { nsec } => {
                                    manager.login_with_nsec(nsec);
                                }
                                screen::login::Event::ResetLocalSessionData => {
                                    manager.clear_local_session_for_recovery();
                                    manager.dispatch(AppAction::ClearToast);
                                }
                                screen::login::Event::ResetRelayConfig => {
                                    manager.reset_relay_config_to_defaults();
                                }
                            }
                        }
                    }
                }
                Message::RelativeTimeTick => {
                    self.retry_follow_list_if_needed();
                }
            },
        }

        Task::none()
    }

    fn view(&self) -> Element<'_, Message> {
        match self {
            DesktopApp::BootError { error } => container(
                column![
                    text("Pika Desktop").size(24).color(theme::TEXT_PRIMARY),
                    text(error).color(theme::DANGER),
                ]
                .spacing(12),
            )
            .center_x(Fill)
            .center_y(Fill)
            .style(theme::surface_style)
            .into(),
            DesktopApp::Loaded {
                app_version_display,
                avatar_cache,
                manager,
                screen,
                state,
                ..
            } => match screen {
                Screen::Home(ref home) => home
                    .view(&state, &avatar_cache, &app_version_display)
                    .map(Message::Home),
                Screen::Login(ref login) => login.view(&state, &manager).map(Message::Login),
            },
        }
    }

    // ── Core state synchronisation ──────────────────────────────────────────

    fn sync_from_manager(&mut self) {
        match self {
            DesktopApp::BootError { .. } => {}
            DesktopApp::Loaded {
                avatar_cache,
                cached_profiles,
                manager,
                screen,
                state,
                ..
            } => {
                let latest = manager.state();
                if latest.rev == state.rev {
                    self.retry_follow_list_if_needed();
                    return;
                }

                // Detect auth transitions for screen changes.
                let was_logged_out = matches!(state.auth, AuthState::LoggedOut);
                let now_logged_in = matches!(latest.auth, AuthState::LoggedIn { .. });
                let was_logged_in = matches!(state.auth, AuthState::LoggedIn { .. });
                let now_logged_out = matches!(latest.auth, AuthState::LoggedOut);

                if was_logged_out && now_logged_in {
                    // Login succeeded → transition to Main screen.
                    manager.dispatch(AppAction::Foregrounded);
                    if !matches!(screen, Screen::Home(_)) {
                        *screen = Screen::Home(Box::new(screen::home::State::new(&latest)));
                    }
                } else if was_logged_in && now_logged_out {
                    // Logged out externally (e.g. session expired) → show Login.
                    avatar_cache.borrow_mut().clear();
                    if !matches!(screen, Screen::Login(_)) {
                        *screen = Screen::Login(screen::login::State::new());
                    }
                }

                // Delegate screen-specific sync.
                if let Screen::Home(ref mut home) = screen {
                    home.sync_from_update(&state, &latest, manager, &cached_profiles);
                }

                *state = latest;
                self.retry_follow_list_if_needed();
            }
        }
    }

    fn retry_follow_list_if_needed(&self) {
        match self {
            DesktopApp::BootError { .. } => {}
            DesktopApp::Loaded {
                manager,
                screen,
                state,
                ..
            } => {
                let needs_follows = if let Screen::Home(ref home) = screen {
                    home.needs_follow_list()
                } else {
                    false
                };
                if needs_follows && state.follow_list.is_empty() && !state.busy.fetching_follow_list
                {
                    manager.dispatch(AppAction::RefreshFollowList);
                }
            }
        }
    }
}

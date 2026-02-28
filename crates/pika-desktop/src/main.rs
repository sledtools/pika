mod app_manager;
mod design;
mod icons;
mod screen;
mod theme;
mod utils;
mod video;
mod video_shader;
mod views;

use app_manager::AppManager;
use design::ALL_THEMES;
use iced::widget::{column, container, text};
use iced::{Element, Fill, Font, Size, Subscription, Task, Theme};
use pika_core::{project_desktop, AppAction, AppState, AuthState, CallStatus, DesktopShellMode};
use std::path::PathBuf;
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
        .theme(active_theme)
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

// ── Theme persistence ───────────────────────────────────────────────────────

/// Path to the file that stores the user's selected theme index.
fn theme_persistence_path() -> Option<PathBuf> {
    let dir = app_manager::resolve_data_dir().ok()?;
    Some(dir.join("desktop_theme.txt"))
}

/// Load the persisted theme index from disk, falling back to `0` (Dark).
fn load_persisted_theme_index() -> usize {
    let path = match theme_persistence_path() {
        Some(p) => p,
        None => return 0,
    };
    match std::fs::read_to_string(&path) {
        Ok(contents) => contents
            .trim()
            .parse::<usize>()
            .unwrap_or(0)
            .min(ALL_THEMES.len().saturating_sub(1)),
        Err(_) => 0,
    }
}

/// Save the active theme index to disk (best-effort, errors are ignored).
fn save_persisted_theme_index(index: usize) {
    if let Some(path) = theme_persistence_path() {
        let _ = std::fs::write(path, index.to_string());
    }
}

fn active_theme(state: &DesktopApp) -> Theme {
    match state {
        DesktopApp::BootError { .. } => Theme::Dark,
        DesktopApp::Loaded {
            active_theme_index,
            screen,
            ..
        } => {
            // If the theme picker is open and previewing, use the preview index;
            // otherwise use the committed active index.
            let effective_index = if let Screen::Home(ref home) = screen {
                home.preview_theme_index.unwrap_or(*active_theme_index)
            } else {
                *active_theme_index
            };

            let entry = ALL_THEMES.get(effective_index).unwrap_or(&ALL_THEMES[0]);

            iced::Theme::from(entry)
        }
    }
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
    WindowEvent(iced::Event),
}

#[allow(clippy::large_enum_variant)]
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
        /// Index into `design::ALL_THEMES` for the currently active theme.
        active_theme_index: usize,
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
                    active_theme_index: {
                        let idx = load_persisted_theme_index();
                        design::set_active(idx);
                        idx
                    },
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

                // Listen for window file-drop events (drag-and-drop) and keyboard events.
                subs.push(iced::event::listen().map(Message::WindowEvent));

                Subscription::batch(subs)
            }
        }
    }

    fn update(&mut self, message: Message) -> Task<Message> {
        // ── Keyboard events (handled before destructuring to avoid borrow conflicts)
        if let Message::WindowEvent(iced::Event::Keyboard(ref kb_event)) = message {
            if let Some(task) = self.handle_keyboard_event(kb_event) {
                return task;
            }
        }

        match self {
            DesktopApp::BootError { .. } => {}
            DesktopApp::Loaded {
                active_theme_index,
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
                            home_state.update(message, state, manager, cached_profiles)
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
                                screen::home::Event::ThemeChanged { index } => {
                                    *active_theme_index = index;
                                    design::set_active(index);
                                    save_persisted_theme_index(index);
                                }
                                screen::home::Event::ThemePreview { index } => {
                                    // Update the global active theme so style
                                    // functions immediately reflect the preview.
                                    let effective = index.unwrap_or(*active_theme_index);
                                    design::set_active(effective);
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
                Message::WindowEvent(event) => {
                    // ── Window events (file drops, etc.) ────────────
                    // (Keyboard events are handled at the top of update()
                    //  before the destructuring match to avoid borrow conflicts.)
                    if let iced::Event::Window(window_event) = event {
                        if let Screen::Home(ref mut home_state) = screen {
                            match window_event {
                                iced::window::Event::FileDropped(path) => {
                                    if let Some(event) = home_state.update(
                                        screen::home::Message::Conversation(
                                            views::conversation::Message::FilesDropped(vec![path]),
                                        ),
                                        state,
                                        manager,
                                        cached_profiles,
                                    ) {
                                        match event {
                                            screen::home::Event::Task(task) => {
                                                return task.map(Message::Home);
                                            }
                                            screen::home::Event::AppAction(action) => {
                                                manager.dispatch(action);
                                            }
                                            _ => {}
                                        }
                                    }
                                }
                                iced::window::Event::FileHovered(_) => {
                                    let _ = home_state.update(
                                        screen::home::Message::Conversation(
                                            views::conversation::Message::FileHovered,
                                        ),
                                        state,
                                        manager,
                                        cached_profiles,
                                    );
                                }
                                iced::window::Event::FilesHoveredLeft => {
                                    let _ = home_state.update(
                                        screen::home::Message::Conversation(
                                            views::conversation::Message::FileHoverLeft,
                                        ),
                                        state,
                                        manager,
                                        cached_profiles,
                                    );
                                }
                                _ => {}
                            }
                        }
                    }
                }
            },
        }

        Task::none()
    }

    fn view(&self) -> Element<'_, Message> {
        match self {
            DesktopApp::BootError { error } => container(
                column![
                    text("Pika Desktop").size(24).color(theme::text_primary()),
                    text(error).color(theme::danger()),
                ]
                .spacing(12),
            )
            .center_x(Fill)
            .center_y(Fill)
            .style(theme::surface_style)
            .into(),
            DesktopApp::Loaded {
                active_theme_index,
                app_version_display,
                avatar_cache,
                manager,
                screen,
                state,
                ..
            } => match screen {
                Screen::Home(ref home) => home
                    .view(
                        state,
                        avatar_cache,
                        app_version_display,
                        *active_theme_index,
                    )
                    .map(Message::Home),
                Screen::Login(ref login) => login.view(state, manager).map(Message::Login),
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
                    home.sync_from_update(state, &latest, manager, cached_profiles);
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

    // ── Keyboard event handling ─────────────────────────────────────────────

    fn handle_keyboard_event(&mut self, event: &iced::keyboard::Event) -> Option<Task<Message>> {
        // We only care about key-press events.
        let (key, modifiers) = match event {
            iced::keyboard::Event::KeyPressed { key, modifiers, .. } => (key, modifiers),
            _ => return None,
        };

        // Only process when on the Home screen.
        let DesktopApp::Loaded {
            screen: Screen::Home(ref mut home_state),
            state,
            manager,
            cached_profiles,
            active_theme_index,
            ..
        } = self
        else {
            return None;
        };

        let is_cmd = modifiers.command();

        // ── Overlay-specific key routing ────────────────────────────
        // When command palette is open, route Esc / ArrowUp / ArrowDown / Enter.
        if home_state.has_command_palette() {
            let msg = match key {
                iced::keyboard::Key::Named(iced::keyboard::key::Named::Escape) => Some(
                    screen::home::Message::CommandPalette(views::command_palette::Message::Dismiss),
                ),
                iced::keyboard::Key::Named(iced::keyboard::key::Named::ArrowUp) => Some(
                    screen::home::Message::CommandPalette(views::command_palette::Message::ArrowUp),
                ),
                iced::keyboard::Key::Named(iced::keyboard::key::Named::ArrowDown) => {
                    Some(screen::home::Message::CommandPalette(
                        views::command_palette::Message::ArrowDown,
                    ))
                }
                iced::keyboard::Key::Named(iced::keyboard::key::Named::Enter) => Some(
                    screen::home::Message::CommandPalette(views::command_palette::Message::Confirm),
                ),
                _ => None,
            };

            if let Some(msg) = msg {
                if let Some(event) = home_state.update(msg, state, manager, cached_profiles) {
                    return Some(Self::handle_home_event(event, manager, active_theme_index));
                }
                return Some(Task::none());
            }
            // Let other keys (typing) fall through to the text input naturally.
            return None;
        }

        // When theme picker is open, route Esc / ArrowUp / ArrowDown / Enter.
        if home_state.has_theme_picker() {
            let msg = match key {
                iced::keyboard::Key::Named(iced::keyboard::key::Named::Escape) => Some(
                    screen::home::Message::ThemePicker(views::theme_picker::Message::Dismiss),
                ),
                iced::keyboard::Key::Named(iced::keyboard::key::Named::ArrowUp) => Some(
                    screen::home::Message::ThemePicker(views::theme_picker::Message::ArrowUp),
                ),
                iced::keyboard::Key::Named(iced::keyboard::key::Named::ArrowDown) => Some(
                    screen::home::Message::ThemePicker(views::theme_picker::Message::ArrowDown),
                ),
                iced::keyboard::Key::Named(iced::keyboard::key::Named::Enter) => Some(
                    screen::home::Message::ThemePicker(views::theme_picker::Message::Confirm),
                ),
                _ => None,
            };

            if let Some(msg) = msg {
                if let Some(event) = home_state.update(msg, state, manager, cached_profiles) {
                    return Some(Self::handle_home_event(event, manager, active_theme_index));
                }
                return Some(Task::none());
            }
            return None;
        }

        // ── Global shortcuts (no overlay open) ──────────────────────
        if is_cmd {
            match key {
                // Cmd+K → open command palette
                iced::keyboard::Key::Character(c) if c.as_str() == "k" => {
                    let msg = screen::home::Message::OpenCommandPalette;
                    if let Some(event) = home_state.update(msg, state, manager, cached_profiles) {
                        return Some(Self::handle_home_event(event, manager, active_theme_index));
                    }
                    return Some(Task::none());
                }
                // Cmd+T → open theme picker
                iced::keyboard::Key::Character(c) if c.as_str() == "t" => {
                    // Store the current active theme index on the home state
                    // so the picker knows what "original" means.
                    home_state.preview_theme_index = Some(*active_theme_index);
                    let msg = screen::home::Message::OpenThemePicker;
                    if let Some(event) = home_state.update(msg, state, manager, cached_profiles) {
                        return Some(Self::handle_home_event(event, manager, active_theme_index));
                    }
                    return Some(Task::none());
                }
                _ => {}
            }
        }

        None
    }

    /// Convert a home screen event into an iced Task, mutating top-level state
    /// as needed (theme index, logout, etc.).
    fn handle_home_event(
        event: screen::home::Event,
        manager: &AppManager,
        active_theme_index: &mut usize,
    ) -> Task<Message> {
        match event {
            screen::home::Event::AppAction(action) => {
                manager.dispatch(action);
                Task::none()
            }
            screen::home::Event::Logout => {
                // Logout is handled in the normal update path; shouldn't
                // reach here from keyboard shortcuts, but handle gracefully.
                Task::none()
            }
            screen::home::Event::Task(task) => task.map(Message::Home),
            screen::home::Event::ThemeChanged { index } => {
                *active_theme_index = index;
                design::set_active(index);
                save_persisted_theme_index(index);
                Task::none()
            }
            screen::home::Event::ThemePreview { index } => {
                // Update the global active theme so style functions
                // immediately reflect the preview.
                let effective = index.unwrap_or(*active_theme_index);
                design::set_active(effective);
                Task::none()
            }
        }
    }
}

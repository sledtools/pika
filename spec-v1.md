# Marmot App: MVP Spec

*Final spec produced by merging pi-spec.md and codex-spec.md via negotiation. All decisions are agreed by both agents.*

## What We're Building

A simple messaging app for iOS and Android. First version supports:

- Nostr account creation and login (generate or import a keypair)
- 2-person text chat (create a chat with someone by their npub)
- Send and receive text messages

That's it. No group chat, no media, no push notifications, no profiles.

## Architecture Overview

Rust owns all application state and business logic. SwiftUI (iOS) and Jetpack Compose (Android) are pure renderers — they display whatever Rust tells them to display, and forward user actions back to Rust. The native layer does zero interpretation, filtering, or assembly of state.

```
┌─────────────────────────────────────────────┐
│  SwiftUI / Jetpack Compose                  │
│  Pure render of AppState                    │
│  Forwards user actions to Rust              │
└──────────────────┬──────────────────────────┘
                   │
          Actions ↓ │ ↑ AppUpdate (full state slices, versioned)
                   │
┌──────────────────▼──────────────────────────┐
│  Thin Native Manager                        │
│  @Observable / mutableStateOf               │
│  Implements AppReconciler callback          │
│  Hops updates to main thread                │
│  Checks rev continuity, resyncs on gap      │
└──────────────────┬──────────────────────────┘
                   │
                   │ UniFFI (generated FFI bindings)
                   │
┌──────────────────▼──────────────────────────┐
│  Rust Core                                  │
│                                             │
│  FfiApp (UniFFI object)                     │
│    ├── state() → AppState snapshot (w/ rev) │
│    ├── dispatch(Action)                     │
│    └── listen_for_updates(AppReconciler)    │
│                                             │
│  Internals:                                 │
│    ├── AppCore (owns all mutable state)     │
│    ├── MDK (MLS/Nostr protocol)             │
│    ├── Nostr relay subscriptions            │
│    └── SQLite persistence                   │
└─────────────────────────────────────────────┘
```

## Data Flow

All data flows in one direction. There are no exceptions.

1. User taps something in the UI
2. Native code calls `rust.dispatch(action)` — fire and forget
3. Rust mutates its internal state, increments `rev`
4. Rust sends an `AppUpdate` (with `rev`) through a flume channel
5. Listener thread receives it and calls `reconciler.reconcile(update)` across FFI
6. Native manager checks `rev` continuity, applies the update to observable state on the main thread (or resyncs via `state()` on gap)
7. SwiftUI / Compose re-renders

The native side never pulls state on its own initiative after initialization, except on `rev` gap detection or foreground resume.

## State Model

Rust maintains a single `AppState` struct that represents exactly what the screen should show. Nothing more. If something isn't visible on screen, it's not in this struct (it may exist in Rust's internal state, but the UI doesn't see it).

### AppState

```rust
#[derive(uniffi::Record, Clone, Debug)]
pub struct AppState {
    pub rev: u64,
    pub router: Router,
    pub auth: AuthState,
    pub chat_list: Vec<ChatSummary>,
    pub current_chat: Option<ChatViewState>,
    pub toast: Option<String>,
}
```

### Router

```rust
#[derive(uniffi::Record, Clone, Debug)]
pub struct Router {
    pub default_screen: Screen,
    pub screen_stack: Vec<Screen>,
}

#[derive(uniffi::Enum, Clone, Debug, PartialEq)]
pub enum Screen {
    Login,
    ChatList,
    Chat { chat_id: String },
    NewChat,
}
```

Navigation is driven by Rust via the `Router`. The `default_screen` is the root (e.g., `ChatList` when logged in, `Login` when not). The `screen_stack` contains screens pushed on top. The native side binds this stack directly to `NavigationStack` (iOS) or `NavDisplay` (Android), giving full native navigation — swipe-back gestures, system back button, animated transitions — all for free.

When the user swipes back on iOS or taps the system back button on Android, the platform pops the stack and the native code dispatches `UpdateScreenStack` to Rust with the new (shorter) stack. Rust updates its state to match. Since the UI already popped visually, the update from Rust is a no-op on screen. Navigation flows in both directions but Rust is always the source of truth.

### Auth

```rust
#[derive(uniffi::Enum, Clone, Debug)]
pub enum AuthState {
    LoggedOut,
    LoggedIn { npub: String, pubkey: String },
}
```

### Chat List

```rust
#[derive(uniffi::Record, Clone, Debug)]
pub struct ChatSummary {
    pub chat_id: String,
    pub peer_npub: String,
    pub peer_name: Option<String>,
    pub last_message: Option<String>,
    pub last_message_at: Option<i64>,
    pub unread_count: u32,
}
```

### Chat View

```rust
#[derive(uniffi::Record, Clone, Debug)]
pub struct ChatViewState {
    pub chat_id: String,
    pub peer_npub: String,
    pub peer_name: Option<String>,
    pub messages: Vec<ChatMessage>,
    pub can_load_older: bool,
}

#[derive(uniffi::Record, Clone, Debug)]
pub struct ChatMessage {
    pub id: String,
    pub sender_pubkey: String,
    pub content: String,
    pub timestamp: i64,
    pub is_mine: bool,
    pub delivery: MessageDeliveryState,
}

#[derive(uniffi::Enum, Clone, Debug)]
pub enum MessageDeliveryState {
    Pending,
    Sent,
    Failed { reason: String },
}
```

## Actions

The UI dispatches these. They are the only way the native side communicates intent to Rust.

```rust
#[derive(uniffi::Enum, Debug)]
pub enum AppAction {
    // Auth
    CreateAccount,
    Login { nsec: String },
    RestoreSession { nsec: String },
    Logout,

    // Navigation
    PushScreen { screen: Screen },
    UpdateScreenStack { stack: Vec<Screen> },

    // Chat
    CreateChat { peer_npub: String },
    SendMessage { chat_id: String, content: String },
    RetryMessage { chat_id: String, message_id: String },
    OpenChat { chat_id: String },
    LoadOlderMessages { chat_id: String, before_message_id: String, limit: u32 },

    // UI
    ClearToast,

    // Lifecycle
    Foregrounded,
}
```

Actions are fire-and-forget. They never return values. Results come back as state updates through the reconciler.

`dispatch()` **must return immediately**. It enqueues the action onto an internal channel and never performs blocking work on the caller's thread. This is a contract — the native side calls `dispatch()` from the main UI thread, and any blocking would freeze the app. Internally, a dedicated background thread or tokio task processes the action queue. Any synchronous validation inside `dispatch()` must be strictly bounded (microseconds) and must never do IO.

`PushScreen` is for Rust-initiated navigation (e.g., after creating a chat, push to `Chat`). `UpdateScreenStack` is for platform-initiated navigation (e.g., user swipes back, platform pops the stack and sends the new shorter stack to Rust). This two-action split keeps the bidirectional sync clean: Rust pushes via state updates, the platform reports back via `UpdateScreenStack`.

## Updates

Rust sends these to the native side when state changes. Every update carries a monotonic `rev` and the **full current value** for the affected slice of state. Not a diff, not an incremental append — the complete current truth.

```rust
#[derive(uniffi::Enum, Clone, Debug)]
pub enum AppUpdate {
    FullState(AppState),
    RouterChanged { rev: u64, router: Router },
    AuthChanged { rev: u64, auth: AuthState },
    ChatListChanged { rev: u64, chat_list: Vec<ChatSummary> },
    CurrentChatChanged { rev: u64, current_chat: Option<ChatViewState> },
    ToastChanged { rev: u64, toast: Option<String> },
}
```

### Why Full Slices

It's impossible to get out of sync. Every update is a complete snapshot of that piece of state. If something goes wrong — a bug, a race, whatever — the next update corrects it. The system is self-healing.

For this MVP, the performance cost is negligible. A chat with 300 messages is ~60KB. Cloning that in Rust is microseconds. Serializing it across FFI at chat-message rates (a few per second) is nothing.

### Revision Counter (`rev`)

Rust maintains a monotonic `rev: u64` that increments on every state transition that affects view state. Every `AppUpdate` carries `rev`. `AppState` snapshots from `state()` also carry `rev`.

Native maintains `last_rev_applied`. On each update:
- If `update.rev <= last_rev_applied`: drop as stale (can occur after a resync snapshot is applied).
- If `update.rev == last_rev_applied + 1`: apply normally.
- If `update.rev > last_rev_applied + 1`: **forward gap** — resync by pulling `state()`, replacing mirrored view state, and setting `last_rev_applied = snapshot.rev`.

Native should coalesce resync triggers (only one in flight at a time).

In debug builds, a forward gap can optionally trigger an assertion/log to surface bugs early, but the app must still be able to recover via resync.

Lifecycle events (foreground/background) are modeled as actions (`dispatch(...)`) rather than native pulling and overwriting state out-of-band.

## FFI Surface

One UniFFI object. One callback interface. That's it.

```rust
#[uniffi::export(callback_interface)]
pub trait AppReconciler: Send + Sync + 'static {
    fn reconcile(&self, update: AppUpdate);
}

#[derive(uniffi::Object)]
pub struct FfiApp;

#[uniffi::export]
impl FfiApp {
    #[uniffi::constructor]
    pub fn new(data_dir: String) -> Arc<Self>;

    /// Get the full current state snapshot (with rev). Called at startup
    /// and on forward-gap resync. This method must be fast and must not
    /// depend on the actor thread being idle (it should always return the
    /// last committed snapshot).
    pub fn state(&self) -> AppState;

    /// Dispatch an action from the UI. Fire and forget. Must return immediately.
    pub fn dispatch(&self, action: AppAction);

    /// Register for state updates. Spawns a listener thread that calls
    /// reconciler.reconcile() whenever state changes. The thread runs
    /// for the lifetime of the app.
    pub fn listen_for_updates(&self, reconciler: Box<dyn AppReconciler>);
}
```

The native side interacts with exactly these three methods after construction. `state()` once at init (and on resync), `listen_for_updates()` once at init, and `dispatch()` whenever the user does something.

## Key Storage

Nostr private keys (nsec) are stored in native secure storage, never in Rust-managed persistence.

- **iOS:** Keychain
- **Android:** Keystore-backed encrypted storage

**Flow:**
1. On account creation: Rust generates keys, returns the nsec to native via an update. Native stores it in secure storage.
2. On login (import): Native receives nsec from user input, stores it in secure storage, then dispatches `Login { nsec }` to Rust.
3. On app startup (session restore): Native reads nsec from secure storage and dispatches `RestoreSession { nsec }` to Rust.
4. Rust never persists plaintext secret keys to SQLite or any Rust-managed store. It may persist non-secret derived identifiers (e.g., pubkey).
5. Export: Native reads from secure storage and presents to user, optionally gated behind biometric/PIN.

## Ephemeral UI State

The following UI state stays in Swift/Kotlin and is never sent to Rust:

- Scroll position, scroll velocity, drag offsets
- Focus state, keyboard visibility, text cursor position
- Animation progress, interactive transition state
- Haptics triggers, transient view-local toggles

Rust may receive coarse intents triggered by ephemeral state (e.g., `LoadOlderMessages` when the user scrolls near the top of a chat), but should never receive per-frame or per-gesture updates.

## Rust Internals

### AppCore

All mutable state lives in a single `AppCore` struct, structured as a single "app actor." It is not shared across threads behind locks. Instead, it lives in one place, and all mutations go through `dispatch()` which enqueues to an internal channel for serialized processing.

```rust
pub struct AppCore {
    state: AppState,
    rev: u64,
    update_sender: Sender<AppUpdate>,
    // Internal state not exposed to UI
    mdk: MDK<MdkSqliteStorage>,
    nostr_keys: Option<Keys>,
    // ... relay connections, etc.
}
```

When an action arrives, `AppCore` processes it, mutates `self.state`, increments `rev`, and sends the relevant `AppUpdate` through the channel:

```rust
impl AppCore {
    fn handle_action(&mut self, action: AppAction) {
        match action {
            AppAction::SendMessage { chat_id, content } => {
                // 1. Create pending message with stable ID (optimistic UI)
                let message = ChatMessage {
                    id: generate_id(),
                    sender_pubkey: self.my_pubkey(),
                    content: content.clone(),
                    timestamp: now(),
                    is_mine: true,
                    delivery: MessageDeliveryState::Pending,
                };
                if let Some(chat) = self.state.current_chat.as_mut() {
                    chat.messages.push(message);
                }
                self.emit(AppUpdate::CurrentChatChanged {
                    rev: self.next_rev(),
                    current_chat: self.state.current_chat.clone(),
                });

                // 2. Kick off async send via MDK
                // (result comes back through internal channel, transitions
                //  message to Sent or Failed, emits another update)
            }
            // ...
        }
    }

    fn next_rev(&mut self) -> u64 {
        self.rev += 1;
        self.state.rev = self.rev;
        self.rev
    }

    fn emit(&self, update: AppUpdate) {
        let _ = self.update_sender.send(update);
    }
}
```

### Channel Setup

One `flume::unbounded()` channel. The sender lives in `AppCore`. The receiver is used by the listener thread.

`flume::unbounded().send()` never drops messages (it can only fail if the receiver is dropped, which means the app is shutting down). The listener thread processes messages one at a time, synchronously calling the reconciler callback. `DispatchQueue.main.async` and Kotlin's main dispatcher don't drop enqueued work. So the transport is lossless end to end — but `rev` provides defense-in-depth against bugs.

### Async Work

MDK and Nostr operations are async. A tokio runtime is initialized once at app startup (inside `FfiApp::new()`). Async results are fed back into `AppCore`'s state and emitted as updates.

The important invariant is: all state mutations happen in one place (the actor), and every mutation that changes UI-visible state increments `rev` and emits an update.

### Background Events

When a message arrives from a Nostr relay subscription:

1. Rust receives the Nostr event
2. Rust processes it through MDK (MLS decryption)
3. Rust decides where it goes:
   - If it's for the currently-open chat → append to `state.current_chat.messages`, emit `CurrentChatChanged`
   - Update the relevant `state.chat_list` entry's preview/unread count, emit `ChatListChanged`
4. Persist to SQLite

The native side never decides where a message belongs. Rust already did that.

## iOS Implementation

### AppManager

One `@Observable` object for the entire app. Implements the UniFFI `AppReconciler` protocol. Tracks `rev` for gap detection.

```swift
@Observable final class AppManager: AppReconciler {
    let rust: FfiApp
    var state: AppState
    private var lastRevApplied: UInt64 = 0

    init() {
        let dataDir = FileManager.default
            .urls(for: .documentDirectory, in: .userDomainMask)
            .first!.path
        let rust = FfiApp(dataDir: dataDir)
        self.rust = rust
        let initial = rust.state()
        self.state = initial
        self.lastRevApplied = initial.rev
        rust.listenForUpdates(reconciler: self)
    }

    func reconcile(update: AppUpdate) {
        DispatchQueue.main.async { [weak self] in
            guard let self else { return }
            let updateRev = update.rev
            if updateRev != self.lastRevApplied + 1 {
                // Rev gap — resync
                assertionFailure("Rev gap: expected \(self.lastRevApplied + 1), got \(updateRev)")
                let snapshot = self.rust.state()
                self.state = snapshot
                self.lastRevApplied = snapshot.rev
                return
            }
            self.lastRevApplied = updateRev
            switch update {
            case .fullState(let s):
                self.state = s
            case .routerChanged(_, let router):
                self.state.router = router
            case .authChanged(_, let auth):
                self.state.auth = auth
            case .chatListChanged(_, let list):
                self.state.chatList = list
            case .currentChatChanged(_, let chat):
                self.state.currentChat = chat
            case .toastChanged(_, let toast):
                self.state.toast = toast
            }
        }
    }

    func dispatch(_ action: AppAction) {
        rust.dispatch(action: action)
    }

    func onForeground() {
        let snapshot = rust.state()
        DispatchQueue.main.async {
            self.state = snapshot
            self.lastRevApplied = snapshot.rev
        }
    }
}
```

### Views

Views read `manager.state` and call `manager.dispatch()`. No logic.

The root view uses `NavigationStack(path:)` bound to the router's screen stack. This gives full native navigation: swipe-back gestures, animated push/pop transitions, and navigation bar — all driven by Rust state. When the user swipes back, `NavigationStack` pops the path, `onChange` fires, and the new stack is dispatched to Rust via `UpdateScreenStack`.

```swift
struct ContentView: View {
    @Bindable var manager: AppManager

    var body: some View {
        Group {
            switch manager.state.router.defaultScreen {
            case .login:
                LoginView(manager: manager)
            default:
                NavigationStack(path: $manager.state.router.screenStack) {
                    screenView(manager: manager,
                               screen: manager.state.router.defaultScreen)
                        .navigationDestination(for: Screen.self) { screen in
                            screenView(manager: manager, screen: screen)
                        }
                }
                .onChange(of: manager.state.router.screenStack) { old, new in
                    if new.count < old.count {
                        manager.dispatch(.updateScreenStack(stack: new))
                    }
                }
            }
        }
    }
}

@ViewBuilder
func screenView(manager: AppManager, screen: Screen) -> some View {
    switch screen {
    case .chatList:
        ChatListView(manager: manager)
    case .chat:
        ChatView(manager: manager)
    case .newChat:
        NewChatView(manager: manager)
    case .login:
        LoginView(manager: manager)
    }
}

struct ChatListView: View {
    let manager: AppManager

    var body: some View {
        List(manager.state.chatList, id: \.chatId) { chat in
            ChatRow(chat: chat)
                .onTapGesture {
                    manager.dispatch(.openChat(chatId: chat.chatId))
                }
        }
        .navigationTitle("Chats")
        .toolbar {
            Button("New Chat") {
                manager.dispatch(.pushScreen(screen: .newChat))
            }
        }
    }
}

struct ChatView: View {
    let manager: AppManager
    @State private var messageText = ""

    var body: some View {
        if let chat = manager.state.currentChat {
            VStack {
                ScrollView {
                    LazyVStack {
                        ForEach(chat.messages, id: \.id) { message in
                            MessageBubble(message: message)
                        }
                    }
                }
                HStack {
                    TextField("Message", text: $messageText)
                    Button("Send") {
                        manager.dispatch(.sendMessage(
                            chatId: chat.chatId,
                            content: messageText
                        ))
                        messageText = ""
                    }
                }
                .padding()
            }
            .navigationTitle(chat.peerName ?? chat.peerNpub)
        }
    }
}

struct LoginView: View {
    let manager: AppManager
    @State private var nsecInput = ""

    var body: some View {
        VStack(spacing: 20) {
            Button("Create New Account") {
                manager.dispatch(.createAccount)
            }
            Divider()
            TextField("Enter nsec...", text: $nsecInput)
            Button("Login") {
                manager.dispatch(.login(nsec: nsecInput))
            }
        }
        .padding()
    }
}
```

The only local `@State` in views is for text input fields (what the user is typing but hasn't submitted yet). Everything else comes from `manager.state`.

## Android Implementation

### AppManager

Same pattern, Kotlin version. Tracks `rev` for gap detection.

```kotlin
@Stable
class AppManager private constructor(context: Context) : AppReconciler {
    private val mainScope = CoroutineScope(SupervisorJob() + Dispatchers.Main.immediate)
    private val rust: FfiApp
    private var lastRevApplied: ULong = 0u

    var state by mutableStateOf(AppState(
        rev = 0u,
        router = Router(
            defaultScreen = Screen.Login,
            screenStack = emptyList(),
        ),
        auth = AuthState.LoggedOut,
        chatList = emptyList(),
        currentChat = null,
        toast = null,
    ))
        private set

    init {
        val dataDir = context.filesDir.absolutePath
        rust = FfiApp(dataDir)
        val initial = rust.state()
        state = initial
        lastRevApplied = initial.rev
        rust.listenForUpdates(this)
    }

    override fun reconcile(update: AppUpdate) {
        mainScope.launch {
            val updateRev = update.rev
            if (updateRev != lastRevApplied + 1u) {
                // Rev gap — resync
                val snapshot = rust.state()
                state = snapshot
                lastRevApplied = snapshot.rev
                return@launch
            }
            lastRevApplied = updateRev
            when (update) {
                is AppUpdate.FullState -> state = update.v1
                is AppUpdate.RouterChanged -> state = state.copy(router = update.router)
                is AppUpdate.AuthChanged -> state = state.copy(auth = update.auth)
                is AppUpdate.ChatListChanged -> state = state.copy(chatList = update.chatList)
                is AppUpdate.CurrentChatChanged -> state = state.copy(currentChat = update.currentChat)
                is AppUpdate.ToastChanged -> state = state.copy(toast = update.toast)
            }
        }
    }

    fun dispatch(action: AppAction) {
        rust.dispatch(action)
    }

    fun popScreen() {
        val stack = state.router.screenStack
        if (stack.isNotEmpty()) {
            dispatch(AppAction.UpdateScreenStack(stack.dropLast(1)))
        }
    }

    fun onForeground() {
        mainScope.launch {
            val snapshot = rust.state()
            state = snapshot
            lastRevApplied = snapshot.rev
        }
    }

    companion object {
        @Volatile private var instance: AppManager? = null
        fun getInstance(context: Context): AppManager =
            instance ?: synchronized(this) {
                instance ?: AppManager(context.applicationContext).also { instance = it }
            }
    }
}
```

### Composables

The root composable uses the screen stack to drive navigation. `BackHandler` intercepts the system back button/gesture and pops the stack via Rust.

```kotlin
@Composable
fun MarmotApp(manager: AppManager) {
    val router = manager.state.router

    when (router.defaultScreen) {
        is Screen.Login -> LoginScreen(manager)
        else -> {
            BackHandler(enabled = router.screenStack.isNotEmpty()) {
                manager.popScreen()
            }

            val currentScreen = router.screenStack.lastOrNull()
                ?: router.defaultScreen

            AnimatedContent(targetState = currentScreen) { screen ->
                ScreenContent(manager, screen)
            }
        }
    }
}

@Composable
fun ScreenContent(manager: AppManager, screen: Screen) {
    when (screen) {
        is Screen.ChatList -> ChatListScreen(manager)
        is Screen.Chat -> ChatScreen(manager)
        is Screen.NewChat -> NewChatScreen(manager)
        is Screen.Login -> LoginScreen(manager)
    }
}

@Composable
fun ChatListScreen(manager: AppManager) {
    Scaffold(
        topBar = {
            TopAppBar(
                title = { Text("Chats") },
                actions = {
                    IconButton(onClick = {
                        manager.dispatch(AppAction.PushScreen(Screen.NewChat))
                    }) {
                        Icon(Icons.Default.Add, "New Chat")
                    }
                }
            )
        }
    ) { padding ->
        LazyColumn(modifier = Modifier.padding(padding)) {
            items(manager.state.chatList) { chat ->
                ChatRow(
                    chat = chat,
                    onClick = { manager.dispatch(AppAction.OpenChat(chat.chatId)) }
                )
            }
        }
    }
}

@Composable
fun ChatScreen(manager: AppManager) {
    val chat = manager.state.currentChat ?: return
    var messageText by remember { mutableStateOf("") }

    Column {
        LazyColumn(modifier = Modifier.weight(1f)) {
            items(chat.messages) { message ->
                MessageBubble(message = message)
            }
        }
        Row(modifier = Modifier.padding(8.dp)) {
            TextField(
                value = messageText,
                onValueChange = { messageText = it },
                modifier = Modifier.weight(1f),
                placeholder = { Text("Message") }
            )
            Button(
                onClick = {
                    manager.dispatch(AppAction.SendMessage(chat.chatId, messageText))
                    messageText = ""
                }
            ) {
                Text("Send")
            }
        }
    }
}
```

## Testing

The biggest advantage of this architecture: nearly everything is testable in pure Rust with `cargo test`. No simulators, no emulators, no UI test frameworks.

### TestReconciler

A mock that collects updates. This is the primary testing tool.

```rust
#[cfg(test)]
struct TestReconciler {
    updates: Arc<Mutex<Vec<AppUpdate>>>,
}

impl TestReconciler {
    fn new() -> (Self, Arc<Mutex<Vec<AppUpdate>>>) {
        let updates = Arc::new(Mutex::new(vec![]));
        (Self { updates: updates.clone() }, updates)
    }
}

impl AppReconciler for TestReconciler {
    fn reconcile(&self, update: AppUpdate) {
        self.updates.lock().unwrap().push(update);
    }
}
```

### Integration Tests

Test full user flows through the exact same `FfiApp` interface that native code uses.

```rust
#[test]
fn test_create_account_navigates_to_chat_list() {
    let app = FfiApp::new("/tmp/test".into());
    let (reconciler, updates) = TestReconciler::new();
    app.listen_for_updates(Box::new(reconciler));

    assert_eq!(app.state().router.default_screen, Screen::Login);
    assert!(matches!(app.state().auth, AuthState::LoggedOut));

    app.dispatch(AppAction::CreateAccount);
    std::thread::sleep(std::time::Duration::from_millis(100));

    let updates = updates.lock().unwrap();
    assert!(updates.iter().any(|u| matches!(u, AppUpdate::AuthChanged { auth: AuthState::LoggedIn { .. }, .. })));
    assert!(updates.iter().any(|u| matches!(u,
        AppUpdate::RouterChanged { router: Router { default_screen: Screen::ChatList, .. }, .. }
    )));
}

#[test]
fn test_send_message_appears_in_chat() {
    let app = setup_logged_in_app();
    let (reconciler, updates) = TestReconciler::new();
    app.listen_for_updates(Box::new(reconciler));

    app.dispatch(AppAction::CreateChat { peer_npub: "npub1...".into() });
    std::thread::sleep(std::time::Duration::from_millis(100));

    let chat_id = app.state().chat_list[0].chat_id.clone();
    app.dispatch(AppAction::OpenChat { chat_id: chat_id.clone() });
    std::thread::sleep(std::time::Duration::from_millis(50));

    app.dispatch(AppAction::SendMessage { chat_id, content: "hello".into() });
    std::thread::sleep(std::time::Duration::from_millis(100));

    let state = app.state();
    let chat = state.current_chat.unwrap();
    assert!(chat.messages.iter().any(|m| m.content == "hello"));
    // Initially pending
    assert!(chat.messages.iter().any(|m|
        m.content == "hello" && matches!(m.delivery, MessageDeliveryState::Pending)
    ));
}

#[test]
fn test_navigation_push_and_pop() {
    let app = setup_logged_in_app();
    let (reconciler, _) = TestReconciler::new();
    app.listen_for_updates(Box::new(reconciler));

    let router = app.state().router;
    assert_eq!(router.default_screen, Screen::ChatList);
    assert!(router.screen_stack.is_empty());

    app.dispatch(AppAction::PushScreen { screen: Screen::NewChat });
    std::thread::sleep(std::time::Duration::from_millis(50));
    assert_eq!(app.state().router.screen_stack, vec![Screen::NewChat]);

    app.dispatch(AppAction::UpdateScreenStack { stack: vec![] });
    std::thread::sleep(std::time::Duration::from_millis(50));
    assert!(app.state().router.screen_stack.is_empty());
    assert_eq!(app.state().router.default_screen, Screen::ChatList);
}

#[test]
fn test_rev_continuity() {
    let app = FfiApp::new("/tmp/test-rev".into());
    let (reconciler, updates) = TestReconciler::new();
    app.listen_for_updates(Box::new(reconciler));

    let initial_rev = app.state().rev;
    app.dispatch(AppAction::CreateAccount);
    std::thread::sleep(std::time::Duration::from_millis(100));

    let updates = updates.lock().unwrap();
    let revs: Vec<u64> = updates.iter().map(|u| u.rev()).collect();
    // Revs should be consecutive starting from initial_rev + 1
    for (i, rev) in revs.iter().enumerate() {
        assert_eq!(*rev, initial_rev + 1 + i as u64);
    }
}
```

### Two-Party Integration Tests

For testing actual message delivery between two users (requires MDK with in-memory storage or a local relay):

```rust
#[test]
fn test_alice_sends_bob_receives() {
    let alice = FfiApp::new("/tmp/alice".into());
    let bob = FfiApp::new("/tmp/bob".into());

    alice.dispatch(AppAction::CreateAccount);
    bob.dispatch(AppAction::CreateAccount);
    // ... set up relay, exchange keys, create chat ...
    // ... alice sends message, verify bob's state updates ...
}
```

This is a later-stage test that depends on MDK integration details. But the architecture supports it naturally because everything goes through the same `FfiApp` interface.

## Project Layout

```
marmot-app/
├── rust/
│   ├── Cargo.toml
│   ├── uniffi.toml
│   └── src/
│       ├── lib.rs           # uniffi::setup_scaffolding!, FfiApp, exports
│       ├── core.rs          # AppCore: state, handle_action, emit
│       ├── state.rs         # AppState, Screen, AuthState, etc.
│       ├── actions.rs       # AppAction enum
│       ├── updates.rs       # AppUpdate enum, AppReconciler trait
│       ├── logging.rs       # Platform-native logging init
│       ├── mdk.rs           # MDK wrapper (protocol operations)
│       ├── nostr.rs         # Nostr relay connections, subscriptions
│       └── persistence.rs   # SQLite storage for messages, accounts
├── ios/
│   └── Marmot/
│       ├── MarmotApp.swift  # @main, create AppManager
│       ├── AppManager.swift # AppReconciler implementation
│       ├── ContentView.swift
│       └── Views/
│           ├── LoginView.swift
│           ├── ChatListView.swift
│           ├── ChatView.swift
│           └── NewChatView.swift
├── android/
│   └── app/src/main/
│       ├── java/com/marmot/app/
│       │   ├── MainActivity.kt
│       │   ├── AppManager.kt
│       │   ├── MarmotApp.kt     # top-level Composable
│       │   └── ui/
│       │       ├── LoginScreen.kt
│       │       ├── ChatListScreen.kt
│       │       ├── ChatScreen.kt
│       │       └── NewChatScreen.kt
│       └── jniLibs/             # compiled .so files
├── justfile                      # build commands
└── spec.md                       # this document
```

## Build System

Use a justfile with commands for each platform:

- `just build-ios` — compile Rust for iOS targets, generate Swift bindings with uniffi-bindgen, copy into Xcode project
- `just build-android` — compile Rust for Android targets with cargo-ndk, generate Kotlin bindings, copy into Gradle project
- `just test` — run `cargo test` on the Rust core

The Rust crate compiles as a `cdylib` (for Android .so) and `staticlib` (for iOS .a). UniFFI generates the Swift and Kotlin bindings from the Rust source.

## Screens

Four screens total.

### Login

- "Create New Account" button → dispatches `CreateAccount`
- Text field for nsec + "Login" button → dispatches `Login { nsec }`
- On success, Rust changes default screen to `ChatList` and auth to `LoggedIn`

### Chat List

- Shows `state.chat_list` sorted by `last_message_at`
- Each row shows peer name/npub, last message preview, unread count
- Tap a row → dispatches `OpenChat { chat_id }`
- "New Chat" button → dispatches `PushScreen { screen: NewChat }`
- Logout option → dispatches `Logout`

### Chat

- Shows `state.current_chat.messages` in a scrollable list
- Messages styled as bubbles, left/right based on `is_mine`
- Delivery state indicator per message (pending spinner, sent checkmark, failed with retry)
- Text input + send button at bottom
- Send → dispatches `SendMessage { chat_id, content }`
- Scroll near top → dispatches `LoadOlderMessages` (if `can_load_older`)
- Back → native swipe-back / system back pops `NavigationStack` / `BackHandler`, which dispatches `UpdateScreenStack` to Rust

### New Chat

- Text field for peer's npub
- "Start Chat" button → dispatches `CreateChat { peer_npub }`
- On success, Rust creates the chat, navigates to `Chat { chat_id }`

## Logging

All Rust code uses the `tracing` crate for logging. At app startup, the tracing subscriber is configured with platform-native layers so that logs go to the right place on each platform — no extra FFI needed.

### Platform Layers

- **iOS:** `tracing-oslog` — writes to Apple's unified logging system (os_log)
- **Android:** `paranoid-android` — writes to Android's logcat
- **Tests / desktop:** `tracing-subscriber::fmt` — writes to stdout

```rust
pub fn init_logging() {
    use tracing_subscriber::prelude::*;

    #[cfg(target_os = "ios")]
    {
        let os_log = tracing_oslog::OsLogger::new(
            "com.marmot.app",
            "default"
        );
        tracing_subscriber::registry()
            .with(os_log)
            .init();
    }

    #[cfg(target_os = "android")]
    {
        paranoid_android::init("marmot");
    }

    #[cfg(not(any(target_os = "ios", target_os = "android")))]
    {
        tracing_subscriber::fmt()
            .with_env_filter("marmot=debug,info")
            .init();
    }
}
```

This is called once at the start of `FfiApp::new()`, before anything else.

### Log Security

Never log secret keys, raw message plaintext in release builds, or sensitive protocol material. Prefer structured fields that can be redacted or disabled in release builds.

### Crash Reporting (Planned Extension)

v1 does not include crash reporter breadcrumb forwarding. Once a crash reporter SDK is selected (e.g., Sentry, Crashlytics), a narrow UniFFI `LogSink` callback interface can be added to forward selected warn/error lines and lifecycle events. It must be rate-limited and must never forward secrets.

### Cargo Dependencies

```toml
[dependencies]
tracing = "0.1"
tracing-subscriber = { version = "0.3", features = ["env-filter"] }

[target.'cfg(target_os = "ios")'.dependencies]
tracing-oslog = "0.3"

[target.'cfg(target_os = "android")'.dependencies]
paranoid-android = "0.2"
```

## MDK Integration (High Level)

The Rust core depends on `mdk-core` and `mdk-sqlite-storage` as Rust library crates. We do **not** use `mdk-uniffi` — that's for apps that call MDK directly from Swift/Kotlin. We wrap MDK in our own Rust code and expose our own simpler interface.

Key MDK operations the Rust core will use internally:

- `mdk.create_group(...)` — create an MLS group for a new chat
- `mdk.create_message(...)` — encrypt a message for a group
- `mdk.process_message(...)` — decrypt a received message
- `mdk.create_key_package_for_event(...)` — publish key packages so others can invite us
- `mdk.process_welcome(...)` / `mdk.accept_welcome(...)` — handle group invitations

The native side knows nothing about MLS, Nostr events, key packages, or welcomes. It sees chats and messages.

## Lifecycle

### App Launch

1. Native creates `FfiApp(dataDir)` — this initializes logging, tokio runtime, and loads persisted state.
2. Native reads nsec from secure storage (if exists) and dispatches `RestoreSession { nsec }`.
3. Native calls `state()` to get the initial snapshot (with `rev`), sets mirrored view state.
4. Native calls `listen_for_updates()` — listener thread starts.
5. Subsequent updates flow through the reconciler.

### Foreground Resume

Native pulls `state()` and replaces all mirrored view state. This handles any state changes that occurred while the listener was potentially stale.

### Background

Rust may keep network tasks running depending on platform constraints. v1 can simply reconnect/resync on resume.

## What's Not in V1

- Group chat (> 2 people)
- Media / file sharing
- Push notifications
- Contact list / user search / profiles
- Multiple accounts
- Message reactions, replies, editing, deletion
- Read receipts
- Typing indicators
- Settings screen
- Onboarding flow
- Crash reporter breadcrumb forwarding (planned extension)

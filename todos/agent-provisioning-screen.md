# Agent Provisioning Screen

Immediately navigate into a loading/provisioning screen when the user taps "New Agent", instead of sitting on the chat list for ~40 seconds.

## Design

- New `Screen::AgentProvisioning` variant in Rust router
- New `AgentProvisioningState` in `AppState` with phase + diagnostic info
- Rust pushes this screen immediately on button tap, updates phase as the flow progresses
- When the flow completes, Rust swaps the screen to `Screen::Chat { chat_id }`
- If the user swipes back, Rust cancels the agent flow (same as app kill behavior)
- iOS renders a chat-shell-like view with disabled input and centered loading content

## Phases

The provisioning state exposes a phase enum to the UI:

```text
Ensuring        → "Requesting agent..."
Provisioning    → "Starting microVM... (attempt 3/45)"
Recovering      → "Recovering agent..."
PublishingKeyPkg→ "Publishing key package..."
CreatingChat    → "Creating encrypted chat..."
```

Each phase maps to a status string. The UI also shows elapsed time and the agent npub once available (returned by the initial ensure response, even in `creating` state).

## Rust changes

### state.rs

Add to `Screen` enum:

```rust
AgentProvisioning,
```

Add new struct:

```rust
#[derive(uniffi::Record, Clone, Debug)]
pub struct AgentProvisioningState {
    pub phase: AgentProvisioningPhase,
    pub agent_npub: Option<String>,
    pub status_message: String,
    pub elapsed_secs: u32,
    pub poll_attempt: Option<u32>,
    pub poll_max: Option<u32>,
}

#[derive(uniffi::Enum, Clone, Debug, PartialEq)]
pub enum AgentProvisioningPhase {
    Ensuring,
    Provisioning,
    Recovering,
    PublishingKeyPackage,
    CreatingChat,
    Error,
}
```

Add to `AppState`:

```rust
pub agent_provisioning: Option<AgentProvisioningState>,
```

### agent.rs — Break `run_agent_flow` into progress-reporting steps

Currently `run_agent_flow` is a single async function that runs the entire ensure → poll → return loop. The problem: it can't report intermediate progress back to the UI because it only sends one `InternalEvent::AgentFlowCompleted` at the end.

**Approach:** Add a new `InternalEvent::AgentFlowProgress` variant. Have `run_agent_flow` send progress events at each phase transition via the existing `core_sender` channel.

```rust
// In updates.rs, add:
AgentFlowProgress {
    flow_token: u64,
    phase: AgentProvisioningPhase,
    agent_npub: Option<String>,
    poll_attempt: Option<u32>,
},
```

Modify `run_agent_flow` signature to accept a `tx: mpsc::UnboundedSender<CoreMsg>` and `flow_token: u64` so it can send progress. At each transition point:

```rust
// After ensure_agent succeeds:
let _ = tx.send(CoreMsg::Internal(Box::new(InternalEvent::AgentFlowProgress {
    flow_token,
    phase: AgentProvisioningPhase::Provisioning,
    agent_npub: Some(ensure_response.agent_id.clone()),
    poll_attempt: Some(0),
})));

// At each poll iteration:
let _ = tx.send(CoreMsg::Internal(Box::new(InternalEvent::AgentFlowProgress {
    flow_token,
    phase: AgentProvisioningPhase::Provisioning,
    agent_npub: Some(state.agent_id.clone()),
    poll_attempt: Some(attempt),
})));

// When triggering recovery:
let _ = tx.send(CoreMsg::Internal(Box::new(InternalEvent::AgentFlowProgress {
    flow_token,
    phase: AgentProvisioningPhase::Recovering,
    agent_npub: ...,
    poll_attempt: Some(attempt),
})));
```

### agent.rs — `ensure_agent()` changes

After validation passes and before spawning the async task:

1. Set `agent_provisioning` state with phase `Ensuring`
2. Push `Screen::AgentProvisioning` onto the router
3. Emit router + state
4. Then spawn the async task (same as today but with progress reporting)

```rust
// After setting busy and before spawning:
self.state.agent_provisioning = Some(AgentProvisioningState {
    phase: AgentProvisioningPhase::Ensuring,
    agent_npub: None,
    status_message: "Requesting agent...".to_string(),
    elapsed_secs: 0,
    poll_attempt: None,
    poll_max: None,
});
self.open_agent_provisioning_screen();
self.emit_state();
```

### core/mod.rs — New handler for progress events

```rust
fn handle_agent_flow_progress(
    &mut self,
    flow_token: u64,
    phase: AgentProvisioningPhase,
    agent_npub: Option<String>,
    poll_attempt: Option<u32>,
) {
    if flow_token != self.agent_flow_token { return; }

    let status_message = match &phase {
        AgentProvisioningPhase::Ensuring => "Requesting agent...".to_string(),
        AgentProvisioningPhase::Provisioning => {
            if let Some(attempt) = poll_attempt {
                format!("Starting microVM... (attempt {}/{})", attempt, AGENT_POLL_MAX_ATTEMPTS)
            } else {
                "Starting microVM...".to_string()
            }
        }
        AgentProvisioningPhase::Recovering => "Recovering agent...".to_string(),
        AgentProvisioningPhase::PublishingKeyPackage => "Publishing key package...".to_string(),
        AgentProvisioningPhase::CreatingChat => "Creating encrypted chat...".to_string(),
        AgentProvisioningPhase::Error => "Error".to_string(),
    };

    self.state.agent_provisioning = Some(AgentProvisioningState {
        phase,
        agent_npub,
        status_message,
        elapsed_secs: /* compute from flow start time */,
        poll_attempt,
        poll_max: Some(AGENT_POLL_MAX_ATTEMPTS),
    });
    self.emit_state();
}
```

### core/mod.rs — Handle `UpdateScreenStack` (back-swipe cancellation)

In the existing `UpdateScreenStack` handler (~line 5088), detect when `AgentProvisioning` was popped:

```rust
AppAction::UpdateScreenStack { stack } => {
    // Detect if agent provisioning screen was removed by user navigation.
    let had_provisioning = self.state.router.screen_stack
        .iter().any(|s| matches!(s, Screen::AgentProvisioning));
    let has_provisioning = stack.iter().any(|s| matches!(s, Screen::AgentProvisioning));

    if had_provisioning && !has_provisioning {
        // User swiped back — cancel the agent flow.
        self.invalidate_agent_flow();
        self.state.agent_provisioning = None;
    }

    self.state.router.screen_stack = stack;
    self.sync_current_chat_to_router();
    self.emit_router();
}
```

### agent.rs — `handle_agent_flow_completed` changes

On success, update provisioning phase to `CreatingChat` before calling `open_or_create_direct_chat_for_agent`. The chat-open call will replace the `AgentProvisioning` screen with `Screen::Chat`.

On error, update provisioning phase to `Error` with the error message but keep the screen up so the user sees the error (instead of a toast they might miss while on the chat list).

After the Chat screen is pushed (in `open_or_create_direct_chat_for_agent`), clear `agent_provisioning`:

```rust
self.state.agent_provisioning = None;
```

### agent.rs — `open_or_create_direct_chat_for_agent` changes

In `open_chat_screen`, make sure it replaces `AgentProvisioning` in the stack (similar to how it already replaces `NewChat`). Add `Screen::AgentProvisioning` to the filter in `open_chat_screen` (~mod.rs line 2890):

```rust
// Existing: pops NewChat/NewGroupChat
// Add: also pop AgentProvisioning
self.state.router.screen_stack.retain(|s| !matches!(s,
    Screen::NewChat | Screen::NewGroupChat | Screen::AgentProvisioning
));
```

### session.rs — `stop_session` cleanup

Already calls `invalidate_agent_flow()`. Additionally clear provisioning state:

```rust
self.state.agent_provisioning = None;
```

And remove `AgentProvisioning` from screen stack if present.

### Elapsed time

Two options:
1. **Simple:** Store `agent_flow_start: Option<Instant>` on `AppCore`. Compute `elapsed_secs` in each progress handler call as `start.elapsed().as_secs() as u32`.
2. **Timer-based:** Spawn a 1-second interval timer that updates `elapsed_secs`. More complex, more real-time.

Recommend option 1 — elapsed time updates every ~2 seconds (at each poll) which is good enough.

## iOS changes

### ContentView.swift

Add a case in `screenView` for the new screen variant:

```swift
case .agentProvisioning:
    AgentProvisioningView(
        state: appState.agentProvisioning
    )
```

### AgentProvisioningView.swift (new file)

A simple view that looks like a chat shell in a loading state:

```swift
struct AgentProvisioningView: View {
    let state: AgentProvisioningState?

    var body: some View {
        VStack {
            Spacer()

            // Loading indicator
            ProgressView()
                .scaleEffect(1.5)
                .padding(.bottom, 16)

            // Status message
            Text(state?.statusMessage ?? "Starting agent...")
                .font(.headline)
                .foregroundStyle(.secondary)

            // Poll progress (if in provisioning phase)
            if let attempt = state?.pollAttempt, let max = state?.pollMax {
                Text("\(attempt) / \(max)")
                    .font(.caption)
                    .foregroundStyle(.tertiary)
            }

            // Elapsed time
            if let elapsed = state?.elapsedSecs, elapsed > 0 {
                Text("\(elapsed)s elapsed")
                    .font(.caption2)
                    .foregroundStyle(.tertiary)
            }

            // Agent npub once available
            if let npub = state?.agentNpub {
                Text(npub.prefix(20) + "...")
                    .font(.caption2)
                    .monospaced()
                    .foregroundStyle(.tertiary)
                    .padding(.top, 8)
            }

            Spacer()

            // Disabled input bar placeholder
            HStack {
                Text("Message")
                    .foregroundStyle(.quaternary)
                    .padding(.horizontal, 12)
                    .padding(.vertical, 8)
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .background(.fill.tertiary, in: RoundedRectangle(cornerRadius: 20))
            }
            .padding()
            .disabled(true)
        }
        .navigationTitle(state?.agentNpub.map { String($0.prefix(12)) + "..." } ?? "New Agent")
        .navigationBarTitleDisplayMode(.inline)
    }
}
```

This is intentionally simple. The nav bar shows the agent npub (truncated) once available, or "New Agent" as placeholder. The center shows a spinner with phase text. The bottom has a disabled message input placeholder to give it the chat-shell feel.

### Error state on the provisioning screen

When the phase is `Error`, replace the spinner with an error icon and a "Try Again" button:

```swift
if state?.phase == .error {
    Image(systemName: "exclamationmark.triangle")
    Text(state?.statusMessage ?? "Something went wrong")
    Button("Try Again") { /* dispatch .ensureAgent again */ }
}
```

This needs an `onRetry` callback passed from ContentView.

## Android changes

Minimal — Android just needs a matching composable that reads `appState.agentProvisioning`. The Kotlin bindings are auto-generated from the Rust uniffi types. Implementation deferred to a follow-up.

## What does NOT change

- `ChatView` — no changes, no phantom chat concept
- `ChatListView` — still only shows real MLS chats from storage
- `run_agent_flow` — same logic, just gains progress reporting
- Key package publish flow — unchanged, just reports its phase
- Server API — no changes
- MLS group creation — unchanged

## Edge cases

| Scenario | Behavior |
|----------|----------|
| App killed on provisioning screen | Restart → chat list, no orphaned state, tap button to recover |
| Swipe back | Cancel flow, return to chat list, button re-enabled |
| Agent already exists (re-tap) | Push provisioning screen, poll finds ready quickly, transition to chat |
| Existing chat with agent | Provisioning screen → brief flash → existing chat opens |
| Network lost mid-flow | Poll fails → error phase shown on provisioning screen |
| Timeout (90s) | Error phase with "Try Again" button |
| Key package publish fails | Error phase with message, retry available |
| Logout during provisioning | `stop_session` clears everything, screen resets to login |

## Implementation order

1. Rust `state.rs` — add `AgentProvisioningPhase`, `AgentProvisioningState`, `Screen::AgentProvisioning`, field on `AppState`
2. Rust `updates.rs` — add `AgentFlowProgress` variant to `InternalEvent`
3. Rust `agent.rs` — modify `run_agent_flow` to accept tx/token and send progress events; modify `ensure_agent` to push provisioning screen; modify `handle_agent_flow_completed` for error-on-screen; modify `open_or_create_direct_chat_for_agent` to clear provisioning state
4. Rust `core/mod.rs` — add `handle_agent_flow_progress` handler; update `UpdateScreenStack` for back-swipe cancellation; update `open_chat_screen` to pop `AgentProvisioning`; wire event dispatch
5. Rust `core/session.rs` — clear provisioning state in `stop_session`
6. Regenerate Swift/Kotlin bindings
7. iOS `AgentProvisioningView.swift` — new file
8. iOS `ContentView.swift` — add `.agentProvisioning` case in `screenView`
9. Rebuild xcframeworks, smoke test on simulator

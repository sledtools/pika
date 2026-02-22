## Iced View Module Pattern

- Each module has a `State` struct that holds state for that module
- Each module defines a `Message` struct for the messages that the view code emits
- Each module may have an `Event` enum that raises actions to be performed by a higher level

Using the same name for `State`, `Message`, and `Event` in each module is fine. We don't need to prefix names with things like `ChatMessage`. Rather we just use scoping rules at the calling level (`chat::Message`).

The `State` struct is like:

```rust
impl State {
    /// Data needed to kick off the state
    pub fn new() -> State { /* */ }

    /// View code that emits messages, takes immutable reference to self
    pub fn view(&self) -> Element<Message> {
        /* view code */
    }

    /// Messages go down the update chain, Events bubble back up
    pub fn update(&mut self, message: Message) -> Option<Event> {
        match message {
            /* Apply updates */
        }
    }
}
```

Instead of passing down mutable references to top-level data structures, send `Event`s back up the stack and make updates at the higher level. This pattern is most clearly illustrated by the logout flow. A logout message triggers with the button press, and events roll up the stack to the top level. At the top, the session is destroyed, and the screen is set to the login screen.

---
summary: Design system plan for pika-desktop, modeled on libcosmic's token architecture
read_when:
  - working on pika-desktop theming or design tokens
  - adding new widget styles or components to the desktop app
---

# Pika Desktop Design System Plan

A strategy and reference for building a design system for `pika-desktop` that draws from
libcosmic's architecture while covering the specific needs of a messaging app.

---

## 1. Why libcosmic as a reference (not a dependency)

Libcosmic is the most mature iced-based design system in production. It solves real problems:
tokenized theming, density scaling, state-aware widget styling, and a layered color system.
But it is tightly coupled to the COSMIC desktop (D-Bus, XDG icons, Wayland popups,
`cosmic-config`, header-bar window chrome). Pulling it in as a dependency would be heavy
and wrong for a cross-platform chat app.

**The approach**: Study libcosmic's *design token architecture* and *component state model*,
then implement a slim, pika-specific version on top of raw iced 0.14. No libcosmic crate
dependency. Just the ideas.

---

## 2. Current state of pika-desktop styling

Today the desktop app has:

- **`theme.rs`** (388 lines): 21 color constants, ~15 container style functions, ~6 button
  style functions, 1 input style. All hardcoded `Color` values.
- **Spacing**: Hardcoded pixel values scattered across every view (8, 12, 16, 24...).
- **Typography**: Two fonts (Ubuntu Sans Mono, Noto Color Emoji). Font sizes are magic
  numbers (10, 12, 14, 16, 20, 22, 36).
- **Border radii**: Hardcoded per-widget (4, 6, 8, 12, 100).
- **No density or light-mode support**. Dark-only, single density.

This works but makes it hard to iterate on the look, add light mode, or keep views
consistent as the app grows.

---

## 3. Design token system

Model after libcosmic's `cosmic-theme` crate. Define a single `PikaTheme` struct that
holds every design decision as a token.

### 3.1 Color tokens

Libcosmic uses a three-layer system: **Container** (surface) → **Component** (interactive
widget) → **state** (hover, pressed, selected, disabled, focus). Each Container holds its
own on-color, divider color, and nested Component. This is the right model for a chat app
where message bubbles, the rail, and overlay sheets all live at different visual depths.

```
PikaTheme
├── background: Surface       // app background (the "canvas")
│   ├── base: Color           // e.g. #1a1a2e
│   ├── on: Color             // text on this surface
│   ├── divider: Color
│   └── component: Component  // widgets sitting on this surface
├── primary: Surface          // elevated surfaces (rail, header, input bar)
├── secondary: Surface        // popovers, sheets, pickers
│
├── accent: Component         // primary interactive color (blue)
├── success: Component        // green (call accept, online indicator)
├── danger: Component         // red (destructive actions, call decline)
├── warning: Component        // orange/amber (muted indicator, failed delivery)
│
├── sent_bubble: BubbleStyle  // sent message bubble
├── received_bubble: BubbleStyle // received message bubble
│
├── spacing: Spacing
├── radii: Radii
├── typography: Typography
└── is_dark: bool
```

**Component** (mirrors libcosmic's `Component`):
```
Component
├── base: Color       // default
├── hover: Color      // mouse over
├── pressed: Color    // active press
├── selected: Color   // selected state
├── disabled: Color   // disabled
├── on: Color         // text/icon on this component
├── border: Color     // border color
└── focus: Color      // keyboard focus ring
```

**BubbleStyle** (chat-specific, no libcosmic equivalent):
```
BubbleStyle
├── background: Color
├── on: Color             // text color
├── on_secondary: Color   // timestamp, delivery indicator
├── link: Color           // inline links
├── radius: [f32; 4]      // per-corner for grouping (single/first/middle/last)
```

This eliminates the 21 loose color constants and makes light mode a matter of providing
a second `PikaTheme` instance.

### 3.2 Spacing scale

Adopt libcosmic's named scale directly. It's sane and has density support:

| Token      | Standard | Compact | Spacious |
|------------|----------|---------|----------|
| `none`     | 0        | 0       | 0        |
| `xxxs`     | 4        | 4       | 4        |
| `xxs`      | 8        | 8       | 8        |
| `xs`       | 12       | 12      | 12       |
| `s`        | 16       | 16      | 16       |
| `m`        | 24       | 16      | 32       |
| `l`        | 32       | 32      | 32       |
| `xl`       | 48       | 48      | 48       |

Views should never contain bare pixel literals. Always `theme.spacing.xs` etc.

### 3.3 Corner radii

| Token      | Value  | Usage                                   |
|------------|--------|-----------------------------------------|
| `none`     | 0      | sharp edges                             |
| `xs`       | 4      | small chips, badges                     |
| `s`        | 8      | buttons, inputs                         |
| `m`        | 12     | cards, message bubbles (default)        |
| `l`        | 16     | containers, sheets                      |
| `xl`       | 24     | large cards                             |
| `full`     | 9999   | circles (avatars), pills                |

Message bubble grouped radii follow iOS's pattern: 12pt for outer corners, 4pt for
the corner adjacent to the next message in a group.

### 3.4 Typography scale

Two fonts are fine. Define named sizes instead of magic numbers:

| Token        | Size | Weight   | Usage                              |
|--------------|------|----------|------------------------------------|
| `display`    | 28   | Bold     | login hero text                    |
| `title`      | 20   | Semibold | view titles, conversation header   |
| `headline`   | 16   | Semibold | section headers, group name        |
| `body`       | 14   | Regular  | messages, descriptions, inputs     |
| `body_strong`| 14   | Semibold | sender names, labels               |
| `caption`    | 12   | Regular  | timestamps, secondary info         |
| `small`      | 10   | Regular  | badges, delivery indicators        |

Emoji size should be `body + 2` for inline, `body + 4` for reaction chips.

---

## 4. Component library

What libcosmic provides vs. what pika needs, organized by category.

### 4.1 Atoms (lowest-level building blocks)

| Component         | libcosmic equivalent     | Pika notes                                  |
|-------------------|--------------------------|---------------------------------------------|
| **PikaButton**    | `button::text/icon/link` | Primary, secondary, danger, ghost variants.  |
| **PikaTextInput** | `text_input`, `search_input`, `secure_input` | Standard, search (with icon), secure (nsec). |
| **PikaIcon**      | `icon::from_name`        | Embed SVG icons. No XDG themes needed.       |
| **PikaAvatar**    | (none)                   | Already exists. Add status dot (online/call).|
| **PikaBadge**     | (none)                   | Unread count pill. Accent bg, white text.    |
| **PikaDivider**   | `divider::horizontal`    | 1px line using surface divider color.        |
| **PikaToggler**   | `toggler`                | For settings if we add them.                 |

### 4.2 Molecules (composed from atoms)

| Component              | libcosmic equivalent     | Pika notes                                                    |
|------------------------|--------------------------|---------------------------------------------------------------|
| **ChatListItem**       | `segmented_button` entity| Avatar + name + preview + time + badge. Selected/hover states.|
| **MessageBubble**      | (none)                   | Sent/received. Grouped corners. Reply preview. Reactions.     |
| **ReactionChip**       | (none)                   | Emoji + count. Highlighted if user reacted.                   |
| **EmojiPicker**        | (none)                   | Quick-react bar (6-8 emojis) + future full grid.              |
| **MentionPicker**      | (none)                   | Autocomplete list triggered by `@`.                           |
| **InputBar**           | (none)                   | Text input + send button + attachment + reply preview.        |
| **ToastBar**           | `Toast` / `Toaster`      | Follow libcosmic: auto-dismiss, max N visible, action button. |
| **HeaderBar**          | `HeaderBar`              | Simplified: no window chrome, just title + action buttons.    |
| **ProfileCard**        | `settings::section`      | Avatar + name + about + npub. Reuse for my-profile, peer.    |
| **MemberRow**          | `settings::item`         | Avatar + name + role badge. Optional remove action.           |
| **CallBanner**         | (none)                   | Incoming call strip. Accept/decline buttons.                  |
| **CallControls**       | (none)                   | Mute/camera/end row with icon buttons.                        |

### 4.3 Organisms (full screen sections)

| Component              | libcosmic equivalent     | Pika notes                                        |
|------------------------|--------------------------|---------------------------------------------------|
| **ChatRail**           | `NavBar`                 | Fixed-width sidebar. Scrollable list + buttons.   |
| **ConversationView**   | (none)                   | Header + message list + input bar.                |
| **LoginScreen**        | (none)                   | Centered card with form.                          |
| **ProfileSheet**       | `ContextDrawer`          | Slide-over or inline. My profile / peer profile.  |
| **GroupInfoSheet**     | `ContextDrawer`          | Member list, rename, add member, leave.           |
| **NewChatSheet**       | (none)                   | Search/select contact, manual npub entry.         |
| **NewGroupSheet**      | (none)                   | Name + member chips + search.                     |
| **CallScreen**         | (none)                   | Full overlay. Avatar/video + controls + timer.    |

---

## 5. Styling strategy

### 5.1 Style functions, not style structs

Libcosmic uses two patterns for styling:
1. **Closure-based**: `Button::Custom(Box<dyn Fn(&Theme, Status) -> Appearance>)` for
   one-off styles.
2. **Enum-based**: Style enums like `button::Style::Suggested` that look up colors from
   the theme.

For pika, prefer **theme method functions** that take the widget status and return an
`Appearance`. This is what `theme.rs` already does, just formalize it:

```rust
impl PikaTheme {
    // Buttons
    pub fn button_primary(&self, status: button::Status) -> button::Style { ... }
    pub fn button_secondary(&self, status: button::Status) -> button::Style { ... }
    pub fn button_danger(&self, status: button::Status) -> button::Style { ... }
    pub fn button_ghost(&self, status: button::Status) -> button::Style { ... }

    // Containers
    pub fn surface(&self) -> container::Style { ... }        // background layer
    pub fn rail(&self) -> container::Style { ... }           // primary layer
    pub fn card(&self) -> container::Style { ... }           // elevated card
    pub fn bubble_sent(&self) -> container::Style { ... }
    pub fn bubble_received(&self) -> container::Style { ... }

    // Inputs
    pub fn text_input(&self, status: text_input::Status) -> text_input::Style { ... }
}
```

Views call `theme.button_primary` instead of `primary_button_style`. The theme object
carries all the tokens, so changing the palette changes everything.

### 5.2 Layer-aware rendering

Adopt libcosmic's layer concept: widgets should know what surface they sit on so they
can pick the right `on` (foreground) color. The `PikaTheme` carries a `current_layer`
that views set when entering a new surface:

```rust
let rail_theme = theme.with_layer(Layer::Primary);
// Now rail_theme.on_color() returns primary.on, not background.on
```

This makes it trivial to add a light mode later — each layer's colors are defined once
in the theme.

### 5.3 What to borrow from iOS

The iOS SwiftUI code uses semantic SwiftUI colors (`.secondary`, `.tertiary`) and material
backgrounds (`.ultraThinMaterial`). Map these concepts:

| iOS pattern                | Pika desktop token              |
|----------------------------|---------------------------------|
| `.primary` foreground      | `surface.on`                    |
| `.secondary` foreground    | `surface.on` at 60% opacity     |
| `.tertiary` foreground     | `surface.on` at 40% opacity     |
| `.ultraThinMaterial`       | `secondary.base` (semi-transparent if frosted glass lands in iced) |
| `Color.blue`               | `accent.base`                   |
| `Color.red` / destructive  | `danger.base`                   |
| `Color.green` / call accept| `success.base`                  |
| `.gray.opacity(0.2)`       | `received_bubble.background`    |

---

## 6. Feature gap: what needs to be built (not in libcosmic)

These are the chat-specific components that have no libcosmic or iced built-in equivalent.
They are the core of the design system work.

### 6.1 Message timeline

The scrollable message list with:
- **Date separators** between day boundaries.
- **Grouped bubbles**: consecutive messages from the same sender within a time window
  share a visual group (smaller inter-message gap, adjusted corner radii).
- **Scroll anchoring**: when new messages arrive while scrolled up, don't jump to bottom.
  Show a "scroll to bottom" pill.
- **Lazy rendering**: only render visible messages. Iced's `lazy()` + `scrollable()` can
  handle this but needs care with variable-height items.

Reference: iOS groups messages and uses `UnevenRoundedRectangle` for per-corner radii.
The desktop should do the same with iced's `border::rounded()`.

### 6.2 Rich message content

iOS supports:
- **Markdown** in messages (via MarkdownUI library)
- **Image thumbnails** with download overlay and fullscreen viewer
- **Voice messages** with waveform + play/pause + duration
- **File attachments** with icon + name + download button
- **Reply quotes** with colored sidebar and sender name

For the desktop, build these as composable `Element`-returning functions that live in a
`message_content` module. Start with text + reply quotes + image thumbnails. Voice and
file attachments can come later.

### 6.3 Emoji reactions

Already implemented on desktop. Formalize the visual treatment:
- Reaction chip: `received_bubble.background` bg, `caption` text, `radii.full` corners.
- User's own reaction: `accent.base` at 20% opacity bg, `accent.on` text.
- Quick-react bar: 6 emojis in a horizontal row, same bg as received bubble.
- Long-term: full emoji picker grid (categorized, searchable). This is a significant
  widget — consider a standalone module or crate.

### 6.4 Input bar

Composite widget:
- Optional reply preview (colored sidebar + dismiss button)
- Text input (auto-grow would be ideal but iced `text_input` is single-line;
  `text_editor` is multi-line but heavier — evaluate tradeoffs)
- Media attach button (left)
- Send button (right, accent color, only enabled when input non-empty)
- Mention autocomplete overlay (positioned above input)

### 6.5 Call UI

- Full-screen overlay with its own color context (darker background)
- Avatar or video frame centered
- Control bar at bottom (icon buttons: mute, camera, end)
- Timer display (monospace digits)
- Incoming call banner (top bar, green bg, accept/decline)

---

## 7. Migration path

### Phase 1: Token extraction

1. Create `src/design/tokens.rs` with the `PikaTheme`, `Surface`, `Component`,
   `BubbleStyle`, `Spacing`, `Radii`, `Typography` structs.
2. Create `PikaTheme::dark_default()` that reproduces today's exact look using the
   existing 21 color constants.
3. Replace bare color constants in `theme.rs` style functions with token lookups.
4. Replace bare pixel values in views with `theme.spacing.*` and `theme.radii.*`.

**Goal**: Zero visual change. Just structured access to the same values.

### Phase 2: Component formalization

1. Move each style function into a method on `PikaTheme`.
2. Extract reusable view helpers into `src/design/components/` — start with
   `avatar.rs`, `badge.rs`, `button.rs`, `toast.rs`.
3. Each component is a function `fn pika_button(..., theme: &PikaTheme) -> Element`
   or a builder struct.

### Phase 3: Light mode

1. Create `PikaTheme::light_default()` with inverted surface/text colors.
2. Wire up a toggle in settings or my-profile.
3. Because all views read from the theme, this should mostly Just Work.

### Phase 4: Chat-specific components

1. Formalize `MessageBubble` as a proper component with grouped-corner logic.
2. Build `MessageTimeline` with date separators, grouping, scroll anchoring.
3. Build `InputBar` as a composite component.
4. Build `EmojiPicker` (grid mode).

### Phase 5: Polish and density

1. Add `Density` enum (compact / standard) that adjusts `spacing.m`.
2. Audit touch targets — minimum 32px for compact, 44px for standard.
3. Add transitions/animations where iced supports them (opacity fades for
   toast appear/dismiss, hover state transitions).

---

## 8. File structure

```
crates/pika-desktop/src/
├── design/
│   ├── mod.rs              // re-exports
│   ├── tokens.rs           // PikaTheme, Surface, Component, Spacing, Radii, Typography
│   ├── styles.rs           // PikaTheme methods (button_primary, surface, bubble_sent, etc.)
│   └── components/
│       ├── mod.rs
│       ├── avatar.rs       // avatar circle with optional status dot
│       ├── badge.rs        // unread count pill
│       ├── button.rs       // pika_button(), pika_icon_button()
│       ├── chat_list_item.rs
│       ├── message_bubble.rs
│       ├── reaction_chip.rs
│       ├── input_bar.rs
│       ├── toast.rs
│       ├── header_bar.rs
│       ├── member_row.rs
│       └── call_controls.rs
├── screen/
│   ├── mod.rs
│   ├── home.rs             // main 3-pane layout
│   └── login.rs            // login card
├── views/                  // screen sub-sections (conversation, group_info, etc.)
│   └── ...
└── main.rs
```

The `design/` module is the design system. `screen/` and `views/` consume it.
Views should never import `iced::Color` directly — only `design::PikaTheme`.

---

## 9. Key libcosmic patterns to adopt

| Pattern | What it is | How to adapt |
|---------|-----------|--------------||
| **Layered surfaces** | `background` -> `primary` -> `secondary`, each with its own `on` color | Same structure, three layers is enough for a chat app |
| **Component state model** | Every interactive element has base/hover/pressed/selected/disabled/focus colors | Apply to buttons, chat list items, reaction chips |
| **Spacing scale with density** | Named tokens (xxs through xl), density adjusts `m` | Copy the scale directly |
| **Settings section/item** | Pre-composed section > item pattern for preferences pages | Use for group-info member list, profile fields |
| **Toast system** | Auto-dismiss, max visible count, action button | Replace current toast with this pattern |
| **Nav bar model** | Entity-based model with data attachment | Chat rail is conceptually a nav bar with richer items |

### Patterns to skip

| Pattern | Why skip |
|---------|----------|
| **XDG icon themes** | Cross-platform app, embed SVGs directly |
| **cosmic-config** | Pika has its own config system in pika_core |
| **D-Bus single instance** | Not needed for chat app |
| **Header bar window chrome** | Use native window decorations |
| **Wayland popup positioning** | Cross-platform, use iced overlays |

---

## 10. Reference: iOS features -> desktop component mapping

| iOS feature | iOS component | Desktop component | Priority |
|-------------|---------------|-------------------|----------|
| Chat list | `ChatListView` | `ChatRail` + `ChatListItem` | Done (exists) |
| Message timeline | `ChatView` scrollable | `ConversationView` + `MessageTimeline` | High |
| Message bubbles | grouped `MessageRow` | `MessageBubble` with grouped corners | High |
| Text input + send | `TextEditor` + send button | `InputBar` composite | High |
| Reply preview | blue bar + snippet | Reply component in `InputBar` | High |
| Reactions | quick bar + picker sheet | `ReactionChip` + `EmojiPicker` | Medium |
| Typing indicator | animated dots | Text-based indicator | Medium |
| Image thumbnails | `AsyncImage` + download | Image widget + download overlay | Medium |
| Fullscreen image | `FullscreenImageViewer` | Modal overlay with zoom | Medium |
| Voice messages | `VoiceMessageView` waveform | Waveform widget + playback | Low |
| Voice recording | `VoiceRecordingView` | Recording UI + waveform | Low |
| Audio/video calls | `CallScreenView` | `CallScreen` overlay | Done (exists) |
| Call banner | incoming call pill | `CallBanner` strip | Done (exists) |
| Login | `LoginView` | `LoginScreen` | Done (exists) |
| My profile | `MyNpubQrSheet` | `MyProfileView` | Done (exists) |
| Peer profile | `PeerProfileSheet` | `PeerProfileView` | Done (exists) |
| Group info | `GroupInfoView` | `GroupInfoView` | Done (exists) |
| New chat | `NewChatView` | `NewChatView` | Done (exists) |
| New group | `NewGroupChatView` | `NewGroupChatView` | Done (exists) |
| QR scanner | `QrScannerSheet` | Not applicable (desktop) | Skip |
| Push notifications | `NotificationSettingsView` | System notifications (future) | Low |
| Media picker | `PhotosPicker` | Native file dialog | Low |
| Mention autocomplete | member list popup | `MentionPicker` | Done (exists) |

---

## 11. Open questions

1. **Should we support system-native dark/light detection?** Iced can query the system
   theme on macOS/Windows. Worth wiring up.
2. **Multi-line message input?** iOS uses a growing `TextEditor`. Iced's `text_editor` is
   available but heavier than `text_input`. Evaluate whether the UX benefit justifies it.
3. **Frosted glass / blur?** iOS uses `.ultraThinMaterial` extensively. Iced doesn't have
   native blur support. Can approximate with semi-transparent backgrounds. Not critical.
4. **Accessibility audit?** Libcosmic has high-contrast mode and density scaling. Worth
   planning for but not blocking on.
5. **Animation?** Iced has limited animation support. Toast fade-in/out and hover
   transitions are achievable. Complex animations (typing dots) may need custom widgets.

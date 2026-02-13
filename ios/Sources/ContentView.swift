import SwiftUI

struct ContentView: View {
    @Bindable var manager: AppManager
    @State private var visibleToast: String? = nil
    @State private var navPath: [Screen] = []

    var body: some View {
        let router = manager.state.router
        Group {
            switch router.defaultScreen {
            case .login:
                LoginView(manager: manager)
            default:
                NavigationStack(path: $navPath) {
                    screenView(manager: manager, screen: router.defaultScreen)
                        .navigationDestination(for: Screen.self) { screen in
                            screenView(manager: manager, screen: screen)
                        }
                }
                .onAppear {
                    // Initial mount: seed the path from Rust.
                    navPath = manager.state.router.screenStack
                }
                // Drive native navigation from Rust's router, but avoid feeding those changes
                // back to Rust as "platform pops".
                .onChange(of: manager.state.router.screenStack) { _, new in
                    navPath = new
                }
                .onChange(of: navPath) { old, new in
                    // Ignore Rust-driven syncs.
                    if new == manager.state.router.screenStack { return }
                    // Only report platform-initiated pops (e.g. swipe-back).
                    if new.count < old.count {
                        manager.dispatch(.updateScreenStack(stack: new))
                    }
                }
            }
        }
        .overlay(alignment: .top) {
            if let toast = visibleToast {
                Text(toast)
                    .font(.subheadline.weight(.medium))
                    .foregroundStyle(.white)
                    .padding(.horizontal, 16)
                    .padding(.vertical, 10)
                    .background(.black.opacity(0.82), in: RoundedRectangle(cornerRadius: 10))
                    .padding(.horizontal, 24)
                    .padding(.top, 8)
                    .transition(.move(edge: .top).combined(with: .opacity))
                    .accessibilityIdentifier("pika_toast")
                    .onTapGesture { withAnimation { visibleToast = nil } }
                    .allowsHitTesting(true)
            }
        }
        .animation(.easeInOut(duration: 0.25), value: visibleToast)
        .onChange(of: manager.state.toast) { _, new in
            guard let message = new else { return }
            // Show the non-blocking overlay and immediately clear Rust state so it
            // doesn't re-show on state resync. The overlay manages its own lifetime.
            withAnimation { visibleToast = message }
            manager.dispatch(.clearToast)
            // Auto-dismiss after 3 seconds.
            let captured = message
            Task { @MainActor in
                try? await Task.sleep(for: .seconds(3))
                withAnimation {
                    if visibleToast == captured { visibleToast = nil }
                }
            }
        }
    }
}

@ViewBuilder
private func screenView(manager: AppManager, screen: Screen) -> some View {
    switch screen {
    case .login:
        LoginView(manager: manager)
    case .chatList:
        ChatListView(manager: manager)
    case .newChat:
        NewChatView(manager: manager)
    case .chat(let chatId):
        ChatView(manager: manager, chatId: chatId)
    }
}

#if DEBUG
#Preview("Logged Out") {
    ContentView(manager: PreviewFactory.manager(PreviewAppState.loggedOut))
}

#Preview("Chat List") {
    ContentView(manager: PreviewFactory.manager(PreviewAppState.chatListPopulated))
}
#endif

import SwiftUI

struct LoginView: View {
    let state: LoginViewState
    let onCreateAccount: @MainActor () -> Void
    let onLogin: @MainActor (String) -> Void
    let onBunkerLogin: @MainActor (String) -> Void
    let onNostrConnectLogin: @MainActor () -> Void
    let onResetNostrConnectPairing: @MainActor () -> Void
    @State private var nsecInput = ""
    @State private var bunkerUriInput = ""

    var body: some View {
        let createBusy = state.creatingAccount
        let loginBusy = state.loggingIn
        let anyBusy = createBusy || loginBusy

        VStack(spacing: 0) {
            Spacer()

            Image("PikaLogo")
                .resizable()
                .scaledToFit()
                .frame(width: 140, height: 140)
                .clipShape(RoundedRectangle(cornerRadius: 28))

            Text("Pika")
                .font(.largeTitle.weight(.bold))
                .padding(.top, 16)

            Text("Encrypted messaging over Nostr")
                .font(.subheadline)
                .foregroundStyle(.secondary)
                .padding(.top, 4)

            Spacer()

            VStack(spacing: 12) {
                Button {
                    onCreateAccount()
                } label: {
                    if createBusy {
                        ProgressView()
                            .tint(.white)
                            .frame(maxWidth: .infinity)
                    } else {
                        Text("Create Account")
                            .frame(maxWidth: .infinity)
                    }
                }
                .buttonStyle(.borderedProminent)
                .controlSize(.large)
                .disabled(anyBusy)
                .accessibilityIdentifier(TestIds.loginCreateAccount)

                HStack {
                    Rectangle()
                        .frame(height: 1)
                        .foregroundStyle(.quaternary)
                    Text("or")
                        .font(.caption)
                        .foregroundStyle(.tertiary)
                    Rectangle()
                        .frame(height: 1)
                        .foregroundStyle(.quaternary)
                }
                .padding(.vertical, 4)

                TextField("Enter your nsec", text: $nsecInput)
                    .textInputAutocapitalization(.never)
                    .autocorrectionDisabled()
                    .textFieldStyle(.roundedBorder)
                    .disabled(anyBusy)
                    .accessibilityIdentifier(TestIds.loginNsecInput)

                Button {
                    onLogin(nsecInput)
                } label: {
                    if loginBusy {
                        ProgressView()
                            .frame(maxWidth: .infinity)
                    } else {
                        Text("Log In")
                            .frame(maxWidth: .infinity)
                    }
                }
                .buttonStyle(.bordered)
                .controlSize(.large)
                .disabled(anyBusy || nsecInput.isEmpty)
                .accessibilityIdentifier(TestIds.loginSubmit)

                TextField("Enter bunker URI", text: $bunkerUriInput)
                    .textInputAutocapitalization(.never)
                    .autocorrectionDisabled()
                    .textFieldStyle(.roundedBorder)
                    .disabled(anyBusy)
                    .accessibilityIdentifier(TestIds.loginBunkerUriInput)

                Button {
                    onBunkerLogin(bunkerUriInput)
                } label: {
                    if loginBusy {
                        ProgressView()
                            .frame(maxWidth: .infinity)
                    } else {
                        Text("Log In with Bunker")
                            .frame(maxWidth: .infinity)
                    }
                }
                .buttonStyle(.bordered)
                .controlSize(.large)
                .disabled(anyBusy || bunkerUriInput.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty)
                .accessibilityIdentifier(TestIds.loginBunkerSubmit)

                Button {
                    onNostrConnectLogin()
                } label: {
                    if loginBusy {
                        ProgressView()
                            .frame(maxWidth: .infinity)
                    } else {
                        Text("Log In with Nostr Connect")
                            .frame(maxWidth: .infinity)
                    }
                }
                .buttonStyle(.borderedProminent)
                .controlSize(.large)
                .disabled(anyBusy)
                .accessibilityIdentifier(TestIds.loginNostrConnectSubmit)

                Button("Reset Nostr Connect Pairing") {
                    onResetNostrConnectPairing()
                }
                .buttonStyle(.bordered)
                .controlSize(.regular)
                .accessibilityIdentifier(TestIds.loginNostrConnectReset)
            }
            .padding(.bottom, 32)
        }
        .padding(.horizontal, 28)
    }
}

#if DEBUG
#Preview("Login") {
    LoginView(
        state: LoginViewState(creatingAccount: false, loggingIn: false),
        onCreateAccount: {},
        onLogin: { _ in },
        onBunkerLogin: { _ in },
        onNostrConnectLogin: {},
        onResetNostrConnectPairing: {}
    )
}

#Preview("Login - Busy") {
    LoginView(
        state: LoginViewState(creatingAccount: false, loggingIn: true),
        onCreateAccount: {},
        onLogin: { _ in },
        onBunkerLogin: { _ in },
        onNostrConnectLogin: {},
        onResetNostrConnectPairing: {}
    )
}

#Preview("Login - Creating") {
    LoginView(
        state: LoginViewState(creatingAccount: true, loggingIn: false),
        onCreateAccount: {},
        onLogin: { _ in },
        onBunkerLogin: { _ in },
        onNostrConnectLogin: {},
        onResetNostrConnectPairing: {}
    )
}
#endif

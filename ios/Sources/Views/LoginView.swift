import SwiftUI

struct LoginView: View {
    let manager: AppManager
    @State private var nsecInput = ""

    var body: some View {
        let busy = manager.state.busy
        let createBusy = busy.creatingAccount
        let loginBusy = busy.loggingIn
        let anyBusy = createBusy || loginBusy

        VStack(spacing: 16) {
            Text("Pika")
                .font(.largeTitle.weight(.semibold))

            Button {
                manager.dispatch(.createAccount)
            } label: {
                if createBusy {
                    ProgressView()
                        .tint(.white)
                } else {
                    Text("Create Account")
                }
            }
            .buttonStyle(.borderedProminent)
            .disabled(anyBusy)
            .accessibilityIdentifier(TestIds.loginCreateAccount)

            Divider().padding(.vertical, 8)

            TextField("nsec (mock)", text: $nsecInput)
                .textInputAutocapitalization(.never)
                .autocorrectionDisabled()
                .textFieldStyle(.roundedBorder)
                .disabled(anyBusy)
                .accessibilityIdentifier(TestIds.loginNsecInput)

            Button {
                manager.login(nsec: nsecInput)
            } label: {
                if loginBusy {
                    ProgressView()
                } else {
                    Text("Login")
                }
            }
            .buttonStyle(.bordered)
            .disabled(anyBusy)
            .accessibilityIdentifier(TestIds.loginSubmit)
        }
        .padding(20)
    }
}

#if DEBUG
#Preview("Login") {
    LoginView(manager: PreviewFactory.manager(PreviewAppState.loggedOut))
}

#Preview("Login - Busy") {
    LoginView(manager: PreviewFactory.manager(PreviewAppState.loggingIn))
}
#endif

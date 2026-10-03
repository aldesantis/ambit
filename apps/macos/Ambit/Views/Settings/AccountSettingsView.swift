import SwiftUI

/// The GitHub account pane: who is signed in, sign in, sign out.
struct AccountSettingsView: View {
    @Environment(AppModel.self) private var model
    @State private var showingSignIn = false
    @State private var confirmingSignOut = false

    var body: some View {
        let account = model.account

        Form {
            Section {
                if !account.isAvailable {
                    LabeledContent("GitHub") {
                        Text("Sign-in unavailable")
                            .foregroundStyle(.secondary)
                    }
                    Text(GitHubAuthError.unavailable.localizedDescription)
                        .font(.callout)
                        .foregroundStyle(.secondary)
                        .accessibilityIdentifier("account.unavailable")
                } else if let username = account.username {
                    LabeledContent("Signed in as") {
                        Text(username)
                            .accessibilityIdentifier("account.username")
                    }
                    HStack {
                        Button("Sign In Again…") { showingSignIn = true }
                        Spacer()
                        Button("Sign Out…") { confirmingSignOut = true }
                            .accessibilityIdentifier("account.signOut")
                    }
                } else {
                    LabeledContent("GitHub") {
                        Text("Not signed in")
                            .foregroundStyle(.secondary)
                            .accessibilityIdentifier("account.signedOut")
                    }
                    Button("Sign In with GitHub…") { showingSignIn = true }
                        .accessibilityIdentifier("account.signIn")
                }
            } header: {
                Text("GitHub")
            } footer: {
                Text("Signing in lets Ambit read private catalogs on GitHub.com. Public and local catalogs work without it.")
                    .foregroundStyle(.secondary)
            }

            if case let .error(error) = account.state, !showingSignIn {
                Section {
                    Label(error.localizedDescription, systemImage: "exclamationmark.triangle")
                        .foregroundStyle(.secondary)
                    Button("Dismiss") { account.dismissError() }
                }
            }
        }
        .formStyle(.grouped)
        .gitHubSignInSheet(isPresented: $showingSignIn, account: account)
        .confirmationDialog("Sign out of GitHub?", isPresented: $confirmingSignOut) {
            Button("Sign Out", role: .destructive) {
                Task { await account.signOut() }
            }
            .accessibilityIdentifier("account.confirmSignOut")
        } message: {
            Text("Private GitHub catalogs stop loading until you sign in again. Installed capabilities, configurations and cached catalogs stay.")
        }
    }
}

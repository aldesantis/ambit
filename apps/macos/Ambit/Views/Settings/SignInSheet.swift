import AppKit
import SwiftUI

extension View {
    /// Presents the GitHub sign-in sheet, which starts the device flow when it appears and closes
    /// itself once the user is signed in. Any view that offers "Sign In Again" uses this.
    func gitHubSignInSheet(isPresented: Binding<Bool>, account: GitHubAccountModel) -> some View {
        sheet(isPresented: isPresented) {
            SignInSheet(account: account)
        }
    }
}

/// Shows the device-flow code with Copy and Open GitHub, waits for authorization, and offers
/// Cancel. Closing the sheet any way cancels a sign-in still in progress.
struct SignInSheet: View {
    let account: GitHubAccountModel
    @Environment(\.dismiss) private var dismiss
    @State private var copied = false

    var body: some View {
        VStack(alignment: .leading, spacing: 16) {
            Text("Sign in with GitHub")
                .font(.title2.bold())

            content
                .frame(maxWidth: .infinity, alignment: .leading)

            HStack {
                Spacer()
                if case .error(let error) = account.state, error != .unavailable {
                    Button("Try Again") { account.signIn() }
                        .accessibilityIdentifier("signIn.retry")
                }
                Button(account.isSigningIn ? "Cancel" : "Close") {
                    account.cancelSignIn()
                    account.dismissError()
                    dismiss()
                }
                .keyboardShortcut(.cancelAction)
                .accessibilityIdentifier("signIn.cancel")
            }
        }
        .padding(20)
        .frame(width: 420)
        .onAppear {
            if !account.isSigningIn {
                account.signIn()
            }
        }
        .onDisappear {
            account.cancelSignIn()
        }
        .onChange(of: account.state) { _, state in
            if case .signedIn = state {
                dismiss()
            }
        }
    }

    @ViewBuilder private var content: some View {
        switch account.state {
        case .requestingCode, .signedOut, .signedIn:
            HStack(spacing: 8) {
                ProgressView().controlSize(.small)
                Text("Contacting GitHub…")
            }
        case let .awaitingUser(code, verificationURL, expiresAt):
            VStack(alignment: .leading, spacing: 12) {
                Text("Enter this code on GitHub to let Ambit read your repositories:")
                HStack(spacing: 12) {
                    Text(code)
                        .font(.system(.largeTitle, design: .monospaced).weight(.semibold))
                        .textSelection(.enabled)
                        .accessibilityIdentifier("signIn.code")
                    Button(copied ? "Copied" : "Copy", systemImage: copied ? "checkmark" : "doc.on.doc") {
                        NSPasteboard.general.clearContents()
                        NSPasteboard.general.setString(code, forType: .string)
                        copied = true
                    }
                    .accessibilityIdentifier("signIn.copy")
                }
                Button("Open GitHub") {
                    NSWorkspace.shared.open(verificationURL)
                }
                .buttonStyle(.borderedProminent)
                .accessibilityIdentifier("signIn.openGitHub")
                HStack(spacing: 8) {
                    ProgressView().controlSize(.small)
                    Text("Waiting for authorization. The code expires \(expiresAt, style: .relative).")
                        .foregroundStyle(.secondary)
                }
                .font(.callout)
            }
        case let .error(error):
            Label(error.localizedDescription, systemImage: "exclamationmark.triangle")
                .accessibilityIdentifier("signIn.error")
        }
    }
}

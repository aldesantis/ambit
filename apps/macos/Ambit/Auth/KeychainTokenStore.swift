import Foundation
import Security
import Synchronization

/// Where the signed-in account lives between launches. Holds at most one account.
protocol GitHubTokenStore: Sendable {
    func load() throws -> GitHubAccount?
    /// Replaces any stored account.
    func save(_ account: GitHubAccount) throws
    /// Succeeds when nothing is stored.
    func delete() throws
}

/// A generic password item: the service names the app, the account attribute holds the GitHub
/// username, and the data holds the token.
///
/// The item lives in the login (file-based) keychain. The data protection keychain needs a
/// keychain access group entitlement, which ad-hoc signed builds cannot carry.
struct KeychainTokenStore: GitHubTokenStore {
    static let defaultService = "com.nebulab.ambit.github"

    var service: String = Self.defaultService

    func load() throws -> GitHubAccount? {
        let query: [CFString: Any] = [
            kSecClass: kSecClassGenericPassword,
            kSecAttrService: service,
            kSecMatchLimit: kSecMatchLimitOne,
            kSecReturnAttributes: true,
            kSecReturnData: true,
        ]
        var result: CFTypeRef?
        let status = SecItemCopyMatching(query as CFDictionary, &result)
        if status == errSecItemNotFound {
            return nil
        }
        try Self.check(status)

        guard let item = result as? [CFString: Any], let username = item[kSecAttrAccount] as? String,
            let data = item[kSecValueData] as? Data, let token = String(data: data, encoding: .utf8), !token.isEmpty
        else {
            return nil
        }
        return GitHubAccount(username: username, token: GitHubToken(token))
    }

    func save(_ account: GitHubAccount) throws {
        try delete()

        let attributes: [CFString: Any] = [
            kSecClass: kSecClassGenericPassword,
            kSecAttrService: service,
            kSecAttrAccount: account.username,
            kSecAttrLabel: "Ambit GitHub sign-in",
            kSecValueData: Data(account.token.value.utf8),
        ]
        try Self.check(SecItemAdd(attributes as CFDictionary, nil))
    }

    func delete() throws {
        // Without a match limit, SecItemDelete removes every item of the service.
        let query: [CFString: Any] = [kSecClass: kSecClassGenericPassword, kSecAttrService: service]
        let status = SecItemDelete(query as CFDictionary)
        if status == errSecItemNotFound {
            return
        }
        try Self.check(status)
    }

    private static func check(_ status: OSStatus) throws {
        guard status != errSecSuccess else {
            return
        }
        let message = SecCopyErrorMessageString(status, nil) as String? ?? "OSStatus \(status)"
        throw GitHubAuthError.keychain(message)
    }
}

/// Keeps the account in memory only: unit tests, the unit test host, and UI tests with
/// `AMBIT_TEST_KEYCHAIN=memory`.
final class InMemoryTokenStore: GitHubTokenStore {
    private let account: Mutex<GitHubAccount?>

    init(_ account: GitHubAccount? = nil) {
        self.account = Mutex(account)
    }

    func load() throws -> GitHubAccount? { account.withLock { $0 } }
    func save(_ account: GitHubAccount) throws { self.account.withLock { $0 = account } }
    func delete() throws { account.withLock { $0 = nil } }
}

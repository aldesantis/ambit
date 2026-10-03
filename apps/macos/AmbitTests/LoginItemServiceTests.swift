import Foundation
import Testing

@testable import Ambit

@MainActor
struct LoginItemServiceTests {
    private let sandbox: ProjectRegistryTests.Sandbox
    private let control = InMemoryLoginItemControl()

    init() throws {
        sandbox = try ProjectRegistryTests.Sandbox()
    }

    private func service() -> LoginItemService {
        LoginItemService(control: control, store: AppStateStore(directory: sandbox.state))
    }

    @Test func firstRunEnablesLaunchAtLogin() {
        let service = service()

        service.applyFirstRunDefault()

        #expect(control.registerCount == 1)
        #expect(service.isEnabled)
        #expect(service.notice == nil)
        #expect(AppStateStore(directory: sandbox.state).state.launchAtLoginInitialized)
    }

    @Test func laterLaunchesNeverRegisterAgain() {
        service().applyFirstRunDefault()
        // The user turned it off in System Settings.
        control.status = .notRegistered

        let next = service()
        next.applyFirstRunDefault()

        #expect(control.registerCount == 1)
        #expect(!next.isEnabled)
        #expect(next.notice == "Launch at login is turned off in System Settings.")
        #expect(next.showsSystemSettingsAction)
    }

    @Test func theUsersOptOutPersists() {
        let first = service()
        first.applyFirstRunDefault()

        first.setEnabled(false)

        #expect(control.unregisterCount == 1)
        #expect(!first.isEnabled)
        #expect(first.notice == nil, "turned off by the user, nothing to explain")

        let next = service()
        next.applyFirstRunDefault()
        #expect(control.registerCount == 1)
        #expect(!next.isPreferred)
    }

    @Test func turningItBackOnRegisters() {
        let service = service()
        service.applyFirstRunDefault()
        service.setEnabled(false)

        service.setEnabled(true)

        #expect(control.registerCount == 2)
        #expect(service.isEnabled)
        #expect(service.isPreferred)
    }

    @Test func approvalRequiredIsShown() {
        control.statusAfterRegister = .requiresApproval
        let service = service()

        service.applyFirstRunDefault()

        #expect(service.isEnabled)
        #expect(service.notice?.contains("approval") == true)
        #expect(service.showsSystemSettingsAction)
        service.openSystemSettings()
        #expect(control.openedSystemSettings == 1)
    }

    @Test func aFailedFirstRegistrationIsReportedOnce() {
        struct Denied: LocalizedError {
            var errorDescription: String? { "Operation not permitted" }
        }
        control.registerError = Denied()
        let service = service()

        service.applyFirstRunDefault()

        #expect(service.lastError == "Operation not permitted")
        #expect(AppStateStore(directory: sandbox.state).state.launchAtLoginInitialized)
    }

    @Test func notFoundExplainsWithoutTheSettingsAction() {
        control.statusAfterRegister = .notFound
        let service = service()

        service.applyFirstRunDefault()

        #expect(service.notice != nil)
        #expect(!service.showsSystemSettingsAction)
    }

    @Test func olderStateFilesDefaultToEnabled() throws {
        let state = try JSONDecoder().decode(AppState.self, from: Data(#"{"version":1}"#.utf8))

        #expect(state.launchAtLoginPreferred)
        #expect(!state.launchAtLoginInitialized)
    }
}

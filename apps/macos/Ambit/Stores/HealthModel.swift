// The Health area: installation status and doctor findings of a setup's saved config, explained
// in plain language with the affected capability or tool and one next step.
//
// Everything here is read-only and offline. `refreshStatus` re-reads the config and the local
// install state; `checkHealth` runs the engine's doctor. Neither fetches catalogs, so neither can
// notice newer catalog revisions. Repairs go through the normal review of the saved config
// (`reviewSavedConfiguration`), never through a write of this model's own.
//
// The doctor inspects the app's own process environment, which can differ from the environment an
// agent tool starts with. The UI says so; nothing here reads shell startup files or asks for the
// values of environment variables.

import Foundation
import Observation

@MainActor
@Observable
final class HealthModel {
    private(set) var status: SetupStatus?
    private(set) var report: HealthReport?
    private(set) var statusReadAt: Date?
    private(set) var healthCheckedAt: Date?
    private(set) var isRefreshing = false
    private(set) var isChecking = false
    /// Why the last status read or health check failed.
    private(set) var error: EngineError?

    @ObservationIgnored private weak var setup: SetupModel?
    @ObservationIgnored private let tools: [AgentToolInfo]

    init(setup: SetupModel) {
        self.setup = setup
        tools = setup.engine.supportedAgentTools()
    }

    // MARK: Reading

    /// Re-reads the config and the installation state. Never fetches or checks for newer catalog
    /// revisions.
    func refreshStatus() async {
        guard let setup, !isRefreshing else {
            return
        }

        isRefreshing = true
        defer { isRefreshing = false }

        await setup.refresh()
        guard hasSavedConfig else {
            status = nil
            report = nil
            error = nil
            return
        }

        do {
            status = try await setup.session.status()
            statusReadAt = .now
            error = nil
        } catch {
            self.error = EngineError(error)
        }
    }

    /// Runs the doctor against the saved config. Offline and read-only.
    func checkHealth() async {
        guard let setup, !isChecking, hasSavedConfig else {
            return
        }

        isChecking = true
        defer { isChecking = false }

        do {
            let report = try await setup.session.health()
            self.report = report
            status = SetupStatus(items: report.items, artifacts: status?.artifacts ?? [])
            healthCheckedAt = .now
            statusReadAt = .now
            error = nil
        } catch {
            self.error = EngineError(error)
        }
    }

    var hasSavedConfig: Bool {
        if case .valid = setup?.snapshot?.config { true } else { false }
    }

    /// True while an operation of this setup runs, when reading would race its writes.
    var isBusy: Bool {
        guard let setup else {
            return false
        }
        return setup.operations.current?.setup == setup.id
    }

    // MARK: Repair

    /// Why the saved config cannot be reviewed again right now, or `nil` when it can.
    var reapplyBlockedReason: String? {
        guard let setup else {
            return ""
        }
        if !hasSavedConfig {
            return String(localized: "This setup has no saved configuration to reapply.")
        }
        if setup.hasPendingChanges {
            return String(
                localized:
                    "Apply or discard your pending changes first. Reapplying restores the saved configuration.")
        }
        if setup.operations.isBusy {
            return String(localized: "Wait for the running operation to finish.")
        }
        return nil
    }

    /// Opens the normal review of the saved config, so Apply restores what it installs.
    func reviewSavedConfiguration() async {
        guard let setup, reapplyBlockedReason == nil else {
            return
        }
        await setup.startReview()
    }

    // MARK: Derived state

    /// Every installed or expected item with its category and the issues that name it.
    var items: [ItemHealth] {
        let findings = report?.findings ?? []
        return (status?.items ?? []).map { item in
            ItemHealth(
                status: item, category: Self.category(for: item, findings: findings),
                issues: findings.filter { Self.finding($0, concerns: item.item) }.map { Self.issue(for: $0, tools: tools) })
        }
    }

    /// Every doctor finding, explained.
    var issues: [HealthIssue] {
        (report?.findings ?? []).map { Self.issue(for: $0, tools: tools) }
    }

    /// The selected tools that have known limitations, in config order.
    var toolLimitations: [AgentToolInfo] {
        let selected = setup?.configSummary?.harnesses ?? []
        return selected.compactMap { name in tools.first { $0.id == name } }.filter { !$0.limitations.isEmpty }
    }

    /// The setup's installation and health in one value, for a status badge. `nil` until the
    /// status has been read, and for a setup without a saved config.
    var summary: SetupHealthSummary? {
        guard hasSavedConfig, let status else {
            return nil
        }
        return Self.summary(items: items, findings: report?.findings ?? [], artifacts: status.artifacts)
    }

    // MARK: Mapping

    /// The category of one item. Configuration, installation and health stay distinct: an item can
    /// be selected but not installed, and an installed item can still need external setup.
    static func category(for status: ItemStatus, findings: [Finding]) -> ItemCategory {
        switch status.state {
        case .unowned:
            return .ownershipProblem
        case .modified:
            return .drifted
        case .missing:
            let present = status.artifacts.contains { $0.state != .missing }
            return present ? .notFullyInstalled : .selectedNotInstalled
        case .ok:
            let needsSetup = findings.contains { $0.check == DoctorCheckName.expects && finding($0, concerns: status.item) }
            return needsSetup ? .setupRequired : .installed
        }
    }

    /// True when `finding` is about `item`: the engine named it as the subject, or the finding's
    /// detail lists it among the items that need a variable.
    static func finding(_ finding: Finding, concerns item: ItemRef) -> Bool {
        if let subject = finding.subject {
            return subject == item
        }
        guard finding.check == DoctorCheckName.expects else {
            return false
        }

        // The engine's demand lines read `skill "name" expects it`, `MCP server "name" expects it`
        // and `hook "name" expects it`.
        let noun =
            switch item.kind {
            case .skill: "skill"
            case .mcp: "MCP server"
            case .hook: "hook"
            case .pack: "pack"
            }
        return finding.detail.contains { $0.hasPrefix("\(noun) \"\(item.name)\"") }
    }

    static func summary(items: [ItemHealth], findings: [Finding], artifacts: [StatusArtifact]) -> SetupHealthSummary {
        var counts: [ItemCategory: Int] = [:]
        for item in items {
            counts[item.category, default: 0] += 1
        }

        let level: SetupHealthSummary.Level
        if counts[.ownershipProblem] != nil || findings.contains(where: { $0.check == DoctorCheckName.ownership }) {
            level = .needsAttention
        } else if counts[.drifted] != nil || counts[.notFullyInstalled] != nil || counts[.selectedNotInstalled] != nil
            || artifacts.contains(where: { $0.state != .ok })
            || findings.contains(where: { [DoctorCheckName.drift, DoctorCheckName.lock].contains($0.check) })
        {
            level = .notFullyInstalled
        } else if counts[.setupRequired] != nil
            || findings.contains(where: { $0.check == DoctorCheckName.expects })
        {
            level = .setupRequired
        } else {
            level = .installed
        }
        return SetupHealthSummary(level: level, counts: counts)
    }

    /// Explains one doctor finding. The engine's own wording stays available as technical detail.
    static func issue(for finding: Finding, tools: [AgentToolInfo]) -> HealthIssue {
        let toolName = finding.harness.map { name in tools.first { $0.id == name }?.displayName ?? name }
        let affected = finding.subject.map(describe) ?? toolName
        let quoted = firstQuoted(finding.message)
        let agentTool = toolName ?? String(localized: "your agent tool")

        let kind: HealthIssue.Kind
        let title: String
        let explanation: String
        let nextStep: String
        var action: HealthIssue.Action?

        switch finding.check {
        case DoctorCheckName.expects:
            let variable = quoted ?? String(localized: "a variable")
            let declared = finding.detail.contains { $0.hasSuffix(" expects it") }
            nextStep = String(
                localized:
                    "Set \(variable) in the environment \(agentTool) starts with, then restart it. Ambit does not ask for or store the value.")
            if declared {
                kind = .missingPrerequisite
                title = String(localized: "\(variable) is not set")
                explanation = String(
                    localized:
                        "A capability declares that it needs the environment variable \(variable), and it is not set where Ambit runs. The capability is installed, but it may not work until the variable is set.")
            } else {
                kind = .unresolvedReference
                title = String(localized: "\(variable) is referenced but not set")
                explanation = String(
                    localized:
                        "An installed configuration refers to \(variable), which the agent tool fills in when it starts the server. It is not set where Ambit runs, so the server may start without it.")
            }
        case DoctorCheckName.lock:
            kind = .lockOutdated
            title = String(localized: "The installed versions are out of date")
            explanation = String(
                localized:
                    "The versions recorded for this setup no longer match what its saved configuration selects.")
            nextStep = String(localized: "Review and reapply the configuration to install the selected versions.")
            action = .reviewAndReapply
        case DoctorCheckName.drift:
            kind = .drift
            title = String(localized: "Installed files changed")
            explanation = String(
                localized:
                    "\(path(in: finding.message) ?? String(localized: "An installed file")) no longer matches what Ambit installed, because it was edited, moved or deleted outside Ambit.")
            nextStep = String(
                localized: "Review and reapply the configuration. The review lists every file Apply would restore.")
            action = .reviewAndReapply
        case DoctorCheckName.ownership:
            kind = .ownership
            title = String(localized: "A file is in the way")
            explanation = String(
                localized:
                    "\(path(in: finding.message) ?? String(localized: "A file")) exists where Ambit would install, but Ambit has no record of installing it. Ambit never overwrites files it does not own.")
            nextStep = String(
                localized:
                    "If you don't need the file, move or delete it, then review and reapply the configuration.")
            action = .reviewAndReapply
        case DoctorCheckName.mode:
            kind = .installMode
            title = String(localized: "Installed in a different way")
            explanation = String(
                localized:
                    "A capability was installed as a copy or a link where Ambit would now choose the other. Both give the agent tool the same files.")
            nextStep = String(localized: "No action is needed. Reapplying the configuration installs it the usual way.")
        case DoctorCheckName.harness:
            kind = .toolLimitation
            title = String(localized: "\(agentTool) limitation")
            explanation = finding.message
            nextStep = finding.detail.last ?? String(localized: "Check \(agentTool)'s own settings.")
        default:
            kind = .other
            title = finding.message
            explanation = finding.detail.dropLast().joined(separator: " ")
            nextStep = finding.detail.last ?? String(localized: "Review and reapply the configuration.")
        }

        return HealthIssue(
            kind: kind, severity: finding.severity, title: title, explanation: explanation,
            affected: affected ?? demanders(finding) ?? affectedPath(finding), nextStep: nextStep, action: action,
            technical: [finding.message] + finding.detail)
    }

    /// "Skill review from team" and similar, for the affected capability.
    static func describe(_ item: ItemRef) -> String {
        switch item.kind {
        case .skill: String(localized: "Skill \(item.name) from \(item.catalog)")
        case .pack: String(localized: "Pack \(item.name) from \(item.catalog)")
        case .mcp: String(localized: "MCP server \(item.name) from \(item.catalog)")
        case .hook: String(localized: "Hook \(item.name) from \(item.catalog)")
        }
    }

    /// The items an `expects` finding lists as wanting its variable, when the engine set no subject.
    private static func demanders(_ finding: Finding) -> String? {
        guard finding.check == DoctorCheckName.expects else {
            return nil
        }

        let lines = finding.detail.filter { $0.hasSuffix(" expects it") || $0.contains(" references it") }
        let names = lines.compactMap(firstQuoted)
        return names.isEmpty ? nil : names.joined(separator: ", ")
    }

    private static func affectedPath(_ finding: Finding) -> String? {
        [DoctorCheckName.drift, DoctorCheckName.ownership].contains(finding.check) ? path(in: finding.message) : nil
    }

    /// The path a drift or ownership message names: `<path> is modified`, `<path> does not hold
    /// the block install would write`, or `ambit does not own <path>`.
    private static func path(in message: String) -> String? {
        if let range = message.range(of: "does not own ") {
            return String(message[range.upperBound...])
        }
        for separator in [" does not hold ", " is "] {
            if let range = message.range(of: separator) {
                return String(message[..<range.lowerBound])
            }
        }
        return nil
    }

    private static func firstQuoted(_ text: String) -> String? {
        let parts = text.split(separator: "\"", omittingEmptySubsequences: false)
        return parts.count >= 3 ? String(parts[1]) : nil
    }
}

/// The doctor's check names, as the engine reports them in `Finding.check`.
enum DoctorCheckName {
    static let expects = "expects"
    static let lock = "lock"
    static let ownership = "ownership"
    static let drift = "drift"
    static let mode = "mode"
    static let harness = "harness"
}

/// Where one item stands. Ordered by how much it needs the user.
enum ItemCategory: Int, CaseIterable, Comparable, Sendable {
    case installed
    /// Installed, but a declared prerequisite such as an environment variable is missing.
    case setupRequired
    /// Selected in the saved config but none of its files are installed.
    case selectedNotInstalled
    /// Some of its files are installed and some are missing.
    case notFullyInstalled
    /// Its installed files changed outside Ambit.
    case drifted
    /// A file Ambit would install exists and Ambit does not own it.
    case ownershipProblem

    static func < (lhs: Self, rhs: Self) -> Bool { lhs.rawValue < rhs.rawValue }

    var title: String {
        switch self {
        case .installed: String(localized: "Installed")
        case .setupRequired: String(localized: "Installed; setup required")
        case .selectedNotInstalled: String(localized: "Selected, not installed")
        case .notFullyInstalled: String(localized: "Not fully installed")
        case .drifted: String(localized: "Changed since install")
        case .ownershipProblem: String(localized: "Blocked by a file Ambit does not own")
        }
    }
}

struct ItemHealth: Identifiable, Equatable {
    var status: ItemStatus
    var category: ItemCategory
    var issues: [HealthIssue]

    var id: ItemRef { status.item }
}

struct HealthIssue: Identifiable, Equatable {
    enum Kind: Equatable {
        case missingPrerequisite
        case unresolvedReference
        case lockOutdated
        case drift
        case ownership
        case installMode
        case toolLimitation
        case other
    }

    enum Action: Equatable {
        case reviewAndReapply
    }

    var kind: Kind
    var severity: Severity
    var title: String
    var explanation: String
    /// The capability or tool the issue is about, when known.
    var affected: String?
    var nextStep: String
    var action: Action?
    /// The engine's own message and detail lines.
    var technical: [String]

    var id: String { technical.joined(separator: "\n") }
}

/// A setup's installation and health reduced to one level, for a status badge.
struct SetupHealthSummary: Equatable, Sendable {
    enum Level: Int, Comparable, Sendable {
        case installed
        case setupRequired
        case notFullyInstalled
        case needsAttention

        static func < (lhs: Self, rhs: Self) -> Bool { lhs.rawValue < rhs.rawValue }
    }

    var level: Level
    /// How many items are in each category. Absent categories have no items.
    var counts: [ItemCategory: Int]

    var title: String {
        switch level {
        case .installed: String(localized: "Installed")
        case .setupRequired: String(localized: "Installed; setup required")
        case .notFullyInstalled: String(localized: "Not fully installed")
        case .needsAttention: String(localized: "Needs attention")
        }
    }
}

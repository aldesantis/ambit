// Conversions for review, apply, catalog updates, status and health
// (crates/ambit-ffi/src/{review,status}.rs). Where the facade's records are narrower than the
// FFI's, the comments say what is dropped.

import AmbitEngine
import Foundation

extension LiveEngineMapping {
    /// The FFI names items in a diff by kind and name only, so the catalog is left empty.
    static func bundleDiff(_ changes: [AmbitEngine.CapabilityChange]) -> BundleDiff {
        var diff = BundleDiff()
        for change in changes {
            let item = ItemRef(kind: itemKind(change.kind), catalog: "", name: change.name)
            switch change.change {
            case .added: diff.added.append(item)
            case .changed: diff.changed.append(item)
            case .removed: diff.removed.append(item)
            }
        }
        return diff
    }

    static func managedKind(_ kind: AmbitEngine.ManagedKind) -> String {
        switch kind {
        case .skillDirectory: "skillDirectory"
        case .hookDirectory: "hookDirectory"
        case .skillsLink: "skillsLink"
        case .toolConfig: "toolConfig"
        }
    }

    /// Stable identifiers, shared by `Finding.check` and `HealthCheck.name`.
    static func checkName(_ check: AmbitEngine.HealthCheck) -> String {
        switch check {
        case .prerequisites: "prerequisites"
        case .lock: "lock"
        case .ownership: "ownership"
        case .drift: "drift"
        case .mode: "mode"
        case .toolLimitation: "toolLimitation"
        }
    }

    static func checkTitle(_ check: AmbitEngine.HealthCheck) -> String {
        switch check {
        case .prerequisites: String(localized: "Prerequisites")
        case .lock: String(localized: "Lock file")
        case .ownership: String(localized: "Ownership")
        case .drift: String(localized: "Installed files")
        case .mode: String(localized: "Install mode")
        case .toolLimitation: String(localized: "Agent tool limitations")
        }
    }

    static func severity(_ severity: AmbitEngine.Severity) -> Severity {
        switch severity {
        case .problem: .error
        case .warning: .warning
        }
    }

    /// The facade keeps one subject; the first is the one the finding is about.
    static func finding(_ finding: AmbitEngine.HealthFinding) -> Finding {
        Finding(
            check: checkName(finding.check), severity: severity(finding.severity), message: finding.message,
            detail: finding.detail, subject: finding.subjects.first.map(itemRef), harness: finding.harness)
    }

    static func blocker(_ blocker: AmbitEngine.ReviewBlocker) -> Blocker {
        switch blocker {
        case let .config(message, detail):
            .config(error: .config(message: message, detail: detail, path: nil, line: nil))
        case let .unmatched(entry, message, detail):
            .unmatched(entry: selectionEntry(entry), error: .resolution(message: message, detail: detail))
        case let .resolution(message, detail):
            .resolution(error: .resolution(message: message, detail: detail))
        case let .ownership(path, key, message, detail):
            .ownership(conflict: OwnershipConflict(path: path, key: key, message: message, detail: detail))
        }
    }

    static func skippedHook(_ skipped: AmbitEngine.SkippedCombination) -> SkippedHook {
        let reason =
            switch skipped.reason {
            case .noHooks: String(localized: "\(skipped.tool) does not support hooks.")
            case .unsupportedEvent: String(localized: "\(skipped.tool) has no \(skipped.event) event.")
            }
        return SkippedHook(hook: ItemRef(kind: .hook, catalog: "", name: skipped.hook), harness: skipped.tool, reason: reason)
    }

    static func reviewSummary(_ summary: AmbitEngine.ReviewSummary) -> ReviewSummary {
        ReviewSummary(
            config: configChanges(summary.config), diff: bundleDiff(summary.capabilities),
            writes: summary.paths.filter { $0.change != .remove }.map {
                PlannedWrite(path: $0.path, kind: managedKind($0.kind))
            },
            removals: summary.paths.filter { $0.change == .remove }.map {
                PlannedRemoval(path: $0.path, kind: managedKind($0.kind))
            },
            skipped: summary.skipped.map(skippedHook), limitations: summary.limitations.map(finding),
            lockChanged: summary.lockChanged, blockers: summary.blockers.map(blocker), canApply: summary.canApply,
            revisions: summary.revisions.map { RevisionChange(catalog: $0.catalog, before: $0.before, after: $0.after) })
    }

    /// `installed` lists the paths of `reviewed`, the summary of what was applied; the FFI reports
    /// counts only. A retry has no review, so its summary is empty.
    static func applyOutcome(_ outcome: AmbitEngine.ApplyOutcome, reviewed: ReviewSummary?) -> ApplyOutcome {
        switch outcome {
        case .installed:
            return .installed(
                summary: InstallSummary(writes: reviewed?.writes ?? [], removals: reviewed?.removals ?? []))
        case let .notFullyInstalled(saved, _, subject, message, detail):
            // Writing failed; `subject` is the path being written, empty for a whole-setup step.
            return .notFullyInstalled(
                saved: saved, error: .io(message: message, detail: detail, path: subject.isEmpty ? nil : subject))
        }
    }

    static func catalogUpdateCheck(_ check: AmbitEngine.CatalogUpdateCheck) -> CatalogUpdateCheck {
        let freshness: CatalogFreshness =
            switch check.freshness {
            case .outdated: .outdated
            case .current: .current
            case .pinned: .pinned
            case .local: .local
            }
        return CatalogUpdateCheck(
            catalog: check.catalog, freshness: freshness, commit: check.commit, latest: check.latest,
            changes: bundleDiff(check.changes))
    }

    static func reviewedRevision(_ revision: ReviewedRevision) -> AmbitEngine.ReviewedRevision {
        AmbitEngine.ReviewedRevision(catalog: revision.catalog, commit: revision.commit)
    }

    static func artifactState(_ state: AmbitEngine.InstallState) -> ArtifactState {
        switch state {
        case .ok: .ok
        case .missing: .missing
        case .modified: .modified
        case .stale: .stale
        case .unowned: .unowned
        }
    }

    static func statusArtifact(_ artifact: AmbitEngine.ArtifactStatus) -> StatusArtifact {
        StatusArtifact(
            path: artifact.path, kind: managedKind(artifact.kind), state: artifactState(artifact.state),
            detail: artifact.detail.isEmpty ? nil : artifact.detail)
    }

    static func itemStatus(_ status: AmbitEngine.ItemStatus) -> ItemStatus {
        ItemStatus(
            item: itemRef(status.item), state: artifactState(status.state),
            artifacts: status.artifacts.map(statusArtifact))
    }

    static func setupStatus(_ status: AmbitEngine.SetupStatus) -> SetupStatus {
        SetupStatus(items: status.items.map(itemStatus), artifacts: status.artifacts.map(statusArtifact))
    }

    static func healthReport(_ report: AmbitEngine.HealthReport) -> HealthReport {
        HealthReport(
            checks: report.checks.map {
                HealthCheck(name: checkName($0.check), passed: $0.worst == nil, message: checkTitle($0.check))
            },
            findings: report.findings.map(finding), items: report.items.map(itemStatus),
            setupRequired: report.setupRequired.map(itemRef), environmentNote: report.environmentNote)
    }
}

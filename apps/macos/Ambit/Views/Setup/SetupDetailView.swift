import AppKit
import SwiftUI

/// The areas of a setup. Each area's view lives in `Views/<Area>/` and is owned by that area.
enum SetupSection: String, CaseIterable, Identifiable {
    case capabilities, catalogs, agentTools, health

    var id: Self { self }
}

struct SetupDetailView: View {
    let setup: SetupModel
    @State private var section: SetupSection = .capabilities

    var body: some View {
        @Bindable var setup = setup

        VStack(spacing: 0) {
            SetupHeader(setup: setup)
                .padding(.horizontal, 20)
                .padding(.vertical, 16)

            if let failure = setup.installFailure, setup.review == nil {
                InstallFailureBanner(setup: setup, error: failure)
                    .padding([.horizontal, .bottom], 20)
            }

            content
        }
        .navigationTitle(setup.displayName)
        .toolbar {
            ToolbarItem {
                OperationStatusView(operations: setup.operations)
            }
            ToolbarItem {
                Button {
                    Task { await setup.refresh() }
                } label: {
                    Label("Refresh", systemImage: "arrow.clockwise")
                }
                .keyboardShortcut("r")
                .disabled(setup.isLoading)
                .help("Read the setup again (⌘R)")
                .accessibilityIdentifier("setup.refresh")
            }
        }
        .task {
            await setup.refresh()
        }
        .onReceive(NotificationCenter.default.publisher(for: NSApplication.didBecomeActiveNotification)) { _ in
            Task { await setup.refresh() }
        }
        .sheet(item: Binding(get: { setup.review.map(ReviewItem.init) }, set: { if $0 == nil { setup.closeReview() } })) {
            item in
            ReviewSheet(setup: setup, review: item.review)
        }
        .alert(
            "\(setup.fileName) Changed Outside Ambit",
            isPresented: Binding(
                get: { setup.externalChange != nil },
                set: { if !$0, setup.externalChange != nil { setup.keepDraftAfterExternalChange() } })
        ) {
            Button("Reload and Discard Changes", role: .destructive) {
                setup.reloadDiscardingDraft()
            }
            Button("Keep Editing", role: .cancel) {
                setup.keepDraftAfterExternalChange()
            }
        } message: {
            Text(
                "The configuration was changed by another app. Reload it to continue from the new file; your unapplied changes are discarded. If you keep editing, your changes can't be applied until you reload."
            )
        }
        .alert(item: $setup.presentedError) { error in
            Alert(title: Text(error.title), message: Text(error.message))
        }
    }

    @ViewBuilder
    private var content: some View {
        if let error = setup.loadError {
            ConfigErrorView(setup: setup, problem: .unreadable(error))
        } else {
            switch setup.snapshot?.config {
            case nil:
                ProgressView("Reading setup…")
                    .frame(maxWidth: .infinity, maxHeight: .infinity)
            case .missing:
                NewSetupFlow(setup: setup)
            case let .invalid(path, _, problem):
                ConfigErrorView(setup: setup, problem: .invalid(path: path, problem: problem))
            case let .ambiguous(files, problem):
                ConfigErrorView(setup: setup, problem: .ambiguous(files: files, problem: problem))
            case .valid:
                areas
            }
        }
    }

    private var areas: some View {
        TabView(selection: $section) {
            Tab("Capabilities", systemImage: "puzzlepiece.extension", value: .capabilities) {
                CapabilitiesView(setup: setup)
            }
            Tab("Catalogs", systemImage: "books.vertical", value: .catalogs) {
                CatalogsView(setup: setup)
            }
            Tab("Agent Tools", systemImage: "hammer", value: .agentTools) {
                AgentToolsView(setup: setup)
            }
            Tab("Health", systemImage: "stethoscope", value: .health) {
                HealthView(setup: setup)
            }
        }
        .padding([.horizontal, .bottom], 20)
    }
}

/// Gives the open review an identity for `sheet(item:)`.
private struct ReviewItem: Identifiable {
    let review: ReviewModel
    var id: ObjectIdentifier { ObjectIdentifier(review) }
}

/// The saved config was not fully installed. Shown until a retry or apply installs it.
struct InstallFailureBanner: View {
    let setup: SetupModel
    let error: EngineError

    var body: some View {
        HStack(alignment: .top, spacing: 12) {
            Image(systemName: "exclamationmark.triangle.fill")
                .foregroundStyle(.orange)
                .font(.title3)
            VStack(alignment: .leading, spacing: 4) {
                Text("Changes not fully installed")
                    .font(.headline)
                Text("The configuration was saved, but installing it failed. Files already installed were kept.")
                    .foregroundStyle(.secondary)
                EngineErrorText(error: error)
            }
            Spacer()
            Button("Retry Installation") {
                Task { await setup.retryInstall() }
            }
            .disabled(setup.operations.isBusy)
            .accessibilityIdentifier("setup.retryInstall")
        }
        .padding(12)
        .background(.orange.opacity(0.1), in: RoundedRectangle(cornerRadius: 10))
        .accessibilityElement(children: .contain)
        .accessibilityIdentifier("setup.installFailure")
    }
}

/// A compact indicator of the app-wide running operation.
private struct OperationStatusView: View {
    let operations: OperationRunner

    var body: some View {
        if let operation = operations.current {
            HStack(spacing: 6) {
                ProgressView()
                    .controlSize(.small)
                Text(operation.progress?.stage.title ?? operation.title)
                    .font(.callout)
                    .foregroundStyle(.secondary)
            }
            .accessibilityElement(children: .combine)
        }
    }
}

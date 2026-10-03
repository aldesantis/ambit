import AppKit
import SwiftUI

/// A setup whose config cannot be used. The file is never reset or rewritten from here: the
/// user fixes it in another app and retries.
struct ConfigErrorView: View {
    enum Problem {
        case invalid(path: String, problem: ConfigProblem)
        /// Both `ambit.yml` and `ambit.yaml` exist.
        case ambiguous(files: [String], problem: ConfigProblem)
        /// The setup root itself could not be read.
        case unreadable(EngineError)
    }

    let setup: SetupModel
    let problem: Problem

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 16) {
                Label(title, systemImage: "exclamationmark.octagon.fill")
                    .font(.title3.bold())
                    .foregroundStyle(.red)
                    .accessibilityAddTraits(.isHeader)
                    .accessibilityIdentifier("configError.title")

                Text(explanation)

                GroupBox {
                    VStack(alignment: .leading, spacing: 8) {
                        ForEach(files, id: \.self) { file in
                            LabeledContent("File") {
                                Text(file)
                                    .textSelection(.enabled)
                                    .accessibilityIdentifier("configError.path")
                            }
                        }
                        if let line {
                            LabeledContent("Line") {
                                Text(line, format: .number)
                                    .accessibilityIdentifier("configError.line")
                            }
                        }
                        VStack(alignment: .leading, spacing: 2) {
                            Text(message)
                                .accessibilityIdentifier("configError.message")
                            ForEach(Array(detail.enumerated()), id: \.offset) { _, line in
                                Text(line)
                                    .font(.callout.monospaced())
                                    .foregroundStyle(.secondary)
                            }
                        }
                        .textSelection(.enabled)
                    }
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .padding(4)
                }

                HStack {
                    Button("Reveal in Finder") {
                        let urls = (files.isEmpty ? [setup.root.path] : files).map { URL(fileURLWithPath: $0) }
                        NSWorkspace.shared.activateFileViewerSelecting(urls)
                    }
                    .accessibilityIdentifier("configError.reveal")
                    Button("Retry") {
                        Task { await setup.refresh() }
                    }
                    .buttonStyle(.borderedProminent)
                    .disabled(setup.isLoading)
                    .accessibilityIdentifier("configError.retry")
                }
            }
            .frame(maxWidth: 640, alignment: .leading)
            .padding(24)
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
    }

    private var title: String {
        switch problem {
        case .invalid: String(localized: "The configuration can't be read")
        case .ambiguous: String(localized: "There are two configuration files")
        case .unreadable: String(localized: "The setup folder can't be read")
        }
    }

    private var explanation: String {
        switch problem {
        case .invalid:
            String(localized: "Ambit won't change this file. Correct it in a text editor, then retry.")
        case .ambiguous:
            String(localized: "Ambit doesn't know which file to use. Keep one of them, then retry. Ambit won't change either file.")
        case .unreadable:
            String(localized: "Check that the folder exists and that you can open it, then retry.")
        }
    }

    private var files: [String] {
        switch problem {
        case let .invalid(path, _): [path]
        case let .ambiguous(files, _): files
        case let .unreadable(error):
            if case let .io(_, _, path?) = error { [path] } else { [] }
        }
    }

    private var line: UInt32? {
        switch problem {
        case let .invalid(_, problem), let .ambiguous(_, problem): problem.line
        case let .unreadable(error):
            if case let .config(_, _, _, line) = error { line } else { nil }
        }
    }

    private var message: String {
        switch problem {
        case let .invalid(_, problem), let .ambiguous(_, problem): problem.message
        case let .unreadable(error): error.message
        }
    }

    private var detail: [String] {
        switch problem {
        case let .invalid(_, problem), let .ambiguous(_, problem): problem.detail
        case let .unreadable(error): error.detail
        }
    }
}

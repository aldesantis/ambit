import SwiftUI

/// An engine error's message followed by its detail lines.
struct EngineErrorText: View {
    let error: EngineError

    var body: some View {
        VStack(alignment: .leading, spacing: 2) {
            Text(error.message)
            ForEach(Array(error.detail.enumerated()), id: \.offset) { _, line in
                Text(line)
                    .font(.callout.monospaced())
                    .foregroundStyle(.secondary)
            }
        }
        .textSelection(.enabled)
        .fixedSize(horizontal: false, vertical: true)
    }
}

import SwiftUI

/// A skill's `SKILL.md`, read-only: frontmatter fields, then the body as native text.
struct SkillDocumentView: View {
    let document: SkillDocument

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            if !document.frontmatter.isEmpty {
                Grid(alignment: .leadingFirstTextBaseline, horizontalSpacing: 12, verticalSpacing: 4) {
                    ForEach(Array(document.frontmatter.enumerated()), id: \.offset) { _, field in
                        GridRow {
                            Text(field.key)
                                .foregroundStyle(.secondary)
                                .gridColumnAlignment(.trailing)
                            Text(field.value)
                                .textSelection(.enabled)
                        }
                    }
                }
                .font(.callout)
                .padding(10)
                .frame(maxWidth: .infinity, alignment: .leading)
                .background(.quaternary.opacity(0.5), in: RoundedRectangle(cornerRadius: 8))
            }

            MarkdownBlocksView(blocks: MarkdownRenderer.blocks(from: document.body))
        }
        .accessibilityIdentifier("capability.skillDocument")
    }
}

struct MarkdownBlocksView: View {
    let blocks: [MarkdownBlock]

    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            ForEach(Array(blocks.enumerated()), id: \.offset) { _, block in
                MarkdownBlockView(block: block)
            }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
    }
}

private struct MarkdownBlockView: View {
    let block: MarkdownBlock

    var body: some View {
        switch block {
        case let .heading(level, text):
            Text(text)
                .font(font(forHeading: level))
                .accessibilityAddTraits(.isHeader)
                .padding(.top, level <= 2 ? 6 : 2)
        case let .paragraph(text):
            Text(text)
                .textSelection(.enabled)
                .fixedSize(horizontal: false, vertical: true)
        case let .code(_, text):
            monospaced(text)
        case let .literal(text):
            monospaced(text)
        case let .quote(children):
            HStack(alignment: .top, spacing: 8) {
                RoundedRectangle(cornerRadius: 1)
                    .fill(.tertiary)
                    .frame(width: 3)
                MarkdownBlocksView(blocks: children)
                    .foregroundStyle(.secondary)
            }
        case let .list(ordered, start, items):
            VStack(alignment: .leading, spacing: 4) {
                ForEach(Array(items.enumerated()), id: \.offset) { index, item in
                    HStack(alignment: .firstTextBaseline, spacing: 6) {
                        Text(ordered ? "\(start + index)." : "•")
                            .monospacedDigit()
                            .foregroundStyle(.secondary)
                        MarkdownBlocksView(blocks: item)
                    }
                }
            }
        case let .table(header, rows):
            Grid(alignment: .leading, horizontalSpacing: 12, verticalSpacing: 4) {
                GridRow {
                    ForEach(Array(header.enumerated()), id: \.offset) { _, cell in
                        Text(cell).bold()
                    }
                }
                Divider()
                ForEach(Array(rows.enumerated()), id: \.offset) { _, row in
                    GridRow {
                        ForEach(Array(row.enumerated()), id: \.offset) { _, cell in
                            Text(cell)
                        }
                    }
                }
            }
            .font(.callout)
        case .rule:
            Divider()
        }
    }

    private func monospaced(_ text: String) -> some View {
        ScrollView(.horizontal) {
            Text(verbatim: text)
                .font(.system(.callout, design: .monospaced))
                .textSelection(.enabled)
                .padding(10)
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(.quaternary.opacity(0.5), in: RoundedRectangle(cornerRadius: 6))
    }

    private func font(forHeading level: Int) -> Font {
        switch level {
        case 1: .title2.bold()
        case 2: .title3.bold()
        case 3: .headline
        default: .subheadline.bold()
        }
    }
}

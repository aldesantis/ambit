// Turns a SKILL.md body into native blocks for read-only display. Catalog content is untrusted
// and never executes: raw HTML is shown as literal text, images are named by their alt text and
// never loaded, and only http, https and mailto links stay clickable (they open in the system
// browser through SwiftUI's default `openURL`).

import Foundation
import Markdown

indirect enum MarkdownBlock: Hashable {
    case heading(level: Int, text: AttributedString)
    case paragraph(AttributedString)
    case code(language: String?, text: String)
    case quote([MarkdownBlock])
    case list(ordered: Bool, start: Int, items: [[MarkdownBlock]])
    case table(header: [AttributedString], rows: [[AttributedString]])
    case rule
    /// Raw HTML, shown as text.
    case literal(String)
}

enum MarkdownRenderer {
    static func blocks(from source: String) -> [MarkdownBlock] {
        blocks(of: Document(parsing: source, options: [.disableSmartOpts]).children)
    }

    /// The link targets that stay clickable. Everything else renders as plain text.
    static let allowedSchemes: Set<String> = ["http", "https", "mailto"]

    static func safeURL(_ destination: String?) -> URL? {
        guard let destination, let url = URL(string: destination), let scheme = url.scheme?.lowercased(),
            allowedSchemes.contains(scheme)
        else {
            return nil
        }
        return url
    }

    private static func blocks(of children: some Sequence<Markup>) -> [MarkdownBlock] {
        children.compactMap(block)
    }

    private static func block(_ markup: Markup) -> MarkdownBlock? {
        switch markup {
        case let heading as Heading:
            return .heading(level: heading.level, text: inline(heading.children))
        case let paragraph as Paragraph:
            return .paragraph(inline(paragraph.children))
        case let code as CodeBlock:
            return .code(language: code.language, text: code.code.trimmingSuffix("\n"))
        case let quote as BlockQuote:
            return .quote(blocks(of: quote.children))
        case let list as UnorderedList:
            return .list(ordered: false, start: 1, items: list.listItems.map { blocks(of: $0.children) })
        case let list as OrderedList:
            return .list(ordered: true, start: Int(list.startIndex), items: list.listItems.map { blocks(of: $0.children) })
        case let table as Markdown.Table:
            return .table(
                header: table.head.cells.map { inline($0.children) },
                rows: table.body.rows.map { row in row.cells.map { inline($0.children) } })
        case is ThematicBreak:
            return .rule
        case let html as HTMLBlock:
            return .literal(html.rawHTML.trimmingSuffix("\n"))
        default:
            return nil
        }
    }

    static func inline(_ children: some Sequence<Markup>) -> AttributedString {
        children.reduce(into: AttributedString()) { result, child in
            result += inline(child)
        }
    }

    private static func inline(_ markup: Markup) -> AttributedString {
        switch markup {
        case let text as Markdown.Text:
            return AttributedString(text.string)
        case is SoftBreak:
            return AttributedString(" ")
        case is LineBreak:
            return AttributedString("\n")
        case let code as InlineCode:
            var result = AttributedString(code.code)
            result.inlinePresentationIntent = .code
            return result
        case let emphasis as Emphasis:
            return adding(.emphasized, to: inline(emphasis.children))
        case let strong as Strong:
            return adding(.stronglyEmphasized, to: inline(strong.children))
        case let strike as Strikethrough:
            var result = inline(strike.children)
            result.strikethroughStyle = .single
            return result
        case let link as Markdown.Link:
            var result = inline(link.children)
            if result.characters.isEmpty {
                result = AttributedString(link.destination ?? "")
            }
            if let url = safeURL(link.destination) {
                result.link = url
            }
            return result
        case let image as Markdown.Image:
            let alt = String(inline(image.children).characters)
            var result = AttributedString(
                alt.isEmpty ? String(localized: "[image]") : String(localized: "[image: \(alt)]"))
            result.inlinePresentationIntent = .emphasized
            return result
        case let html as InlineHTML:
            return AttributedString(html.rawHTML)
        case let symbol as SymbolLink:
            var result = AttributedString(symbol.destination ?? "")
            result.inlinePresentationIntent = .code
            return result
        default:
            return inline(markup.children)
        }
    }

    private static func adding(_ intent: InlinePresentationIntent, to text: AttributedString) -> AttributedString {
        var text = text
        for run in text.runs {
            let existing = run.inlinePresentationIntent ?? []
            text[run.range].inlinePresentationIntent = existing.union(intent)
        }
        return text
    }
}

extension String {
    fileprivate func trimmingSuffix(_ suffix: String) -> String {
        hasSuffix(suffix) ? String(dropLast(suffix.count)) : self
    }
}

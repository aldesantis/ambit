import SwiftUI

extension ItemKind {
    var title: String {
        switch self {
        case .skill: String(localized: "Skill")
        case .pack: String(localized: "Pack")
        case .mcp: String(localized: "MCP server")
        case .hook: String(localized: "Hook")
        }
    }

    var pluralTitle: String {
        switch self {
        case .skill: String(localized: "Skills")
        case .pack: String(localized: "Packs")
        case .mcp: String(localized: "MCP servers")
        case .hook: String(localized: "Hooks")
        }
    }

    var symbol: String {
        switch self {
        case .skill: "book.pages"
        case .pack: "shippingbox"
        case .mcp: "server.rack"
        case .hook: "bolt"
        }
    }
}

extension ItemRef {
    /// `catalog/name`, as entries address it.
    var address: String { "\(catalog)/\(name)" }

    /// A stable accessibility identifier suffix.
    var identifier: String { "\(kind.rawValue).\(address)" }
}

extension CapabilitiesModel.Reason {
    var title: String {
        switch self {
        case .direct: String(localized: "Direct")
        case .rule: String(localized: "Rule")
        case .pack: String(localized: "Pack")
        case .dependency: String(localized: "Dependency")
        }
    }

    var symbol: String {
        switch self {
        case .direct: "checkmark.circle.fill"
        case .rule: "asterisk.circle"
        case .pack: "shippingbox.circle"
        case .dependency: "arrow.triangle.branch"
        }
    }

    var tint: Color {
        switch self {
        case .direct: .green
        case .rule: .purple
        case .pack: .orange
        case .dependency: .blue
        }
    }
}

struct ReasonBadge: View {
    let reason: CapabilitiesModel.Reason

    var body: some View {
        Label(reason.title, systemImage: reason.symbol)
            .labelStyle(.titleAndIcon)
            .font(.caption2.weight(.medium))
            .foregroundStyle(reason.tint)
            .padding(.horizontal, 6)
            .padding(.vertical, 2)
            .background(reason.tint.opacity(0.12), in: Capsule())
            .accessibilityIdentifier("capability.reason.\(reason)")
    }
}

extension Route {
    /// The route in a sentence, such as "Required by team/review, selected by skill: team/review".
    var explanation: String {
        switch self {
        case let .direct(entry):
            String(localized: "Selected directly by \(entry.displayText)")
        case let .rule(entry):
            String(localized: "Matched by the rule \(entry.displayText)")
        case let .pack(pack, chain):
            String(localized: "Included by the pack \(pack.address)") + Self.via(chain, after: pack)
        case let .dependency(requirer, chain):
            String(localized: "Required by \(requirer.address)") + Self.via(chain, after: requirer)
        }
    }

    /// The rest of the chain after its first link, which the sentence already names.
    private static func via(_ chain: [ItemRef], after first: ItemRef) -> String {
        let rest = chain.drop(while: { $0 == first })
        guard !rest.isEmpty else {
            return ""
        }
        return String(localized: ", which comes from ") + rest.map(\.address).joined(separator: " ← ")
    }
}

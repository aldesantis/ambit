import AmbitEngine
import Testing

/// Proves config edits cross the bindings and keep the author's comments.
struct EditTests {
    private let base = """
        version: 1
        harnesses:
          - claude   # primary
        catalogs:
          - name: company   # shared
            source: path:../company
        requires: []

        """

    @Test func editRoundTripKeepsComments() throws {
        let edited = try editConfig(
            text: base,
            fileName: "ambit.yml",
            edits: [
                .addEntry(entry: EntryAddress(kind: .skill, address: "company/core.*")),
                .setHarnesses(harnesses: ["claude", "cursor"]),
            ]
        )

        #expect(edited.text.contains("  - claude   # primary\n"))
        #expect(edited.text.contains("  - name: company   # shared\n"))
        #expect(edited.summary.harnesses == ["claude", "cursor"])
        #expect(edited.summary.requires == [
            SelectionEntry(kind: .skill, catalog: "company", pattern: "core.*", isRule: true),
        ])

        let changes = try configChanges(base: base, draft: edited.text, fileName: "ambit.yml")
        #expect(changes.harnessesAdded == ["cursor"])
        #expect(changes.entriesAdded.count == 1)
    }

    @Test func aBadAddressThrowsAConfigError() {
        #expect {
            _ = try editConfig(
                text: base,
                fileName: "ambit.yml",
                edits: [.addEntry(entry: EntryAddress(kind: .skill, address: "core"))]
            )
        } throws: { error in
            guard case .Config = error as? EngineError else { return false }
            return true
        }
    }
}

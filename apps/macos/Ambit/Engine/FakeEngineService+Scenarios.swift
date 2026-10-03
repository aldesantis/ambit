// Named fixtures for UI tests, selected with `AMBIT_TEST_ENGINE=<scenario>` (see `LaunchContext`).
// Each scenario configures the Personal setup at the test home; unknown names start empty.

import Foundation

extension FakeEngineService {
    /// A setup with an outdated GitHub catalog, a commit-pinned one, an unreachable one, a current
    /// one and a local folder.
    static func catalogUpdatesFixture(root: String) -> Fixture {
        let text = """
            version: 1
            harnesses:
              - claude
            catalogs:
              - name: team
                source: https://github.com/acme/team
                ref: main
              - name: frozen
                source: https://github.com/acme/frozen
                ref: 0123456789abcdef0123456789abcdef01234567
              - name: private
                source: https://github.com/acme/private
              - name: docs
                source: acme/docs
              - name: notes
                source: ./notes
            requires:
              - skill: team/review
            """
        var fixture = Fixture()
        fixture.config = (try? validConfig(text + "\n", root: root)) ?? .missing
        fixture.catalogChecks = [
            "team": .outdated(
                installed: "1111111111111111111111111111111111111111",
                latest: "2222222222222222222222222222222222222222",
                added: [ItemRef(kind: .skill, catalog: "team", name: "triage")]),
            "private": .failure(
                .network(
                    message: String(localized: "Could not reach github.com/acme/private."), detail: [],
                    kind: .offline)),
        ]
        return fixture
    }
}

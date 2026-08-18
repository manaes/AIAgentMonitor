import UIKit
import Wire
import XCTest
@testable import DesignSystem
@testable import MirrorFeature

final class SessionRowViewTests: XCTestCase {
    private let now = Date(timeIntervalSince1970: 1_000_000)

    private func project(n: String, m: String, r: Float, t: UInt64, s: UInt8) -> MirrorProject {
        Fixture.agent(projects: [(1, n, m, r, t, s)]).pj[0]
    }

    func testActiveRowShowsRate() {
        let v = SessionRowView()
        v.configure(project: project(n: "foo", m: "claude-opus-5", r: 98.25, t: 999_990, s: 0),
                    kind: .claude, now: now)
        XCTAssertEqual(v.leftText, "Claude · foo claude-opus-5")
        XCTAssertEqual(v.rightText, "98 tok/s", "active 면 속도를 보여준다")
        XCTAssertEqual(v.relativeText, "10s ago")
        XCTAssertEqual(v.dotColor, Palette.claudeDot)
    }

    func testIdleRowShowsStatusWordAndAmberDot() {
        let v = SessionRowView()
        v.configure(project: project(n: "bar", m: "m", r: 50, t: 999_900, s: 1),
                    kind: .claude, now: now)
        XCTAssertEqual(v.rightText, "idle", "active 가 아니면 속도 대신 상태 단어")
        XCTAssertEqual(v.dotColor, Palette.idleDot)
    }

    func testDormantRowUsesGreyDot() {
        let v = SessionRowView()
        v.configure(project: project(n: "baz", m: "m", r: 0, t: 900_000, s: 2),
                    kind: .codex, now: now)
        XCTAssertEqual(v.rightText, "dormant")
        XCTAssertEqual(v.dotColor, Palette.dormantDot, "dormant 는 에이전트 색이 아니라 회색")
    }

    func testCodexActiveUsesCodexColor() {
        let v = SessionRowView()
        v.configure(project: project(n: "qux", m: "m", r: 10, t: 999_999, s: 0),
                    kind: .codex, now: now)
        XCTAssertEqual(v.leftText, "Codex · qux m")
        XCTAssertEqual(v.dotColor, Palette.codexDot)
    }

    /// s 가 알려지지 않은 값(3)이면 ActivityStatusCode.unknown 이 된다 — 회색 점은
    /// dormant 와 같이 취급하되, 문구는 "dormant" 로 잘못 표시하지 않고 "unknown" 이어야 한다.
    func testUnknownStatusShowsUnknownWordNotDormant() {
        let v = SessionRowView()
        v.configure(project: project(n: "quux", m: "m", r: 0, t: 900_000, s: 3),
                    kind: .claude, now: now)
        XCTAssertEqual(v.rightText, "unknown", "unknown 을 dormant 로 잘못 표시하면 안 된다")
        XCTAssertEqual(v.dotColor, Palette.dormantDot)
    }

    /// k 가 알려지지 않은 값(2)이면 AgentKindCode.unknown 이 된다.
    func testUnknownAgentKindShowsPlaceholderNameAndDormantDot() {
        let v = SessionRowView()
        v.configure(project: project(n: "corge", m: "m", r: 5, t: 999_999, s: 0),
                    kind: .unknown, now: now)
        XCTAssertEqual(v.leftText, "? · corge m")
        XCTAssertEqual(v.dotColor, Palette.dormantDot, "unknown 에이전트의 active 점도 회색이어야 한다")
    }
}

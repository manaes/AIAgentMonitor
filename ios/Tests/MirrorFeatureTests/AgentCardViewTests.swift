import UIKit
import Wire
import XCTest
@testable import DesignSystem
@testable import MirrorFeature

/// 테스트용 스냅샷 조각을 JSON 으로 만든다.
/// Wire 의 DTO 는 Decodable 전용이라 이 경로로만 인스턴스를 얻을 수 있다.
enum Fixture {
    static func agent(
        k: UInt8 = 0,
        r: Float = 123.5,
        t5: UInt32 = 3000,
        p5: Float? = 62,
        r5: UInt64? = nil,
        pw: Float? = nil,
        rw: UInt64? = nil,
        projects: [(id: UInt32, n: String, m: String, r: Float, t: UInt64, s: UInt8)] = []
    ) -> MirrorAgent {
        func opt<T: CustomStringConvertible>(_ key: String, _ v: T?) -> String {
            v.map { ",\"\(key)\":\($0)" } ?? ""
        }
        let pj = projects.map {
            "{\"id\":\($0.id),\"n\":\"\($0.n)\",\"m\":\"\($0.m)\",\"r\":\($0.r),\"t\":\($0.t),\"s\":\($0.s)}"
        }.joined(separator: ",")
        let json = """
        {"v":1,"t":0,"a":[{"k":\(k),"r":\(r),"t5":\(t5)\
        \(opt("p5", p5))\(opt("r5", r5))\(opt("pw", pw))\(opt("rw", rw))\
        ,"pj":[\(pj)]}]}
        """
        // swiftlint:disable:next force_try
        return try! JSONDecoder().decode(MirrorSnapshot.self, from: Data(json.utf8)).a[0]
    }
}

final class AgentCardViewTests: XCTestCase {
    private let now = Date(timeIntervalSince1970: 1_000_000)

    func testClaudeHeaderAndRate() {
        let v = AgentCardView()
        v.configure(agent: Fixture.agent(k: 0, r: 1234), now: now)
        XCTAssertEqual(v.nameText, "Claude Code")
        XCTAssertEqual(v.rateText, "1.2k")
        XCTAssertEqual(v.dotColor, Palette.claudeDot)
    }

    func testCodexHeader() {
        let v = AgentCardView()
        v.configure(agent: Fixture.agent(k: 1), now: now)
        XCTAssertEqual(v.nameText, "Codex")
        XCTAssertEqual(v.dotColor, Palette.codexDot)
    }

    func testEmDashAndPlaceholderWhenNoProjects() {
        let v = AgentCardView()
        v.configure(agent: Fixture.agent(projects: []), now: now)
        XCTAssertEqual(v.modelText, "—", "원본은 모델이 없으면 em dash")
        XCTAssertEqual(v.projectText, "no active session")
    }

    func testPrimaryProjectPrefersActive() {
        let v = AgentCardView()
        v.configure(agent: Fixture.agent(projects: [
            (1, "idle-one", "model-idle", 0, 999_000, 1),
            (2, "active-one", "model-active", 50, 999_990, 0),
        ]), now: now)
        XCTAssertEqual(v.projectText, "active-one", "active 가 있으면 그것이 대표")
        XCTAssertEqual(v.modelText, "model-active")
    }

    func testFallsBackToFirstProjectWhenNoneActive() {
        let v = AgentCardView()
        v.configure(agent: Fixture.agent(projects: [
            (1, "first-idle", "model-a", 0, 999_000, 1),
            (2, "second-dormant", "model-b", 0, 998_000, 2),
        ]), now: now)
        XCTAssertEqual(v.projectText, "first-idle", "active 가 없으면 첫 번째")
    }

    func testCountdownHiddenWithoutResetTime() {
        let v = AgentCardView()
        v.configure(agent: Fixture.agent(r5: nil), now: now)
        XCTAssertNil(v.countdownText)
    }

    func testCountdownShownWithResetTime() {
        let v = AgentCardView()
        v.configure(agent: Fixture.agent(r5: 1_000_000 + 3661), now: now)
        XCTAssertEqual(v.countdownText, "약 1시간 1분 1초 남음")
    }
}

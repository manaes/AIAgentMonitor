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

    /// k 가 알려지지 않은 값(2)이면 AgentKindCode.unknown 이 된다 — Claude/Codex 로
    /// 잘못 표시하지 않고 별도 문구·회색 점으로 조용히 나타내는지 확인한다.
    func testUnknownAgentKind() {
        let v = AgentCardView()
        v.configure(agent: Fixture.agent(k: 2), now: now)
        XCTAssertEqual(v.nameText, "알 수 없음")
        XCTAssertEqual(v.dotColor, Palette.dormantDot)
    }

    /// autoPct(5h)/weeklyPct(주간)가 뒤바뀌어도 컴파일은 통과하므로(둘 다 Float?),
    /// 실제로 서로 다른 값이 서로 다른 자리에 표시되는지 직접 검증한다.
    func testQuotaBarWiringDoesNotTransposeAutoAndWeekly() {
        let v = AgentCardView()
        v.configure(agent: Fixture.agent(p5: 62, pw: 31), now: now)
        XCTAssertEqual(v.quotaFivePercentText, "62%", "p5(5h)는 5h 행에 표시되어야 한다")
        XCTAssertEqual(v.quotaWeeklyPercentText, "31%", "pw(주간)는 주간 행에 표시되어야 한다")
    }

    /// autoPct(p5)가 없으면(동기화 전) tokens5h(t5)가 폴백 문구에 그대로 이어져야 한다.
    /// t5 와 폴백 텍스트 사이의 배선도 configure() 를 거치는 경계이므로 직접 확인한다.
    func testQuotaBarFallsBackToTokenTotalWhenNoPercent() {
        let v = AgentCardView()
        v.configure(agent: Fixture.agent(t5: 1500, p5: nil), now: now)
        XCTAssertEqual(v.quotaFallbackText, "5h 토큰: 1.5k · 동기화 전")
        XCTAssertNil(v.quotaFivePercentText, "동기화 전에는 퍼센트 행이 아니라 폴백만 보여야 한다")
    }

    /// r5(리셋 시각)가 이미 지났으면 QuotaDisplay.isReset5h 가 true 가 되어 5h 사용률이
    /// autoPct 값과 무관하게 0% 로 표시되어야 한다(원본의 reset_5h 파생과 동일).
    func testQuotaBarShowsZeroPercentAfterReset() {
        let v = AgentCardView()
        v.configure(agent: Fixture.agent(p5: 62, r5: 999_000), now: now)
        XCTAssertEqual(v.quotaFivePercentText, "0%", "리셋 시각이 지났으면 실제 p5 값과 무관하게 0% 여야 한다")
    }

    /// 22pt 숫자와 "tok/s" 사이 간격. `.unit { margin-left: 4px }`(AgentCard.svelte:90)
    /// 만 옮기면 4pt 지만, `.big`(:89)은 flex 가 아닌 일반 블록이라 :59-60 사이의
    /// 줄바꿈이 공백 하나로 접혀 **부모의 22px 폰트로 실제로 그려진다**.
    /// 이 화면에서 유일한 non-flex 컨테이너이고 하필 가장 큰 숫자 옆이다.
    ///   22pt bold 시스템 폰트의 공백 폭 4.72pt + margin 4pt = 8.72pt
    func testGapBetweenRateAndUnitIncludesTheRenderedSpaceOfTheBlockContainer() {
        // 공백 폭이 실제로 4.72pt 인지부터 확인한다 — 이 값이 상수의 근거다.
        let space = (" " as NSString).size(withAttributes: [.font: Typography.bigRate]).width
        XCTAssertEqual(space, 4.72, accuracy: 0.01, "22pt bold 시스템 폰트의 공백 폭")

        XCTAssertEqual(AgentCardView.unitGap, 8.72, accuracy: 0.001,
                       "공백 4.72 + margin-left 4 = 8.72pt")

        let v = AgentCardView()
        v.configure(agent: Fixture.agent(r: 1234), now: now)
        v.frame = CGRect(x: 0, y: 0, width: 337, height: 120)
        v.setNeedsLayout()
        v.layoutIfNeeded()

        // 레이아웃 결과는 3x 기기의 픽셀 격자(1/3pt)로 스냅되므로 8.6667 이 나온다 —
        // 상수 자체는 위에서 정확히 고정했고, 여기서는 그 상수가 실제로 이 간격에
        // 적용됐는지(4pt 로 되돌아가지 않았는지)를 확인한다.
        XCTAssertEqual(
            v.rateToUnitGap, 8.72, accuracy: 0.34,
            "4pt 만 두면 맥보다 약 4.7pt 좁다 — 공백 하나가 렌더링된다는 사실이 빠진 것이다"
        )
    }
}

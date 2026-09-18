import XCTest
import Wire
@testable import WidgetShared

/// `MirrorAgent` 는 멤버와이즈 이니셜라이저가 internal 이라 전송 JSON 으로 만든다 —
/// 덕분에 위젯이 실제로 받는 경로(짧은 키)까지 같이 검증된다.
private func makeAgent(_ json: String) throws -> MirrorAgent {
    try JSONDecoder().decode(MirrorAgent.self, from: Data(json.utf8))
}

final class UsagePresentationTests: XCTestCase {

    private let now = Date(timeIntervalSince1970: 1_758_000_000)

    // MARK: - 주간

    /// 실기에서 잡혔던 회귀: 맥의 `/usage` 안전망은 %만 주고 리셋 시각(rw)은 주지 않는다.
    /// 둘을 묶어 판정하면 앱은 %를 보여주는데 위젯만 "동기화 전"으로 떨어졌다.
    func testWeeklyShowsPercentEvenWithoutResetTime() throws {
        let agent = try makeAgent(#"{"k":0,"r":0,"t5":0,"pw":18.4,"pj":[]}"#)

        let display = UsagePresentation.weekly(for: agent, now: now)

        XCTAssertEqual(display.percentText, "18%")
        XCTAssertEqual(display.percent, 18.4)
        XCTAssertNil(display.countdownText)
        XCTAssertNil(display.fallbackText)
    }

    func testWeeklyIncludesCountdownWhenResetTimeIsPresent() throws {
        let resetAt = UInt64(now.timeIntervalSince1970) + 2 * 86_400 + 3 * 3_600
        let agent = try makeAgent(#"{"k":1,"r":0,"t5":0,"pw":89,"rw":\#(resetAt),"pj":[]}"#)

        let display = UsagePresentation.weekly(for: agent, now: now)

        XCTAssertEqual(display.percentText, "89%")
        XCTAssertEqual(display.countdownText, "약 2일 3시간 남음")
    }

    func testWeeklyClampsPercentAboveHundred() throws {
        let agent = try makeAgent(#"{"k":0,"r":0,"t5":0,"pw":137.2,"pj":[]}"#)

        let display = UsagePresentation.weekly(for: agent, now: now)

        XCTAssertEqual(display.percent, 100)
        XCTAssertEqual(display.percentText, "100%")
    }

    // MARK: - 5h

    func testFiveHourShowsPercentWithoutCountdown() throws {
        let agent = try makeAgent(#"{"k":0,"r":0,"t5":1200,"p5":45.6,"pj":[]}"#)

        let display = UsagePresentation.fiveHour(for: agent)

        XCTAssertEqual(display.percentText, "46%")
        XCTAssertEqual(display.percent, 45.6)
        XCTAssertNil(display.countdownText)
    }

    // MARK: - 폴백 문구

    /// 조회 실패는 %를 감추고 실패 사유를 그대로 보여준다 — 맥은 실패 중에도 마지막
    /// %를 함께 보내주지만 그 숫자는 현재 상태를 말해주지 않는다(MirrorSnapshot 문서).
    func testQuotaErrorHidesPercentAndShowsReason() throws {
        let agent = try makeAgent(#"{"k":0,"r":0,"t5":0,"p5":44,"pw":91,"e":1,"pj":[]}"#)

        let five = UsagePresentation.fiveHour(for: agent)
        let weekly = UsagePresentation.weekly(for: agent, now: now)

        XCTAssertNil(five.percentText)
        XCTAssertNil(five.percent)
        XCTAssertNil(weekly.percentText)
        XCTAssertEqual(five.fallbackText, "로그인 필요")
        XCTAssertEqual(weekly.fallbackText, "로그인 필요")
    }

    /// Codex 처럼 5h 창이 없는 플랜: 주간 값이 왔으므로 조회 자체는 성공했다.
    /// "동기화 전"이 아니라 "지원하지 않음"이어야 한다(맥 `QuotaBarView.note()` 와 동일).
    func testMissingWindowSaysUnsupportedWhenAnotherWindowSynced() throws {
        let agent = try makeAgent(#"{"k":1,"r":0,"t5":0,"pw":89,"pj":[]}"#)

        XCTAssertEqual(UsagePresentation.fiveHour(for: agent).fallbackText, "지원하지 않음")
    }

    func testMissingWindowSaysNotSyncedWhenNothingArrived() throws {
        let agent = try makeAgent(#"{"k":0,"r":0,"t5":0,"pj":[]}"#)

        XCTAssertEqual(UsagePresentation.fiveHour(for: agent).fallbackText, "동기화 전")
        XCTAssertEqual(UsagePresentation.weekly(for: agent, now: now).fallbackText, "동기화 전")
    }
}

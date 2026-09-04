import UIKit
import XCTest
@testable import DesignSystem

final class QuotaBarViewTests: XCTestCase {

    func testShowsBarsWhenSynced() {
        let v = QuotaBarView()
        v.configure(tokens5h: 48210, autoPct: 62.4, weeklyPct: 31.5, isReset5h: false)
        XCTAssertEqual(v.fivePercentText, "62%", "원본은 toFixed(0)")
        XCTAssertEqual(v.weeklyPercentText, "32%", "31.5 는 반올림되어 32")
        XCTAssertNil(v.fallbackText, "동기화됐으면 폴백 문구는 없다")
    }

    func testShowsFallbackBeforeSync() {
        let v = QuotaBarView()
        v.configure(tokens5h: 48210, autoPct: nil, weeklyPct: nil, isReset5h: false)
        XCTAssertNil(v.fivePercentText)
        XCTAssertEqual(v.fallbackText, "5h 토큰: 48.2k · 동기화 전")
    }

    func testWeeklyRowHiddenWhenWeeklyMissing() {
        let v = QuotaBarView()
        v.configure(tokens5h: 0, autoPct: 50, weeklyPct: nil, isReset5h: false)
        XCTAssertEqual(v.fivePercentText, "50%")
        XCTAssertNil(v.weeklyPercentText, "주간 값이 없으면 주간 줄 자체가 없다")
    }

    func testResetShowsZeroPercentNotFallback() {
        let v = QuotaBarView()
        v.configure(tokens5h: 100, autoPct: 62, weeklyPct: nil, isReset5h: true)
        XCTAssertEqual(v.fivePercentText, "0%")
        XCTAssertNil(v.fallbackText)
    }

    func testResetWithoutPriorSyncStillShowsZero() {
        let v = QuotaBarView()
        v.configure(tokens5h: 100, autoPct: nil, weeklyPct: nil, isReset5h: true)
        XCTAssertEqual(v.fivePercentText, "0%", "원본은 reset_5h 를 먼저 평가한다")
    }

    func testPercentUsesAwayFromZeroRoundingLikeSource() {
        let v = QuotaBarView()
        // 짝수 반올림이면 "30%", JS toFixed 와 맞추면 "31%" 다.
        v.configure(tokens5h: 0, autoPct: 30.5, weeklyPct: nil, isReset5h: false)
        XCTAssertEqual(v.fivePercentText, "31%")
    }

    func testFillWidthEncodesPercentAfterLayout() {
        let v = QuotaBarView()
        v.configure(tokens5h: 0, autoPct: 50, weeklyPct: nil, isReset5h: false)
        v.frame = CGRect(x: 0, y: 0, width: 200, height: 60)
        v.layoutIfNeeded()
        // 채움 막대가 트랙의 절반이어야 한다.
        XCTAssertEqual(v.fiveFillRatio ?? -1, 0.5, accuracy: 0.02)
    }

    func testFillWidthIsZeroAtZeroPercent() {
        let v = QuotaBarView()
        v.configure(tokens5h: 0, autoPct: 0, weeklyPct: nil, isReset5h: false)
        v.frame = CGRect(x: 0, y: 0, width: 200, height: 60)
        v.layoutIfNeeded()
        XCTAssertEqual(v.fiveFillRatio ?? -1, 0, accuracy: 0.02)
    }

    func testFillWidthIsFullAtHundredPercent() {
        let v = QuotaBarView()
        v.configure(tokens5h: 0, autoPct: 100, weeklyPct: nil, isReset5h: false)
        v.frame = CGRect(x: 0, y: 0, width: 200, height: 60)
        v.layoutIfNeeded()
        XCTAssertEqual(v.fiveFillRatio ?? -1, 1.0, accuracy: 0.02)
    }

    /// 조회 실패 중에는 값이 있어도 %·막대를 숨기고 로컬 토큰 수만 남긴다.
    /// 로컬 토큰은 서버 한도가 아니라 직접 센 값이라 계속 유효하다.
    func testUnreadableHidesBarsAndSaysWhy() {
        let v = QuotaBarView()
        v.configure(tokens5h: 48210, autoPct: 62.4, weeklyPct: 31.5, isReset5h: false, unreadable: true)
        XCTAssertNil(v.fivePercentText)
        XCTAssertNil(v.weeklyPercentText)
        XCTAssertEqual(v.fallbackText, "5h 토큰: 48.2k · 한도 조회 실패")
    }

    /// unreadable 기본값이 false 라 기존 호출부는 그대로 동작해야 한다.
    func testUnreadableDefaultsToFalse() {
        let v = QuotaBarView()
        v.configure(tokens5h: 48210, autoPct: 62.4, weeklyPct: nil, isReset5h: false)
        XCTAssertEqual(v.fivePercentText, "62%")
        XCTAssertNil(v.fallbackText)
    }
}

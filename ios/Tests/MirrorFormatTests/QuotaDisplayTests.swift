import XCTest
@testable import MirrorFormat

final class QuotaDisplayTests: XCTestCase {

    // QuotaBar.svelte 의 color() 임계치와 정확히 일치해야 한다
    func testGradientThresholds() {
        XCTAssertEqual(QuotaDisplay.gradient(forPercent: 0).startHex, 0x30d158)
        XCTAssertEqual(QuotaDisplay.gradient(forPercent: 0).endHex, 0x34c759)

        XCTAssertEqual(QuotaDisplay.gradient(forPercent: 69.9).endHex, 0x34c759, "70 미만은 녹색 계열")

        XCTAssertEqual(QuotaDisplay.gradient(forPercent: 70).startHex, 0x30d158, "70 이상은 녹→주황")
        XCTAssertEqual(QuotaDisplay.gradient(forPercent: 70).endHex, 0xff9f0a)
        XCTAssertEqual(QuotaDisplay.gradient(forPercent: 89.9).endHex, 0xff9f0a)

        XCTAssertEqual(QuotaDisplay.gradient(forPercent: 90).startHex, 0xff9f0a, "90 이상은 주황→빨강")
        XCTAssertEqual(QuotaDisplay.gradient(forPercent: 90).endHex, 0xff453a)
        XCTAssertEqual(QuotaDisplay.gradient(forPercent: 100).endHex, 0xff453a)
    }

    func testDisplayPercentClampsToHundred() {
        XCTAssertEqual(QuotaDisplay.displayPercent(autoPct: 137.0, isReset: false), 100,
                       "원본이 Math.min(100, …) 으로 자른다")
    }

    func testDisplayPercentIsZeroRightAfterReset() {
        XCTAssertEqual(QuotaDisplay.displayPercent(autoPct: 62.0, isReset: true), 0,
                       "리셋 직후에는 백엔드 갱신 전까지 0% 로 보여준다")
    }

    func testDisplayPercentNilBeforeSync() {
        XCTAssertNil(QuotaDisplay.displayPercent(autoPct: nil, isReset: false),
                     "동기화 전이면 바 대신 토큰 합계를 보여줘야 하므로 nil")
    }

    func testDisplayPercentResetWinsOverNil() {
        XCTAssertEqual(QuotaDisplay.displayPercent(autoPct: nil, isReset: true), 0,
                       "원본은 reset_5h 를 먼저 평가한다")
    }

    func testIsReset5h() {
        let now = Date(timeIntervalSince1970: 1_000_000)
        XCTAssertFalse(QuotaDisplay.isReset5h(resetAt: nil, now: now), "리셋 시각을 모르면 리셋 아님")
        XCTAssertFalse(QuotaDisplay.isReset5h(resetAt: 1_000_001, now: now))
        XCTAssertTrue(QuotaDisplay.isReset5h(resetAt: 1_000_000, now: now), "남은 시간 0 이하면 리셋됨")
        XCTAssertTrue(QuotaDisplay.isReset5h(resetAt: 999_999, now: now))
    }
}

import XCTest
@testable import MirrorFormat

final class MirrorFormatTests: XCTestCase {

    // MARK: tokensPerSec — format.ts 의 formatTokensPerSec 와 동일해야 한다

    func testTokensPerSecUnderOneIsZero() {
        XCTAssertEqual(MirrorFormat.tokensPerSec(0), "0")
        XCTAssertEqual(MirrorFormat.tokensPerSec(0.9), "0")
    }

    func testTokensPerSecUnderThousandHasNoDecimals() {
        XCTAssertEqual(MirrorFormat.tokensPerSec(1), "1")
        XCTAssertEqual(MirrorFormat.tokensPerSec(123.5), "124", "toFixed(0) 는 반올림한다")
        XCTAssertEqual(MirrorFormat.tokensPerSec(999.4), "999")
    }

    func testTokensPerSecThousandAndAboveUsesK() {
        XCTAssertEqual(MirrorFormat.tokensPerSec(1000), "1.0k")
        XCTAssertEqual(MirrorFormat.tokensPerSec(1234), "1.2k")
        XCTAssertEqual(MirrorFormat.tokensPerSec(15678), "15.7k")
    }

    // MARK: tokensTotal — formatTokensTotal 와 동일

    func testTokensTotalBoundaries() {
        XCTAssertEqual(MirrorFormat.tokensTotal(0), "0")
        XCTAssertEqual(MirrorFormat.tokensTotal(999), "999")
        XCTAssertEqual(MirrorFormat.tokensTotal(1000), "1.0k")
        XCTAssertEqual(MirrorFormat.tokensTotal(999_999), "1000.0k", "100만 미만은 k 로 유지된다")
        XCTAssertEqual(MirrorFormat.tokensTotal(1_000_000), "1.00M")
        XCTAssertEqual(MirrorFormat.tokensTotal(2_500_000), "2.50M")
    }

    // MARK: relativeTime — relativeTime 와 동일 (영문 그대로)

    func testRelativeTimeBuckets() {
        let now = Date(timeIntervalSince1970: 1_000_000)
        func at(_ agoSecs: UInt64) -> String {
            MirrorFormat.relativeTime(1_000_000 - agoSecs, now: now)
        }
        XCTAssertEqual(at(0), "just now")
        XCTAssertEqual(at(4), "just now")
        XCTAssertEqual(at(5), "5s ago")
        XCTAssertEqual(at(59), "59s ago")
        XCTAssertEqual(at(60), "1m ago")
        XCTAssertEqual(at(3599), "59m ago")
        XCTAssertEqual(at(3600), "1h ago")
        XCTAssertEqual(at(7200), "2h ago")
    }

    func testRelativeTimeFutureDoesNotUnderflow() {
        let now = Date(timeIntervalSince1970: 1_000_000)
        XCTAssertEqual(
            MirrorFormat.relativeTime(1_000_050, now: now),
            "just now",
            "미래 시각이 와도 UInt64 언더플로로 크래시하면 안 된다"
        )
    }

    // MARK: countdown — AgentCard.svelte 의 countdown 파생과 동일

    func testCountdownFormats() {
        let now = Date(timeIntervalSince1970: 1_000_000)
        XCTAssertEqual(MirrorFormat.countdown(resetAt: 1_000_000 + 3661, now: now), "약 1시간 1분 1초 남음")
        XCTAssertEqual(MirrorFormat.countdown(resetAt: 1_000_000 + 61, now: now), "약 1분 1초 남음",
                       "1시간 미만이면 시간 부분을 생략한다")
        XCTAssertEqual(MirrorFormat.countdown(resetAt: 1_000_000 + 1, now: now), "약 0분 1초 남음")
    }

    func testCountdownAtOrPastResetSaysReset() {
        let now = Date(timeIntervalSince1970: 1_000_000)
        XCTAssertEqual(MirrorFormat.countdown(resetAt: 1_000_000, now: now), "리셋됨")
        XCTAssertEqual(MirrorFormat.countdown(resetAt: 999_000, now: now), "리셋됨")
    }
}

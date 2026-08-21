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

    func testTokensPerSecUsesAwayFromZeroRoundingLikeJS() {
        // 짝수 반올림이면 122 가 나온다. JS toFixed 와 맞추려면 123 이어야 한다.
        XCTAssertEqual(MirrorFormat.tokensPerSec(122.5), "123")
        XCTAssertEqual(MirrorFormat.tokensPerSec(2.5), "3")
        XCTAssertEqual(MirrorFormat.tokensPerSec(123.5), "124")
    }

    /// 오늘 와이어로는 NaN 이 올 수 없다(JSONDecoder 가 숫자가 아닌 리터럴에서 던지고
    /// JSON 자체에 NaN 표현이 없다) — 그래도 동점 판정 로직이 여기서 트랩하면
    /// 화면 전체가 죽으므로 방어적으로 트랩하지 않는지 확인한다. 값 자체는
    /// String(format:) 의 플랫폼 표기(nan/inf)를 그대로 노출하는 것으로 충분하다.
    func testTokensPerSecDoesNotCrashOnNonFiniteInput() {
        XCTAssertEqual(MirrorFormat.tokensPerSec(Float.nan), "nank")
        XCTAssertEqual(MirrorFormat.tokensPerSec(Float.infinity), "infk")
        // 음의 무한대/음의 0은 모두 `v < 1` 분기에서 걸려 toFixed 까지 가지도 않는다.
        XCTAssertEqual(MirrorFormat.tokensPerSec(-Float.infinity), "0")
        XCTAssertEqual(MirrorFormat.tokensPerSec(-0.0), "0")
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

    func testTokensTotalUsesAwayFromZeroRoundingLikeJS() {
        // 평범한 정수 입력에서 갈리던 지점: 1250/1000 = 1.25 는 진짜 동점(tie)이라
        // 짝수 반올림이면 "1.2k", away-from-zero(JS toFixed)면 "1.3k".
        XCTAssertEqual(MirrorFormat.tokensTotal(1250), "1.3k")
        XCTAssertEqual(MirrorFormat.tokensTotal(1_250_000), "1.25M")
        // 1_255_000/1_000_000 은 겉보기와 달리 배정밀도로 정확히 1.255 가 아니라
        // 1.2549999999999998934... 이므로 반올림 방식과 무관하게 "1.25M" 가 맞다
        // (Node 로 (1255000/1000000).toFixed(2) 를 실제 실행해 확인함).
        XCTAssertEqual(MirrorFormat.tokensTotal(1_255_000), "1.25M")
    }

    // MARK: relativeTime — src/lib/format.ts 와 동일한 한글 표기

    func testRelativeTimeBuckets() {
        let now = Date(timeIntervalSince1970: 1_000_000)
        func at(_ agoSecs: UInt64) -> String {
            MirrorFormat.relativeTime(1_000_000 - agoSecs, now: now)
        }
        XCTAssertEqual(at(0), "방금 전")
        XCTAssertEqual(at(4), "방금 전")
        XCTAssertEqual(at(5), "5초 전")
        XCTAssertEqual(at(59), "59초 전")
        XCTAssertEqual(at(60), "1분 전")
        XCTAssertEqual(at(3599), "59분 전")
        XCTAssertEqual(at(3600), "1시간 전")
        XCTAssertEqual(at(7200), "2시간 전")
    }

    func testRelativeTimeFutureDoesNotUnderflow() {
        let now = Date(timeIntervalSince1970: 1_000_000)
        XCTAssertEqual(
            MirrorFormat.relativeTime(1_000_050, now: now),
            "방금 전",
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

    // MARK: 골든 벡터 — Node 로 실제 실행한 format.ts 결과와 대량 대조
    //
    // 손으로 쓴 위 테스트들은 의도를 문서화하는 것이고, 진짜 반올림 일치를 증명하는
    // 것은 이 테이블이다. `docs/ble-protocol/golden/generate-format-parity.mjs` 를
    // Node 로 실행해 만든 JSON 을 그대로 읽어 대조한다 — 기대값을 Swift 쪽에서
    // 다시 계산하지 않는다(그러면 같은 실수를 두 번 할 수 있다).

    private struct ParityTable: Decodable {
        struct TotalCase: Decodable { let n: UInt32; let expected: String }
        struct RateCase: Decodable { let v: Double; let expected: String }
        let tokensTotal: [TotalCase]
        let tokensPerSec: [RateCase]
    }

    private func loadParityTable() throws -> ParityTable {
        let url = try XCTUnwrap(
            Bundle(for: Self.self).url(forResource: "format-parity", withExtension: "json"),
            "골든 벡터가 테스트 번들에 없다"
        )
        return try JSONDecoder().decode(ParityTable.self, from: Data(contentsOf: url))
    }

    func testTokensTotalMatchesGoldenVectors() throws {
        let table = try loadParityTable()
        XCTAssertGreaterThan(table.tokensTotal.count, 1000, "골든 벡터가 예상보다 너무 적다")
        for c in table.tokensTotal {
            XCTAssertEqual(MirrorFormat.tokensTotal(c.n), c.expected, "n=\(c.n)")
        }
    }

    func testTokensPerSecMatchesGoldenVectors() throws {
        let table = try loadParityTable()
        XCTAssertGreaterThan(table.tokensPerSec.count, 1000, "골든 벡터가 예상보다 너무 적다")
        for c in table.tokensPerSec {
            XCTAssertEqual(MirrorFormat.tokensPerSec(Float(c.v)), c.expected, "v=\(c.v)")
        }
    }
}

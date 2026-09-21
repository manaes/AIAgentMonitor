import XCTest
import Wire
@testable import Fleet

/// 통합 보기의 합산 규칙(Ruling 40) 스펙. **%는 계정 단위라 합치면 안 되고, 토큰은
/// 로컬 사용량이라 합쳐야 한다** — 이 둘이 뒤바뀌면 화면의 숫자가 거짓이 된다.
final class UnifiedPresentationTests: XCTestCase {

    private let now = Date(timeIntervalSince1970: 1_758_000_600)

    private func device(_ hex: String, label: String? = nil) -> Device {
        Device(endpointIdHex: hex, token: "t", relayUrl: nil, addresses: [],
               macHostname: nil, userLabel: label, sortIndex: 0)
    }

    /// `agentsJSON` 은 `a` 배열의 원소들(쉼표로 이어 붙인 것).
    private func cached(_ agentsJSON: String, minutesAgo: Int = 0) throws -> CachedSnapshot {
        let json = #"{"v":1,"t":1758000000,"a":[\#(agentsJSON)]}"#
        let snapshot = try JSONDecoder().decode(MirrorSnapshot.self, from: Data(json.utf8))
        return CachedSnapshot(
            snapshot: snapshot,
            fetchedAt: now.addingTimeInterval(TimeInterval(-60 * minutesAgo))
        )
    }

    // MARK: - Ruling 40 — 합칠 값과 하나만 고를 값

    /// %는 계정 단위 서버 한도다. 두 Mac 이 같은 계정을 보고해도 42%+40%=82% 가 아니라
    /// 가장 신선한 쪽의 42% 하나여야 한다.
    func testPercentIsTakenFromFreshestNotSummed() throws {
        let rows = UnifiedPresentation.agentRows(
            sources: [
                (status: .online, cached: try cached(#"{"k":0,"r":1,"t5":10,"p5":40,"pw":30,"pj":[]}"#, minutesAgo: 5)),
                (status: .online, cached: try cached(#"{"k":0,"r":2,"t5":20,"p5":42,"pw":33,"pj":[]}"#, minutesAgo: 0)),
            ],
            now: now
        )

        XCTAssertEqual(rows.count, 1)
        XCTAssertEqual(rows[0].usedPct5h, 42)
        XCTAssertEqual(rows[0].usedPctWeekly, 33)
    }

    /// t5 는 그 Mac 에서 쓴 로컬 토큰이라 합산해야 한다.
    func testTokensAreSummedAcrossDevices() throws {
        let rows = UnifiedPresentation.agentRows(
            sources: [
                // 신선한 쪽을 100 으로 둔다 — "신선한 것 하나만"으로 잘못 구현하면 350 이 아니라 100 이 된다.
                (status: .online, cached: try cached(#"{"k":0,"r":1,"t5":100,"p5":40,"pj":[]}"#, minutesAgo: 0)),
                (status: .online, cached: try cached(#"{"k":0,"r":2,"t5":250,"p5":40,"pj":[]}"#, minutesAgo: 5)),
            ],
            now: now
        )

        XCTAssertEqual(rows.count, 1)
        XCTAssertEqual(rows[0].tokens5h, 350)
    }

    /// **"가장 신선한" 은 오직 시각 하나로 정한다 — 장치 상태로 거르지 않는다.**
    /// 계정 한도는 그 Mac 이 꺼졌다고 틀려지지 않으므로, 2초 전에 꺼진 Mac 이 보낸 값도
    /// 그대로 유효하다. 온라인을 우선하면 오히려 스트림이 느린 장치의 **더 오래된** 값을
    /// 고르게 될 수 있다.
    func testFreshestValueWinsRegardlessOfDeviceStatus() throws {
        let rows = UnifiedPresentation.agentRows(
            sources: [
                (status: .offline, cached: try cached(#"{"k":0,"r":0,"t5":10,"p5":99,"pj":[]}"#, minutesAgo: 0)),
                (status: .online, cached: try cached(#"{"k":0,"r":1,"t5":20,"p5":42,"pj":[]}"#, minutesAgo: 30)),
            ],
            now: now
        )

        XCTAssertEqual(rows.count, 1)
        // 오프라인 쪽이 더 최근이므로 그 값을 쓴다.
        XCTAssertEqual(rows[0].usedPct5h, 99)
        // 토큰은 상태와 무관하게 전부 합산한다.
        XCTAssertEqual(rows[0].tokens5h, 30)
    }

    /// 상태가 같아도 규칙은 그대로 — `fetchedAt` 이 가장 최근인 쪽이다.
    func testFreshestWinsAmongOfflineDevices() throws {
        let rows = UnifiedPresentation.agentRows(
            sources: [
                (status: .offline, cached: try cached(#"{"k":0,"r":0,"t5":10,"p5":11,"pj":[]}"#, minutesAgo: 60)),
                (status: .offline, cached: try cached(#"{"k":0,"r":0,"t5":10,"p5":22,"pj":[]}"#, minutesAgo: 5)),
            ],
            now: now
        )

        XCTAssertEqual(rows[0].usedPct5h, 22)
    }

    func testDeviceCountCountsOnlyReportingDevices() throws {
        let rows = UnifiedPresentation.agentRows(
            sources: [
                (status: .online, cached: try cached(#"{"k":0,"r":1,"t5":10,"p5":40,"pj":[]}"#)),
                (status: .online, cached: try cached(#"{"k":0,"r":1,"t5":10,"p5":40,"pj":[]}"#)),
                // 이 장치는 Codex 만 보고한다 — Claude 쪽 대수에 들어가면 안 된다.
                (status: .online, cached: try cached(#"{"k":1,"r":1,"t5":10,"p5":50,"pj":[]}"#)),
                // 스냅샷이 아예 없는 장치는 어느 쪽도 보고하지 않은 것이다.
                (status: .offline, cached: nil),
            ],
            now: now
        )

        XCTAssertEqual(rows.map(\.name), ["Claude Code", "Codex"])
        XCTAssertEqual(rows[0].deviceCount, 2)
        XCTAssertEqual(rows[1].deviceCount, 1)
    }

    /// 조회 실패 상태는 값을 채택한 스냅샷의 것을 따른다 — 신선한 쪽이 실패 중이면
    /// 낡은 쪽이 멀쩡하다고 해서 정상으로 보여선 안 된다.
    func testQuotaErrorFollowsAdoptedSnapshot() throws {
        let rows = UnifiedPresentation.agentRows(
            sources: [
                (status: .online, cached: try cached(#"{"k":0,"r":1,"t5":10,"p5":40,"pj":[]}"#, minutesAgo: 5)),
                (status: .online, cached: try cached(#"{"k":0,"r":1,"t5":20,"p5":40,"e":1,"pj":[]}"#, minutesAgo: 0)),
            ],
            now: now
        )

        XCTAssertTrue(rows[0].unreadable)
    }

    /// 리셋 판정도 값을 채택한 스냅샷의 `r5` 로 한다.
    func testResetFlagFollowsAdoptedSnapshot() throws {
        let past = UInt64(now.timeIntervalSince1970) - 10
        let rows = UnifiedPresentation.agentRows(
            sources: [
                (status: .online, cached: try cached(#"{"k":0,"r":1,"t5":10,"p5":40,"r5":\#(past),"pj":[]}"#)),
            ],
            now: now
        )

        XCTAssertTrue(rows[0].isReset5h)
    }

    /// 목록/통합에서 에이전트 자리가 뒤바뀌지 않게 개별 보기와 같은 기준을 쓴다.
    func testAgentOrderMatchesIndividualView() throws {
        let rows = UnifiedPresentation.agentRows(
            sources: [
                // 일부러 역순으로 보고하게 둔다.
                (status: .online, cached: try cached(#"{"k":2,"r":1,"t5":10,"pj":[]},{"k":1,"r":1,"t5":10,"pj":[]}"#)),
                (status: .online, cached: try cached(#"{"k":0,"r":1,"t5":10,"pj":[]}"#)),
            ],
            now: now
        )

        let expected = DeviceListPresentation.orderedForDisplay([
            try agent(#"{"k":2,"r":1,"t5":10,"pj":[]}"#),
            try agent(#"{"k":1,"r":1,"t5":10,"pj":[]}"#),
            try agent(#"{"k":0,"r":1,"t5":10,"pj":[]}"#),
        ]).map { DeviceListPresentation.agentName($0.kind) }

        XCTAssertEqual(rows.map(\.name), expected)
        XCTAssertEqual(rows.map(\.name), ["Claude Code", "Codex", "Antigravity"])
    }

    // MARK: - 하단 장치별 속도

    /// tok/s 는 지금 이 순간의 값이라 오프라인이면 아무것도 보여주지 않는다.
    func testOfflineDeviceHasNoRates() throws {
        let row = UnifiedPresentation.deviceRow(
            device: device("aabbccdd", label: "작업실"),
            status: .offline,
            cached: try cached(#"{"k":0,"r":12.5,"t5":10,"p5":40,"pj":[]}"#, minutesAgo: 12)
        )

        XCTAssertEqual(row.title, "작업실")
        XCTAssertEqual(row.statusText, "오프라인")
        XCTAssertEqual(row.rates, [])
    }

    func testOnlineDeviceShowsRatePerAgent() throws {
        let row = UnifiedPresentation.deviceRow(
            device: device("aabbccdd"),
            status: .online,
            cached: try cached(#"{"k":1,"r":3,"t5":10,"pj":[]},{"k":0,"r":12.5,"t5":10,"pj":[]}"#)
        )

        XCTAssertEqual(row.id, "aabbccdd")
        XCTAssertEqual(row.statusText, "연결됨")
        // 개별 보기와 같은 순서 — Claude 가 먼저다.
        XCTAssertEqual(row.rates.map(\.name), ["Claude Code", "Codex"])
        XCTAssertEqual(row.rates.map(\.rateValueText), ["13", "3"], "단위는 화면이 붙인다 — 여기는 숫자만")
    }

    /// 스냅샷을 아직 한 번도 못 받은 온라인 장치는 빈 배열이다.
    func testOnlineDeviceWithoutSnapshotHasNoRates() {
        let row = UnifiedPresentation.deviceRow(
            device: device("aabbccdd"), status: .online, cached: nil
        )
        XCTAssertEqual(row.rates, [])
    }

    private func agent(_ json: String) throws -> MirrorAgent {
        try JSONDecoder().decode(MirrorAgent.self, from: Data(json.utf8))
    }
}

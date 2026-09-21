import XCTest
import Wire
@testable import Fleet

final class DeviceListPresentationTests: XCTestCase {

    private let now = Date(timeIntervalSince1970: 1_758_000_600)

    private func device(_ hex: String, host: String? = nil, label: String? = nil) -> Device {
        Device(endpointIdHex: hex, token: "t", relayUrl: nil, addresses: [],
               macHostname: host, userLabel: label, sortIndex: 0)
    }

    private func cached(agentJSON: String, minutesAgo: Int = 0) throws -> CachedSnapshot {
        let json = #"{"v":1,"t":1758000000,"a":[\#(agentJSON)]}"#
        let snapshot = try JSONDecoder().decode(MirrorSnapshot.self, from: Data(json.utf8))
        return CachedSnapshot(
            snapshot: snapshot,
            fetchedAt: now.addingTimeInterval(TimeInterval(-60 * minutesAgo))
        )
    }

    // MARK: - 이름

    func testTitlePrefersUserLabelOverHostname() {
        let row = DeviceListPresentation.row(
            device: device("aabbccdd", host: "호스트", label: "작업실"),
            status: .online, cached: nil, now: now
        )
        XCTAssertEqual(row.title, "작업실")
    }

    func testTitleFallsBackToEndpointPrefix() {
        let row = DeviceListPresentation.row(
            device: device("aabbccddee"), status: .online, cached: nil, now: now
        )
        XCTAssertEqual(row.title, "aabbccdd")
    }

    // MARK: - 오프라인 표시 규칙

    /// 오프라인이면 tok/s 는 감추고 한도 %는 남긴다.
    func testOfflineHidesRateButKeepsQuota() throws {
        let row = DeviceListPresentation.row(
            device: device("aa"),
            status: .offline,
            cached: try cached(agentJSON: #"{"k":0,"r":12.5,"t5":100,"p5":40,"pw":80,"pj":[]}"#, minutesAgo: 12),
            now: now
        )

        XCTAssertNil(row.agents[0].rateText)
        XCTAssertEqual(row.agents[0].fiveHour?.percentText, "40%")
        XCTAssertEqual(row.agents[0].weekly?.percentText, "80%")
        XCTAssertEqual(row.freshnessText, "12분 전")
    }

    func testOnlineShowsRate() throws {
        let row = DeviceListPresentation.row(
            device: device("aa"),
            status: .online,
            cached: try cached(agentJSON: #"{"k":0,"r":12.5,"t5":100,"p5":40,"pj":[]}"#),
            now: now
        )
        XCTAssertEqual(row.agents[0].rateText, "13 tok/s")
    }

    /// quotaError 가 있으면 한도 %를 감춘다 — 맥은 실패 중에도 마지막 %를 보내지만
    /// 그 숫자는 현재 상태를 말해주지 않는다(MirrorSnapshot 문서).
    func testQuotaErrorHidesPercent() throws {
        let row = DeviceListPresentation.row(
            device: device("aa"),
            status: .online,
            cached: try cached(agentJSON: #"{"k":0,"r":1,"t5":0,"p5":44,"e":1,"pj":[]}"#),
            now: now
        )
        XCTAssertNil(row.agents[0].fiveHour)
    }

    /// weekly 도 fiveHour 와 같은 quotaError 를 공유한다 — `pw` 가 있어도 `e` 가 있으면
    /// "데이터가 애초에 없어서" 가 아니라 "에러라서" nil 이어야 한다.
    func testQuotaErrorHidesWeeklyPercentToo() throws {
        let row = DeviceListPresentation.row(
            device: device("aa"),
            status: .online,
            cached: try cached(agentJSON: #"{"k":0,"r":1,"t5":0,"p5":44,"pw":60,"e":1,"pj":[]}"#),
            now: now
        )
        XCTAssertNil(row.agents[0].fiveHour)
        XCTAssertNil(row.agents[0].weekly)
    }

    /// 100% 를 넘는 값이 들어와도(맥 쪽 반올림/경계 오차 등) 화면에는 100% 로 clamp 한다.
    func testQuotaPercentClampsAt100() throws {
        let row = DeviceListPresentation.row(
            device: device("aa"),
            status: .online,
            cached: try cached(agentJSON: #"{"k":0,"r":1,"t5":0,"p5":150,"pj":[]}"#),
            now: now
        )
        XCTAssertEqual(row.agents[0].fiveHour?.percentText, "100%")
        XCTAssertEqual(row.agents[0].fiveHour?.percent, 100)
    }

    // MARK: - QuotaBarView 로 넘길 원값

    /// 5h 창이 이미 리셋됐으면 `QuotaDisplay.displayPercent` 가 0을 돌려줘야 하는데,
    /// 화면이 `isReset5h: false` 를 상수로 넘기면 리셋 전 낡은 %가 계속 보인다.
    /// 판정은 `now` 기준이므로 표현 계층이 계산해서 실어 보내야 한다.
    func testRowMarksFiveHourResetWhenResetAtIsPast() throws {
        let past = UInt64(now.timeIntervalSince1970) - 1
        let reset = DeviceListPresentation.row(
            device: device("aa"), status: .online,
            cached: try cached(agentJSON: #"{"k":0,"r":1,"t5":10,"p5":44,"r5":\#(past),"pj":[]}"#),
            now: now
        )
        XCTAssertTrue(reset.agents[0].isReset5h)

        let future = UInt64(now.timeIntervalSince1970) + 3600
        let notReset = DeviceListPresentation.row(
            device: device("aa"), status: .online,
            cached: try cached(agentJSON: #"{"k":0,"r":1,"t5":10,"p5":44,"r5":\#(future),"pj":[]}"#),
            now: now
        )
        XCTAssertFalse(notReset.agents[0].isReset5h)
    }

    /// "값이 없다" 와 "조회가 실패했다" 는 다르다 — 전자는 그 플랜에 없는 창이고
    /// 후자는 한도를 못 읽고 있는 상태라 안내 문구가 달라진다.
    func testRowMarksUnreadableWhenQuotaErrorPresent() throws {
        let failing = DeviceListPresentation.row(
            device: device("aa"), status: .online,
            cached: try cached(agentJSON: #"{"k":0,"r":1,"t5":10,"p5":44,"e":1,"pj":[]}"#),
            now: now
        )
        XCTAssertTrue(failing.agents[0].unreadable)

        let missing = DeviceListPresentation.row(
            device: device("aa"), status: .online,
            cached: try cached(agentJSON: #"{"k":0,"r":1,"t5":10,"pj":[]}"#),
            now: now
        )
        XCTAssertFalse(missing.agents[0].unreadable)
    }

    /// %가 없는 플랜에서는 `QuotaBarView` 가 토큰 수로 대체 문구를 만든다. 0을 넘기면
    /// 스냅샷에 들어 있는 실제 값이 버려진다.
    func testRowCarriesTokens5h() throws {
        let row = DeviceListPresentation.row(
            device: device("aa"), status: .online,
            cached: try cached(agentJSON: #"{"k":0,"r":1,"t5":123456,"pw":60,"pj":[]}"#),
            now: now
        )
        XCTAssertEqual(row.agents[0].tokens5h, 123_456)
        XCTAssertNil(row.agents[0].usedPct5h)
        XCTAssertEqual(row.agents[0].usedPctWeekly, 60)
    }

    // MARK: - 에이전트 개수 제한

    func testShowsAtMostTwoAgentsAndCountsTheRest() throws {
        let agents = #"{"k":0,"r":1,"t5":0,"pj":[]},{"k":1,"r":2,"t5":0,"pj":[]},{"k":2,"r":3,"t5":0,"pj":[]}"#
        let row = DeviceListPresentation.row(
            device: device("aa"), status: .online, cached: try cached(agentJSON: agents), now: now
        )

        XCTAssertEqual(row.agents.count, 2)
        XCTAssertEqual(row.hiddenAgentCount, 1)
    }

    /// 입력 순서를 일부러 뒤집는다(k:2 → k:1 → k:0) — `testShowsAtMostTwoAgentsAndCountsTheRest`
    /// 는 이미 claude→codex→기타 순으로 들어와서, orderedForDisplay 를 통과로 바꿔치기해도
    /// 통과했을 것이다. 여기서는 재정렬이 실제로 일어나는지를 확인한다.
    func testOrdersAgentsClaudeFirstThenCodexRegardlessOfInputOrder() throws {
        let agents = #"{"k":2,"r":3,"t5":0,"pj":[]},{"k":1,"r":2,"t5":0,"pj":[]},{"k":0,"r":1,"t5":0,"pj":[]}"#
        let row = DeviceListPresentation.row(
            device: device("aa"), status: .online, cached: try cached(agentJSON: agents), now: now
        )

        XCTAssertEqual(row.agents.map(\.name), ["Claude Code", "Codex"])
        XCTAssertEqual(row.hiddenAgentCount, 1)
    }

    // MARK: - 신선도

    /// `freshness()` 가 Date → epoch 변환을 거쳐 `MirrorFormat.relativeTime` 에 넘기는 접합부.
    /// 12분 전 케이스는 구현을 바꿔도(수동 계산 vs relativeTime) 값이 같아서 이 접합부 자체를
    /// 못 짚는다 — "N초 전" 구간은 두 방식이 갈리므로 여기서 고정한다.
    func testFreshnessShowsSecondsTierUnderAMinute() throws {
        let json = #"{"v":1,"t":1758000000,"a":[{"k":0,"r":1,"t5":0,"pj":[]}]}"#
        let snapshot = try JSONDecoder().decode(MirrorSnapshot.self, from: Data(json.utf8))
        let cachedSnapshot = CachedSnapshot(snapshot: snapshot, fetchedAt: now.addingTimeInterval(-30))

        let row = DeviceListPresentation.row(
            device: device("aa"), status: .offline, cached: cachedSnapshot, now: now
        )

        XCTAssertEqual(row.freshnessText, "30초 전")
    }

    // MARK: - 탭 목적지

    /// 스펙 §7 — "재페어링 필요는 탭하면 그 장치용 QR 스캐너". 그 상태만 재스캔으로 가고
    /// 나머지는 전부 상세다. 특히 `.versionMismatch` 는 같은 종단 상태지만 재스캔해도
    /// Mac 앱 버전이 바뀌지 않으므로 상세에서 안내해야 한다.
    ///
    /// 화면(`didSelectItemAt`)에는 테스트를 걸 수단이 없어서(AppMultiTests 타겟 없음)
    /// 판정만 순수 함수로 빼 두고 여기서 전 케이스를 고정한다.
    func testOnlyNeedsRepairingTapsIntoRePairing() {
        XCTAssertEqual(DeviceListPresentation.tapDestination(for: .needsRepairing), .rePair)

        let goesToDetail: [DeviceStatus] = [
            .idle, .probing, .online, .unstable(failureCount: 1),
            .unstable(failureCount: DeviceStatusMachine.failureThreshold - 1),
            .offline, .versionMismatch,
        ]
        for status in goesToDetail {
            XCTAssertEqual(
                DeviceListPresentation.tapDestination(for: status), .detail,
                "\(status) 를 재스캔으로 보냈다"
            )
        }
    }

    // MARK: - 정렬

    func testSortsOnlineThenUnstableThenOffline() {
        let sorted = DeviceListPresentation.sorted([
            (device("cc"), .offline),
            (device("aa"), .online),
            (device("bb"), .unstable(failureCount: 1)),
        ])
        XCTAssertEqual(sorted.map(\.endpointIdHex), ["aa", "bb", "cc"])
    }

    func testSortIndexBreaksTiesWithinAGroup() {
        var first = device("aa"); first.sortIndex = 5
        var second = device("bb"); second.sortIndex = 1

        let sorted = DeviceListPresentation.sorted([(first, .online), (second, .online)])

        XCTAssertEqual(sorted.map(\.endpointIdHex), ["bb", "aa"])
    }
}

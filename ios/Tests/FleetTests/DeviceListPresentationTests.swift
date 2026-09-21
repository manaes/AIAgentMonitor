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

    // MARK: - 재페어링 스캔 대상 검사

    /// 스펙 §7 — 재페어링은 "그 장치용" 스캐너다. 범용으로 두면 다른 Mac 의 QR 을 스캔했을 때
    /// 고치려던 장치는 망가진 채 두고 엉뚱한 장치가 새로 추가된다.
    func testAcceptsScannedDeviceOnlyMatchesExpectedDevice() {
        XCTAssertTrue(
            DeviceListPresentation.acceptsScannedDevice(expected: nil, scanned: "aabb"),
            "일반 추가(+) 경로는 기대 장치가 없으므로 무엇이든 통과해야 한다"
        )
        XCTAssertTrue(
            DeviceListPresentation.acceptsScannedDevice(expected: "aabb", scanned: "aabb")
        )
        XCTAssertFalse(
            DeviceListPresentation.acceptsScannedDevice(expected: "aabb", scanned: "ccdd"),
            "다른 Mac 의 QR 을 받아들였다"
        )
        XCTAssertTrue(
            DeviceListPresentation.acceptsScannedDevice(expected: "AABB", scanned: "aabb"),
            "대소문자만 다른 같은 장치를 남의 것으로 거절했다"
        )
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

    // MARK: - 드래그 재정렬 (스펙 §6.1)

    /// 표시 순서는 상태 그룹이 1차 키고 sortIndex 는 2차 키다. 그룹을 넘는 드래그를
    /// 허용하면 놓는 순간 `sorted` 가 곧바로 원래 그룹으로 돌려보내, 사용자에게는
    /// "드래그가 먹지 않는" 것으로 보인다. 그래서 놓기 전에 막는다.
    ///
    /// `tapDestination`·`acceptsScannedDevice` 와 같은 이유로 화면 밖에 있다 —
    /// AppMultiTests 타겟이 없어 화면 안에 두면 자동 테스트를 걸 수단이 없다.
    func testAllowsReorderOnlyWithinTheSameStatusGroup() {
        // 같은 그룹
        XCTAssertTrue(DeviceListPresentation.allowsReorder(from: .online, to: .online))
        XCTAssertTrue(DeviceListPresentation.allowsReorder(from: .offline, to: .idle))
        XCTAssertTrue(
            DeviceListPresentation.allowsReorder(from: .probing, to: .unstable(failureCount: 2))
        )
        XCTAssertTrue(
            DeviceListPresentation.allowsReorder(from: .needsRepairing, to: .versionMismatch)
        )

        // 그룹이 다르다
        XCTAssertFalse(DeviceListPresentation.allowsReorder(from: .online, to: .offline))
        XCTAssertFalse(
            DeviceListPresentation.allowsReorder(from: .unstable(failureCount: 1), to: .online)
        )
        XCTAssertFalse(DeviceListPresentation.allowsReorder(from: .offline, to: .needsRepairing))
    }
    // MARK: - 첫 확인 대기

    /// 첫 실행: 모든 장치가 확인 중이고 캐시도 없다 → 로딩 카드 한 장.
    func testAwaitingWhenEveryDeviceIsProbingWithoutCache() {
        XCTAssertTrue(DeviceListPresentation.isAwaitingFirstResult(sources: [
            (status: .probing, cached: nil),
            (status: .idle, cached: nil),
        ]))
    }

    /// 한 대라도 결과에 도달했으면 평소 레이아웃이 맞다 — 보여줄 게 생겼다.
    func testNotAwaitingOnceAnyDeviceSettles() {
        XCTAssertFalse(DeviceListPresentation.isAwaitingFirstResult(sources: [
            (status: .probing, cached: nil),
            (status: .offline, cached: nil),
        ]), "오프라인도 결과다 — 로딩으로 가리면 안 된다")
    }

    /// 캐시가 있으면 확인 중이어도 그 값을 보여줄 수 있다.
    func testNotAwaitingWhenCacheExists() throws {
        let snapshot = try JSONDecoder().decode(
            MirrorSnapshot.self,
            from: Data(#"{"v":1,"t":1758000000,"a":[{"k":0,"r":1,"t5":10,"pj":[]}]}"#.utf8)
        )
        XCTAssertFalse(DeviceListPresentation.isAwaitingFirstResult(sources: [
            (status: .probing, cached: CachedSnapshot(snapshot: snapshot, fetchedAt: Date())),
        ]))
    }

    /// 장치가 없으면 로딩이 아니라 빈 목록이다.
    func testNotAwaitingWithNoDevices() {
        XCTAssertFalse(DeviceListPresentation.isAwaitingFirstResult(sources: []))
    }

}

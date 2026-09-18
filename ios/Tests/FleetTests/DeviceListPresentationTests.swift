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

    // MARK: - 에이전트 개수 제한

    func testShowsAtMostTwoAgentsAndCountsTheRest() throws {
        let agents = #"{"k":0,"r":1,"t5":0,"pj":[]},{"k":1,"r":2,"t5":0,"pj":[]},{"k":2,"r":3,"t5":0,"pj":[]}"#
        let row = DeviceListPresentation.row(
            device: device("aa"), status: .online, cached: try cached(agentJSON: agents), now: now
        )

        XCTAssertEqual(row.agents.count, 2)
        XCTAssertEqual(row.hiddenAgentCount, 1)
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

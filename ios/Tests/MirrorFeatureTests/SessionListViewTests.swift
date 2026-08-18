import UIKit
import Wire
import XCTest
@testable import MirrorFeature

final class SessionListViewTests: XCTestCase {
    private let now = Date(timeIntervalSince1970: 1_000_000)

    private func snapshot(_ json: String) -> MirrorSnapshot {
        // swiftlint:disable:next force_try
        try! JSONDecoder().decode(MirrorSnapshot.self, from: Data(json.utf8))
    }

    func testEmptyMessageWhenNoSessions() {
        // 빈 상태 자체가 아니라 "행이 있다가 없어지는 전환" 을 검증한다. configure 호출
        // 없이 갓 만든 뷰는 두 단정 모두 우연히 통과하므로 그것만으로는 아무것도 증명하지
        // 못한다.
        let v = SessionListView()
        let withRows = #"{"v":1,"t":0,"a":[{"k":0,"r":0,"t5":0,"pj":[{"id":1,"n":"a","m":"m","r":0,"t":1,"s":0}]}]}"#
        v.configure(snapshot: snapshot(withRows), now: now)
        XCTAssertEqual(v.rowCount, 1)
        XCTAssertFalse(v.isEmptyMessageVisible)

        v.configure(snapshot: snapshot(#"{"v":1,"t":0,"a":[]}"#), now: now)
        XCTAssertEqual(v.rowCount, 0)
        XCTAssertTrue(v.isEmptyMessageVisible)
    }

    func testEmptyMessageWhenAgentsPresentButNoProjects() {
        // "에이전트 자체가 없음" 과는 다른 입력이다 — 에이전트는 있지만 프로젝트가
        // 하나도 없는 경우도 같은 빈 상태를 보여야 한다.
        let json = #"{"v":1,"t":0,"a":[{"k":0,"r":0,"t5":0,"pj":[]},{"k":1,"r":0,"t5":0,"pj":[]}]}"#
        let v = SessionListView()
        v.configure(snapshot: snapshot(json), now: now)
        XCTAssertEqual(v.rowCount, 0)
        XCTAssertTrue(v.isEmptyMessageVisible)
    }

    func testSortsByMostRecentActivityAcrossAgents() {
        let json = #"""
        {"v":1,"t":0,"a":[
          {"k":0,"r":0,"t5":0,"pj":[
            {"id":1,"n":"older-claude","m":"m","r":0,"t":999000,"s":1}]},
          {"k":1,"r":0,"t5":0,"pj":[
            {"id":2,"n":"newest-codex","m":"m","r":0,"t":999990,"s":1},
            {"id":3,"n":"middle-codex","m":"m","r":0,"t":999500,"s":1}]}
        ]}
        """#
        let v = SessionListView()
        v.configure(snapshot: snapshot(json), now: now)

        XCTAssertEqual(v.rowCount, 3)
        XCTAssertFalse(v.isEmptyMessageVisible)
        XCTAssertTrue(v.rowText(at: 0)?.contains("newest-codex") == true, "가장 최근 활동이 맨 위")
        XCTAssertTrue(v.rowText(at: 1)?.contains("middle-codex") == true)
        XCTAssertTrue(v.rowText(at: 2)?.contains("older-claude") == true)
    }

    func testRowsAreReusedNotAccumulatedAcrossConfigures() {
        let json = #"{"v":1,"t":0,"a":[{"k":0,"r":0,"t5":0,"pj":[{"id":1,"n":"a","m":"m","r":0,"t":1,"s":0}]}]}"#
        let v = SessionListView()
        v.configure(snapshot: snapshot(json), now: now)
        v.configure(snapshot: snapshot(json), now: now)
        v.configure(snapshot: snapshot(json), now: now)
        XCTAssertEqual(v.rowCount, 1, "1Hz 로 계속 들어오므로 행이 쌓이면 안 된다")
    }
}

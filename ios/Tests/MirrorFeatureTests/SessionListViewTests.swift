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
        let v = SessionListView()
        v.configure(snapshot: snapshot(#"{"v":1,"t":0,"a":[]}"#), now: now)
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

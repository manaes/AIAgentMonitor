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

    /// 구분선 규칙을 고정한다: 첫 번째로 보이는 행 위에는 구분선이 없고, 그 다음
    /// 행들 위에는 있어야 한다. 재사용 풀 때문에 세션 수가 줄어들면 남는 슬롯이
    /// 유령 구분선/공간을 남기지 않아야 한다는 것도 함께 확인한다.
    func testFirstRowHasNoSeparatorButLaterRowsDoAndHiddenSlotsCollapse() {
        let threeRows = #"""
        {"v":1,"t":0,"a":[
          {"k":0,"r":0,"t5":0,"pj":[
            {"id":1,"n":"a","m":"m","r":0,"t":3,"s":0},
            {"id":2,"n":"b","m":"m","r":0,"t":2,"s":0},
            {"id":3,"n":"c","m":"m","r":0,"t":1,"s":0}]}
        ]}
        """#
        let v = SessionListView()
        v.configure(snapshot: snapshot(threeRows), now: now)
        v.frame = CGRect(x: 0, y: 0, width: 337, height: 200)
        v.setNeedsLayout()
        v.layoutIfNeeded()

        XCTAssertEqual(v.separatorHeight(at: 0) ?? -1, 0, accuracy: 0.01, "첫 행 위에는 구분선이 없어야 한다")
        XCTAssertEqual(v.separatorHeight(at: 1) ?? -1, 1, accuracy: 0.01, "두 번째 행부터는 구분선이 있어야 한다")
        XCTAssertEqual(v.separatorHeight(at: 2) ?? -1, 1, accuracy: 0.01)

        // 세션이 1개로 줄어든다 — 풀의 슬롯 3개는 그대로 재사용되지만 2개는 숨어야 한다.
        let oneRow = #"{"v":1,"t":0,"a":[{"k":0,"r":0,"t5":0,"pj":[{"id":1,"n":"a","m":"m","r":0,"t":1,"s":0}]}]}"#
        v.configure(snapshot: snapshot(oneRow), now: now)
        v.setNeedsLayout()
        v.layoutIfNeeded()

        XCTAssertEqual(v.rowCount, 1)
        XCTAssertEqual(v.separatorHeight(at: 0) ?? -1, 0, accuracy: 0.01, "다시 첫 행이 됐으니 구분선이 없어야 한다")
        XCTAssertEqual(v.pooledSlotCount, 3, "풀은 이전 최대치를 그대로 재사용한다")

        // "숨은 여분 슬롯이 공간을 남기지 않는다" 는 슬롯 자신의 bounds 로는 증명할 수
        // 없다 — UIStackView 는 숨은 arranged subview 를 배치 계산에서만 빼고 그
        // 뷰 자신의 마지막 프레임까지 반드시 0 으로 만들지는 않는다(실측: 이전에
        // 두 번째/세 번째로 보였던 슬롯은 숨긴 뒤에도 자기 bounds 는 13pt 로 남아
        // 있었다). 대신 목록 전체가 필요로 하는 높이를, 처음부터 세션 1개로만
        // 만든 목록과 비교한다 — 재사용 풀에 죽은 슬롯 2개가 남아 있어도 두 높이가
        // 같아야 유령 공간이 없다고 말할 수 있다.
        let fittingSize = CGSize(width: 337, height: UIView.layoutFittingCompressedSize.height)
        let shrunkHeight = v.systemLayoutSizeFitting(
            fittingSize, withHorizontalFittingPriority: .required, verticalFittingPriority: .fittingSizeLevel
        ).height

        let freshSingleRow = SessionListView()
        freshSingleRow.configure(snapshot: snapshot(oneRow), now: now)
        let freshHeight = freshSingleRow.systemLayoutSizeFitting(
            fittingSize, withHorizontalFittingPriority: .required, verticalFittingPriority: .fittingSizeLevel
        ).height

        XCTAssertEqual(
            shrunkHeight, freshHeight, accuracy: 0.5,
            "3행에서 1행으로 줄었을 때, 풀에 남은 숨은 슬롯 2개가 처음부터 1행짜리로 " +
            "만든 목록보다 조금이라도 더 큰 높이를 차지하면 안 된다"
        )
    }
}

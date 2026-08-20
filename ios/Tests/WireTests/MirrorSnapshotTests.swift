import XCTest
@testable import Wire

final class MirrorSnapshotTests: XCTestCase {

    /// Rust 가 만든 골든 벡터를 Swift 가 그대로 디코딩할 수 있어야 한다.
    /// 이 테스트가 깨지면 두 언어의 DTO 가 어긋난 것이다.
    func testDecodesGoldenSnapshot() throws {
        let url = try XCTUnwrap(
            Bundle(for: Self.self).url(forResource: "snapshot-sample", withExtension: "json"),
            "골든 벡터가 테스트 번들에 없다. Project.swift 의 resources 설정을 확인하라"
        )
        let data = try Data(contentsOf: url)
        let snap = try JSONDecoder().decode(MirrorSnapshot.self, from: data)

        XCTAssertEqual(snap.v, 1)
        XCTAssertEqual(snap.t, 1_755_500_000)
        XCTAssertEqual(snap.a.count, 1)

        let agent = try XCTUnwrap(snap.a.first)
        XCTAssertEqual(agent.kind, .claude)
        XCTAssertEqual(agent.r, 123.5)
        XCTAssertEqual(agent.t5, 3_000)
        XCTAssertEqual(agent.p5, 62.0)
        XCTAssertEqual(agent.r5, 1_755_512_400)
        XCTAssertNil(agent.pw, "Rust 가 None 이면 키 자체를 생략한다")
        XCTAssertNil(agent.rw)

        let project = try XCTUnwrap(agent.pj.first)
        XCTAssertEqual(project.n, "foo")
        XCTAssertEqual(project.m, "claude-opus-5")
        XCTAssertEqual(project.status, .active)
    }

    /// 주간 쿼터가 실린 골든 벡터. 값이 없는 벡터만으로는 `pw`/`rw` 의 이름 변경이
    /// nil 디코딩과 구분되지 않아, 이름이 바뀌어도 테스트가 계속 통과한다.
    func testDecodesGoldenSnapshotWithWeeklyQuota() throws {
        let url = try XCTUnwrap(
            Bundle(for: Self.self).url(forResource: "snapshot-weekly-sample", withExtension: "json"),
            "주간 쿼터 골든 벡터가 테스트 번들에 없다. Project.swift 의 resources 설정을 확인하라"
        )
        let snap = try JSONDecoder().decode(MirrorSnapshot.self, from: Data(contentsOf: url))
        let agent = try XCTUnwrap(snap.a.first)

        let pw = try XCTUnwrap(agent.pw, "Rust 의 pw 키가 사라졌거나 이름이 바뀌었다")
        let rw = try XCTUnwrap(agent.rw, "Rust 의 rw 키가 사라졌거나 이름이 바뀌었다")
        XCTAssertEqual(pw, 41.5)
        XCTAssertEqual(rw, 1_755_900_000)
        XCTAssertEqual(agent.usedPctWeekly, 41.5)
        XCTAssertEqual(agent.resetAtWeekly, Date(timeIntervalSince1970: 1_755_900_000))
        // 5시간 필드도 그대로 살아 있어야 한다(두 벡터가 서로를 검증한다)
        XCTAssertEqual(agent.p5, 62.0)
        XCTAssertEqual(agent.r5, 1_755_512_400)
    }

    func testDecodesAntigravityAgent() throws {
        let json = #"{"v":1,"t":1,"a":[{"k":2,"r":10.5,"t5":1000,"p5":19.0,"pw":3.0,"pj":[]}]}"#
        let snap = try JSONDecoder().decode(MirrorSnapshot.self, from: Data(json.utf8))
        XCTAssertEqual(snap.a[0].kind, .antigravity)
        XCTAssertEqual(snap.a[0].usedPct5h, 19.0)
        XCTAssertEqual(snap.a[0].usedPctWeekly, 3.0)
    }

    func testDecodesUnknownCodesAsUnknown() throws {
        let json = #"{"v":1,"t":1,"a":[{"k":9,"r":0,"t5":0,"pj":[{"id":1,"n":"x","m":"m","r":0,"t":0,"s":99}]}]}"#
        let snap = try JSONDecoder().decode(MirrorSnapshot.self, from: Data(json.utf8))
        XCTAssertEqual(snap.a[0].kind, .unknown, "모르는 코드는 크래시가 아니라 unknown 이어야 한다")
        XCTAssertEqual(
            snap.a[0].pj[0].status, .unknown,
            "모르는 상태 코드를 dormant 로 뭉뚱그리면 UI 가 조용히 거짓말을 한다"
        )
        XCTAssertEqual(ActivityStatusCode(code: 2), .dormant, "2 는 여전히 dormant 다")
    }
}

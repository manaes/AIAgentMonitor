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

    func testDecodesUnknownStatusAsDormant() throws {
        let json = #"{"v":1,"t":1,"a":[{"k":9,"r":0,"t5":0,"pj":[{"id":1,"n":"x","m":"m","r":0,"t":0,"s":99}]}]}"#
        let snap = try JSONDecoder().decode(MirrorSnapshot.self, from: Data(json.utf8))
        XCTAssertEqual(snap.a[0].kind, .unknown, "모르는 코드는 크래시가 아니라 unknown 이어야 한다")
        XCTAssertEqual(snap.a[0].pj[0].status, .dormant)
    }
}

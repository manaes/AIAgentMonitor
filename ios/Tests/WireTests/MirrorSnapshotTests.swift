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

    /// 조회 실패 코드는 문장이 아니라 숫자로 온다 — Rust 쪽
    /// `quota_error_travels_as_a_code_not_a_message` 와 짝이다.
    func testDecodesQuotaErrorCode() throws {
        let json = #"{"v":1,"t":1,"a":[{"k":1,"r":0,"t5":0,"p5":8.0,"pw":35.0,"e":1,"pj":[]}]}"#
        let snap = try JSONDecoder().decode(MirrorSnapshot.self, from: Data(json.utf8))
        XCTAssertEqual(snap.a[0].quotaError, .auth)
        XCTAssertEqual(snap.a[0].quotaError?.displayText, "로그인 필요")
        // 맥은 실패 중에도 %를 함께 보낸다(구버전 CYD 호환) — 숨길지는 화면이 정한다.
        XCTAssertEqual(snap.a[0].usedPct5h, 8.0)
    }

    /// 정상일 때 맥이 키를 생략하므로 nil 이어야 한다. 여기가 깨지면
    /// 멀쩡한 카드에 경고가 뜬다.
    func testHealthyAgentHasNoQuotaError() throws {
        let json = #"{"v":1,"t":1,"a":[{"k":0,"r":0,"t5":0,"p5":10.0,"pj":[]}]}"#
        let snap = try JSONDecoder().decode(MirrorSnapshot.self, from: Data(json.utf8))
        XCTAssertNil(snap.a[0].quotaError)
    }

    /// 맥이 새 종류를 추가해도 "정상"으로 되돌아가면 안 된다 — 모르는 코드도
    /// "읽을 수 없다"는 사실만은 그대로 보여준다.
    func testUnknownQuotaErrorCodeStillShowsFailure() throws {
        let json = #"{"v":1,"t":1,"a":[{"k":0,"r":0,"t5":0,"e":99,"pj":[]}]}"#
        let snap = try JSONDecoder().decode(MirrorSnapshot.self, from: Data(json.utf8))
        XCTAssertEqual(snap.a[0].quotaError, .unknown)
        XCTAssertEqual(snap.a[0].quotaError?.displayText, "한도 조회 실패")
    }

    /// 코드↔의미 대응은 Rust 의 `error_kind_codes_are_frozen` 과 같은 계약이다.
    func testQuotaErrorCodeMappingIsFrozen() {
        XCTAssertEqual(QuotaErrorKindCode(code: 1), .auth)
        XCTAssertEqual(QuotaErrorKindCode(code: 2), .launch)
        XCTAssertEqual(QuotaErrorKindCode(code: 3), .timeout)
        XCTAssertEqual(QuotaErrorKindCode(code: 4), .other)
    }
}

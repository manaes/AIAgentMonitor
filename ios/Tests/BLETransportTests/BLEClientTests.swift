import XCTest
@testable import BLETransport

/// `BLEClient.handleAuthReply` 의 실제 분기는 `CBPeripheral`/`CBCharacteristic` 을
/// 직접 잡고 있어 호스트 없는 로직 테스트에서 검증할 수 없다. 그 결정만 뽑아낸
/// `BLEClient.decide(_:)` 를 여기서 고정한다 — 전체 브랜치 리뷰가 찾은 C-1(Rejected
/// 를 HELLO 자동 재전송으로 잘못 연결해 무한 루프를 만든 버그)이 5번의 개별 태스크
/// 리뷰를 전부 통과한 직접 원인이 이 사각지대였다.
@MainActor
final class BLEClientTests: XCTestCase {

    private func reply(
        ok: Bool = false, token: String? = nil, left: Int? = nil,
        awaiting: String? = nil, nonce: String? = nil
    ) -> AuthReplyPayload {
        AuthReplyPayload(ok: ok, token: token, left: left, awaiting: awaiting, nonce: nonce)
    }

    func testNonceTakesPriorityEvenThoughOkIsFalse() {
        // Nonce 는 ok:false 라서 다른 ok:false 갈래와 섞이면 안 된다 — 가장 먼저 확인해야 한다.
        XCTAssertEqual(
            BLEClient.decide(reply(ok: false, nonce: "7ac4e19b")),
            .signNonce(nonce: "7ac4e19b")
        )
    }

    func testGrantedStoresTokenAndSubscribes() {
        XCTAssertEqual(
            BLEClient.decide(reply(ok: true, token: "deadbeef")),
            .storeTokenAndSubscribe(token: "deadbeef")
        )
    }

    func testReauthSuccessSubscribesWithoutStoringAnyToken() {
        // Authorized: ok:true 인데 token 은 없다 — Granted 와 반드시 구분돼야 한다.
        XCTAssertEqual(BLEClient.decide(reply(ok: true)), .subscribe)
    }

    func testDeniedReportsRemainingAttempts() {
        XCTAssertEqual(BLEClient.decide(reply(ok: false, left: 2)), .failed(left: 2))
    }

    func testAwaitingCodeAsksForPairing() {
        XCTAssertEqual(BLEClient.decide(reply(ok: false, awaiting: "code")), .awaitCode)
    }

    /// Rejected — C-1 의 핵심. 이 갈래가 HELLO 자동 재전송을 만들면 무한 루프가
    /// 된다(고쳐졌다). 여기서는 최소한 "이 응답이 resetAndAwaitCode 로 분류된다"
    /// 는 것만 고정한다 — 실제 무한 루프 방지(HELLO 를 안 쓰는 것)는
    /// `handleAuthReply` 본문에 있고 CoreBluetooth 없이는 검증할 수 없다.
    func testBareRejectionResetsAndAwaitsCodeWithoutAutomaticResend() {
        XCTAssertEqual(BLEClient.decide(reply(ok: false)), .resetAndAwaitCode)
    }

    /// 6가지 실제 응답 모양이 서로 다른 6가지 액션으로 분류되는지 한 번에 확인한다
    /// — 두 모양이 우연히 같은 액션으로 뭉개지면(예: Nonce 와 Rejected 가 둘 다
    /// resetAndAwaitCode 로 갔던 초안의 버그) 이 테스트가 잡는다.
    func testAllSixReplyShapesMapToDistinctActions() {
        let shapes: [AuthReplyPayload] = [
            reply(ok: false, nonce: "n"),
            reply(ok: true, token: "t"),
            reply(ok: true),
            reply(ok: false, left: 1),
            reply(ok: false, awaiting: "code"),
            reply(ok: false),
        ]
        let actions = shapes.map(BLEClient.decide)
        XCTAssertEqual(Set(actions.map { "\($0)" }).count, 6, "여섯 모양은 서로 다른 액션이어야 한다: \(actions)")
    }
}

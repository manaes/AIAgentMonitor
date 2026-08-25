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
        awaiting: String? = nil, nonce: String? = nil,
        v: Int? = nil, epk: String? = nil, sealed: String? = nil
    ) -> AuthReplyPayload {
        AuthReplyPayload(
            ok: ok, token: token, left: left, awaiting: awaiting, nonce: nonce,
            v: v, epk: epk, sealed: sealed
        )
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

    // MARK: - v2 (`decideV2`)

    /// v2 는 다운그레이드하지 않는다. 거부당해도 v1 으로 물러서지 않는다 —
    /// 물러서면 공격자가 v2 를 방해해 평문으로 끌어내릴 수 있다(스펙 8장).
    ///
    /// `Rejected` 는 두 세대가 공유하는 응답이라 `"v":2` 가 실리지 않는다
    /// (`pairing.rs: to_json_bytes`). 그래서 "v2 가 아니다 → v1 으로 재시도"
    /// 라는 규칙을 세우면 정상적인 거절 하나로 평문 경로가 열린다.
    func testV2NeverFallsBackToV1() {
        for verb in [BLEClient.V2Verb.hello2, .code2, .auth2, .proof2] {
            XCTAssertEqual(
                BLEClient.decideV2(sent: verb, reply: reply(ok: false)),
                .needsPairing,
                "\(verb) 에 대한 거절은 v1 재시도가 아니라 정지여야 한다"
            )
        }
    }

    /// 다운그레이드가 실제로 성립하는 유일한 지점 — **평문 인가를 받아들이는 것**.
    /// `{"ok":true}` 와 `{"ok":true,"token":…}` 는 v1 이 인가를 알리는 두 모양이다.
    /// v2 클라이언트가 이걸 성공으로 읽으면 그 뒤 스냅샷을 평문으로 받게 된다.
    func testPlaintextAuthorizationIsRefused() {
        XCTAssertEqual(
            BLEClient.decideV2(sent: .proof2, reply: reply(ok: true)),
            .needsPairing,
            "v:2 없는 Authorized 는 평문 세션을 여는 것이라 받으면 안 된다"
        )
        XCTAssertEqual(
            BLEClient.decideV2(sent: .code2, reply: reply(ok: true, token: "deadbeef")),
            .needsPairing,
            "v:2 없는 Granted 는 토큰을 평문으로 받는 것이라 받으면 안 된다"
        )
    }

    /// `AwaitingCode2` 와 `Nonce2` 는 `await` 하나만 다르고 필드 구성이 같다.
    /// **보낸 동사로 갈라야 한다** — 필드로 갈랐다가는 페어링 경로와 재연결
    /// 경로가 뒤섞인다.
    func testAwaitingCode2AndNonce2AreToldApartByTheVerbWeSent() {
        let awaitingCode2 = reply(ok: false, awaiting: "code", nonce: "nn", v: 2, epk: "ee")
        let nonce2 = reply(ok: false, nonce: "nn", v: 2, epk: "ee")

        XCTAssertEqual(
            BLEClient.decideV2(sent: .hello2, reply: awaitingCode2),
            .bindCode(epk: "ee", nonce: "nn")
        )
        XCTAssertEqual(
            BLEClient.decideV2(sent: .auth2, reply: nonce2),
            .signSessionProof(epk: "ee", nonce: "nn")
        )
        // 같은 페이로드라도 보낸 동사가 다르면 결정도 달라야 한다. 필드로
        // 갈랐다면 이 두 단언이 서로 같은 값을 내며 통과해버린다.
        XCTAssertEqual(
            BLEClient.decideV2(sent: .auth2, reply: awaitingCode2),
            .signSessionProof(epk: "ee", nonce: "nn"),
            "`await` 필드가 붙어 있어도 AUTH2 를 보냈으면 재연결 경로다"
        )
        XCTAssertEqual(
            BLEClient.decideV2(sent: .hello2, reply: nonce2),
            .bindCode(epk: "ee", nonce: "nn"),
            "`await` 필드가 없어도 HELLO2 를 보냈으면 페어링 경로다"
        )
    }

    func testGranted2CarriesTheSealedToken() {
        XCTAssertEqual(
            BLEClient.decideV2(sent: .code2, reply: reply(ok: true, v: 2, sealed: "abcd")),
            .openSealedToken(sealed: "abcd")
        )
    }

    /// `Authorized2` 는 필드가 없다 — 되돌릴 토큰이 없기 때문이다(스펙 5.1).
    /// 이미 저장된 토큰으로 세션 키를 만들어야 한다.
    func testAuthorized2OpensTheSessionWithNoNewToken() {
        XCTAssertEqual(
            BLEClient.decideV2(sent: .proof2, reply: reply(ok: true, v: 2)),
            .openSession
        )
    }

    /// `Denied` 에도 `"v":2` 는 없다 — 그래도 남은 시도 횟수는 화면에 그대로
    /// 보여줘야 한다. "v2 가 아니면 전부 needsPairing" 으로 뭉개면 사용자는
    /// 코드가 틀렸다는 사실도, 몇 번 남았는지도 알 수 없다.
    func testDeniedStillReportsRemainingAttempts() {
        XCTAssertEqual(
            BLEClient.decideV2(sent: .code2, reply: reply(ok: false, left: 2)),
            .failed(left: 2)
        )
    }

    /// 그 동사에 올 수 없는 성공 응답은 받아들이지 않는다 — 예컨대 `HELLO2` 에
    /// `ok:true` 가 오는 일은 맥 쪽에 없다.
    func testSuccessRepliesThatCannotFollowTheVerbAreRefused() {
        XCTAssertEqual(
            BLEClient.decideV2(sent: .hello2, reply: reply(ok: true, v: 2, sealed: "abcd")),
            .needsPairing
        )
        XCTAssertEqual(
            BLEClient.decideV2(sent: .code2, reply: reply(ok: true, v: 2)),
            .needsPairing,
            "CODE2 성공은 봉인된 토큰을 반드시 들고 온다"
        )
    }
}

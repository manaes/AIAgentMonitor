import CryptoKit
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

    // MARK: - v2 첫 프레임 (`initialSend`) — 두 전송이 공유하는 규칙

    private static let pub32 = Data(repeating: 1, count: 32)

    /// **방금 받은 코드가 저장된 토큰보다 우선한다.** 9637972 에서 고친 버그다 —
    /// 맥이 전체 해제로 토큰을 폐기한 뒤에는 `AUTH2` 재인증이 반드시 거부되고,
    /// 그때 방금 낸 코드는 쓰이지도 못한 채 "연결끊김: needsPairing" 이 된다.
    ///
    /// 이 단언이 BLE 경로도 함께 지킨다 — `beginV2Handshake` 가 조건을 따로
    /// 갖고 있지 않고 이 함수를 부르기 때문이다.
    func testAFreshCodeBeatsAStoredToken() {
        XCTAssertEqual(
            BLEClient.initialSend(hasToken: true, code: "123456", clientPub: Self.pub32),
            BLEClient.V2Send(verb: .hello2, frame: PairingClient.hello2Frame(clientPub: Self.pub32))
        )
    }

    func testAStoredTokenWithNoCodeReconnectsWithAuth2() {
        XCTAssertEqual(
            BLEClient.initialSend(hasToken: true, code: nil, clientPub: Self.pub32),
            BLEClient.V2Send(verb: .auth2, frame: PairingClient.auth2Frame(clientPub: Self.pub32))
        )
    }

    func testWithoutATokenItAlwaysStartsAtHello2() {
        for code in [nil, "654321"] {
            XCTAssertEqual(
                BLEClient.initialSend(hasToken: false, code: code, clientPub: Self.pub32),
                BLEClient.V2Send(verb: .hello2, frame: PairingClient.hello2Frame(clientPub: Self.pub32)),
                "v2 는 HELLO2 없이 CODE2 를 낼 수 없다 — 바인딩할 transcript 가 없다"
            )
        }
    }

    /// 동사와 프레임이 한 값에 묶여 있어야 한다. 따로 계산하면 조건 하나만
    /// 고쳤을 때 어긋나고, 그러면 `AwaitingCode2` 를 `Nonce2` 로 오해해 논스
    /// 없는 `PROOF2` 를 내고 조용히 `needsPairing` 에 앉는다.
    func testTheVerbAlwaysMatchesTheFrameItIsPairedWith() {
        for (hasToken, code) in [(true, nil), (true, "1"), (false, nil), (false, "1")] as [(Bool, String?)] {
            let send = BLEClient.initialSend(hasToken: hasToken, code: code, clientPub: Self.pub32)
            let expected = send.verb == .hello2
                ? PairingClient.hello2Frame(clientPub: Self.pub32)
                : PairingClient.auth2Frame(clientPub: Self.pub32)
            XCTAssertEqual(send.frame, expected, "hasToken=\(hasToken) code=\(String(describing: code))")
        }
    }

    // MARK: - v2 상태 전이 (`advance` / `submit`)

    /// `V2Handshaking` 의 가짜 구현. 크립토 대신 고정 값을 돌려주므로 전이
    /// 자체만 남는다 — 이 결정들이 `CBPeripheral` 을 쥔 코드 안에 있었을 때는
    /// 어떤 테스트도 닿지 못했고, 재도입 금지 두 버그가 정확히 거기서 나왔다.
    private final class FakeHandshake: V2Handshaking {
        let clientPub = Data(repeating: 9, count: 32)
        var agrees = true
        var opensSealed: String? = "aabb"
        /// `agree` 가 몇 번 불렸는지 — 전이가 합의를 건너뛰지 않는지 본다.
        private(set) var agreeCount = 0

        func agree(epkHex: String, nonceHex: String) -> Bool {
            agreeCount += 1
            return agrees
        }
        func codeBinding(code: String) -> Data? { agrees ? Data("bind-\(code)".utf8) : nil }
        func sessionProof(tokenHex: String) -> Data? { agrees ? Data("proof-\(tokenHex)".utf8) : nil }
        func openSealedToken(sealedHex: String) -> String? { opensSealed }

        /// 진짜 `V2Handshake` 와 **같은 전제조건**을 흉내 낸다 — 토큰이 hex 로
        /// 디코드되지 않으면 세션 키를 만들 수 없다.
        func sessionChannel(tokenHex: String) -> SealedChannel? {
            guard let key = Data(hexString: tokenHex) else { return nil }
            let sym = SymmetricKey(data: key.count == 32 ? key : Data(repeating: 0, count: 32))
            return SealedChannel(sendKey: sym, recvKey: sym)
        }
    }

    /// 봉인 안의 토큰은 그냥 JSON 문자열 필드다 — hex 라는 보장이 없다.
    /// 그런 토큰이 오면 **성공을 알리기 전에** 멈춰야 한다. `.openSession` 을
    /// 먼저 내고 나중에 실패하면, 그 사이에 호출부가 쓸 수 없는 자격 증명을
    /// Keychain 에 썼다가 지우게 된다.
    ///
    /// Keychain 자체는 이 번들에서 관측할 수 없다(호스트 앱이 없어 `SecItemAdd`
    /// 가 항상 -34018 로 실패한다, `testTokenStoreRoundTrip` 의 skip 사유).
    /// 그래서 저장이 **일어날 수 없다**는 성질로 고정한다 — `TokenStore.save` 는
    /// `.openSession` 갈래 안에서만 불리고, 이 입력은 그 갈래를 만들지 못한다.
    func testANonHexTokenStopsBeforeAnySessionIsReported() {
        var state = BLEClient.V2ClientState()
        state.sent = .code2
        let fake = FakeHandshake()
        fake.opensSealed = "not-hex-at-all"

        let step = BLEClient.advance(
            state: &state, decision: .openSealedToken(sealed: "s"),
            handshake: fake, storedToken: nil
        )

        XCTAssertEqual(step, .stop, "세션 키를 만들 수 없는 토큰으로 성공을 알리면 안 된다")
        if case .openSession = step {
            XCTFail("여기서 openSession 이 나오면 호출부가 그 토큰을 저장한다")
        }
    }

    /// 재연결 경로도 같다 — 저장된 토큰이 손상됐으면 `Authorized2` 를 받고도 멈춘다.
    func testACorruptStoredTokenStopsOnReconnect() {
        var state = BLEClient.V2ClientState()
        state.sent = .proof2
        XCTAssertEqual(
            BLEClient.advance(
                state: &state, decision: .openSession,
                handshake: FakeHandshake(), storedToken: "not-hex-at-all"
            ),
            .stop
        )
    }

    /// `AwaitingCode2` 인데 코드가 없으면 사용자를 기다린다. **`CODE2` 는
    /// 여기서 나가지 않는다** — 낼 코드가 없다.
    func testBindCodeWithoutACodeWaitsForTheUser() {
        var state = BLEClient.V2ClientState()
        state.sent = .hello2
        let step = BLEClient.advance(
            state: &state, decision: .bindCode(epk: "e", nonce: "n"),
            handshake: FakeHandshake(), storedToken: nil
        )
        XCTAssertEqual(step, .awaitUserCode)
        XCTAssertTrue(state.awaitingUserCode, "CODE2 를 낼 수 있는 상태가 돼야 한다")
    }

    /// QR 처럼 코드를 이미 들고 있으면 사용자를 기다리지 않고 곧바로 낸다.
    func testBindCodeWithAPendingCodeSendsCode2Immediately() {
        var state = BLEClient.V2ClientState()
        state.sent = .hello2
        state.pendingCode = "123456"
        let step = BLEClient.advance(
            state: &state, decision: .bindCode(epk: "e", nonce: "n"),
            handshake: FakeHandshake(), storedToken: nil
        )
        XCTAssertEqual(
            step,
            .send(BLEClient.V2Send(
                verb: .code2,
                frame: PairingClient.code2Frame(binding: Data("bind-123456".utf8))
            ))
        )
        XCTAssertEqual(state.sent, .code2, "보낸 동사가 프레임과 같이 움직여야 한다")
        XCTAssertNil(state.pendingCode, "코드는 한 번 쓰면 소비된다")
        XCTAssertFalse(state.awaitingUserCode, "맥도 CODE2 하나로 핸드셰이크를 소비한다")
    }

    /// 합의가 실패하면(저차 점·형식 오류) 멈춘다. 재시도해도 같은 맥이면 같은 결과다.
    func testAFailedAgreementStops() {
        var state = BLEClient.V2ClientState()
        state.sent = .hello2
        state.pendingCode = "123456"
        let fake = FakeHandshake()
        fake.agrees = false
        XCTAssertEqual(
            BLEClient.advance(
                state: &state, decision: .bindCode(epk: "e", nonce: "n"),
                handshake: fake, storedToken: nil
            ),
            .stop
        )
    }

    /// `.stop` 은 **프레임을 만들지 않는다.** 이게 재도입 금지 두 번째 버그의
    /// BLE 절반이다 — 성공할 수 없는 재전송은 다시 거절당하고, 그 `.disconnected`
    /// 가 QR 스캐너를 깜빡이게 한다.
    func testStopNeverProducesAFrame() {
        let decisions: [BLEClient.V2Action] = [
            .needsPairing,
            .openSealedToken(sealed: "zz"),   // 봉인이 안 열리는 경우
            .signSessionProof(epk: "e", nonce: "n"),   // 저장된 토큰이 없는 경우
            .openSession,                     // 저장된 토큰이 없는 경우
        ]
        for decision in decisions {
            var state = BLEClient.V2ClientState()
            state.sent = .hello2
            state.pendingCode = "123456"
            state.awaitingUserCode = true
            let fake = FakeHandshake()
            fake.opensSealed = nil
            let step = BLEClient.advance(
                state: &state, decision: decision, handshake: fake, storedToken: nil
            )
            XCTAssertEqual(step, .stop, "\(decision)")
            if case .send = step { XCTFail("멈출 때 프레임을 만들면 안 된다: \(decision)") }
            XCTAssertNil(state.sent, "\(decision)")
            XCTAssertNil(state.pendingCode, "만료됐을 코드를 들고 있으면 안 된다: \(decision)")
            XCTAssertFalse(state.awaitingUserCode, "\(decision)")
        }
    }

    func testNonce2SignsWithTheStoredToken() {
        var state = BLEClient.V2ClientState()
        state.sent = .auth2
        let step = BLEClient.advance(
            state: &state, decision: .signSessionProof(epk: "e", nonce: "n"),
            handshake: FakeHandshake(), storedToken: "cafe"
        )
        XCTAssertEqual(
            step,
            .send(BLEClient.V2Send(
                verb: .proof2,
                frame: PairingClient.proof2Frame(proof: Data("proof-cafe".utf8))
            ))
        )
        XCTAssertEqual(state.sent, .proof2)
    }

    /// `Granted2` 는 새 토큰이라 저장해야 하고, `Authorized2` 는 이미 저장된
    /// 토큰을 쓴다 — 이 둘이 뭉개지면 재연결마다 Keychain 을 덮어쓰게 된다.
    func testGranted2StoresTheTokenAndAuthorized2DoesNot() {
        var pairing = BLEClient.V2ClientState()
        pairing.sent = .code2
        guard case .openSession(let token, _, let store) = BLEClient.advance(
            state: &pairing, decision: .openSealedToken(sealed: "s"),
            handshake: FakeHandshake(), storedToken: nil
        ) else { return XCTFail("Granted2 는 세션을 연다") }
        XCTAssertEqual(token, "aabb")
        XCTAssertTrue(store, "Granted2 의 토큰은 새 토큰이라 저장해야 한다")
        XCTAssertNil(pairing.sent, "인가된 뒤에는 기다리는 응답이 없다")

        var reconnect = BLEClient.V2ClientState()
        reconnect.sent = .proof2
        guard case .openSession(let token2, _, let store2) = BLEClient.advance(
            state: &reconnect, decision: .openSession,
            handshake: FakeHandshake(), storedToken: "cafe"
        ) else { return XCTFail("Authorized2 는 세션을 연다") }
        XCTAssertEqual(token2, "cafe")
        XCTAssertFalse(store2, "이미 저장된 토큰을 다시 쓰면 재연결마다 Keychain 을 덮어쓴다")
    }

    /// 코드가 틀렸다. 맥은 `CODE2` 하나로 핸드셰이크를 소비했으므로 다시 넣으려면
    /// `HELLO2` 부터다 — 그래서 `awaitingUserCode` 를 내려둬야 한다.
    func testDeniedClearsTheHandshakeSoTheNextTryRestarts() {
        var state = BLEClient.V2ClientState()
        state.sent = .code2
        state.awaitingUserCode = true
        XCTAssertEqual(
            BLEClient.advance(
                state: &state, decision: .failed(left: 2),
                handshake: FakeHandshake(), storedToken: nil
            ),
            .failed(left: 2)
        )
        XCTAssertFalse(state.awaitingUserCode)
        XCTAssertNil(state.pendingCode)
    }

    /// **`CODE2` 는 `awaitingUserCode` 에서만 나간다.** 그 밖에서는 코드를 들고
    /// `HELLO2` 부터 다시 시작한다 — 연결 시점에 맥 화면의 창은 보통 아직
    /// 닫혀 있고, 사용자가 그 뒤에 연다.
    func testSubmitOutsideAwaitingUserCodeRestartsCarryingTheCode() {
        var state = BLEClient.V2ClientState()
        XCTAssertEqual(BLEClient.submit(code: "123456", state: &state, handshake: FakeHandshake()), .restart)
        XCTAssertEqual(state.pendingCode, "123456", "다시 시작할 때 쓰려면 들고 있어야 한다")
        XCTAssertEqual(
            BLEClient.initialSend(hasToken: true, code: state.pendingCode, clientPub: Self.pub32).verb,
            .hello2,
            "코드를 들고 있으면 저장된 토큰이 있어도 HELLO2 다"
        )
    }

    /// 핸드셰이크가 아예 없어도(연결 직후) 같은 판단이어야 한다.
    func testSubmitWithNoHandshakeRestarts() {
        var state = BLEClient.V2ClientState()
        state.awaitingUserCode = true
        XCTAssertEqual(BLEClient.submit(code: "123456", state: &state, handshake: nil), .restart)
        XCTAssertEqual(state.pendingCode, "123456")
    }

    func testSubmitInAwaitingUserCodeSendsCode2() {
        var state = BLEClient.V2ClientState()
        state.sent = .hello2
        state.awaitingUserCode = true
        XCTAssertEqual(
            BLEClient.submit(code: "123456", state: &state, handshake: FakeHandshake()),
            .send(BLEClient.V2Send(
                verb: .code2,
                frame: PairingClient.code2Frame(binding: Data("bind-123456".utf8))
            ))
        )
        XCTAssertEqual(state.sent, .code2)
        XCTAssertFalse(state.awaitingUserCode)
    }

    /// 재연결은 사용자가 낸 코드를 **유지한다**(`.stop` 이 버리는 것과 갈린다) —
    /// 코드를 넣자마자 링크가 한 번 끊겼다고 다시 입력하게 만들면 안 된다.
    func testResetForNewConnectionKeepsThePendingCodeButDropsTheVerb() {
        var state = BLEClient.V2ClientState()
        state.sent = .code2
        state.awaitingUserCode = true
        state.pendingCode = "123456"
        state.resetForNewConnection()
        XCTAssertNil(state.sent, "임시 키가 바뀌므로 기다리던 응답도 무의미하다")
        XCTAssertFalse(state.awaitingUserCode)
        XCTAssertEqual(state.pendingCode, "123456")
    }
}

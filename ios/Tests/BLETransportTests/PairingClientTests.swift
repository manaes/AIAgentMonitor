import CryptoKit
import XCTest
@testable import BLETransport

final class PairingClientTests: XCTestCase {

    func testFrameEncodingMatchesRustParser() {
        XCTAssertEqual(String(data: PairingClient.helloFrame(), encoding: .utf8), "HELLO")
        XCTAssertEqual(String(data: PairingClient.codeFrame("123456"), encoding: .utf8), "CODE:123456")
        XCTAssertEqual(String(data: PairingClient.authFrame(), encoding: .utf8), "AUTH")
    }

    func testParsesGrant() {
        let d = Data(#"{"ok":true,"token":"deadbeef"}"#.utf8)
        let r = PairingClient.parse(d)
        XCTAssertEqual(r?.ok, true)
        XCTAssertEqual(r?.token, "deadbeef")
    }

    func testParsesDenialWithRemainingAttempts() {
        let d = Data(#"{"ok":false,"left":3}"#.utf8)
        let r = PairingClient.parse(d)
        XCTAssertEqual(r?.ok, false)
        XCTAssertEqual(r?.left, 3)
        XCTAssertNil(r?.token)
    }

    func testParsesAwaitingCode() {
        let d = Data(#"{"ok":false,"await":"code"}"#.utf8)
        let r = PairingClient.parse(d)
        XCTAssertEqual(r?.ok, false)
        XCTAssertEqual(r?.awaiting, "code")
    }

    func testParsesBareRejection() {
        let r = PairingClient.parse(Data(#"{"ok":false}"#.utf8))
        XCTAssertEqual(r?.ok, false)
        XCTAssertNil(r?.left)
    }

    func testMalformedPayloadReturnsNil() {
        XCTAssertNil(PairingClient.parse(Data("not json".utf8)))
    }

    func testTokenStoreRoundTrip() throws {
        throw XCTSkip("""
            Keychain 은 프로세스의 앱 정체성이 있어야 access-group 을 정하는데,
            이 타겟은 다른 4개와 같은 호스트 없는 로직 테스트 번들이라 SecItemAdd 가
            항상 errSecMissingEntitlement(-34018) 로 실패한다. 코드는 표준
            Security.framework 사용이라 실제 앱(호스트 있음)에서는 문제없이 동작할
            것으로 판단하며, 실제 왕복 검증은 3단계 Task 7 의 실기기 확인 절차로
            넘긴다(docs/ble-protocol/DEVICE-TEST.md).
            """)
        // 아래 원래 단언은 지우지 않는다 — 언젠가 호스트 앱이 붙으면 그대로 살아난다.
        TokenStore.clear()
        XCTAssertNil(TokenStore.load(), "지운 뒤에는 없어야 한다")
        TokenStore.save("cafebabe")
        XCTAssertEqual(TokenStore.load(), "cafebabe")
        TokenStore.save("f00dface")
        XCTAssertEqual(TokenStore.load(), "f00dface", "덮어쓰기가 되어야 한다")
        TokenStore.clear()
        XCTAssertNil(TokenStore.load())
    }

    /// 개정: 원래 브리프에 없던 케이스. 재인증 흐름의 두 번째 단계(AUTH 에 대한 응답) —
    /// 이 필드가 없으면 PROOF 를 만들 논스를 얻을 방법이 없다.
    func testParsesNonceChallenge() {
        let d = Data(#"{"ok":false,"nonce":"7ac4e19b2d5f8067c3a1e9d4b6f02358"}"#.utf8)
        let r = PairingClient.parse(d)
        XCTAssertEqual(r?.ok, false)
        XCTAssertEqual(r?.nonce, "7ac4e19b2d5f8067c3a1e9d4b6f02358")
        XCTAssertNil(r?.token)
    }

    /// 개정: 원래 브리프에 없던 케이스. 재인증(PROOF) 성공 응답 — 최초 코드 인가(Granted)와
    /// 달리 토큰을 다시 돌려주지 않는다(되돌릴 비밀이 없다, 스펙 5.1).
    func testParsesReauthSuccessWithNoToken() {
        let r = PairingClient.parse(Data(#"{"ok":true}"#.utf8))
        XCTAssertEqual(r?.ok, true)
        XCTAssertNil(r?.token)
    }

    /// 개정: Step 1 원문 docstring 이 "이 태스크의 테스트가 hmac-sample.json 을 읽어
    /// 검증해야 한다" 고 말했지만 정작 그 테스트가 빠져 있었다. Rust 팀리드가 Python 으로
    /// 독립 재계산해 이 파일의 proof 값과 일치를 확인해뒀다(golden, 94c5af7).
    func testProofFrameMatchesGoldenVector() throws {
        struct Golden: Decodable { let token: String; let nonce: String; let proof: String }
        let url = try XCTUnwrap(
            Bundle(for: Self.self).url(forResource: "hmac-sample", withExtension: "json"),
            "골든 벡터가 테스트 번들에 없다"
        )
        let golden = try JSONDecoder().decode(Golden.self, from: Data(contentsOf: url))
        let frame = try XCTUnwrap(PairingClient.proofFrame(token: golden.token, nonce: golden.nonce))
        let text = try XCTUnwrap(String(data: frame, encoding: .utf8))
        XCTAssertEqual(text, "PROOF:\(golden.proof)")
    }

    // MARK: - v2

    /// Rust `parse_auth_request` 가 `strip_prefix` 로 읽는 네 동사. 접두사 하나만
    /// 어긋나도 맥은 `Malformed` → `Rejected` 로 답하고, 클라이언트는 이유 없이
    /// `needsPairing` 에 앉는다.
    func testV2FrameEncodingMatchesRustParser() {
        let pub32 = Data(repeating: 0xAB, count: 32)
        let hex = String(repeating: "ab", count: 32)
        XCTAssertEqual(
            String(data: PairingClient.hello2Frame(clientPub: pub32), encoding: .utf8),
            "HELLO2:\(hex)"
        )
        XCTAssertEqual(
            String(data: PairingClient.auth2Frame(clientPub: pub32), encoding: .utf8),
            "AUTH2:\(hex)"
        )
        XCTAssertEqual(
            String(data: PairingClient.code2Frame(binding: Data([0x01, 0x02])), encoding: .utf8),
            "CODE2:0102"
        )
        XCTAssertEqual(
            String(data: PairingClient.proof2Frame(proof: Data([0xde, 0xad])), encoding: .utf8),
            "PROOF2:dead"
        )
    }

    /// `AwaitingCode2`/`Nonce2`/`Granted2` 의 v2 전용 필드가 실제로 뽑혀야 한다.
    /// 하나라도 nil 로 떨어지면 화면은 아무 로그 없이 비어 있게 된다.
    func testParsesV2Replies() {
        let awaiting = PairingClient.parse(
            Data(#"{"ok":false,"v":2,"await":"code","epk":"aa","nonce":"bb"}"#.utf8)
        )
        XCTAssertEqual(awaiting?.v, 2)
        XCTAssertEqual(awaiting?.epk, "aa")
        XCTAssertEqual(awaiting?.nonce, "bb")
        XCTAssertEqual(awaiting?.awaiting, "code")

        let nonce2 = PairingClient.parse(Data(#"{"ok":false,"v":2,"epk":"aa","nonce":"bb"}"#.utf8))
        XCTAssertEqual(nonce2?.v, 2)
        XCTAssertNil(nonce2?.awaiting, "Nonce2 는 `await` 이 없다 — 이 차이가 전부다")

        let granted2 = PairingClient.parse(Data(#"{"ok":true,"v":2,"sealed":"cc"}"#.utf8))
        XCTAssertEqual(granted2?.ok, true)
        XCTAssertEqual(granted2?.sealed, "cc")

        XCTAssertEqual(PairingClient.parse(Data(#"{"ok":true,"v":2}"#.utf8))?.v, 2)
    }

    /// v1 응답에는 `v` 가 없다. 이 값이 0 이나 1 로 튀면 다운그레이드 판정이 무너진다.
    func testV1RepliesCarryNoVersionMarker() {
        XCTAssertNil(PairingClient.parse(Data(#"{"ok":true}"#.utf8))?.v)
        XCTAssertNil(PairingClient.parse(Data(#"{"ok":false,"left":3}"#.utf8))?.v)
    }

    /// 저차 점(전부 0인 공개키)은 공유 비밀을 상수로 만든다. Rust 는
    /// `was_contributory()` 로 `None` 을 돌려주는데, 클라이언트가 이걸 받아주면
    /// 양쪽이 "공격자가 아는 상수"에 합의한 꼴이 된다.
    func testLowOrderServerKeyIsRejected() {
        let hs = V2Handshake()
        XCTAssertFalse(
            hs.agree(epkHex: String(repeating: "00", count: 32), nonceHex: "00112233"),
            "저차 점으로 만든 공유 비밀에는 합의하지 않는다"
        )
        XCTAssertNil(hs.codeBinding(code: "123456"), "합의 실패 뒤에는 아무 값도 나오면 안 된다")
    }

    /// **임시 키는 한 번만 쓴다.** 이 불변식을 지키는 건 `agree` 안의
    /// `privateKey = nil` 한 줄뿐인데, 지워도 성공 경로는 아무 티가 안 난다.
    /// 두 번째 합의가 통하면 그 키는 더 이상 임시가 아니고, 서로 다른 두 세션이
    /// 같은 개인키를 공유한다 — 하나가 새면 다른 하나도 함께 열린다.
    func testTheEphemeralKeyIsUsedExactlyOnce() {
        let hs = V2Handshake()
        let first = Curve25519.KeyAgreement.PrivateKey().publicKey.rawRepresentation
        let second = Curve25519.KeyAgreement.PrivateKey().publicKey.rawRepresentation
        XCTAssertTrue(hs.agree(epkHex: first.hexString, nonceHex: "00112233"))
        XCTAssertFalse(
            hs.agree(epkHex: second.hexString, nonceHex: "44556677"),
            "임시 키는 한 번만 쓴다"
        )
    }

    /// 두 번째 합의가 거절될 뿐 아니라, **첫 합의의 결과가 그대로 남아야 한다** —
    /// 두 번째 논스로 덮어써지면 세션 키의 salt 가 맥과 어긋난다.
    func testASecondAgreementDoesNotOverwriteTheFirst() {
        let hs = V2Handshake()
        let serverPub = Curve25519.KeyAgreement.PrivateKey().publicKey.rawRepresentation
        XCTAssertTrue(hs.agree(epkHex: serverPub.hexString, nonceHex: "00112233"))
        let binding = hs.codeBinding(code: "123456")
        XCTAssertFalse(hs.agree(epkHex: serverPub.hexString, nonceHex: "ffffffff"))
        XCTAssertEqual(hs.codeBinding(code: "123456"), binding)
    }

    /// 형식이 어긋난 `epk`(길이·문자)로는 합의하지 않는다 — 여기서 통과시키면
    /// 이후 파생 키가 조용히 엉뚱한 값이 된다.
    func testMalformedServerKeyIsRejected() {
        XCTAssertFalse(V2Handshake().agree(epkHex: "zz", nonceHex: "00"))
        XCTAssertFalse(
            V2Handshake().agree(epkHex: String(repeating: "aa", count: 31), nonceHex: "00"),
            "32바이트가 아니면 X25519 공개키가 아니다"
        )
    }

    /// 맥이 `Granted2` 에서 하는 일을 그대로 흉내 내, 클라이언트가 토큰을 꺼내고
    /// **방향이 뒤집힌** 세션 채널을 만드는지 본다. 이 뒤집기(`c2s` 가 송신,
    /// `s2c` 가 수신)를 틀리면 연결은 되는데 스냅샷이 한 장도 안 열린다.
    func testPairingRoundTripYieldsTokenAndAWorkingSessionChannel() throws {
        let hs = V2Handshake()
        // 맥 역할: 임시 키를 만들고 클라이언트 공개키와 합의한다.
        let serverPriv = Curve25519.KeyAgreement.PrivateKey()
        let serverPub = serverPriv.publicKey.rawRepresentation
        let clientPubKey = try Curve25519.KeyAgreement.PublicKey(rawRepresentation: hs.clientPub)
        let ss = try serverPriv.sharedSecretFromKeyAgreement(with: clientPubKey)
            .withUnsafeBytes { Data($0) }
        let nonce = Data(repeating: 0x5A, count: 16)
        let token = String(repeating: "7f", count: 16)

        // 맥은 토큰 한 건을 k_pair 로 봉인한다(양방향 같은 키).
        let kPair = CryptoV2.derivePairKey(sharedSecret: ss, nonce: nonce)
        let sealedHex = try SealedChannel(sendKey: kPair, recvKey: kPair)
            .seal(Data(#"{"token":"\#(token)"}"#.utf8)).hexString

        XCTAssertTrue(hs.agree(epkHex: serverPub.hexString, nonceHex: nonce.hexString))
        XCTAssertEqual(hs.openSealedToken(sealedHex: sealedHex), token)

        // 맥은 (s2c, c2s) 로 채널을 만든다. 클라이언트 채널이 그 반대여야 열린다.
        let keys = CryptoV2.deriveSessionKeys(
            sharedSecret: ss, token: try XCTUnwrap(Data(hexString: token)), nonce: nonce
        )
        let mac = SealedChannel(sendKey: keys.s2c, recvKey: keys.c2s)
        let client = try XCTUnwrap(hs.sessionChannel(tokenHex: token))
        let snapshot = Data(#"{"v":1}"#.utf8)
        XCTAssertEqual(try client.open(mac.seal(snapshot)), snapshot)
    }

    /// 재연결 증명. 맥은 저장된 토큰 후보들에 대해 같은 계산을 돌려 대조하므로,
    /// transcript 가 한 바이트라도 다르면(=능동적 MITM) 통과하지 못한다.
    func testSessionProofMatchesWhatTheMacRecomputes() throws {
        let hs = V2Handshake()
        let serverPriv = Curve25519.KeyAgreement.PrivateKey()
        let serverPub = serverPriv.publicKey.rawRepresentation
        // 증명은 토큰·논스·transcript 만으로 계산된다 — 공유 비밀은 쓰이지 않는다.
        let nonce = Data(repeating: 0x11, count: 16)
        let token = String(repeating: "3c", count: 16)

        XCTAssertTrue(hs.agree(epkHex: serverPub.hexString, nonceHex: nonce.hexString))

        // 맥이 계산하는 값 — transcript 는 **클라이언트 키가 먼저**다.
        let expected = CryptoV2.sessionProof(
            token: try XCTUnwrap(Data(hexString: token)),
            nonce: nonce,
            transcript: CryptoV2.transcript(clientPub: hs.clientPub, serverPub: serverPub)
        )
        XCTAssertEqual(hs.sessionProof(tokenHex: token), expected)
    }
}

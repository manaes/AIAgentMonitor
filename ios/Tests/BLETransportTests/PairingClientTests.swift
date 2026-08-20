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
}

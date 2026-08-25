import BLETransport
import XCTest
@testable import NetworkTransport

final class NetworkClientTests: XCTestCase {
    func testParsesValidQrPayload() {
        let parsed = NetworkClient.parseQrPayload("aim://pair?endpoint=abcdef01&code=123456")
        XCTAssertEqual(parsed?.endpointIdHex, "abcdef01")
        XCTAssertEqual(parsed?.code, "123456")
        XCTAssertNil(parsed?.relayUrl)
        XCTAssertEqual(parsed?.addresses, [])
    }

    /// relay/addr 는 URL 인코딩을 피하려고 hex 로 실려온다 — EndpointId 만으로는
    /// discovery 에 등록이 안 돼 있어 dial 이 실패하므로(실기기 확인) 반드시
    /// 함께 파싱돼야 한다.
    func testParsesRelayAndAddresses() {
        let relayHex = Data("https://relay.example.com".utf8).map { String(format: "%02x", $0) }.joined()
        let addrHex1 = Data("192.168.1.5:12345".utf8).map { String(format: "%02x", $0) }.joined()
        let addrHex2 = Data("[fe80::1]:12345".utf8).map { String(format: "%02x", $0) }.joined()
        let payload = "aim://pair?endpoint=abcdef01&code=123456&relay=\(relayHex)&addr=\(addrHex1)&addr=\(addrHex2)"

        let parsed = NetworkClient.parseQrPayload(payload)
        XCTAssertEqual(parsed?.relayUrl, "https://relay.example.com")
        XCTAssertEqual(parsed?.addresses, ["192.168.1.5:12345", "[fe80::1]:12345"])
    }

    func testRejectsWrongScheme() {
        XCTAssertNil(NetworkClient.parseQrPayload("https://pair?endpoint=abcdef01&code=123456"))
    }

    func testRejectsMissingCode() {
        XCTAssertNil(NetworkClient.parseQrPayload("aim://pair?endpoint=abcdef01"))
    }

    func testRejectsMissingEndpoint() {
        XCTAssertNil(NetworkClient.parseQrPayload("aim://pair?code=123456"))
    }

    func testRejectsGarbage() {
        XCTAssertNil(NetworkClient.parseQrPayload("not a url at all"))
    }

    // MARK: - 첫 프레임 선택 (QR 스캔 vs 저장된 토큰)

    /// QR 을 방금 스캔했다는 건 "새로 페어링하겠다" 는 명시적 의사다. 그런데
    /// 초안은 저장된 토큰이 있으면 그걸 **우선**해 AUTH 를 보냈다 — Mac 에서
    /// 전체 해제로 토큰이 폐기된 뒤엔 그 재인증이 반드시 거부되고,
    /// needsPairing 으로 떨어지면서 방금 스캔한 코드는 쓰이지도 못했다.
    func testAFreshlyScannedCodeWinsOverAStoredToken() {
        XCTAssertEqual(
            NetworkClient.initialFrame(hasToken: true, code: "123456"),
            PairingClient.codeFrame("123456"),
            "코드를 들고 있으면 저장된 토큰이 있어도 그 코드를 쓴다"
        )
    }

    /// 코드 없이 재연결하는 평소 경로 — 저장된 토큰으로 조용히 재인증한다.
    func testStoredTokenIsUsedWhenReconnectingWithoutACode() {
        XCTAssertEqual(
            NetworkClient.initialFrame(hasToken: true, code: nil),
            PairingClient.authFrame()
        )
    }

    /// 토큰도 코드도 없으면 HELLO 로 시작해 창이 열려 있는지 물어본다.
    func testHelloWhenThereIsNeitherTokenNorCode() {
        XCTAssertEqual(
            NetworkClient.initialFrame(hasToken: false, code: nil),
            PairingClient.helloFrame()
        )
    }

    /// 토큰이 없고 코드만 있으면(첫 페어링) 바로 코드를 낸다 — Mac 은 HELLO
    /// 없이 온 CODE: 도 받는다(pairing.rs: code_without_prior_hello_still_grants).
    func testCodeIsSentDirectlyOnFirstPairing() {
        XCTAssertEqual(
            NetworkClient.initialFrame(hasToken: false, code: "654321"),
            PairingClient.codeFrame("654321")
        )
    }

    // MARK: - v2 첫 프레임

    private let clientPub = Data(repeating: 1, count: 32)

    /// v1 에서 겪은 버그를 v2 에서 반복하지 않는다. 맥이 토큰을 이미 폐기한
    /// 경우 토큰 재인증(`AUTH2`)은 반드시 거부되고, 그 사이 방금 스캔한 코드는
    /// 쓰이지도 못한 채 `needsPairing` 으로 떨어진다.
    func testAFreshCodeWinsOverAStoredTokenInV2() {
        XCTAssertEqual(
            NetworkClient.initialFrameV2(hasToken: true, code: "123456", clientPub: clientPub),
            PairingClient.hello2Frame(clientPub: clientPub),
            "코드가 있으면 HELLO2 로 시작한다"
        )
    }

    func testStoredTokenWithoutCodeUsesAuth2() {
        XCTAssertEqual(
            NetworkClient.initialFrameV2(hasToken: true, code: nil, clientPub: clientPub),
            PairingClient.auth2Frame(clientPub: clientPub),
            "코드가 없고 토큰이 있으면 AUTH2"
        )
    }

    /// v1 은 `CODE:` 를 HELLO 없이 바로 낼 수 있었지만 v2 는 못 낸다 —
    /// `CODE2` 의 바인딩은 `HELLO2` 가 만든 transcript 위에서만 계산되고,
    /// 맥도 핸드셰이크가 없으면 곧바로 거절한다(`pairing.rs: Code2`).
    func testFirstPairingStillStartsWithHello2() {
        XCTAssertEqual(
            NetworkClient.initialFrameV2(hasToken: false, code: "654321", clientPub: clientPub),
            PairingClient.hello2Frame(clientPub: clientPub)
        )
        XCTAssertEqual(
            NetworkClient.initialFrameV2(hasToken: false, code: nil, clientPub: clientPub),
            PairingClient.hello2Frame(clientPub: clientPub)
        )
    }

    // MARK: - 스냅샷 줄 분류 (NDJSON)

    /// 이 스트림은 0x0A 로 프레임을 나눈다. 봉인 프레임은 임의의 이진 바이트라
    /// 0x0A 를 그대로 담을 수 있어 날 것으로는 실을 수 없다 — 맥은 hex 문자열
    /// 한 줄로 보낸다(`network/mod.rs: snapshot_line`).
    func testSealedLineIsHexDecoded() {
        XCTAssertEqual(
            NetworkClient.classifyLine(Data("00ff10".utf8)),
            .sealed(Data([0x00, 0xFF, 0x10]))
        )
    }

    /// `{` 로 시작하는 줄은 맥이 평문 JSON 을 보냈다는 뜻이다. v2 세션에서는
    /// 일어날 수 없고(맥은 채널이 있으면 반드시 봉인한다), 일어났다면 그건
    /// 다운그레이드다 — JSON 으로 디코드해 화면에 올리면 안 된다.
    func testPlaintextJsonLineIsNotTreatedAsASnapshot() {
        XCTAssertEqual(
            NetworkClient.classifyLine(Data(#"{"v":1,"agents":[]}"#.utf8)),
            .plaintextJSON
        )
    }

    /// hex 도 JSON 도 아닌 줄. 조용히 `continue` 하면 화면이 영영 비어 있는
    /// 이유를 알 수 없으므로 별도 갈래로 둔다.
    func testGarbageLineIsUnusable() {
        XCTAssertEqual(NetworkClient.classifyLine(Data("zzz".utf8)), .unusable)
        XCTAssertEqual(NetworkClient.classifyLine(Data("0f0".utf8)), .unusable, "홀수 길이 hex")
        XCTAssertEqual(NetworkClient.classifyLine(Data()), .unusable, "연속 개행으로 나오는 빈 줄")
    }
}

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

    // MARK: - v2 첫 프레임

    private let clientPub = Data(repeating: 1, count: 32)

    /// v1 에서 겪은 버그를 v2 에서 반복하지 않는다. 맥이 토큰을 이미 폐기한
    /// 경우 토큰 재인증(`AUTH2`)은 반드시 거부되고, 그 사이 방금 스캔한 코드는
    /// 쓰이지도 못한 채 `needsPairing` 으로 떨어진다.
    ///
    /// 규칙 자체는 `BLEClient.initialSend` 한 곳에만 있다(두 전송이 공유한다).
    /// 여기서는 **네트워크 경로가 그걸 쓴다**는 사실을 고정한다.
    func testAFreshCodeWinsOverAStoredTokenInV2() {
        XCTAssertEqual(
            BLEClient.initialSend(hasToken: true, code: "123456", clientPub: clientPub).frame,
            PairingClient.hello2Frame(clientPub: clientPub),
            "코드가 있으면 HELLO2 로 시작한다"
        )
    }

    func testStoredTokenWithoutCodeUsesAuth2() {
        XCTAssertEqual(
            BLEClient.initialSend(hasToken: true, code: nil, clientPub: clientPub).frame,
            PairingClient.auth2Frame(clientPub: clientPub),
            "코드가 없고 토큰이 있으면 AUTH2"
        )
    }

    /// v1 은 `CODE:` 를 HELLO 없이 바로 낼 수 있었지만 v2 는 못 낸다 —
    /// `CODE2` 의 바인딩은 `HELLO2` 가 만든 transcript 위에서만 계산되고,
    /// 맥도 핸드셰이크가 없으면 곧바로 거절한다(`pairing.rs: Code2`).
    func testFirstPairingStillStartsWithHello2() {
        XCTAssertEqual(
            BLEClient.initialSend(hasToken: false, code: "654321", clientPub: clientPub).frame,
            PairingClient.hello2Frame(clientPub: clientPub)
        )
        XCTAssertEqual(
            BLEClient.initialSend(hasToken: false, code: nil, clientPub: clientPub).frame,
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

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
}

import XCTest
@testable import NetworkTransport

final class NetworkClientTests: XCTestCase {
    func testParsesValidQrPayload() {
        let parsed = NetworkClient.parseQrPayload("aim://pair?endpoint=abcdef01&code=123456")
        XCTAssertEqual(parsed?.endpointIdHex, "abcdef01")
        XCTAssertEqual(parsed?.code, "123456")
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

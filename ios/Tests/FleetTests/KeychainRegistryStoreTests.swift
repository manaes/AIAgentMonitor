import XCTest
@testable import Fleet

final class KeychainRegistryStoreTests: XCTestCase {

    /// 실제 Keychain 왕복은 호스트 앱 없는 유닛 테스트 번들에서 돌지 않는다
    /// (errSecMissingEntitlement). BLETransportTests/PairingClientTests.swift:39-47
    /// 과 같은 이유로 스킵하고, 실제 검증은 Task 15 실기 확인에서 한다.
    func testRoundTripOnDevice() throws {
        throw XCTSkip("호스트 없는 테스트 번들에서는 Keychain 접근이 거부된다 — 실기 검증(Task 15) 항목")
    }

    /// access group 을 넣지 않는 것이 이 앱의 설계 결정이다. 실수로 되살아나면
    /// 기존 앱이 겪은 팀 접두사 문제를 그대로 물려받는다.
    func testQueryDoesNotUseAnAccessGroup() {
        let store = KeychainRegistryStore(service: "test.service")
        let query = store.baseQuery()

        XCTAssertNil(query[kSecAttrAccessGroup as String])
        XCTAssertEqual(query[kSecAttrService as String] as? String, "test.service")
    }
}

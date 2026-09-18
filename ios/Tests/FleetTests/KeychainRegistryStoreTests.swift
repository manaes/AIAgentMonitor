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

    /// `read()`가 "저장된 게 없다"(nil)와 "읽기 실패"(throw)를 구분하는지 확인한다.
    ///
    /// 이 테스트는 **호스트 없는 유닛 테스트 번들이라는 환경 특성에 기대고 있다**:
    /// 이 번들에서는 `SecItemCopyMatching`이 한 번도 쓰인 적 없는 서비스에 대해서도
    /// `errSecItemNotFound`가 아니라 `errSecMissingEntitlement`(-34018)로 실패한다 —
    /// `testRoundTripOnDevice`를 스킵하게 만드는 것과 같은 원인이며, 같은 선례를 따른다
    /// (`BLETransportTests/PairingClientTests.swift:39-47`). 그래서 별도 모킹 없이도
    /// `read()`의 throw 경로를 오늘 이 번들에서 실행할 수 있다.
    ///
    /// ⚠️ 훗날 이 타겟에 호스트 앱이 붙으면 같은 호출이 `errSecItemNotFound` → `nil`
    /// 경로를 타게 되어 이 테스트는 실패한다. 그건 `read()`의 회귀가 아니라 이 테스트가
    /// 전제하는 환경이 바뀐 것이므로, 그때는 이 테스트를 다시 검토해야 한다.
    func testReadThrowsWhenKeychainAccessIsDenied() {
        // 다른 테스트/앱이 실제로 쓴 항목과 절대 겹치지 않도록 고유 접두사 + UUID 사용.
        let store = KeychainRegistryStore(service: "test.KeychainRegistryStoreTests.\(UUID().uuidString)")

        XCTAssertThrowsError(try store.read()) { error in
            let nsError = error as NSError
            XCTAssertEqual(nsError.domain, NSOSStatusErrorDomain)
        }
    }
}

import XCTest
@testable import NetworkTransport

/// `.fixed` 슬롯이 전역 Keychain 슬롯을 절대 건드리지 않는다는 것을 고정한다.
/// 이게 깨지면 장치 B 의 인증 실패가 1:1 앱의 페어링 토큰까지 지운다.
/// `.shared` 는 실제 Keychain 을 건드리므로 여기서 시험하지 않는다.
final class TokenSlotTests: XCTestCase {

    func testFixedSlotLoadsTheGivenToken() {
        XCTAssertEqual(NetworkClient.TokenSlot.fixed("abc").load(), "abc")
    }

    func testFixedSlotIgnoresClear() {
        let slot = NetworkClient.TokenSlot.fixed("abc")
        slot.clear()
        XCTAssertEqual(slot.load(), "abc")
    }

    func testFixedSlotIgnoresSaveButReportsSuccess() {
        let slot = NetworkClient.TokenSlot.fixed("abc")
        // 저장은 no-op 이지만 실패로 보고하면 호출부가 "토큰 저장 실패" 경고를 낸다.
        XCTAssertTrue(slot.save("zzz"))
        XCTAssertEqual(slot.load(), "abc")
    }
}

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

    /// `.ephemeral` 은 코드 페어링 전용이다. 저장된 토큰이 없어야 인증이
    /// `.bindCode`(코드 경로)로 들어간다 — 토큰이 있다고 하면 `.signSessionProof` 로
    /// 새고, 아직 토큰이 없는 AppMulti 는 거기서 needsPairing 으로 떨어진다.
    func testEphemeralSlotLoadsNil() {
        XCTAssertNil(NetworkClient.TokenSlot.ephemeral.load())
    }

    /// 발급 토큰은 `ProbeResult.issuedToken` 으로 돌려주고 슬롯에는 남기지 않는다.
    /// 여기서 전역 슬롯에 썼다면 장치를 하나 추가할 때마다 1:1 앱의 페어링이 덮어써진다.
    /// clear 도 no-op 이어야 한다 — 페어링 실패가 1:1 앱의 토큰을 지우면 안 된다.
    func testEphemeralSlotIgnoresSaveAndClear() {
        let slot = NetworkClient.TokenSlot.ephemeral
        XCTAssertTrue(slot.save("issued-token"), "저장할 곳이 없는 게 정상이므로 실패로 보고하면 안 된다")
        XCTAssertNil(slot.load(), "발급 토큰이 슬롯에 남았다")
        slot.clear()
        XCTAssertNil(slot.load())
    }
}

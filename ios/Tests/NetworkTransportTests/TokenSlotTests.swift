import XCTest
@testable import NetworkTransport

/// 전역 슬롯 접근을 세는 스파이. `.ephemeral`/`.fixed` 가 전역을 **건드렸는지**를
/// 값이 아니라 호출로 관측한다 — 값만 보면 `.ephemeral` 이 전역에 써도 `load` 가 nil 이라
/// 통과해버린다.
private final class SpyTokenStore: SharedTokenStoring {
    var loadCount = 0
    var saveCount = 0
    var clearCount = 0
    var savedTokens: [String] = []
    var stored: String?

    func load() -> String? {
        loadCount += 1
        return stored
    }

    func save(_ token: String) -> Bool {
        saveCount += 1
        savedTokens.append(token)
        stored = token
        return true
    }

    func clear() {
        clearCount += 1
        stored = nil
    }
}

/// `.fixed`/`.ephemeral` 슬롯이 전역 Keychain 슬롯을 절대 건드리지 않는다는 것을 고정한다.
/// 이게 깨지면 장치 B 의 인증 실패가 1:1 앱의 페어링 토큰까지 지운다.
/// 실제 Keychain 대신 스파이를 끼워 넣으므로(`sharedTokenStore`) 테스트 번들에
/// keychain-access-group 엔타이틀먼트가 없어도 `.shared` 까지 관측할 수 있다.
final class TokenSlotTests: XCTestCase {

    private var spy: SpyTokenStore!
    private var originalStore: SharedTokenStoring!

    override func setUp() {
        super.setUp()
        originalStore = sharedTokenStore
        spy = SpyTokenStore()
        sharedTokenStore = spy
    }

    override func tearDown() {
        sharedTokenStore = originalStore
        spy = nil
        super.tearDown()
    }

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
    func testEphemeralSlotIgnoresSaveAndClear() {
        let slot = NetworkClient.TokenSlot.ephemeral
        XCTAssertTrue(slot.save("issued-token"), "저장할 곳이 없는 게 정상이므로 실패로 보고하면 안 된다")
        XCTAssertNil(slot.load(), "발급 토큰이 슬롯에 남았다")
        slot.clear()
        XCTAssertNil(slot.load())
    }

    /// 여기서 전역 슬롯에 썼다면 장치를 하나 추가할 때마다 1:1 앱의 페어링이 덮어써진다.
    /// clear 도 닿으면 안 된다 — 페어링 실패가 1:1 앱의 토큰을 지운다.
    func testEphemeralSlotNeverTouchesSharedStore() {
        let slot = NetworkClient.TokenSlot.ephemeral
        _ = slot.load()
        _ = slot.save("issued-token")
        slot.clear()

        XCTAssertEqual(spy.loadCount, 0, "전역 슬롯을 읽었다")
        XCTAssertEqual(spy.saveCount, 0, "발급 토큰을 전역 슬롯에 썼다 — 1:1 앱의 페어링이 덮어써진다")
        XCTAssertEqual(spy.clearCount, 0, "전역 슬롯을 지웠다 — 1:1 앱의 토큰이 날아간다")
    }

    /// `.fixed` 도 같다. AppMulti 는 장치마다 토큰이 달라 전역 슬롯에 쓸 것이 없다.
    func testFixedSlotNeverTouchesSharedStore() {
        let slot = NetworkClient.TokenSlot.fixed("device-token")
        _ = slot.load()
        _ = slot.save("other-token")
        slot.clear()

        XCTAssertEqual(spy.loadCount, 0, "전역 슬롯을 읽었다")
        XCTAssertEqual(spy.saveCount, 0, "전역 슬롯에 썼다")
        XCTAssertEqual(spy.clearCount, 0, "전역 슬롯을 지웠다")
    }

    /// 반대 방향도 고정한다 — `.shared` 가 전역 슬롯을 안 쓰게 되면 1:1 앱이 매번
    /// 재페어링을 요구한다.
    func testSharedSlotDelegatesToSharedStore() {
        spy.stored = "저장된-토큰"
        let slot = NetworkClient.TokenSlot.shared

        XCTAssertEqual(slot.load(), "저장된-토큰")
        XCTAssertTrue(slot.save("새-토큰"))
        slot.clear()

        XCTAssertEqual(spy.loadCount, 1)
        XCTAssertEqual(spy.saveCount, 1)
        XCTAssertEqual(spy.savedTokens, ["새-토큰"], "저장 값이 그대로 전달되지 않았다")
        XCTAssertEqual(spy.clearCount, 1)
    }
}

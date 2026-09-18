import XCTest
@testable import NetworkTransport

final class IrohEndpointProviderTests: XCTestCase {

    /// 두 번 불러도 같은 Endpoint 여야 한다 — 이게 깨지면 장치마다 Endpoint 가
    /// 하나씩 생겨 16대에서 소켓·relay 연결이 16벌이 된다.
    func testEndpointIsCreatedOnceAndShared() async throws {
        let provider = IrohEndpointProvider()

        let first = try await provider.endpoint()
        let second = try await provider.endpoint()

        XCTAssertTrue(first === second)
    }

    /// 동시에 여러 세션이 요청해도 bind 는 한 번만 일어나야 한다.
    func testConcurrentRequestsShareASingleBind() async throws {
        let provider = IrohEndpointProvider()

        async let a = provider.endpoint()
        async let b = provider.endpoint()
        async let c = provider.endpoint()
        let (x, y, z) = try await (a, b, c)

        XCTAssertTrue(x === y)
        XCTAssertTrue(y === z)
    }
}

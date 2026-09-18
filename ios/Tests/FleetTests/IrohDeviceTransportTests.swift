import XCTest
import NetworkTransport
@testable import Fleet

final class IrohDeviceTransportTests: XCTestCase {

    /// 인증 거부가 .unreachable 로 뭉개지면 재시도로 풀리지 않는 상태를 계속 재시도하게 된다
    /// (NetworkClient.swift:213-222 의 교훈).
    func testAuthFailureMapsToAuthRejected() {
        XCTAssertEqual(IrohDeviceTransport.mapError(NetworkClientError.authFailed), .authRejected)
        XCTAssertEqual(IrohDeviceTransport.mapError(NetworkClientError.needsPairing), .authRejected)
    }

    func testVersionMismatchMapsToVersionMismatch() {
        XCTAssertEqual(IrohDeviceTransport.mapError(NetworkClientError.versionMismatch), .versionMismatch)
    }

    func testTimeoutMapsToUnreachable() {
        XCTAssertEqual(IrohDeviceTransport.mapError(NetworkClientError.fetchTimedOut), .unreachable)
    }

    func testUnknownErrorMapsToUnreachable() {
        struct Boom: Error {}
        XCTAssertEqual(IrohDeviceTransport.mapError(Boom()), .unreachable)
    }
}

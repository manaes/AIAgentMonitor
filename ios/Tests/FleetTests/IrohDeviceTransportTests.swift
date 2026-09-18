import XCTest
import NetworkTransport
import Wire
@testable import Fleet

private func makeSnapshot(rate: Float) throws -> MirrorSnapshot {
    let json = #"{"v":1,"t":1758000000,"a":[{"k":0,"r":\#(rate),"t5":0,"pj":[]}]}"#
    return try JSONDecoder().decode(MirrorSnapshot.self, from: Data(json.utf8))
}

/// 스냅샷 몇 장을 흘린 뒤 에러로 끝나거나(failure) 정상 종료하는 upstream.
/// `NetworkClient.snapshotStream` 이 내놓는 것과 같은 모양이다 — 본문에서 던져지는
/// 에러는 `continuation.finish(throwing:)` 로 소비자에게 간다.
private func makeUpstream(
    snapshots: [MirrorSnapshot], failure: Error?
) -> AsyncThrowingStream<MirrorSnapshot, Error> {
    AsyncThrowingStream { continuation in
        for snapshot in snapshots { continuation.yield(snapshot) }
        if let failure {
            continuation.finish(throwing: failure)
        } else {
            continuation.finish()
        }
    }
}

/// upstream 을 끝까지 소비해 받은 스냅샷과 잡힌 에러를 돌려준다.
private func drain(
    _ stream: AsyncThrowingStream<MirrorSnapshot, Error>
) async -> (snapshots: [MirrorSnapshot], error: Error?) {
    var received: [MirrorSnapshot] = []
    do {
        for try await snapshot in stream { received.append(snapshot) }
        return (received, nil)
    } catch {
        return (received, error)
    }
}

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

    // MARK: - 스트림 본문 에러 매핑
    //
    // `probe` 의 do/catch 는 호출만 감싼다 — 스트림 **본문**에서 던져지는 에러는
    // 그 catch 를 거치지 않으므로 `mappingErrors` 가 없으면 날것 그대로 나가고,
    // `DeviceSession` 의 `catch let error as DeviceTransportError` 분기가 영영
    // 매치되지 않는다.

    func testMidStreamVersionMismatchIsMappedToDeviceTransportError() async throws {
        let snapshot = try makeSnapshot(rate: 3)
        let mapped = IrohDeviceTransport.mappingErrors(
            makeUpstream(snapshots: [snapshot], failure: NetworkClientError.versionMismatch)
        )

        let (received, error) = await drain(mapped)
        XCTAssertEqual(received.count, 1)
        XCTAssertEqual(error as? DeviceTransportError, .versionMismatch)
    }

    func testMidStreamNeedsPairingIsMappedToAuthRejected() async throws {
        let mapped = IrohDeviceTransport.mappingErrors(
            makeUpstream(snapshots: [], failure: NetworkClientError.needsPairing)
        )

        let (received, error) = await drain(mapped)
        XCTAssertTrue(received.isEmpty)
        XCTAssertEqual(error as? DeviceTransportError, .authRejected)
    }

    func testMidStreamUnknownErrorIsMappedToUnreachable() async throws {
        struct Boom: Error {}
        let mapped = IrohDeviceTransport.mappingErrors(
            makeUpstream(snapshots: [], failure: Boom())
        )

        let (received, error) = await drain(mapped)
        XCTAssertTrue(received.isEmpty)
        XCTAssertEqual(error as? DeviceTransportError, .unreachable)
    }

    func testCleanUpstreamEndFinishesWithoutError() async throws {
        let snapshots = [try makeSnapshot(rate: 1), try makeSnapshot(rate: 2)]
        let mapped = IrohDeviceTransport.mappingErrors(
            makeUpstream(snapshots: snapshots, failure: nil)
        )

        let (received, error) = await drain(mapped)
        XCTAssertEqual(received.count, 2)
        XCTAssertNil(error)
    }
}

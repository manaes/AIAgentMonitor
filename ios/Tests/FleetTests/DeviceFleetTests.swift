import XCTest
import Wire
@testable import Fleet

/// 동시에 몇 개가 실행 중이었는지 관찰하는 가짜 전송.
private final class CountingTransport: DeviceTransport, @unchecked Sendable {
    private let lock = NSLock()
    private var active = 0
    private(set) var maxConcurrent = 0

    func probe(device: Device, timeoutSeconds: Double) async throws -> AsyncThrowingStream<MirrorSnapshot, Error> {
        lock.lock(); active += 1; maxConcurrent = max(maxConcurrent, active); lock.unlock()
        try await Task.sleep(nanoseconds: 50_000_000)
        lock.lock(); active -= 1; lock.unlock()
        throw DeviceTransportError.unreachable
    }
}

@MainActor
final class DeviceFleetTests: XCTestCase {

    private func devices(_ count: Int) -> [Device] {
        (0..<count).map {
            Device(endpointIdHex: String(format: "%02x", $0), token: "t", relayUrl: nil,
                   addresses: [], macHostname: nil, userLabel: nil, sortIndex: $0)
        }
    }

    func testCreatesOneSessionPerDevice() {
        let fleet = DeviceFleet(
            devices: devices(5),
            transportFactory: { _ in CountingTransport() },
            cache: nil
        )
        XCTAssertEqual(fleet.sessions.count, 5)
    }

    /// 순차 probe 는 꺼진 Mac 의 타임아웃이 직렬로 쌓여 목록이 100초 넘게 안 잡힌다.
    /// 그렇다고 16개를 한꺼번에 던지면 릴레이에 몰린다.
    func testInitialRoundRespectsConcurrencyLimit() async {
        let transport = CountingTransport()
        let fleet = DeviceFleet(
            devices: devices(16),
            transportFactory: { _ in transport },
            cache: nil
        )

        await fleet.startInitialRound()

        XCTAssertLessThanOrEqual(transport.maxConcurrent, DeviceFleet.probeConcurrency)
        XCTAssertGreaterThan(transport.maxConcurrent, 1, "직렬로 돌았다")
    }

    func testAllSessionsEndUpOfflineWhenNothingIsReachable() async {
        let fleet = DeviceFleet(
            devices: devices(4),
            transportFactory: { _ in CountingTransport() },
            cache: nil
        )

        // 임계값(3회)만큼 라운드를 돌린다.
        for _ in 0..<DeviceStatusMachine.failureThreshold {
            await fleet.startInitialRound()
        }

        XCTAssertTrue(fleet.sessions.allSatisfy { $0.status == .offline })
    }
}

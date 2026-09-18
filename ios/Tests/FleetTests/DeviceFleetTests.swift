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

/// 끝나지 않는 스냅샷 스트림을 돌려주는 가짜 전송. 세션이 online 에 머무는 상황을
/// 만들어야 `stopAll()` 이 실제로 무언가를 멈추는지 볼 수 있다.
private final class OpenStreamTransport: DeviceTransport, @unchecked Sendable {
    private let snapshot: MirrorSnapshot

    init(snapshot: MirrorSnapshot) {
        self.snapshot = snapshot
    }

    func probe(device: Device, timeoutSeconds: Double) async throws -> AsyncThrowingStream<MirrorSnapshot, Error> {
        let snapshot = self.snapshot
        return AsyncThrowingStream { continuation in
            continuation.yield(snapshot)
            // finish 하지 않는다 — 소비자가 끊어야만 끝난다.
        }
    }
}

private func makeSnapshot() throws -> MirrorSnapshot {
    let json = #"{"v":1,"t":1758000000,"a":[{"k":0,"r":1,"t5":0,"pj":[]}]}"#
    return try JSONDecoder().decode(MirrorSnapshot.self, from: Data(json.utf8))
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

    /// 목록 화면은 레지스트리가 바뀔 때마다 fleet 을 다시 만든다. 옛 fleet 을 멈추지 않으면
    /// 그 세션들이 `while true` 스트림을 계속 소비해 삭제된 장치의 QUIC 연결이 누수되고
    /// 남아 있는 장치는 이중 연결이 된다.
    func testStopAllStopsEverySession() async throws {
        let transport = OpenStreamTransport(snapshot: try makeSnapshot())
        // probeConcurrency(4) 이하로 둔다 — 끝나지 않는 스트림이라 대기열에 남으면
        // 영원히 probe 를 시작하지 못한다.
        let fleet = DeviceFleet(
            devices: devices(3),
            transportFactory: { _ in transport },
            cache: nil
        )

        // 스트림이 끝나지 않으므로 startInitialRound 는 stopAll 전에는 반환하지 않는다.
        let roundFinished = expectation(description: "초기 라운드 종료")
        let round = Task { await fleet.startInitialRound(); roundFinished.fulfill() }
        await waitUntil { fleet.sessions.allSatisfy { $0.status == .online } }
        XCTAssertTrue(fleet.sessions.allSatisfy { $0.status == .online }, "전원 online 이어야 검증할 상황이 된다")

        fleet.stopAll()

        XCTAssertTrue(fleet.sessions.allSatisfy { $0.status == .idle })
        // 취소가 스트림까지 내려가야 라운드가 끝난다 — 매달리면 연결이 안 닫힌 것이다.
        await fulfillment(of: [roundFinished], timeout: 1)
        _ = round
    }

    /// 조건이 만족될 때까지 기다린다. 고정 sleep 과 달리 느린 CI 에서도 흔들리지 않는다.
    private func waitUntil(timeout: TimeInterval = 1, _ condition: () -> Bool) async {
        let deadline = Date().addingTimeInterval(timeout)
        while !condition(), Date() < deadline {
            try? await Task.sleep(nanoseconds: 5_000_000)
        }
    }
}

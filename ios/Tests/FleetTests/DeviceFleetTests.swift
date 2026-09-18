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

/// 스냅샷을 흘린 뒤 **끝나지 않는** 스트림을 돌려주는 가짜 전송. 실제 전송의 성질
/// (연결이 살아 있는 한 스트림이 끝나지 않는다)을 재현한다 — 세션이 online 에 머무는
/// 상황을 만들어야 라운드가 슬롯을 반납하는지, `stopAll()` 이 실제로 끊는지 볼 수 있다.
private final class OpenStreamTransport: DeviceTransport, @unchecked Sendable {
    private let snapshots: [MirrorSnapshot]
    private let onTerminate: (@Sendable () -> Void)?

    init(snapshots: [MirrorSnapshot], onTerminate: (@Sendable () -> Void)? = nil) {
        self.snapshots = snapshots
        self.onTerminate = onTerminate
    }

    func probe(device: Device, timeoutSeconds: Double) async throws -> AsyncThrowingStream<MirrorSnapshot, Error> {
        let snapshots = self.snapshots
        let onTerminate = self.onTerminate
        return AsyncThrowingStream { continuation in
            continuation.onTermination = { _ in onTerminate?() }
            for snapshot in snapshots { continuation.yield(snapshot) }
            // finish 하지 않는다 — 소비자가 끊어야만 끝난다.
        }
    }
}

private func makeSnapshot(rate: Float = 1) throws -> MirrorSnapshot {
    let json = #"{"v":1,"t":1758000000,"a":[{"k":0,"r":\#(rate),"t5":0,"pj":[]}]}"#
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
    /// 남아 있는 장치는 이중 연결이 된다. 상태가 `.idle` 이 되는 것만으로는 부족하고,
    /// 스트림이 실제로 닫혀야 하므로 세션 수만큼의 종료 콜백을 단언한다.
    func testStopAllStopsEverySession() async throws {
        let terminations = expectation(description: "모든 스트림 종료")
        terminations.expectedFulfillmentCount = 3
        let transport = OpenStreamTransport(
            snapshots: [try makeSnapshot()], onTerminate: { terminations.fulfill() }
        )
        let fleet = DeviceFleet(
            devices: devices(3),
            transportFactory: { _ in transport },
            cache: nil
        )

        await fleet.startInitialRound()
        XCTAssertTrue(fleet.sessions.allSatisfy { $0.status == .online }, "전원 online 이어야 검증할 상황이 된다")

        fleet.stopAll()

        XCTAssertTrue(fleet.sessions.allSatisfy { $0.status == .idle })
        // 취소가 스트림까지 내려가야 연결이 닫힌다.
        await fulfillment(of: [terminations], timeout: 1)
    }

    /// 동시성 슬롯은 dial/인증에만 쓰인다. 온라인이 된 세션이 슬롯을 영구 점유하면
    /// 5대째부터는 probe 조차 시작하지 못하고(스펙 §4.4 의 "16대 동시 스트리밍" 이 4대에서
    /// 멈춘다), 라운드가 반환하지 않아 포어그라운드 복귀·3분 타이머의 Task 가 쌓인다.
    func testInitialRoundReturnsWhileStreamsStayOpenAndProbesEveryDevice() async throws {
        let transport = OpenStreamTransport(snapshots: [try makeSnapshot()])
        // 동시성 4를 넘기는 6대 — 슬롯을 반납하지 않으면 4대만 online 이 된다.
        let fleet = DeviceFleet(
            devices: devices(6),
            transportFactory: { _ in transport },
            cache: nil
        )

        let returned = expectation(description: "초기 라운드 반환")
        Task { await fleet.startInitialRound(); returned.fulfill() }
        await fulfillment(of: [returned], timeout: 2)

        XCTAssertEqual(fleet.sessions.count, 6)
        XCTAssertTrue(
            fleet.sessions.allSatisfy { $0.status == .online },
            "온라인 세션이 슬롯을 점유해 일부 장치가 probe 조차 못 했다: "
                + fleet.sessions.map { "\($0.status)" }.joined(separator: ",")
        )

        fleet.stopAll()
    }

    /// 당겨서 새로고침은 오프라인 장치가 **실제로 복구될 때** 끝나야 한다. 소비까지
    /// 기다리면 바로 그 경우에만 스피너가 영원히 남는다.
    func testRetriggerOfflineReturnsWhenDeviceComesBackOnline() async throws {
        let transport = OpenStreamTransport(snapshots: [try makeSnapshot()])
        let fleet = DeviceFleet(
            devices: devices(1),
            transportFactory: { _ in transport },
            cache: nil
        )
        XCTAssertEqual(fleet.sessions[0].status, .idle)

        let returned = expectation(description: "retriggerOffline 반환")
        Task { await fleet.retriggerOffline(); returned.fulfill() }
        await fulfillment(of: [returned], timeout: 1)

        XCTAssertEqual(fleet.sessions[0].status, .online)

        fleet.stopAll()
    }

    /// 스펙 §5.2 — 캐시 쓰기는 스로틀한다. 스트리밍 중에는 스냅샷이 초당 들어오는데
    /// 그때마다 원자적 파일 쓰기를 하면 16대분 디스크 I/O 가 그대로 쌓인다.
    func testCacheWritesAreThrottledAndFlushedOnDemand() async throws {
        let directory = FileManager.default.temporaryDirectory
            .appendingPathComponent("fleet-cache-\(UUID().uuidString)", isDirectory: true)
        defer { try? FileManager.default.removeItem(at: directory) }
        let cache = DeviceSnapshotCache(directory: directory)

        let first = try makeSnapshot(rate: 1)
        let second = try makeSnapshot(rate: 2)
        let transport = OpenStreamTransport(snapshots: [first, second])
        let fleet = DeviceFleet(
            devices: devices(1),
            transportFactory: { _ in transport },
            cache: cache
        )

        await fleet.startInitialRound()
        await waitUntil { fleet.sessions[0].latest == second }

        XCTAssertEqual(
            cache.load(endpointIdHex: "00")?.snapshot, first,
            "스로틀 간격(\(DeviceFleet.cacheWriteInterval)초) 안인데 두 번째 스냅샷까지 썼다"
        )

        fleet.flushCache()

        XCTAssertEqual(cache.load(endpointIdHex: "00")?.snapshot, second, "flushCache 가 마지막 상태를 쓰지 않았다")

        fleet.stopAll()
    }

    /// 조건이 만족될 때까지 기다린다. 고정 sleep 과 달리 느린 CI 에서도 흔들리지 않는다.
    private func waitUntil(timeout: TimeInterval = 1, _ condition: () -> Bool) async {
        let deadline = Date().addingTimeInterval(timeout)
        while !condition(), Date() < deadline {
            try? await Task.sleep(nanoseconds: 5_000_000)
        }
    }
}

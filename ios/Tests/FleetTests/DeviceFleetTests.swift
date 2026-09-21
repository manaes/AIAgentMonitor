import XCTest
import Wire
import os
@testable import Fleet

/// 동시에 몇 개가 실행 중이었는지 관찰하는 가짜 전송.
///
/// 잠금이 `NSLock` 이 아니라 `OSAllocatedUnfairLock` 인 이유: `probe` 가 async 라
/// `NSLock.lock()/unlock()` 은 "비동기 컨텍스트에서 사용 불가" 경고를 내고 Swift 6
/// 언어 모드에서는 에러다.
private final class CountingTransport: DeviceTransport, @unchecked Sendable {
    private struct Counts { var active = 0; var maxConcurrent = 0 }
    private let counts = OSAllocatedUnfairLock(initialState: Counts())
    var maxConcurrent: Int { counts.withLock { $0.maxConcurrent } }

    func probe(device: Device, timeoutSeconds: Double) async throws -> AsyncThrowingStream<MirrorSnapshot, Error> {
        counts.withLock { $0.active += 1; $0.maxConcurrent = max($0.maxConcurrent, $0.active) }
        try await Task.sleep(nanoseconds: 50_000_000)
        counts.withLock { $0.active -= 1 }
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

/// 스냅샷을 흘린 뒤 **연결 끊김**으로 끝나는 가짜 전송. online 이던 세션이 마지막
/// 스냅샷을 그대로 든 채 unstable 로 떨어지는 상황을 만든다.
private final class DroppingStreamTransport: DeviceTransport, @unchecked Sendable {
    private let snapshots: [MirrorSnapshot]

    init(snapshots: [MirrorSnapshot]) {
        self.snapshots = snapshots
    }

    func probe(device: Device, timeoutSeconds: Double) async throws -> AsyncThrowingStream<MirrorSnapshot, Error> {
        let snapshots = self.snapshots
        return AsyncThrowingStream { continuation in
            for snapshot in snapshots { continuation.yield(snapshot) }
            continuation.finish(throwing: DeviceTransportError.unreachable)
        }
    }
}

/// 첫 probe 는 스냅샷을 흘린 뒤 끊기고(online → unstable(1)), 이후 probe 는 dial 자체가
/// 실패한다. 실기의 "보고 있던 Mac 이 잠들었다" 를 재현한다 — 한 번 끊긴 뒤에는 다시
/// 붙지도 못하므로, 재탐색이 실제로 걸리면 실패 카운트가 쌓여 `.offline` 로 진행해야 한다.
private final class DropThenUnreachableTransport: DeviceTransport, @unchecked Sendable {
    private let snapshots: [MirrorSnapshot]
    /// `CountingTransport` 와 같은 이유로 `OSAllocatedUnfairLock` 을 쓴다(async 안전).
    private let counter = OSAllocatedUnfairLock(initialState: 0)
    /// 재탐색이 실제로 걸렸는지는 상태값보다 probe 호출 수가 확실한 증거다.
    var probeCount: Int { counter.withLock { $0 } }

    init(snapshots: [MirrorSnapshot]) {
        self.snapshots = snapshots
    }

    func probe(device: Device, timeoutSeconds: Double) async throws -> AsyncThrowingStream<MirrorSnapshot, Error> {
        let isFirst = counter.withLock { count -> Bool in
            count += 1
            return count == 1
        }
        guard isFirst else { throw DeviceTransportError.unreachable }
        let snapshots = self.snapshots
        return AsyncThrowingStream { continuation in
            for snapshot in snapshots { continuation.yield(snapshot) }
            continuation.finish(throwing: DeviceTransportError.unreachable)
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

    /// Ruling 33 — 3분 타이머·당겨서 새로고침은 `.unstable` 세션도 대상으로 삼아야 한다.
    ///
    /// 스트림이 끊겨 `.unstable(1)` 이 된 장치를 여기서 빼면 그 장치를 다시 probe 하는
    /// 주체가 앱 안에 하나도 남지 않는다 — 실패 카운트가 안 올라 `.offline` 로도 못 가고
    /// "재연결 중"에 영구 정지한다.
    func testRetriggerOfflineIncludesUnstableSessions() async throws {
        let transport = DropThenUnreachableTransport(snapshots: [try makeSnapshot()])
        let fleet = DeviceFleet(
            devices: devices(1),
            transportFactory: { _ in transport },
            cache: nil
        )

        await fleet.startInitialRound()
        // 스트림 종료(에러)는 세션의 probeTask 안에서 처리되므로 전이를 기다린다.
        await waitUntil { fleet.sessions[0].status == .unstable(failureCount: 1) }
        XCTAssertEqual(fleet.sessions[0].status, .unstable(failureCount: 1))
        XCTAssertEqual(transport.probeCount, 1)

        await fleet.retriggerOffline()

        XCTAssertEqual(transport.probeCount, 2, ".unstable 세션이 재탐색 대상에서 빠졌다")
        await waitUntil { fleet.sessions[0].status == .unstable(failureCount: 2) }
        XCTAssertEqual(
            fleet.sessions[0].status, .unstable(failureCount: 2),
            "재시도가 실패 카운트를 초기화했다 — 이러면 .offline 에 영영 도달하지 못한다"
        )

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

    /// flushCache 는 상태로 거르지 않는다. 방금 연결이 끊겨 unstable 로 떨어진 장치도
    /// 마지막 성공 스냅샷을 그대로 들고 있고, 목록 화면은 오프라인 장치에 바로 그 캐시를
    /// 보여준다 — online 만 쓰면 가장 쓸모 있는 스냅샷이 통째로 버려진다.
    func testFlushCacheWritesSessionThatIsNoLongerOnline() async throws {
        let directory = FileManager.default.temporaryDirectory
            .appendingPathComponent("fleet-cache-\(UUID().uuidString)", isDirectory: true)
        defer { try? FileManager.default.removeItem(at: directory) }
        let cache = DeviceSnapshotCache(directory: directory)

        let first = try makeSnapshot(rate: 1)
        let second = try makeSnapshot(rate: 2)
        let fleet = DeviceFleet(
            devices: devices(1),
            transportFactory: { _ in DroppingStreamTransport(snapshots: [first, second]) },
            cache: cache
        )

        await fleet.startInitialRound()
        // 두 번째 스냅샷까지 받고 연결이 끊겨 online 에서 내려온 상태를 기다린다.
        await waitUntil { fleet.sessions[0].latest == second && fleet.sessions[0].status != .online }

        XCTAssertNotEqual(fleet.sessions[0].status, .online, "끊긴 뒤에도 online 이면 검증할 상황이 아니다")
        XCTAssertEqual(fleet.sessions[0].latest, second, "마지막 스냅샷이 남아 있어야 한다")
        XCTAssertEqual(
            cache.load(endpointIdHex: "00")?.snapshot, first,
            "스로틀 간격 안인데 두 번째 스냅샷까지 썼다"
        )

        fleet.flushCache()

        XCTAssertEqual(
            cache.load(endpointIdHex: "00")?.snapshot, second,
            "online 이 아니라는 이유로 마지막 스냅샷을 버렸다"
        )

        fleet.stopAll()
    }

    /// Ruling 34 — `flushCache()` 는 **자기가 가진 세션**을 무조건 다시 쓴다. 방금 지운
    /// 캐시 파일이라도 예외가 아니다.
    ///
    /// 장치 삭제 경로(`DeviceListViewController.removeDevice`)가 이 성질에 걸려 있다.
    /// `cache.remove(...)` 를 먼저 하고 `startFleet()` 을 부르면, `startFleet()` 첫 줄의
    /// `flushCache()` 가 **아직 그 장치의 세션을 들고 있는 옛 fleet** 에서 돌아 방금 지운
    /// 파일을 되살린다. 그래서 순서가 `startFleet()` → `cache.remove(...)` 여야 한다 —
    /// 그 시점의 새 fleet 에는 그 장치가 없으므로 이후 어떤 flush 도 파일을 되살리지 못한다.
    func testFlushCacheRewritesEverySessionItOwnsEvenAfterCacheRemoval() async throws {
        let directory = FileManager.default.temporaryDirectory
            .appendingPathComponent("fleet-cache-\(UUID().uuidString)", isDirectory: true)
        defer { try? FileManager.default.removeItem(at: directory) }
        let cache = DeviceSnapshotCache(directory: directory)

        let snapshot = try makeSnapshot(rate: 7)
        let fleet = DeviceFleet(
            devices: devices(1),
            transportFactory: { _ in OpenStreamTransport(snapshots: [snapshot]) },
            cache: cache
        )

        await fleet.startInitialRound()
        await waitUntil { fleet.sessions[0].latest == snapshot }

        cache.remove(endpointIdHex: "00")
        XCTAssertNil(cache.load(endpointIdHex: "00"), "삭제가 먹지 않았다면 이 테스트는 의미가 없다")

        fleet.flushCache()

        XCTAssertEqual(
            cache.load(endpointIdHex: "00")?.snapshot, snapshot,
            "세션을 아직 들고 있는 fleet 의 flushCache 가 파일을 되살리지 않았다 — 삭제 순서 제약의 근거가 사라졌다"
        )

        fleet.stopAll()
    }

    /// Ruling 34 — 그 장치를 더 이상 갖지 않는 fleet 의 `flushCache()` 는 파일을 되살리지
    /// 않는다. 삭제 순서를 뒤집었을 때 실제로 안전해지는 이유가 이것이다.
    func testFlushCacheDoesNotRewriteDeviceTheFleetNoLongerHas() async throws {
        let directory = FileManager.default.temporaryDirectory
            .appendingPathComponent("fleet-cache-\(UUID().uuidString)", isDirectory: true)
        defer { try? FileManager.default.removeItem(at: directory) }
        let cache = DeviceSnapshotCache(directory: directory)

        let snapshot = try makeSnapshot(rate: 7)
        let old = DeviceFleet(
            devices: devices(2),
            transportFactory: { _ in OpenStreamTransport(snapshots: [snapshot]) },
            cache: cache
        )
        await old.startInitialRound()
        await waitUntil { old.sessions.allSatisfy { $0.latest == snapshot } }
        old.flushCache()
        XCTAssertNotNil(cache.load(endpointIdHex: "01"))

        // 삭제 흐름: 남은 장치(00)만으로 새 fleet 을 만들고 옛 fleet 을 정리한 뒤,
        // **그 다음에** 캐시 파일을 지운다.
        let new = DeviceFleet(
            devices: Array(devices(2).prefix(1)),
            transportFactory: { _ in OpenStreamTransport(snapshots: [snapshot]) },
            cache: cache
        )
        old.flushCache()
        old.onChange = nil
        old.stopAll()
        cache.remove(endpointIdHex: "01")

        new.flushCache()

        XCTAssertNil(
            cache.load(endpointIdHex: "01"),
            "새 fleet 이 갖지도 않은 장치의 캐시 파일이 되살아났다"
        )
        XCTAssertNotNil(cache.load(endpointIdHex: "00"), "남은 장치의 캐시는 그대로여야 한다")

        new.stopAll()
    }

    /// Ruling 38 — 드래그 재정렬은 **연결을 끊지 않고** 표시 순서만 바꾼다.
    ///
    /// 레지스트리에 새 sortIndex 를 저장한 뒤 fleet 을 다시 만들면 16대의 QUIC 연결이
    /// 전부 끊긴다. 그래서 저장된 순서를 살아 있는 세션에 그대로 얹는다 — 그러지 않으면
    /// 다음 갱신(스트리밍 중에는 매초)에서 옛 sortIndex 로 다시 정렬돼 방금 옮긴 자리가
    /// 튕겨 돌아간다.
    func testApplySortOrderUpdatesLiveSessionsWithoutStoppingThem() async throws {
        let transport = OpenStreamTransport(snapshots: [try makeSnapshot()])
        let fleet = DeviceFleet(
            devices: devices(3),
            transportFactory: { _ in transport },
            cache: nil
        )
        await fleet.startInitialRound()
        await waitUntil { fleet.sessions.allSatisfy { $0.status == .online } }

        // 화면이 "02, 00, 01" 로 끌어다 놓았고 레지스트리가 그 순서로 0..2 를 매겼다.
        var reordered = devices(3)
        reordered[2].sortIndex = 0
        reordered[0].sortIndex = 1
        reordered[1].sortIndex = 2

        fleet.applySortOrder(reordered)

        XCTAssertEqual(
            DeviceListPresentation.sorted(fleet.sessions.map { ($0.device, $0.status) })
                .map(\.endpointIdHex),
            ["02", "00", "01"]
        )
        XCTAssertTrue(
            fleet.sessions.allSatisfy { $0.status == .online },
            "재정렬이 연결을 끊었다 — 순서만 바꿔야 한다"
        )

        fleet.stopAll()
    }

    /// Ruling 23 — 상세 화면은 `DeviceSession.onChange` 를 덮어쓰지 않고 fleet 이 주는
    /// `onSessionChange` 로 한 세션을 구독한다. `onChange` 는 목록용이라 계속 살아 있어야
    /// 하고(캐시 쓰기·목록 갱신이 그 경로에 있다), `onSessionChange` 는 **바뀐 그 세션**을
    /// 넘겨야 상세가 자기 세션인지 가려낼 수 있다.
    func testOnSessionChangeReportsChangedSessionAndOnChangeStillFires() async throws {
        let transport = OpenStreamTransport(snapshots: [try makeSnapshot()])
        let fleet = DeviceFleet(
            devices: devices(1),
            transportFactory: { _ in transport },
            cache: nil
        )

        var reported: [DeviceSession] = []
        var listRefreshCount = 0
        fleet.onSessionChange = { reported.append($0) }
        fleet.onChange = { listRefreshCount += 1 }

        await fleet.startInitialRound()
        await waitUntil { fleet.sessions[0].status == .online }

        XCTAssertFalse(reported.isEmpty, "onSessionChange 가 한 번도 불리지 않았다")
        XCTAssertTrue(
            reported.allSatisfy { $0 === fleet.sessions[0] },
            "바뀐 세션이 아닌 다른 인스턴스를 넘겼다 — 상세 화면이 identity 로 걸러낼 수 없다"
        )
        XCTAssertGreaterThan(
            listRefreshCount, 0,
            "onSessionChange 를 추가하면서 목록용 onChange 가 끊겼다"
        )

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

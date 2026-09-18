import XCTest
import Wire
@testable import Fleet

private func makeSnapshot(rate: Float) throws -> MirrorSnapshot {
    let json = #"{"v":1,"t":1758000000,"a":[{"k":0,"r":\#(rate),"t5":0,"pj":[]}]}"#
    return try JSONDecoder().decode(MirrorSnapshot.self, from: Data(json.utf8))
}

/// 결과를 시험이 직접 정하는 가짜 전송.
private final class FakeTransport: DeviceTransport, @unchecked Sendable {
    enum Outcome {
        case stream([MirrorSnapshot])
        case fail(DeviceTransportError)
        /// 진행 중인 probe 를 흉내낸다. `stop()` 이 그 사이에 끼어드는 경쟁을 재현할 때 쓴다.
        /// 테스트 전용이라 지속시간을 짧게 조절할 수 있게 열어둔다(기본 300ms).
        case hang(nanoseconds: UInt64 = 300_000_000)
        /// 이미 online 이 된 뒤(스냅샷을 한 번 이상 흘려보낸 뒤) 스트림 도중에 종단
        /// 상태를 만나는 경우를 흉내낸다 — `consume()` 이 그 타입을 무시하고 전부
        /// `connectionLost` 로 뭉개는 회귀를 잡기 위한 픽스처.
        case streamThenFail([MirrorSnapshot], DeviceTransportError)
        /// 스냅샷 하나를 흘린 뒤 **끝나지 않는** 스트림. 소비자가 끊으면 `onTerminate` 가
        /// 불린다 — `stop()` 이 스트림을 실제로 닫는지(취소 연쇄) 고정하는 픽스처다.
        /// generation 가드만으로는 여기서 연결이 닫히지 않는다: 다음 스냅샷이 와야 가드에
        /// 걸리는데 이 스트림은 더 보내지 않는다(조용한 Mac).
        case openStream(MirrorSnapshot, onTerminate: @Sendable () -> Void)
    }
    var outcomes: [Outcome] = []
    private(set) var probeCount = 0

    func probe(device: Device, timeoutSeconds: Double) async throws -> AsyncThrowingStream<MirrorSnapshot, Error> {
        probeCount += 1
        let outcome = outcomes.isEmpty ? Outcome.fail(.unreachable) : outcomes.removeFirst()
        switch outcome {
        case .fail(let error):
            throw error
        case .hang(let nanoseconds):
            try await Task.sleep(nanoseconds: nanoseconds)
            throw DeviceTransportError.unreachable
        case .stream(let snapshots):
            return AsyncThrowingStream { continuation in
                for s in snapshots { continuation.yield(s) }
                continuation.finish()
            }
        case .streamThenFail(let snapshots, let error):
            return AsyncThrowingStream { continuation in
                for s in snapshots { continuation.yield(s) }
                continuation.finish(throwing: error)
            }
        case .openStream(let snapshot, let onTerminate):
            return AsyncThrowingStream { continuation in
                continuation.onTermination = { _ in onTerminate() }
                continuation.yield(snapshot)
                // finish 하지 않는다 — 소비자가 끊어야만 끝난다.
            }
        }
    }
}

@MainActor
final class DeviceSessionTests: XCTestCase {

    private func makeDevice() -> Device {
        Device(endpointIdHex: "aa", token: "t", relayUrl: nil, addresses: [],
               macHostname: nil, userLabel: nil, sortIndex: 0)
    }

    func testSuccessfulProbeGoesOnlineAndPublishesSnapshot() async throws {
        let transport = FakeTransport()
        transport.outcomes = [.stream([try makeSnapshot(rate: 3)])]
        let session = DeviceSession(device: makeDevice(), transport: transport)

        await session.probeNow()

        XCTAssertEqual(session.status, .online)
        XCTAssertEqual(session.latest?.agents.first?.ratePerSec, 3)
    }

    /// Ruling A: latestAt 은 성공적인 probe 로 채워지고 stop() 으로 지워져야 한다.
    /// 오프라인 장치가 마지막 스냅샷을 계속 들고 있을 때, 목록 화면이 그 스냅샷의
    /// 수신 시각(latestAt)으로 신선도를 계산할 수 있어야 "방금 전"으로 오판하지 않는다.
    func testLatestAtIsPopulatedOnSuccessAndClearedByStop() async throws {
        let transport = FakeTransport()
        transport.outcomes = [.stream([try makeSnapshot(rate: 3)])]
        let session = DeviceSession(device: makeDevice(), transport: transport)

        XCTAssertNil(session.latestAt)

        let before = Date()
        await session.probeNow()
        let after = Date()

        let latestAt = try XCTUnwrap(session.latestAt)
        XCTAssertGreaterThanOrEqual(latestAt, before)
        XCTAssertLessThanOrEqual(latestAt, after)

        session.stop()
        XCTAssertNil(session.latestAt)
        XCTAssertNil(session.latest)
    }

    func testThreeFailuresGoOffline() async {
        let transport = FakeTransport()
        transport.outcomes = [.fail(.unreachable), .fail(.unreachable), .fail(.unreachable)]
        let session = DeviceSession(device: makeDevice(), transport: transport)

        await session.probeNow()
        await session.probeNow()
        await session.probeNow()

        XCTAssertEqual(session.status, .offline)
    }

    /// 인증 거부는 재시도하지 않는다 — 트리거가 와도 probe 를 다시 부르면 안 된다.
    func testAuthRejectionStopsRetrying() async {
        let transport = FakeTransport()
        transport.outcomes = [.fail(.authRejected)]
        let session = DeviceSession(device: makeDevice(), transport: transport)

        await session.probeNow()
        XCTAssertEqual(session.status, .needsRepairing)

        await session.retrigger()
        XCTAssertEqual(session.status, .needsRepairing)
        XCTAssertEqual(transport.probeCount, 1, "종단 상태인데 probe 를 다시 불렀다")
    }

    func testVersionMismatchIsTerminal() async {
        let transport = FakeTransport()
        transport.outcomes = [.fail(.versionMismatch)]
        let session = DeviceSession(device: makeDevice(), transport: transport)

        await session.probeNow()

        XCTAssertEqual(session.status, .versionMismatch)
    }

    /// 이미 online 인 장치가 스트림 도중에(probe 성공 이후) 버전 불일치를 만나도
    /// 종단 상태에 도달해야 한다. `consume()` 이 스트림 에러 타입을 안 가리고 전부
    /// `connectionLost` 로 처리하면, 이 장치는 `.unstable` 로만 갔다가 재시도되고
    /// 절대 `.versionMismatch` 에 이르지 못한다 — Task 12 리뷰에서 발견된 회귀.
    func testMidStreamVersionMismatchAfterOnlineReachesTerminalState() async throws {
        let transport = FakeTransport()
        transport.outcomes = [.streamThenFail([try makeSnapshot(rate: 1)], .versionMismatch)]
        let session = DeviceSession(device: makeDevice(), transport: transport)

        await session.probeNow()

        XCTAssertEqual(session.status, .versionMismatch)
    }

    /// 위와 같은 회귀를 인증 거부 쪽에서도 검증한다.
    func testMidStreamAuthRejectionAfterOnlineReachesTerminalState() async throws {
        let transport = FakeTransport()
        transport.outcomes = [.streamThenFail([try makeSnapshot(rate: 1)], .authRejected)]
        let session = DeviceSession(device: makeDevice(), transport: transport)

        await session.probeNow()

        XCTAssertEqual(session.status, .needsRepairing)
    }

    /// stop() 이후 늦게 도착한 결과가 상태를 오염시키면 안 된다 —
    /// 2026-09-17 TunnelKit 리뷰의 버그 클래스.
    ///
    /// 이 경우는 probeNow() 호출 자체가 stop() 보다 나중이라 완전히 순차적이다 — 진입 시점의
    /// `isStopped` 가드가 막아준다. 진짜 경쟁(진행 중이던 probe 가 stop() 이후에 완료되는 경우)은
    /// `testInFlightProbeResultArrivingAfterStopIsIgnored` 가 따로 검증한다.
    func testResultArrivingAfterStopIsIgnored() async throws {
        let transport = FakeTransport()
        transport.outcomes = [.stream([try makeSnapshot(rate: 9)])]
        let session = DeviceSession(device: makeDevice(), transport: transport)

        session.stop()
        await session.probeNow()

        XCTAssertEqual(session.status, .idle)
        XCTAssertNil(session.latest)
    }

    /// 이 태스크가 막으려는 실제 버그 클래스: probe 가 아직 전송 계층에 붙어 있는 도중에
    /// stop() 이 끼어들고, 그 뒤에야 (실패로) 완료된다. `isStopped` 가드는 probeNow() 진입
    /// 시점에만 검사하므로 이미 시작된 호출은 막지 못한다 — 여기서 실제로 검증해야 하는 건
    /// generation 카운터다: stop() 이 generation 을 올려버리면, 나중에 도착하는 완료 콜백의
    /// `current == generation` 검사가 거짓이 되어 상태 갱신이 조용히 버려져야 한다.
    func testInFlightProbeResultArrivingAfterStopIsIgnored() async throws {
        let transport = FakeTransport()
        transport.outcomes = [.hang()]
        let session = DeviceSession(device: makeDevice(), transport: transport)

        let probeTask = Task { await session.probeNow() }
        // probeNow() 가 transport.probe(...) 호출부까지 진입해서 실제로 hang 에 들어갈
        // 시간을 준다 — stop() 이 "그 사이"에 끼어드는 걸 운이 아니라 순서로 보장한다.
        try await Task.sleep(nanoseconds: 50_000_000)
        XCTAssertEqual(session.status, .probing, "stop() 이전에 probe 가 실제로 진행 중이어야 경쟁을 재현한다")

        session.stop()
        await probeTask.value

        XCTAssertEqual(session.status, .idle)
        XCTAssertNil(session.latest)
        XCTAssertNil(session.latestAt)
    }

    /// retrigger() 는 이미 probing 중일 때 두 번째 probe 를 시작하면 안 된다.
    /// `DeviceStatusMachine.next(.probing, on: .retriggered)` 는 항등 매핑으로 `.probing` 을
    /// 그대로 돌려주는데, 이는 `.offline → .probing` 전이와 값이 같아서 상태값만으로는 구분이
    /// 안 된다 — `guard status != .probing` 가드가 없으면 진행 중인 probe 에 재요청이 겹쳐
    /// generation 이 올라가고, 방금 시작된 probe 자체가 스스로를 "늦게 도착한 결과"로 만들어
    /// 버린다. 상태값이 아니라 probeCount 로 검증한다 — 상태는 우연히 같아 보일 수 있어도
    /// 두 번째 probe() 호출은 가드 실패의 명백한 증거이기 때문이다.
    func testRetriggerIgnoresRequestWhileProbeIsInFlight() async throws {
        let transport = FakeTransport()
        transport.outcomes = [.hang()]
        let session = DeviceSession(device: makeDevice(), transport: transport)

        let probeTask = Task { await session.probeNow() }
        try await Task.sleep(nanoseconds: 50_000_000)
        XCTAssertEqual(session.status, .probing, "probing 중이어야 이 테스트가 검증하려는 경합이 재현된다")

        await session.retrigger()

        XCTAssertEqual(transport.probeCount, 1, "probing 중인데 retrigger 가 두 번째 probe 를 시작했다")

        // 진행 중이던 probe 를 마저 끝내서 태스크가 새지 않게 한다.
        await probeTask.value
    }

    /// `stop()` 은 진행 중인 스트림을 **실제로 닫아야** 한다. generation 가드는 늦게 도착한
    /// 결과를 무시할 뿐이라, 스냅샷이 더 오지 않는 조용한 Mac 의 스트림은 영원히 열린 채
    /// 남는다 — 레지스트리가 바뀔 때마다 fleet 을 다시 만드는 목록 화면에서 이는 QUIC
    /// 연결 누수가 된다. 취소 연쇄(probeTask.cancel → for try await → onTermination)로만
    /// 닫히므로 `onTerminate` 콜백으로 고정한다.
    func testStopCancelsInFlightStreamAndClosesIt() async throws {
        let transport = FakeTransport()
        let terminated = expectation(description: "스트림 종료 콜백")
        transport.outcomes = [.openStream(try makeSnapshot(rate: 4), onTerminate: { terminated.fulfill() })]
        let session = DeviceSession(device: makeDevice(), transport: transport)

        // probeNow 의 반환도 기대치로 감싼다 — 그냥 `await probing.value` 로 두면 회귀 시
        // 테스트가 깔끔히 실패하지 않고 영원히 매달려 스위트 전체를 멈춘다.
        let returned = expectation(description: "probeNow 반환")
        let probing = Task { await session.probeNow(); returned.fulfill() }
        await waitUntil { session.status == .online }
        XCTAssertEqual(session.status, .online, "스트림이 열려 online 이어야 이 테스트가 검증할 상황이 된다")

        session.stop()

        await fulfillment(of: [terminated, returned], timeout: 1)
        XCTAssertEqual(session.status, .idle)
        _ = probing
    }

    /// 상태가 바뀔 때까지 기다린다. 고정 sleep 과 달리 느린 CI 에서도 흔들리지 않는다.
    private func waitUntil(
        timeout: TimeInterval = 1, _ condition: () -> Bool
    ) async {
        let deadline = Date().addingTimeInterval(timeout)
        while !condition(), Date() < deadline {
            try? await Task.sleep(nanoseconds: 5_000_000)
        }
    }

    func testOfflineDeviceProbesAgainOnRetrigger() async throws {
        let transport = FakeTransport()
        transport.outcomes = [
            .fail(.unreachable), .fail(.unreachable), .fail(.unreachable),
            .stream([try makeSnapshot(rate: 1)]),
        ]
        let session = DeviceSession(device: makeDevice(), transport: transport)
        for _ in 0..<3 { await session.probeNow() }
        XCTAssertEqual(session.status, .offline)

        await session.retrigger()

        XCTAssertEqual(session.status, .online)
        XCTAssertEqual(transport.probeCount, 4)
    }
}

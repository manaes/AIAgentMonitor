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
        case hang
    }
    var outcomes: [Outcome] = []
    private(set) var probeCount = 0

    func probe(device: Device, timeoutSeconds: Double) async throws -> AsyncThrowingStream<MirrorSnapshot, Error> {
        probeCount += 1
        let outcome = outcomes.isEmpty ? Outcome.fail(.unreachable) : outcomes.removeFirst()
        switch outcome {
        case .fail(let error):
            throw error
        case .hang:
            try await Task.sleep(nanoseconds: 10_000_000_000)
            throw DeviceTransportError.unreachable
        case .stream(let snapshots):
            return AsyncThrowingStream { continuation in
                for s in snapshots { continuation.yield(s) }
                continuation.finish()
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

    /// stop() 이후 늦게 도착한 결과가 상태를 오염시키면 안 된다 —
    /// 2026-09-17 TunnelKit 리뷰의 버그 클래스.
    func testResultArrivingAfterStopIsIgnored() async throws {
        let transport = FakeTransport()
        transport.outcomes = [.stream([try makeSnapshot(rate: 9)])]
        let session = DeviceSession(device: makeDevice(), transport: transport)

        session.stop()
        await session.probeNow()

        XCTAssertEqual(session.status, .idle)
        XCTAssertNil(session.latest)
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

import XCTest
@testable import Fleet

final class DeviceStatusTests: XCTestCase {

    private func next(_ status: DeviceStatus, _ event: DeviceEvent) -> DeviceStatus {
        DeviceStatusMachine.next(status, on: event)
    }

    func testProbeSuccessGoesOnline() {
        XCTAssertEqual(next(.probing, .probeSucceeded), .online)
    }

    func testConnectionLossFromOnlineBecomesUnstable() {
        XCTAssertEqual(next(.online, .connectionLost), .unstable(failureCount: 1))
    }

    /// 연속 3회 실패해야 오프라인으로 떨어진다 — 일시적 끊김과 실제 꺼짐을 구분한다.
    func testOfflineOnlyAfterThreeConsecutiveFailures() {
        var status = next(.online, .connectionLost)      // 1
        XCTAssertEqual(status, .unstable(failureCount: 1))
        status = next(status, .probeFailed)               // 2
        XCTAssertEqual(status, .unstable(failureCount: 2))
        status = next(status, .probeFailed)               // 3
        XCTAssertEqual(status, .offline)
    }

    /// 중간에 한 번 성공하면 실패 카운터가 리셋돼야 한다.
    func testSuccessResetsFailureCount() {
        var status = next(.online, .connectionLost)
        status = next(status, .probeFailed)
        XCTAssertEqual(status, .unstable(failureCount: 2))

        status = next(status, .probeSucceeded)
        XCTAssertEqual(status, .online)

        XCTAssertEqual(next(status, .connectionLost), .unstable(failureCount: 1))
    }

    /// 인증 거부는 재시도로 풀리지 않는다. NetworkClient.swift:213-222 의 교훈 —
    /// 성공할 수 없는 재시도가 QR 스캐너를 깜빡이게 만든 버그가 있었다.
    func testAuthRejectionIsTerminalAndNotRetried() {
        XCTAssertEqual(next(.probing, .authRejected), .needsRepairing)
        // 재탐색 트리거가 와도 상태가 바뀌지 않는다.
        XCTAssertEqual(next(.needsRepairing, .retriggered), .needsRepairing)
        XCTAssertEqual(next(.needsRepairing, .probeFailed), .needsRepairing)
    }

    func testVersionMismatchIsTerminal() {
        XCTAssertEqual(next(.probing, .versionRejected), .versionMismatch)
        XCTAssertEqual(next(.versionMismatch, .retriggered), .versionMismatch)
    }

    /// 오프라인 탈출은 트리거(타이머/포어그라운드/당겨서 새로고침)로만 일어난다.
    func testOfflineLeavesOnlyOnRetrigger() {
        XCTAssertEqual(next(.offline, .probeFailed), .offline)
        XCTAssertEqual(next(.offline, .retriggered), .probing)
    }

    func testStreamEstablishedKeepsOnline() {
        XCTAssertEqual(next(.online, .streamEstablished), .online)
    }

    /// 재시도 중 probeStarted 가 실패 카운트를 초기화하면 안 된다.
    /// Task 10 세션은 재시도마다 probeStarted 를 보내는데, 이걸 .probing 으로
    /// 되돌리면 currentFailureCount 가 0으로 리셋돼서 3회 실패해도 절대
    /// offline 에 도달하지 못한다.
    func testProbeStartedPreservesUnstableFailureCount() {
        let status = DeviceStatus.unstable(failureCount: 2)
        XCTAssertEqual(next(status, .probeStarted), .unstable(failureCount: 2))
        XCTAssertEqual(next(status, .probeStarted), status)
        // 카운트가 보존됐으니 다음 실패로 곧장 offline 에 도달해야 한다.
        XCTAssertEqual(next(next(status, .probeStarted), .probeFailed), .offline)
    }
}

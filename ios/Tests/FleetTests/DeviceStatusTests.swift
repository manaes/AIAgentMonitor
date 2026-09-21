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

    /// .unstable 이 아닌 상태에서는 probeStarted 가 그대로 .probing 으로 전이돼야
    /// 한다. testProbeStartedPreservesUnstableFailureCount 는 "unstable 은 보존"만
    /// 고정하므로, 나머지 절반("그 외는 probing 으로 전이")도 따로 고정해야
    /// 나중에 이 분기가 no-op 으로 퇴화해도 테스트가 잡아낼 수 있다.
    func testProbeStartedBecomesProbingFromNonUnstableStates() {
        XCTAssertEqual(next(.idle, .probeStarted), .probing)
        XCTAssertEqual(next(.online, .probeStarted), .probing)

        // .offline 에서도 probeStarted 는 .probing 으로 전이한다 — 이것이
        // "오프라인 탈출은 retriggered 로만 일어난다"는 제약을 어기는 게 아니다.
        // 실제 탈출 게이트는 DeviceSession.retrigger() 쪽에 있다: 그 함수는
        // DeviceStatusMachine.next(status, on: .retriggered) 결과를 상태에
        // 대입하는 게 아니라 "재탐색을 허용할지" 판단하는 가드로만 쓰고, 실제로
        // 상태를 .offline 에서 옮기는 건 그 뒤에 이어지는 probeNow() 호출이
        // 발생시키는 probeStarted 이벤트다. 만약 여기서 probeStarted 가
        // .offline 을 그대로 유지해버리면, 트리거로 재탐색을 허용해놓고도
        // 실제로 프로빙하는 동안 화면에는 계속 "오프라인"이 떠 있게 된다.
        // 즉 "트리거 없이는 재탐색 자체가 시작되지 않는다"는 게이트는 이
        // 상태기계가 아니라 세션의 retrigger() 가드에 있고, 여기서는 이미
        // 시작된 프로빙을 상태에 반영하기만 하면 된다.
        XCTAssertEqual(next(.offline, .probeStarted), .probing)
    }

    /// Ruling 33 — "재탐색 대상인가" 판정을 전 케이스로 고정한다.
    ///
    /// 이 판정은 3분 타이머(`DeviceFleet.retriggerOffline`)와 세션 가드
    /// (`DeviceSession.retrigger`)가 **같이** 쓴다. 두 곳에 따로 적혀 어긋났을 때
    /// `.unstable` 장치가 영영 "재연결 중"에 머무는 버그가 났다 — 재탐색이 막히면
    /// 실패 카운트가 오르지 않아 `.offline` 로도 가지 못한다.
    func testIsRetriggerableCoversEveryStatus() {
        // 붙어 있지 않은 상태 — 다시 붙어봐야 한다.
        XCTAssertTrue(DeviceStatus.idle.isRetriggerable)
        XCTAssertTrue(DeviceStatus.offline.isRetriggerable)
        XCTAssertTrue(DeviceStatus.unstable(failureCount: 1).isRetriggerable)
        XCTAssertTrue(DeviceStatus.unstable(failureCount: 2).isRetriggerable)

        // 이미 붙어 있거나 붙는 중 — 재요청은 진행 중인 probe 를 무효화한다.
        XCTAssertFalse(DeviceStatus.probing.isRetriggerable)
        XCTAssertFalse(DeviceStatus.online.isRetriggerable)

        // 종단 상태 — 재시도로는 절대 풀리지 않는다.
        XCTAssertFalse(DeviceStatus.needsRepairing.isRetriggerable)
        XCTAssertFalse(DeviceStatus.versionMismatch.isRetriggerable)
    }
}

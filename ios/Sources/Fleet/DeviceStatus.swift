import Foundation

public enum DeviceStatus: Equatable, Sendable {
    case idle
    case probing
    case online
    /// 연결이 끊겨 재시도 중. `failureCount` 가 임계값에 닿으면 offline 이 된다.
    case unstable(failureCount: Int)
    case offline
    /// 토큰 폐기·Mac 재설치. 재시도로는 절대 풀리지 않는다.
    case needsRepairing
    case versionMismatch
}

public enum DeviceEvent: Equatable, Sendable {
    case probeStarted
    case probeSucceeded
    case streamEstablished
    case connectionLost
    case probeFailed
    case authRejected
    case versionRejected
    /// 타이머(3분)/포어그라운드 복귀/당겨서 새로고침
    case retriggered
}

/// 장치 하나의 상태 전이 규칙. 순수 함수라 테스트로 고정한다.
public struct DeviceStatusMachine {
    public static let failureThreshold = 3

    public static func next(_ status: DeviceStatus, on event: DeviceEvent) -> DeviceStatus {
        // 종단 상태는 어떤 이벤트로도 벗어나지 않는다. 사용자가 다시 페어링해야만
        // 레지스트리가 바뀌고 새 세션이 생긴다.
        switch status {
        case .needsRepairing, .versionMismatch:
            return status
        default:
            break
        }

        switch event {
        case .authRejected:
            return .needsRepairing
        case .versionRejected:
            return .versionMismatch
        case .probeStarted:
            // 세션은 재시도마다 probeStarted 를 보낸다. .unstable(n) 을 .probing 으로
            // 되돌리면 실패 카운트가 사라져서 3회 실패해도 offline 에 도달할 수 없다.
            // "재연결 중" 표시는 .unstable 만으로 이미 충분하다.
            if case .unstable = status {
                return status
            }
            return .probing
        case .probeSucceeded, .streamEstablished:
            return .online
        case .connectionLost, .probeFailed:
            let failures = currentFailureCount(status) + 1
            return failures >= failureThreshold ? .offline : .unstable(failureCount: failures)
        case .retriggered:
            return status == .offline ? .probing : status
        }
    }

    private static func currentFailureCount(_ status: DeviceStatus) -> Int {
        switch status {
        case .unstable(let count): return count
        case .offline: return failureThreshold
        default: return 0
        }
    }
}

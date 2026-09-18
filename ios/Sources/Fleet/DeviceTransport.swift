import Foundation
import Wire

public enum DeviceTransportError: Error, Equatable {
    /// 도달 불가(타임아웃·연결 실패). 재시도로 풀릴 수 있다.
    case unreachable
    /// 토큰이 거부됐다. 재시도로는 절대 풀리지 않는다.
    case authRejected
    case versionMismatch
}

/// 장치 하나에 붙어 스냅샷을 흘려보내는 전송 계층.
///
/// probe 성공 시 **그 연결을 그대로 유지한 채** 스트림을 돌려준다. 다시 dial 하면
/// 가장 비싼 부분(hole-punch·QUIC·인증 왕복)을 두 번 낸다.
///
/// ## 구현체가 반드시 지켜야 하는 스트림 종료 규약
/// `DeviceSession.consume(_:generation:)` 은 스트림이 **에러 없이** 끝나는 것을
/// "지금 당장은 더 보낼 스냅샷이 없다"는 뜻으로만 해석하고 마지막 상태(대개 `.online`)를
/// 그대로 둔다 — 종단 상태로 취급하지 않는다. 그러므로 스트림 도중에 발견한 종단 상태
/// (버전 불일치, 인증 만료·폐기, 연결 단절 등)를 **정상 종료(에러 없는 `continuation.finish()`
/// 나 `return`)로 표현하면 안 된다.** 예를 들어 `NetworkClient` 의 기존 루프처럼
/// `wantsRunning = false; return` 으로 조용히 빠져나가는 패턴을 그대로 이 프로토콜의
/// 구현에 옮기면, 이미 `.online` 인 장치가 버전 불일치를 뒤늦게 만났을 때 스트림만 조용히
/// 끝나고 상태는 `.online` 에 얼어붙은 채 다시는 갱신되지 않는다. 종단 상태를 발견하면
/// 반드시 해당하는 `DeviceTransportError` 를 **던져야** 한다.
public protocol DeviceTransport: Sendable {
    func probe(device: Device, timeoutSeconds: Double) async throws -> AsyncThrowingStream<MirrorSnapshot, Error>
}

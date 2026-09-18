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
public protocol DeviceTransport: Sendable {
    func probe(device: Device, timeoutSeconds: Double) async throws -> AsyncThrowingStream<MirrorSnapshot, Error>
}

import Foundation
import NetworkTransport
import Wire

/// 실제 iroh 전송. `NetworkClient.probe`로 연결을 열고 **그 연결을 그대로**
/// `snapshotStream`으로 이어 붙인다 — 다시 dial 하면 hole-punch·QUIC·인증
/// 왕복이라는 가장 비싼 부분을 두 번 내게 된다.
public struct IrohDeviceTransport: DeviceTransport {
    private let provider: IrohEndpointProvider

    public init(provider: IrohEndpointProvider = .shared) {
        self.provider = provider
    }

    public func probe(
        device: Device, timeoutSeconds: Double
    ) async throws -> AsyncThrowingStream<MirrorSnapshot, Error> {
        let client = await NetworkClient(endpointProvider: provider)
        do {
            let result = try await client.probe(
                endpointIdHex: device.endpointIdHex,
                relayUrl: device.relayUrl,
                addresses: device.addresses,
                timeoutSeconds: timeoutSeconds
            )
            return await client.snapshotStream(from: result)
        } catch {
            throw Self.mapError(error)
        }
    }

    /// 재시도로 풀리는 실패와 그렇지 않은 실패를 가른다. 이 매핑이 틀리면
    /// 인증 거부처럼 절대 풀리지 않는 실패를 `.unreachable`로 뭉개 무한
    /// 재시도에 빠뜨리거나(`NetworkClient.swift`의 QR 스캐너 깜빡임 버그와
    /// 같은 클래스), 반대로 일시적 실패를 종단 상태로 취급해 포기하게 만든다.
    static func mapError(_ error: Error) -> DeviceTransportError {
        switch error {
        case NetworkClientError.authFailed, NetworkClientError.needsPairing:
            return .authRejected
        case NetworkClientError.versionMismatch:
            return .versionMismatch
        default:
            return .unreachable
        }
    }
}

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
                timeoutSeconds: timeoutSeconds,
                // 장치마다 토큰이 다르다. 넘기지 않으면 전역 Keychain 슬롯 하나로
                // 전부 인증을 시도하고, 실패하면 그 슬롯을 지워 1:1 앱의 페어링까지 날린다.
                token: device.token
            )
            return Self.mappingErrors(await client.snapshotStream(from: result))
        } catch {
            throw Self.mapError(error)
        }
    }

    /// 스트림 본문에서 던져진 NetworkClientError 를 DeviceTransportError 로 바꿔 다시 던진다.
    ///
    /// `probe` 의 do/catch 는 **호출**만 감싼다 — `snapshotStream` 은 throwing 이 아니고
    /// 즉시 반환하므로, 스트림이 도는 도중 던져지는 에러는 그 catch 를 절대 거치지 않는다.
    /// 감싸지 않으면 `NetworkClientError.versionMismatch` 가 날것으로 나가고,
    /// `DeviceSession.consume` 의 `catch let error as DeviceTransportError` 분기가
    /// 영영 매치되지 않아 종단 상태가 `.unstable` → `.offline` 로 격하돼 무한 재시도가 된다.
    static func mappingErrors(
        _ upstream: AsyncThrowingStream<MirrorSnapshot, Error>
    ) -> AsyncThrowingStream<MirrorSnapshot, Error> {
        AsyncThrowingStream { continuation in
            let task = Task {
                do {
                    for try await snapshot in upstream { continuation.yield(snapshot) }
                    continuation.finish()
                } catch {
                    // CancellationError 도 여기서 `.unreachable` 이 되는데, 취소는
                    // 세션 정리 경로라 `DeviceSession` 의 세대 검사가 걸러낸다.
                    continuation.finish(throwing: Self.mapError(error))
                }
            }
            // 소비자가 멈추면 이 태스크를 끊고, 그러면 upstream 의 onTermination 이
            // 연쇄로 불려 QUIC 연결까지 닫힌다.
            continuation.onTermination = { _ in task.cancel() }
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

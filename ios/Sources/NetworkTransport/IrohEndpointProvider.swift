import Foundation
import IrohLib

/// iroh Endpoint 를 **하나만** 만들어 공유한다.
///
/// Endpoint 는 소켓 하나가 아니라 자체 UDP 소켓 + relay 연결 + discovery 상태를
/// 끌고 다니는 무거운 객체다. 장치마다 `EndpointBuilder().bind()` 를 하면 그게
/// 그대로 N벌이 된다. QUIC 은 같은 소켓 위에서 연결을 다중화하므로, Endpoint 는
/// 하나로 두고 `connect(addr:alpn:)` 만 대상마다 호출하면 된다.
public actor IrohEndpointProvider {
    public static let shared = IrohEndpointProvider()

    private var endpoint: Endpoint?
    /// 생성 중인 Task 를 공유한다. 이게 없으면 동시에 들어온 요청마다 bind 가
    /// 돌아 Endpoint 가 여러 개 생긴다.
    private var bindTask: Task<Endpoint, Error>?

    public init() {}

    public func endpoint() async throws -> Endpoint {
        if let endpoint { return endpoint }
        if let bindTask { return try await bindTask.value }

        let task = Task<Endpoint, Error> {
            let builder = EndpointBuilder()
            builder.applyN0()
            builder.alpns(alpns: [NetworkClient.alpnData])
            return try await builder.bind()
        }
        bindTask = task
        do {
            let bound = try await task.value
            endpoint = bound
            bindTask = nil
            return bound
        } catch {
            // 실패한 Task 를 남겨두면 이후 요청이 영원히 같은 실패를 되받는다.
            bindTask = nil
            throw error
        }
    }
}

import Combine
import Wire

/// `MirrorViewController` 가 BLE/네트워크 어느 쪽이든 구체 타입 없이 들고 있을 수
/// 있게 하는 최소 추상화. `BLETransport` 모듈에 두는 이유는 `NetworkTransport` 가
/// 이미 이 모듈에 의존하기 때문(`BLEClient.decide`/`PairingClient`/`TokenStore` 재사용)
/// — 반대로 `BLETransport` 가 `NetworkTransport` 를 아는 건 안 된다.
@MainActor
public protocol MirrorTransport: AnyObject {
    var state: AnyPublisher<ConnectionState, Never> { get }
    var snapshots: AnyPublisher<MirrorSnapshot, Never> { get }
    func start()
    func stop()
}

extension BLEClient: MirrorTransport {}

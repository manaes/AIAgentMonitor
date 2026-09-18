import Fleet
import Foundation
import NetworkTransport

/// 앱 전역 조립 지점. 레지스트리·캐시·fleet 을 여기서만 만든다.
@MainActor
final class AppEnvironment {
    let registry = DeviceRegistry(store: KeychainRegistryStore())
    let cache = DeviceSnapshotCache(directory: DeviceSnapshotCache.defaultDirectory())

    /// 레지스트리가 손상된 경우. 빈 목록으로 조용히 시작하지 않고 화면에 알린다.
    private(set) var registryError: Error?

    func makeFleet() -> DeviceFleet {
        let devices: [Device]
        do {
            devices = try registry.load()
        } catch {
            registryError = error
            devices = []
        }
        return DeviceFleet(
            devices: devices,
            transportFactory: { _ in IrohDeviceTransport() },
            cache: cache
        )
    }
}

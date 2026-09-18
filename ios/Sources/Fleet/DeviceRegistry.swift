import Foundation

public enum DeviceRegistryError: Error, Equatable {
    case deviceLimitReached
    case corrupted
}

/// 레지스트리 바이트를 어디에 둘지. 실제 앱은 Keychain, 테스트는 인메모리를 쓴다.
public protocol DeviceRegistryStore {
    func read() throws -> Data?
    func write(_ data: Data) throws
}

/// 장치 목록 전체를 **항목 하나**로 저장한다. 16대면 몇 KB라 읽기/쓰기 한 번이면
/// 되고 원자적이다. 장치마다 항목을 따로 두면 열거하려고 kSecMatchLimitAll 쿼리를
/// 돌리고 계정 문자열을 파싱해야 한다.
public final class DeviceRegistry {
    public static let maxDevices = 16

    private let store: DeviceRegistryStore

    public init(store: DeviceRegistryStore) {
        self.store = store
    }

    public func load() throws -> [Device] {
        guard let data = try store.read(), !data.isEmpty else { return [] }
        do {
            return try JSONDecoder().decode([Device].self, from: data)
        } catch {
            // 빈 목록으로 조용히 시작하면 16대 페어링이 통째로 날아간다.
            // 호출부가 사용자에게 알릴 수 있도록 던진다.
            throw DeviceRegistryError.corrupted
        }
    }

    /// 같은 `endpointIdHex` 가 이미 있으면 연결 정보만 갱신하고, 사용자가 붙인
    /// 이름(`userLabel`)과 정렬 순서는 보존한다.
    @discardableResult
    public func upsert(_ device: Device) throws -> [Device] {
        var devices = try load()
        if let index = devices.firstIndex(where: { $0.endpointIdHex == device.endpointIdHex }) {
            var merged = device
            merged.userLabel = devices[index].userLabel ?? device.userLabel
            merged.sortIndex = devices[index].sortIndex
            devices[index] = merged
        } else {
            guard devices.count < Self.maxDevices else {
                throw DeviceRegistryError.deviceLimitReached
            }
            var appended = device
            appended.sortIndex = (devices.map(\.sortIndex).max() ?? -1) + 1
            devices.append(appended)
        }
        try persist(devices)
        return devices
    }

    @discardableResult
    public func remove(endpointIdHex: String) throws -> [Device] {
        var devices = try load()
        devices.removeAll { $0.endpointIdHex == endpointIdHex }
        try persist(devices)
        return devices
    }

    private func persist(_ devices: [Device]) throws {
        try store.write(try JSONEncoder().encode(devices))
    }
}

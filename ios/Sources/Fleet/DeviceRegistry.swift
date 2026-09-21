import Foundation

public enum DeviceRegistryError: Error, Equatable {
    case deviceLimitReached
    case corrupted
}

public extension DeviceRegistryError {
    /// 레지스트리 손상 안내(Ruling 18). 목록 화면과 장치 추가 화면이 같은 사고를 서로 다른
    /// 말로 설명하지 않도록 문구를 한 곳에 둔다.
    static let corruptedTitle = "저장된 장치 목록을 읽지 못했습니다"
    static let corruptedAdvice = "새로 페어링하면 기존 목록을 덮어씁니다."
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

    /// 같은 `endpointIdHex` 가 이미 있으면 연결 정보를 갱신하고 정렬 순서는 보존한다.
    @discardableResult
    public func upsert(_ device: Device) throws -> [Device] {
        var devices = try load()
        if let index = devices.firstIndex(where: { $0.endpointIdHex == device.endpointIdHex }) {
            var merged = device
            // 들어온 이름이 있으면 그게 이긴다. 재스캔은 "이름을 고치는" 유일한 경로라
            // 기존 이름이 무조건 이기면 사용자가 한 번 잘못 지은 이름을 영영 못 고친다.
            // 이름을 비워 보내면(nil) 기존 이름을 보존한다.
            merged.userLabel = device.userLabel ?? devices[index].userLabel
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

public extension Collection where Element == Device {
    /// 이 목록이 `endpointIdHex` 를 **새 장치로** 더 받을 수 없는 상태인지.
    ///
    /// 이미 등록된 장치면 갱신이라 한도와 무관하게 false 다 — 재스캔은 이름과 연결 정보를
    /// 고치는 유일한 경로다(Ruling 25/28). `upsert` 의 한도 규칙과 같은 판정을 페어링을
    /// 돌리기 **전에** 쓰려고 순수 함수로 뺐다. 화면이 QR 을 읽어 장치 신원을 안 직후
    /// 이걸로 거절하면, 10초짜리 핸드셰이크와 Mac 쪽 코드 소비를 통째로 아낀다.
    func rejectsNewDevice(endpointIdHex: String) -> Bool {
        guard !contains(where: { $0.endpointIdHex == endpointIdHex }) else { return false }
        return count >= DeviceRegistry.maxDevices
    }
}

import XCTest
@testable import Fleet

/// 테스트용 인메모리 저장소. 실제 Keychain 은 호스트 없는 유닛 테스트 번들에서
/// errSecMissingEntitlement 로 실패한다(BLETransportTests/PairingClientTests.swift:39-47
/// 의 선례) — 그래서 저장소를 프로토콜로 주입받는다.
private final class MemoryStore: DeviceRegistryStore {
    var data: Data?
    func read() throws -> Data? { data }
    func write(_ data: Data) throws { self.data = data }
}

final class DeviceRegistryTests: XCTestCase {

    private func makeDevice(_ hex: String, name: String? = nil) -> Device {
        Device(
            endpointIdHex: hex,
            token: "token-\(hex)",
            relayUrl: nil,
            addresses: [],
            macHostname: name,
            userLabel: nil,
            sortIndex: 0
        )
    }

    func testEmptyRegistryLoadsAsEmptyList() throws {
        let registry = DeviceRegistry(store: MemoryStore())
        XCTAssertEqual(try registry.load(), [])
    }

    func testUpsertRoundTrips() throws {
        let store = MemoryStore()
        let registry = DeviceRegistry(store: store)

        _ = try registry.upsert(makeDevice("aa", name: "집 맥"))

        XCTAssertEqual(try DeviceRegistry(store: store).load(), [makeDevice("aa", name: "집 맥")])
    }

    /// 같은 Mac 을 다시 스캔하면 새 항목을 만들지 않고 갱신한다 — Mac 의 IP 가
    /// 바뀌었을 때 재스캔으로 고치는 경로가 된다.
    func testRescanningSameEndpointMergesInsteadOfAdding() throws {
        let registry = DeviceRegistry(store: MemoryStore())
        _ = try registry.upsert(makeDevice("aa"))

        var updated = makeDevice("aa")
        updated.addresses = ["192.168.0.5:1234"]
        updated.token = "새-토큰"
        let devices = try registry.upsert(updated)

        XCTAssertEqual(devices.count, 1)
        XCTAssertEqual(devices[0].addresses, ["192.168.0.5:1234"])
        XCTAssertEqual(devices[0].token, "새-토큰")
    }

    /// 이름을 비워 보내면(nil) 재스캔은 연결 정보만 갱신하고 기존 이름을 보존한다.
    func testUpsertKeepsExistingLabelWhenIncomingIsNil() throws {
        let registry = DeviceRegistry(store: MemoryStore())
        var first = makeDevice("aa")
        first.userLabel = "작업실"
        _ = try registry.upsert(first)

        let devices = try registry.upsert(makeDevice("aa", name: "새-호스트명"))

        XCTAssertEqual(devices[0].userLabel, "작업실")
        XCTAssertEqual(devices[0].macHostname, "새-호스트명")
    }

    /// 재스캔은 이름을 고치는 **유일한** 경로다(앱 어디에도 이름 변경 화면이 없다).
    /// 기존 이름이 무조건 이기면 한 번 잘못 지은 이름을 영영 못 고친다.
    func testUpsertOverwritesLabelWhenIncomingIsProvided() throws {
        let registry = DeviceRegistry(store: MemoryStore())
        var first = makeDevice("aa")
        first.userLabel = "작업실"
        let insertedSortIndex = try registry.upsert(first)[0].sortIndex

        var rescanned = makeDevice("aa")
        rescanned.userLabel = "거실 맥"
        let devices = try registry.upsert(rescanned)

        XCTAssertEqual(devices[0].userLabel, "거실 맥")
        // 이름이 바뀌어도 정렬 순서는 그대로여야 한다 — 목록에서 자리가 튀면 안 된다.
        XCTAssertEqual(devices[0].sortIndex, insertedSortIndex)
    }

    func testDeviceLimitIsEnforced() throws {
        let registry = DeviceRegistry(store: MemoryStore())
        for i in 0..<DeviceRegistry.maxDevices {
            _ = try registry.upsert(makeDevice(String(format: "%02x", i)))
        }

        XCTAssertThrowsError(try registry.upsert(makeDevice("ff"))) { error in
            XCTAssertEqual(error as? DeviceRegistryError, .deviceLimitReached)
        }
    }

    /// 한도에 도달했어도 기존 장치는 **실제로 갱신까지** 돼야 한다. 재스캔은 이름과 연결
    /// 정보를 고치는 유일한 경로라(Ruling 25/28), 16대를 채운 사용자가 여기서 막히면
    /// 장치를 하나 지웠다 다시 페어링하는 막다른 길밖에 남지 않는다.
    func testUpsertAtLimitStillUpdatesExistingDevice() throws {
        let registry = DeviceRegistry(store: MemoryStore())
        for i in 0..<DeviceRegistry.maxDevices {
            _ = try registry.upsert(makeDevice(String(format: "%02x", i)))
        }

        var rescanned = makeDevice("00", name: "새-호스트명")
        rescanned.userLabel = "작업실 맥"
        let devices = try registry.upsert(rescanned)

        XCTAssertEqual(devices.count, DeviceRegistry.maxDevices, "갱신이 항목을 늘리면 안 된다")
        let updated = try XCTUnwrap(devices.first { $0.endpointIdHex == "00" })
        XCTAssertEqual(updated.userLabel, "작업실 맥")
        XCTAssertEqual(updated.macHostname, "새-호스트명")
    }

    /// 한도에 도달했어도 **기존 장치 갱신**은 허용돼야 한다.
    func testLimitDoesNotBlockUpdatingAnExistingDevice() throws {
        let registry = DeviceRegistry(store: MemoryStore())
        for i in 0..<DeviceRegistry.maxDevices {
            _ = try registry.upsert(makeDevice(String(format: "%02x", i)))
        }

        XCTAssertNoThrow(try registry.upsert(makeDevice("00", name: "갱신")))
    }

    /// 손상된 레지스트리를 빈 것으로 취급하면 16대 페어링이 조용히 날아간다.
    func testCorruptedRegistryThrowsInsteadOfSilentlyResetting() {
        let store = MemoryStore()
        store.data = Data("이건 JSON 이 아니다".utf8)

        XCTAssertThrowsError(try DeviceRegistry(store: store).load()) { error in
            XCTAssertEqual(error as? DeviceRegistryError, .corrupted)
        }
    }

    // MARK: - 사전 판정(페어링 전)

    /// 페어링을 돌리기 전에 새 장치를 거절할 수 있어야 한다. 끝까지 돌린 뒤 거절하면
    /// 사용자는 10초를 버리고 Mac 은 이미 코드를 소비해 토큰을 발급해버린다.
    func testRejectsNewDeviceWhenFull() throws {
        let devices = (0..<DeviceRegistry.maxDevices).map { makeDevice(String(format: "%02x", $0)) }

        XCTAssertTrue(devices.rejectsNewDevice(endpointIdHex: "ff"))
    }

    /// 가득 차 있어도 **이미 등록된** 장치는 통과해야 한다. 재스캔이 이름과 연결 정보를
    /// 고치는 유일한 경로라, 여기서 막으면 16대를 채운 사용자는 이름을 영영 못 고친다.
    func testAcceptsKnownDeviceWhenFull() throws {
        let devices = (0..<DeviceRegistry.maxDevices).map { makeDevice(String(format: "%02x", $0)) }

        XCTAssertFalse(devices.rejectsNewDevice(endpointIdHex: "00"))
    }

    func testAcceptsNewDeviceBelowLimit() throws {
        XCTAssertFalse([makeDevice("aa")].rejectsNewDevice(endpointIdHex: "bb"))
    }

    func testRemoveDeletesOnlyTheNamedDevice() throws {
        let registry = DeviceRegistry(store: MemoryStore())
        _ = try registry.upsert(makeDevice("aa"))
        _ = try registry.upsert(makeDevice("bb"))

        let devices = try registry.remove(endpointIdHex: "aa")

        XCTAssertEqual(devices.map(\.endpointIdHex), ["bb"])
    }

    // MARK: - 드래그 재정렬 (스펙 §6.1)

    /// 스펙 §6.1 은 그룹 안 정렬을 "sortIndex(드래그) 순" 으로 못 박는다. 드롭 시점에
    /// 화면이 보이는 순서 전체를 넘기면 그 순서대로 0..n-1 이 다시 매겨져야 한다.
    func testReorderAssignsSortIndexInGivenOrder() throws {
        let registry = DeviceRegistry(store: MemoryStore())
        for hex in ["aa", "bb", "cc"] { _ = try registry.upsert(makeDevice(hex)) }

        let devices = try registry.reorder(["cc", "aa", "bb"])

        XCTAssertEqual(devices.map(\.endpointIdHex), ["cc", "aa", "bb"])
        XCTAssertEqual(devices.map(\.sortIndex), [0, 1, 2])
        // 저장까지 됐는지는 새 인스턴스로 다시 읽어 확인한다.
        XCTAssertEqual(
            try registry.load().sorted { $0.sortIndex < $1.sortIndex }.map(\.endpointIdHex),
            ["cc", "aa", "bb"]
        )
    }

    /// 목록에 없는 hex 는 무시한다 — 화면이 넘긴 순서와 레지스트리가 어긋날 수 있고
    /// (드롭 도중 다른 경로로 장치가 지워졌다), 그때 없는 장치를 되살리면 안 된다.
    func testReorderIgnoresUnknownHexes() throws {
        let registry = DeviceRegistry(store: MemoryStore())
        for hex in ["aa", "bb"] { _ = try registry.upsert(makeDevice(hex)) }

        let devices = try registry.reorder(["bb", "ff", "aa"])

        XCTAssertEqual(devices.map(\.endpointIdHex), ["bb", "aa"])
        XCTAssertEqual(devices.map(\.sortIndex), [0, 1])
    }

    /// 순서에 안 들어 있는 등록 장치는 지우지 않고 뒤에 이어 붙인다. 화면이 일부만
    /// 넘기더라도 장치가 사라지면 안 된다 — 레지스트리는 16대 페어링 전체다.
    func testReorderKeepsUnnamedDevicesAfterTheNamedOnes() throws {
        let registry = DeviceRegistry(store: MemoryStore())
        for hex in ["aa", "bb", "cc"] { _ = try registry.upsert(makeDevice(hex)) }

        let devices = try registry.reorder(["cc"])

        XCTAssertEqual(devices.map(\.endpointIdHex), ["cc", "aa", "bb"])
        XCTAssertEqual(devices.map(\.sortIndex), [0, 1, 2])
    }

    /// 저장이 실패하면 조용히 넘어가지 않는다 — 호출부가 순서를 되돌리고 알릴 수 있어야 한다.
    func testReorderThrowsWhenPersistFails() throws {
        let store = FailingWriteStore()
        let registry = DeviceRegistry(store: store)
        store.allowWrites = true
        for hex in ["aa", "bb"] { _ = try registry.upsert(makeDevice(hex)) }
        store.allowWrites = false

        XCTAssertThrowsError(try registry.reorder(["bb", "aa"]))
    }
}

/// 쓰기를 실패시킬 수 있는 저장소. Keychain 쓰기 실패(용량·권한)를 흉내낸다.
private final class FailingWriteStore: DeviceRegistryStore {
    struct WriteFailed: Error {}
    var data: Data?
    var allowWrites = true

    func read() throws -> Data? { data }
    func write(_ data: Data) throws {
        guard allowWrites else { throw WriteFailed() }
        self.data = data
    }
}

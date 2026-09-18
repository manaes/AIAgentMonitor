import XCTest
import Wire
@testable import Fleet

final class DeviceSnapshotCacheTests: XCTestCase {

    private var directory: URL!

    override func setUpWithError() throws {
        directory = FileManager.default.temporaryDirectory
            .appendingPathComponent(UUID().uuidString, isDirectory: true)
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
    }

    override func tearDownWithError() throws {
        try? FileManager.default.removeItem(at: directory)
    }

    private func makeSnapshot() throws -> MirrorSnapshot {
        let json = #"{"v":1,"t":1758000000,"a":[{"k":0,"r":1.5,"t5":1200,"p5":40,"pj":[]}]}"#
        return try JSONDecoder().decode(MirrorSnapshot.self, from: Data(json.utf8))
    }

    func testLoadReturnsNilWhenNothingSaved() {
        let cache = DeviceSnapshotCache(directory: directory)
        XCTAssertNil(cache.load(endpointIdHex: "aa"))
    }

    func testSavedSnapshotRoundTrips() throws {
        let cache = DeviceSnapshotCache(directory: directory)
        let snapshot = try makeSnapshot()
        let fetchedAt = Date(timeIntervalSince1970: 1_758_000_100)

        cache.save(snapshot, fetchedAt: fetchedAt, endpointIdHex: "aa")

        let loaded = cache.load(endpointIdHex: "aa")
        XCTAssertEqual(loaded?.snapshot, snapshot)
        XCTAssertEqual(loaded?.fetchedAt, fetchedAt)
    }

    /// 장치별로 분리돼야 한다 — 한 Mac 의 캐시가 다른 Mac 에 보이면 안 된다.
    func testCachesAreIsolatedPerDevice() throws {
        let cache = DeviceSnapshotCache(directory: directory)
        cache.save(try makeSnapshot(), fetchedAt: Date(), endpointIdHex: "aa")

        XCTAssertNil(cache.load(endpointIdHex: "bb"))
    }

    /// 장치를 목록에서 지우면 캐시 파일도 없어져야 한다 — 남으면 고아 파일이 쌓이고,
    /// 같은 Mac 을 다시 페어링했을 때 연결되기도 전에 낡은 스냅샷이 먼저 보인다.
    func testRemoveDeletesCachedSnapshot() throws {
        let cache = DeviceSnapshotCache(directory: directory)
        cache.save(try makeSnapshot(), fetchedAt: Date(), endpointIdHex: "aa")
        XCTAssertNotNil(cache.load(endpointIdHex: "aa"))

        cache.remove(endpointIdHex: "aa")

        XCTAssertNil(cache.load(endpointIdHex: "aa"))
    }

    /// 저장된 적 없는 장치를 지우는 건 정상 경로다(캐시가 비어 있는 상태에서 삭제).
    func testRemoveIsHarmlessWhenNothingSaved() {
        let cache = DeviceSnapshotCache(directory: directory)
        cache.remove(endpointIdHex: "bb")
        XCTAssertNil(cache.load(endpointIdHex: "bb"))
    }

    /// endpointIdHex 가 파일명이 되므로 경로 조작 문자가 섞이면 안 된다.
    func testNonHexIdentifierIsRejected() throws {
        let cache = DeviceSnapshotCache(directory: directory)

        cache.save(try makeSnapshot(), fetchedAt: Date(), endpointIdHex: "../../etc/passwd")

        XCTAssertNil(cache.load(endpointIdHex: "../../etc/passwd"))
    }
}

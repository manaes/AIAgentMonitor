import XCTest
import Wire
@testable import WidgetShared

final class UsageCacheStoreTests: XCTestCase {

    override func tearDown() {
        UsageCacheStore.clear()
        super.tearDown()
    }

    private func loadGoldenSnapshot() throws -> MirrorSnapshot {
        let url = try XCTUnwrap(
            Bundle(for: Self.self).url(forResource: "snapshot-sample", withExtension: "json")
        )
        return try JSONDecoder().decode(MirrorSnapshot.self, from: Data(contentsOf: url))
    }

    func testLoadReturnsNilWhenNothingSaved() {
        XCTAssertNil(UsageCacheStore.load())
    }

    func testSavedSnapshotRoundTrips() throws {
        let snap = try loadGoldenSnapshot()
        let fetchedAt = Date(timeIntervalSince1970: 1_755_500_100)

        UsageCacheStore.save(snap, fetchedAt: fetchedAt)
        let loaded = UsageCacheStore.load()

        XCTAssertEqual(loaded?.snapshot, snap)
        XCTAssertEqual(loaded?.fetchedAt, fetchedAt)
    }

    func testClearRemovesSavedValue() throws {
        let snap = try loadGoldenSnapshot()
        UsageCacheStore.save(snap, fetchedAt: Date())
        UsageCacheStore.clear()
        XCTAssertNil(UsageCacheStore.load())
    }

    func testWidgetKindIsStableIdentifier() {
        XCTAssertEqual(widgetKind, "AIMonitorWidget")
    }
}

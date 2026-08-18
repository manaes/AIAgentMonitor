import XCTest
import CoreBluetooth
@testable import BLETransport

final class ConnectionStateTests: XCTestCase {

    func testUUIDsMatchSpec() {
        XCTAssertEqual(MirrorUUIDs.service, CBUUID(string: "07A98A35-16C7-4BBA-A296-E28B78B7E683"))
        XCTAssertEqual(MirrorUUIDs.info, CBUUID(string: "F494FC3B-ED50-4561-AADE-1A310C5732E6"))
        XCTAssertEqual(MirrorUUIDs.auth, CBUUID(string: "1403603A-4C78-4899-A2B8-FDA198101900"))
        XCTAssertEqual(MirrorUUIDs.snapshot, CBUUID(string: "0AE789AA-EF38-4A35-9E72-A7CD7AD995D5"))
        XCTAssertEqual(MirrorUUIDs.triggers, CBUUID(string: "4F60A8C2-F181-4717-AEE3-07C4D7846597"))
    }

    func testStateDescriptionsAreUserFacing() {
        XCTAssertEqual(ConnectionState.idle.label, "대기 중")
        XCTAssertEqual(ConnectionState.scanning.label, "Mac 찾는 중…")
        XCTAssertEqual(ConnectionState.streaming.label, "연결됨")
        XCTAssertEqual(
            ConnectionState.disconnected(reason: "범위 이탈").label,
            "연결 끊김 · 범위 이탈",
            "사유가 화면에 그대로 드러나야 원인이 미궁이 되지 않는다"
        )
    }

    func testBluetoothOffIsDistinctFromDisconnected() {
        XCTAssertEqual(ConnectionState.bluetoothOff.label, "블루투스가 꺼져 있습니다")
    }

    func testVersionMismatchIsDistinctFromDisconnected() {
        XCTAssertEqual(ConnectionState.versionMismatch.label, "앱 업데이트 필요 · 프로토콜 버전 불일치")
    }
}

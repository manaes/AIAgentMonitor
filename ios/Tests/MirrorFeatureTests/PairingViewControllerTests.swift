import UIKit
import XCTest
@testable import MirrorFeature

@MainActor
final class PairingViewControllerTests: XCTestCase {

    private func loaded() -> PairingViewController {
        let vc = PairingViewController()
        vc.loadViewIfNeeded()
        return vc
    }

    func testConfirmDisabledUntilSixDigits() {
        let vc = loaded()
        XCTAssertFalse(vc.confirmEnabled, "빈 입력에서는 확인이 비활성이어야 한다")

        vc.simulateCodeEntry("123")
        XCTAssertFalse(vc.confirmEnabled, "6자리 미만이면 아직 비활성")

        vc.simulateCodeEntry("123456")
        XCTAssertTrue(vc.confirmEnabled, "6자리가 채워지면 활성화돼야 한다")
    }

    func testSubmitPassesTheEnteredCode() {
        let vc = loaded()
        var submitted: String?
        vc.onSubmit = { submitted = $0 }

        vc.simulateCodeEntry("654321")
        vc.simulateConfirmTap()

        XCTAssertEqual(submitted, "654321")
    }

    func testConfirmTapWithFewerThanSixDigitsDoesNothing() {
        let vc = loaded()
        var callCount = 0
        vc.onSubmit = { _ in callCount += 1 }

        vc.simulateCodeEntry("42")
        vc.simulateConfirmTap()

        XCTAssertEqual(callCount, 0, "6자리가 안 됐으면 제출되면 안 된다")
    }

    func testAttemptsLabelHiddenWhenNilAndShownOtherwise() {
        let vc = loaded()
        XCTAssertNil(vc.attemptsText, "처음 진입했을 때는 아직 실패한 적이 없으니 문구가 없어야 한다")

        vc.setAttemptsRemaining(3)
        XCTAssertEqual(vc.attemptsText, "코드가 틀렸습니다 · 3회 남음")

        vc.setAttemptsRemaining(nil)
        XCTAssertNil(vc.attemptsText, "다시 nil 을 주면 문구도 지워져야 한다")
    }
}

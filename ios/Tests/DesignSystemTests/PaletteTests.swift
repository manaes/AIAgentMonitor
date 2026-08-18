import UIKit
import XCTest
@testable import DesignSystem

final class PaletteTests: XCTestCase {

    private func rgb(_ c: UIColor) -> (Int, Int, Int) {
        var r: CGFloat = 0, g: CGFloat = 0, b: CGFloat = 0, a: CGFloat = 0
        c.getRed(&r, green: &g, blue: &b, alpha: &a)
        return (Int((r * 255).rounded()), Int((g * 255).rounded()), Int((b * 255).rounded()))
    }

    func testHexInitProducesExactChannels() {
        XCTAssertEqual(rgb(UIColor(hex: 0x30d158)).0, 0x30)
        XCTAssertEqual(rgb(UIColor(hex: 0x30d158)).1, 0xd1)
        XCTAssertEqual(rgb(UIColor(hex: 0x30d158)).2, 0x58)
        XCTAssertTrue(rgb(UIColor(hex: 0x000000)) == (0, 0, 0))
        XCTAssertTrue(rgb(UIColor(hex: 0xffffff)) == (255, 255, 255))
    }

    /// macOS Detail 창과 같은 색이어야 한다. 값이 바뀌면 두 화면이 달라진다.
    /// (Int, Int, Int) 튜플은 Equatable 프로토콜을 준수할 수 없어 XCTAssertEqual 을
    /// 쓸 수 없다. 대신 튜플 전용 `==` 연산자로 비교한다.
    func testPaletteMatchesMacOS() {
        XCTAssertTrue(rgb(Palette.claudeDot) == rgb(UIColor(hex: 0x30d158)))
        XCTAssertTrue(rgb(Palette.codexDot) == rgb(UIColor(hex: 0xff9f0a)))
        XCTAssertTrue(rgb(Palette.cardBackground) == rgb(UIColor(hex: 0x2c2c2e)))
        XCTAssertTrue(rgb(Palette.barTrack) == rgb(UIColor(hex: 0x1c1c1e)))
        XCTAssertTrue(rgb(Palette.separator) == rgb(UIColor(hex: 0x3a3a3c)))
        XCTAssertTrue(rgb(Palette.subtle) == rgb(UIColor(hex: 0x8e8e93)))
        XCTAssertTrue(rgb(Palette.fainter) == rgb(UIColor(hex: 0x636366)))
        XCTAssertTrue(rgb(Palette.primaryText) == rgb(UIColor(hex: 0xf2f2f7)))
        XCTAssertTrue(rgb(Palette.percent) == rgb(UIColor(hex: 0x30d158)))
        XCTAssertTrue(rgb(Palette.countdown) == rgb(UIColor(hex: 0xff9f0a)))
        XCTAssertTrue(rgb(Palette.rate) == rgb(UIColor(hex: 0x0a84ff)))
        XCTAssertTrue(rgb(Palette.dormantDot) == rgb(UIColor(hex: 0x636366)))
        XCTAssertTrue(rgb(Palette.idleDot) == rgb(UIColor(hex: 0xff9f0a)))
    }

    func testDotViewRendersAsCircleForItsIntrinsicSize() {
        let dot = DotView(diameter: 8)
        dot.sizeToFit()
        XCTAssertEqual(dot.intrinsicContentSize, CGSize(width: 8, height: 8))
        XCTAssertEqual(dot.layer.cornerRadius * 2, dot.intrinsicContentSize.width, accuracy: 0.01,
                       "지름의 절반이어야 원으로 보인다")
        XCTAssertEqual(dot.intrinsicContentSize.width, dot.intrinsicContentSize.height,
                       "가로세로가 같아야 원이다")
    }

    /// AgentCard.svelte:88 의 `.name` 은 크기를 지정하지 않아 app.css:8 의
    /// body 12px 를 상속한다. 값이 다시 흔들리지 않도록 고정한다.
    func testNameFontMatchesInheritedBodySize() {
        XCTAssertEqual(Typography.name.pointSize, 12)
    }

    func testDotViewColorIsApplied() {
        let dot = DotView(diameter: 6)
        dot.color = Palette.idleDot
        XCTAssertTrue(rgb(dot.backgroundColor ?? .clear) == rgb(Palette.idleDot))
    }
}

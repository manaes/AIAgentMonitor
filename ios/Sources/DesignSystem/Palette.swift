import UIKit

public extension UIColor {
    /// 0xRRGGBB 정수로 색을 만든다. macOS 쪽 CSS 값을 그대로 옮기기 위한 것.
    convenience init(hex: UInt32) {
        self.init(
            red: CGFloat((hex >> 16) & 0xff) / 255,
            green: CGFloat((hex >> 8) & 0xff) / 255,
            blue: CGFloat(hex & 0xff) / 255,
            alpha: 1
        )
    }
}

/// macOS Detail 창의 색을 그대로 옮긴 토큰. 두 화면을 나란히 놓았을 때
/// 같아 보이는 것이 목적이므로 값을 임의로 바꾸지 않는다.
public enum Palette {
    public static let claudeDot = UIColor(hex: 0x30d158)
    public static let codexDot = UIColor(hex: 0xff9f0a)
    public static let antigravityDot = UIColor(hex: 0x388bfd)
    public static let idleDot = UIColor(hex: 0xff9f0a)
    public static let dormantDot = UIColor(hex: 0x636366)

    public static let cardBackground = UIColor(hex: 0x2c2c2e)
    public static let barTrack = UIColor(hex: 0x1c1c1e)
    /// app.css:17 `.window-root { background: #1c1c1e }`. barTrack 과 같은 값이지만
    /// 역할이 다르므로(창 배경 vs 사용량 바 트랙) 따로 이름 붙인다 — 나중에 둘 중
    /// 하나만 바뀌어도 서로 끌려가지 않는다.
    public static let windowBackground = UIColor(hex: 0x1c1c1e)
    public static let separator = UIColor(hex: 0x3a3a3c)

    public static let primaryText = UIColor(hex: 0xf2f2f7)
    public static let subtle = UIColor(hex: 0x8e8e93)
    public static let fainter = UIColor(hex: 0x636366)

    public static let percent = UIColor(hex: 0x30d158)
    public static let countdown = UIColor(hex: 0xff9f0a)
    public static let rate = UIColor(hex: 0x0a84ff)
}

import UIKit

/// macOS 쪽 폰트 크기를 그대로 옮긴다. 숫자는 자릿수가 흔들리지 않도록
/// tabular figures 를 쓴다(원본의 font-variant-numeric: tabular-nums).
public enum Typography {
    public static let bigRate = monospacedDigit(ofSize: 22, weight: .bold)
    public static let percent = monospacedDigit(ofSize: 13, weight: .bold)
    public static let body = UIFont.systemFont(ofSize: 11)
    public static let rate = monospacedDigit(ofSize: 11, weight: .semibold)
    public static let countdown = monospacedDigit(ofSize: 11, weight: .semibold)
    public static let label = UIFont.systemFont(ofSize: 10)
    public static let sectionLabel = UIFont.systemFont(ofSize: 9)
    /// AgentCard.svelte:88 의 `.name` 은 font-weight: 600 만 지정하고 크기는
    /// app.css:8 의 body 12px 를 상속한다.
    public static let name = UIFont.systemFont(ofSize: 12, weight: .semibold)

    private static func monospacedDigit(ofSize size: CGFloat, weight: UIFont.Weight) -> UIFont {
        UIFont.monospacedDigitSystemFont(ofSize: size, weight: weight)
    }
}

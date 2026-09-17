import UIKit

/// 상태 표시용 원형 점. 지름이 고정이라 레이아웃 후 코너를 반지름으로 맞춘다.
public final class DotView: UIView {
    private let diameter: CGFloat
    private var currentPulseDuration: TimeInterval?

    public var color: UIColor = .clear {
        didSet { backgroundColor = color }
    }

    public init(diameter: CGFloat) {
        self.diameter = diameter
        super.init(frame: .zero)
        layer.cornerRadius = diameter / 2
        layer.masksToBounds = true
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) { fatalError("init(coder:) 는 쓰지 않는다") }

    public override var intrinsicContentSize: CGSize {
        CGSize(width: diameter, height: diameter)
    }

    /// `AgentCard.svelte`의 `.dot`(scale 1→1.7→1, opacity 1→0.5→1) pulse를 그대로 이식.
    /// 같은 duration이 반복 전달되면(매 `configure()`마다) 애니메이션을 다시 얹지
    /// 않는다 — CAAnimation을 매번 새로 시작하면 값이 안 바뀌어도 박자가 끊겨 보인다.
    public func setPulseDuration(_ duration: TimeInterval) {
        guard currentPulseDuration != duration else { return }
        currentPulseDuration = duration

        let scale = CAKeyframeAnimation(keyPath: "transform.scale")
        scale.values = [1.0, 1.7, 1.0]
        scale.keyTimes = [0, 0.5, 1]

        let opacity = CAKeyframeAnimation(keyPath: "opacity")
        opacity.values = [1.0, 0.5, 1.0]
        opacity.keyTimes = [0, 0.5, 1]

        let group = CAAnimationGroup()
        group.animations = [scale, opacity]
        group.duration = duration
        group.repeatCount = .infinity
        group.timingFunction = CAMediaTimingFunction(name: .easeInEaseOut)

        layer.add(group, forKey: "pulse")
    }
}

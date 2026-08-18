import UIKit

/// 상태 표시용 원형 점. 지름이 고정이라 레이아웃 후 코너를 반지름으로 맞춘다.
public final class DotView: UIView {
    private let diameter: CGFloat

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
}

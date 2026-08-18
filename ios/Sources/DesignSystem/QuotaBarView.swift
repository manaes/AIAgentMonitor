import MirrorFormat
import SnapKit
import UIKit

/// `QuotaBar.svelte` 이식. 세 가지 표시 상태를 가진다.
/// 1) 동기화 후: 5h 바 (+ 주간 값이 있으면 주간 바)
/// 2) 리셋 직후: 5h 를 0% 로
/// 3) 동기화 전: 바 대신 "5h 토큰: N · 동기화 전"
public final class QuotaBarView: UIView {

    private let fiveRow = PercentRow(title: "5h")
    private let weeklyRow = PercentRow(title: "주간")
    private let fallbackLabel = UILabel()
    private let stack = UIStackView()

    /// 테스트에서 표시 결과를 확인하기 위한 읽기 전용 창구.
    public var fivePercentText: String? { fiveRow.isHidden ? nil : fiveRow.percentText }
    public var weeklyPercentText: String? { weeklyRow.isHidden ? nil : weeklyRow.percentText }
    public var fallbackText: String? { fallbackLabel.isHidden ? nil : fallbackLabel.text }
    /// 5h 채움 막대의 실제 폭 비율(레이아웃 이후). 트랙 대비 채움 폭을 검증하기 위한 창구.
    public var fiveFillRatio: CGFloat? { fiveRow.isHidden ? nil : fiveRow.fillRatio }

    public init() {
        super.init(frame: .zero)
        stack.axis = .vertical
        stack.spacing = 6
        addSubview(stack)
        stack.snp.makeConstraints { $0.edges.equalToSuperview() }

        fallbackLabel.font = Typography.label
        fallbackLabel.textColor = Palette.subtle

        [fiveRow, weeklyRow, fallbackLabel].forEach(stack.addArrangedSubview)

        // configure() 가 처음 호출되기 전까지는 원본에 없는 "4번째 상태"(셋 다 동시에
        // 보임)가 되지 않도록 전부 숨겨서 시작한다. tokens5h 값이 아직 없어
        // 폴백 문구("N · 동기화 전")를 의미 있게 채울 수도 없으므로, 폴백을 먼저
        // 보여주는 대신 숨김 상태를 기본값으로 택했다.
        fiveRow.isHidden = true
        weeklyRow.isHidden = true
        fallbackLabel.isHidden = true
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) { fatalError("init(coder:) 는 쓰지 않는다") }

    public func configure(tokens5h: UInt32, autoPct: Float?, weeklyPct: Float?, isReset5h: Bool) {
        let pct = QuotaDisplay.displayPercent(autoPct: autoPct, isReset: isReset5h)

        if let pct {
            fiveRow.isHidden = false
            fallbackLabel.isHidden = true
            fiveRow.apply(percent: pct)

            if let weeklyPct {
                weeklyRow.isHidden = false
                weeklyRow.apply(percent: min(100, weeklyPct))
            } else {
                weeklyRow.isHidden = true
            }
        } else {
            fiveRow.isHidden = true
            weeklyRow.isHidden = true
            fallbackLabel.isHidden = false
            // 원본은 tokens_in + tokens_out 을 합쳐 보여주는데, 전송 DTO 의 t5 가 이미 그 합이다.
            fallbackLabel.text = "5h 토큰: \(MirrorFormat.tokensTotal(tokens5h)) · 동기화 전"
        }
    }
}

/// 라벨 + 퍼센트 + 진행 바 한 세트.
private final class PercentRow: UIView {
    private let titleLabel = UILabel()
    private let percentLabel = UILabel()
    private let track = UIView()
    private let fill = UIView()
    private let gradient = CAGradientLayer()
    private var ratio: CGFloat = 0

    var percentText: String? { percentLabel.text }
    /// 트랙 대비 채움 막대의 실제 폭 비율(레이아웃 후에만 유의미).
    var fillRatio: CGFloat? {
        let trackWidth = track.bounds.width
        guard trackWidth > 0 else { return nil }
        return fill.bounds.width / trackWidth
    }

    init(title: String) {
        super.init(frame: .zero)
        titleLabel.text = title
        titleLabel.font = Typography.label
        titleLabel.textColor = Palette.subtle

        percentLabel.font = Typography.percent
        percentLabel.textColor = Palette.percent
        percentLabel.textAlignment = .right

        track.backgroundColor = Palette.barTrack
        track.layer.cornerRadius = 3
        track.layer.masksToBounds = true

        gradient.startPoint = CGPoint(x: 0, y: 0.5)
        gradient.endPoint = CGPoint(x: 1, y: 0.5)
        fill.layer.addSublayer(gradient)
        fill.layer.cornerRadius = 3
        fill.layer.masksToBounds = true

        [titleLabel, percentLabel, track].forEach(addSubview)
        track.addSubview(fill)

        titleLabel.snp.makeConstraints { make in
            make.leading.top.equalToSuperview()
        }
        percentLabel.snp.makeConstraints { make in
            make.trailing.equalToSuperview()
            make.firstBaseline.equalTo(titleLabel.snp.firstBaseline)
        }
        track.snp.makeConstraints { make in
            make.top.equalTo(percentLabel.snp.bottom).offset(3)
            make.leading.trailing.bottom.equalToSuperview()
            make.height.equalTo(6)
        }
        fill.snp.makeConstraints { make in
            make.leading.top.bottom.equalToSuperview()
            make.width.equalToSuperview().multipliedBy(0).priority(.high)
        }
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) { fatalError("init(coder:) 는 쓰지 않는다") }

    func apply(percent: Float) {
        // %.0f 는 C 의 짝수 반올림이라 정확히 .5 인 값에서 JS 의 toFixed(0)(항상 0에서
        // 먼 쪽으로 반올림)와 어긋난다. MirrorFormat.toFixed 가 그 규칙을 이미 golden
        // table 로 맞춰뒀으므로 자체 포맷 대신 그대로 재사용한다.
        percentLabel.text = MirrorFormat.toFixed(Double(percent), 0) + "%"
        ratio = CGFloat(max(0, min(100, percent)) / 100)
        let g = QuotaDisplay.gradient(forPercent: percent)
        gradient.colors = [UIColor(hex: g.startHex).cgColor, UIColor(hex: g.endHex).cgColor]
        fill.snp.remakeConstraints { make in
            make.leading.top.bottom.equalToSuperview()
            make.width.equalToSuperview().multipliedBy(max(ratio, 0.0001)).priority(.high)
        }
        setNeedsLayout()
    }

    override func layoutSubviews() {
        super.layoutSubviews()
        gradient.frame = fill.bounds
    }
}

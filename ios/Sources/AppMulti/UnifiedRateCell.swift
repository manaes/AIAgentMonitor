import DesignSystem
import Fleet
import SnapKit
import UIKit

/// 통합 보기 하단 한 줄 = Mac 한 대. tok/s 만 보여준다 — 한도는 상단에서 이미 합쳐
/// 보여줬으므로 여기서 또 그리면 같은 값을 장치 수만큼 반복하게 된다.
///
/// ```
/// [이름] ................ [연결됨]
/// ┌──────────┐ ┌──────────┐
/// │Claude Code│ │Codex     │
/// │    34.7k │ │        0 │
/// │    tok/s │ │    tok/s │
/// └──────────┘ └──────────┘
/// 카드가 넘치면 가로로 스크롤한다.
/// ```
final class UnifiedRateCell: UICollectionViewListCell {
    private let titleLabel = UILabel()
    private let statusLabel = UILabel()
    private let rateScroll = UIScrollView()
    private let rateStack = UIStackView()

    override init(frame: CGRect) {
        super.init(frame: frame)
        titleLabel.font = Typography.name
        titleLabel.textColor = Palette.primaryText
        statusLabel.font = Typography.label
        statusLabel.textColor = Palette.subtle

        // 장치 이름이 길어도 상태 라벨을 밀어내지 않게 한다.
        titleLabel.setContentCompressionResistancePriority(.defaultLow, for: .horizontal)
        titleLabel.lineBreakMode = .byTruncatingTail

        // 에이전트마다 작은 카드를 만들어 **가로로 쭉 나열**한다. 칸을 균등 분할하지 않고
        // 카드 폭을 고정한 뒤 넘치면 가로로 스크롤한다 — 균등 분할이면 에이전트가 늘수록
        // 칸이 좁아져 주인공인 숫자를 키울 수 없다.
        rateStack.axis = .horizontal
        rateStack.alignment = .top
        rateStack.spacing = 10
        rateScroll.showsHorizontalScrollIndicator = false
        rateScroll.addSubview(rateStack)
        rateStack.snp.makeConstraints { $0.edges.equalToSuperview() }
        // 스크롤 뷰 높이는 카드 높이를 그대로 따라간다(세로 스크롤은 없다).
        rateScroll.snp.makeConstraints { $0.height.equalTo(rateStack.snp.height) }

        let header = UIStackView(arrangedSubviews: [titleLabel, UIView(), statusLabel])
        header.axis = .horizontal
        header.spacing = 6
        header.alignment = .firstBaseline

        let root = UIStackView(arrangedSubviews: [header, rateScroll])
        root.axis = .vertical
        root.spacing = 10

        automaticallyUpdatesBackgroundConfiguration = false
        configurationUpdateHandler = { cell, state in
            var background = UIBackgroundConfiguration.listGroupedCell()
            background.backgroundColor = state.isHighlighted
                ? Palette.separator   // 눌린 동안만 한 단계 밝게
                : Palette.cardBackground
            cell.backgroundConfiguration = background
        }

        contentView.addSubview(root)
        root.snp.makeConstraints { make in
            make.edges.equalToSuperview().inset(UIEdgeInsets(top: 12, left: 16, bottom: 12, right: 16))
        }
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) { fatalError("init(coder:) 는 쓰지 않는다") }

    func configure(_ model: UnifiedDeviceRow) {
        titleLabel.text = model.title
        statusLabel.text = model.statusText

        // arrangedSubviews 에서 빼는 것만으로는 뷰가 남으므로 superview 에서 떼어낸다.
        rateStack.arrangedSubviews.forEach { $0.removeFromSuperview() }
        for rate in model.rates {
            rateStack.addArrangedSubview(makeRateCard(rate))
        }
        // 오프라인이면 rates 가 비어 있다 — 빈 스택을 남기면 루트 스택의 간격 8pt 만 붙는다.
        rateScroll.isHidden = model.rates.isEmpty
    }

    /// 카드 하나 = 에이전트 하나. 이름 위, 숫자 가운데, 단위 아래로 쌓아 **정사각형**으로 둔다.
    /// 단위를 숫자 옆이 아니라 아래로 내리면 필요한 폭이 줄어 정사각형이 나온다.
    ///
    /// 폭은 숫자 크기를 따라간다 — "34.7k" 를 51pt 로 그리면 글자만 135pt 라, 카드가 작으면
    /// 자동 축소가 걸려 키운 효과가 그대로 사라진다.
    private static let cardSide: CGFloat = 160

    private func makeRateCard(_ rate: UnifiedRate) -> UIView {
        let card = UIView()
        // 장치 카드(cardBackground) 위에 한 단계 어두운 카드를 얹어 경계를 낸다 —
        // 한도 막대의 트랙과 같은 색이라 화면 전체의 "값이 놓이는 자리" 톤이 일관된다.
        card.backgroundColor = Palette.barTrack
        card.layer.cornerRadius = 14
        card.layer.masksToBounds = true

        let name = UILabel()
        name.font = Typography.medium
        name.textColor = Palette.subtle
        name.text = rate.name
        name.lineBreakMode = .byTruncatingTail

        let value = UILabel()
        value.font = Typography.hugeRate
        value.textColor = Palette.rate
        value.text = rate.rateValueText
        // "1.2M" 처럼 길어져도 자르지 않고 줄여서 보여준다.
        value.textAlignment = .right
        value.adjustsFontSizeToFitWidth = true
        value.minimumScaleFactor = 0.5

        let unit = UILabel()
        unit.font = Typography.hugeRateUnit
        unit.textColor = Palette.subtle
        unit.text = "tok/s"
        unit.textAlignment = .right

        // 값은 오른쪽에 붙인다. 스택 정렬(.trailing)이 아니라 라벨을 카드 폭만큼 채우고
        // textAlignment 로 미는 방식이어야 한다 — .trailing 은 라벨 폭이 글자 폭이 되어
        // adjustsFontSizeToFitWidth 가 걸리지 않고 그냥 넘친다.
        let valueStack = UIStackView(arrangedSubviews: [value, unit])
        valueStack.axis = .vertical
        valueStack.alignment = .fill
        valueStack.spacing = 0

        card.addSubview(name)
        card.addSubview(valueStack)
        // 가운데 정렬은 위아래로 빈 띠를 만든다 — 이름은 위, 값은 아래에 고정한다.
        name.snp.makeConstraints { make in
            make.top.equalToSuperview().offset(12)
            make.leading.trailing.equalToSuperview().inset(12)
        }
        valueStack.snp.makeConstraints { make in
            make.leading.trailing.equalToSuperview().inset(12)
            make.bottom.equalToSuperview().offset(-12)
            make.top.greaterThanOrEqualTo(name.snp.bottom).offset(6)
        }
        card.snp.makeConstraints { make in
            make.width.equalTo(Self.cardSide)
            make.height.equalTo(Self.cardSide)
        }
        return card
    }
}

import DesignSystem
import Fleet
import SnapKit
import UIKit

/// 통합 보기 하단 한 줄 = Mac 한 대. tok/s 만 보여준다 — 한도는 상단에서 이미 합쳐
/// 보여줬으므로 여기서 또 그리면 같은 값을 장치 수만큼 반복하게 된다.
///
/// ```
/// [이름] ................ [연결됨]
/// Claude Code      Codex
/// 34.7k tok/s      0 tok/s
/// ```
final class UnifiedRateCell: UICollectionViewListCell {
    private let titleLabel = UILabel()
    private let statusLabel = UILabel()
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

        // 에이전트를 세로로 쌓지 않고 **가로로 나란히** 둔다. 속도만 보여주는 카드라
        // 한 줄에 다 들어오고, 그래야 tok/s 숫자에 높이를 크게 줄 수 있다.
        rateStack.axis = .horizontal
        rateStack.distribution = .fillEqually
        rateStack.alignment = .top
        rateStack.spacing = 12

        let header = UIStackView(arrangedSubviews: [titleLabel, UIView(), statusLabel])
        header.axis = .horizontal
        header.spacing = 6
        header.alignment = .firstBaseline

        let root = UIStackView(arrangedSubviews: [header, rateStack])
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
            rateStack.addArrangedSubview(makeRateColumn(rate))
        }
        // 오프라인이면 rates 가 비어 있다 — 빈 스택을 남기면 루트 스택의 간격 8pt 만 붙는다.
        rateStack.isHidden = model.rates.isEmpty
    }

    /// 한 칸 = 에이전트 하나. 이름은 작게 위에, tok/s 는 크게 아래에.
    private func makeRateColumn(_ rate: UnifiedRate) -> UIView {
        let name = UILabel()
        name.font = Typography.label
        name.textColor = Palette.subtle
        name.text = rate.name
        name.lineBreakMode = .byTruncatingTail

        let value = UILabel()
        value.font = Typography.bigRate
        value.textColor = Palette.rate
        value.text = rate.rateValueText
        // 에이전트가 셋 이상이면 칸이 좁아진다 — 자르기보다 줄여서 보여준다.
        value.adjustsFontSizeToFitWidth = true
        value.minimumScaleFactor = 0.6

        let unit = UILabel()
        unit.font = Typography.label
        unit.textColor = Palette.subtle
        unit.text = "tok/s"

        // 단위를 숫자의 baseline 에 맞춰 붙인다(1:1 앱 카드와 같은 모양).
        let valueRow = UIStackView(arrangedSubviews: [value, unit, UIView()])
        valueRow.axis = .horizontal
        valueRow.spacing = 4
        valueRow.alignment = .firstBaseline

        let column = UIStackView(arrangedSubviews: [name, valueRow])
        column.axis = .vertical
        column.spacing = 2
        return column
    }
}

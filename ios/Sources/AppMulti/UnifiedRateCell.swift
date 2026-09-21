import DesignSystem
import Fleet
import SnapKit
import UIKit

/// 통합 보기 하단 한 줄 = Mac 한 대. tok/s 만 보여준다 — 한도는 상단에서 이미 합쳐
/// 보여줬으므로 여기서 또 그리면 같은 값을 장치 수만큼 반복하게 된다.
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

        rateStack.axis = .vertical
        rateStack.spacing = 6

        let header = UIStackView(arrangedSubviews: [titleLabel, UIView(), statusLabel])
        header.axis = .horizontal
        header.spacing = 6
        header.alignment = .firstBaseline

        let root = UIStackView(arrangedSubviews: [header, rateStack])
        root.axis = .vertical
        root.spacing = 8

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
            rateStack.addArrangedSubview(makeRateRow(rate))
        }
        // 오프라인이면 rates 가 비어 있다 — 빈 스택을 남기면 루트 스택의 간격 8pt 만 붙는다.
        rateStack.isHidden = model.rates.isEmpty
    }

    private func makeRateRow(_ rate: UnifiedRate) -> UIView {
        let name = UILabel()
        name.font = Typography.strong
        name.textColor = Palette.primaryText
        name.text = rate.name

        let value = UILabel()
        value.font = Typography.rate
        value.textColor = Palette.rate
        value.text = rate.rateText
        value.textAlignment = .right

        let row = UIStackView(arrangedSubviews: [name, value])
        row.axis = .horizontal
        row.spacing = 8
        return row
    }
}

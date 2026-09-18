import DesignSystem
import Fleet
import SnapKit
import UIKit

/// Mac 한 대. 에이전트는 최대 2개까지 보여주고 나머지는 "+N" 으로 접는다 —
/// 16대까지 쓰므로 가변 높이면 한 화면에 3~4대밖에 못 넣는다.
final class DeviceCell: UICollectionViewListCell {
    static let reuseIdentifier = "DeviceCell"

    private let titleLabel = UILabel()
    private let statusLabel = UILabel()
    private let freshnessLabel = UILabel()
    private let agentStack = UIStackView()
    private let moreLabel = UILabel()

    override init(frame: CGRect) {
        super.init(frame: frame)
        titleLabel.font = Typography.name
        titleLabel.textColor = Palette.primaryText
        statusLabel.font = Typography.label
        statusLabel.textColor = Palette.subtle
        freshnessLabel.font = Typography.label
        freshnessLabel.textColor = Palette.fainter
        moreLabel.font = Typography.label
        moreLabel.textColor = Palette.subtle

        // 장치 이름이 길어도 상태/신선도 라벨을 밀어내지 않게 한다.
        titleLabel.setContentCompressionResistancePriority(.defaultLow, for: .horizontal)
        titleLabel.lineBreakMode = .byTruncatingTail

        agentStack.axis = .vertical
        agentStack.spacing = 6

        let header = UIStackView(arrangedSubviews: [titleLabel, statusLabel, UIView(), freshnessLabel])
        header.axis = .horizontal
        header.spacing = 6
        header.alignment = .firstBaseline

        let root = UIStackView(arrangedSubviews: [header, agentStack, moreLabel])
        root.axis = .vertical
        root.spacing = 8

        contentView.addSubview(root)
        root.snp.makeConstraints { make in
            make.edges.equalToSuperview().inset(UIEdgeInsets(top: 12, left: 16, bottom: 12, right: 16))
        }
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) { fatalError("init(coder:) 는 쓰지 않는다") }

    func configure(_ model: DeviceRowModel) {
        titleLabel.text = model.title
        statusLabel.text = model.statusText
        freshnessLabel.text = model.freshnessText
        freshnessLabel.isHidden = model.freshnessText == nil

        // arrangedSubviews 에서 빼는 것만으로는 뷰가 남으므로 superview 에서 떼어낸다.
        agentStack.arrangedSubviews.forEach { $0.removeFromSuperview() }
        for agent in model.agents {
            agentStack.addArrangedSubview(makeAgentRow(agent))
        }

        moreLabel.text = model.hiddenAgentCount > 0 ? "+\(model.hiddenAgentCount)" : nil
        moreLabel.isHidden = model.hiddenAgentCount == 0
    }

    private func makeAgentRow(_ agent: AgentRowModel) -> UIView {
        let name = UILabel()
        name.font = Typography.strong
        name.textColor = Palette.primaryText
        name.text = agent.name

        let rate = UILabel()
        rate.font = Typography.rate
        rate.textColor = Palette.rate
        rate.text = agent.rateText          // 오프라인이면 nil → 빈 칸
        rate.textAlignment = .right

        let quota = QuotaBarView()
        quota.configure(
            tokens5h: 0,
            autoPct: agent.fiveHour?.percent,
            weeklyPct: agent.weekly?.percent,
            isReset5h: false,
            unreadable: agent.fiveHour == nil && agent.weekly == nil
        )

        let header = UIStackView(arrangedSubviews: [name, rate])
        header.axis = .horizontal
        header.spacing = 8

        let row = UIStackView(arrangedSubviews: [header, quota])
        row.axis = .vertical
        row.spacing = 4
        return row
    }
}

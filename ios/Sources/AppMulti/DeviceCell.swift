import DesignSystem
import Fleet
import SnapKit
import UIKit

/// Mac 한 대. 에이전트는 최대 2개까지 보여주고 나머지는 "+N" 으로 접는다 —
/// 16대까지 쓰므로 가변 높이면 한 화면에 3~4대밖에 못 넣는다.
final class DeviceCell: UICollectionViewListCell {
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
        // 에이전트 블록(이름+tok/s 줄 + 한도 막대 두 줄)끼리 붙어 있으면 어디까지가 한
        // 에이전트인지 구분이 안 된다는 지적이 있었다(실기). 막대 사이 간격(6)보다
        // 확실히 넓혀 블록 경계를 만든다.
        // 에이전트 블록 하나가 3줄(이름/5h/주간)이라, 블록 안 간격(3~6pt)과 확실히 차이가
        // 나야 어디서 끊기는지 읽힌다. 14 로는 부족하다는 실기 피드백.
        agentStack.spacing = 22

        let header = UIStackView(arrangedSubviews: [titleLabel, statusLabel, UIView(), freshnessLabel])
        header.axis = .horizontal
        header.spacing = 6
        header.alignment = .firstBaseline

        let root = UIStackView(arrangedSubviews: [header, agentStack, moreLabel])
        root.axis = .vertical
        root.spacing = 8

        // 카드 배경을 직접 준다. 기본값(listGroupedCell)의 다크 모드 색은 `Palette.barTrack`
        // 과 같은 #1c1c1e 라, 한도 막대의 트랙이 배경에 완전히 묻혀 그래프가 없는 것처럼
        // 보였다(실기에서 관찰). 1:1 앱의 카드와 같은 색을 쓰면 트랙이 한 단계 어두운
        // 홈처럼 드러난다.
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
        // 빈 스택을 남겨두면 에이전트가 없는 행에도 루트 스택의 간격 8pt 가 붙는다.
        agentStack.isHidden = model.agents.isEmpty

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

        // 원값을 그대로 넘긴다 — 리셋 직후 0% 처리와 조회 실패 시 % 숨김은 QuotaDisplay /
        // QuotaBarView 가 이미 갖고 있는 규칙이다. 1:1 앱 AgentCardView 와 같은 호출이다.
        let quota = QuotaBarView()
        quota.configure(
            tokens5h: agent.tokens5h,
            autoPct: agent.usedPct5h,
            weeklyPct: agent.usedPctWeekly,
            isReset5h: agent.isReset5h,
            unreadable: agent.unreadable
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

import DesignSystem
import Fleet
import SnapKit
import UIKit

/// 통합 보기 상단 한 줄 = 에이전트 종류 하나. 여러 Mac 이 같은 계정으로 보고한 한도를
/// 하나로 합쳐 보여준다(합산 규칙은 `UnifiedPresentation` — Ruling 40).
/// tok/s 는 여기 없다 — 그건 장치별 값이라 하단 섹션의 몫이다.
final class UnifiedQuotaCell: UICollectionViewListCell {
    private let nameLabel = UILabel()
    private let countLabel = UILabel()
    private let quota = QuotaBarView()

    override init(frame: CGRect) {
        super.init(frame: frame)
        nameLabel.font = Typography.name
        nameLabel.textColor = Palette.primaryText
        countLabel.font = Typography.label
        countLabel.textColor = Palette.subtle
        countLabel.textAlignment = .right

        let header = UIStackView(arrangedSubviews: [nameLabel, countLabel])
        header.axis = .horizontal
        header.spacing = 8

        let root = UIStackView(arrangedSubviews: [header, quota])
        root.axis = .vertical
        root.spacing = 6

        // DeviceCell 과 같은 이유로 배경을 직접 준다 — 기본값의 다크 모드 색이
        // `Palette.barTrack` 과 같아 한도 막대의 트랙이 배경에 묻힌다.
        automaticallyUpdatesBackgroundConfiguration = false
        configurationUpdateHandler = { cell, _ in
            var background = UIBackgroundConfiguration.listGroupedCell()
            background.backgroundColor = Palette.cardBackground
            cell.backgroundConfiguration = background
        }

        contentView.addSubview(root)
        root.snp.makeConstraints { make in
            make.edges.equalToSuperview().inset(UIEdgeInsets(top: 12, left: 16, bottom: 12, right: 16))
        }
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) { fatalError("init(coder:) 는 쓰지 않는다") }

    func configure(_ model: UnifiedAgentRow) {
        nameLabel.text = model.name
        countLabel.text = "\(model.deviceCount)대"
        // 원값을 그대로 넘긴다 — 리셋 직후 0% 처리와 조회 실패 시 % 숨김은
        // QuotaDisplay / QuotaBarView 가 이미 갖고 있는 규칙이다(DeviceCell 과 같은 호출).
        quota.configure(
            tokens5h: model.tokens5h,
            autoPct: model.usedPct5h,
            weeklyPct: model.usedPctWeekly,
            isReset5h: model.isReset5h,
            unreadable: model.unreadable
        )
    }
}

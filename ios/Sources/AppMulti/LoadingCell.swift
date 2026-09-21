import DesignSystem
import SnapKit
import UIKit

/// 첫 확인이 끝나기 전에 보여주는 카드 한 장.
///
/// 확인 중에는 보여줄 값이 하나도 없다 — 캐시도 없고 스냅샷도 아직이다. 그 상태로 평소
/// 레이아웃을 그리면 통합 보기에서 "한도 (통합)" 헤더만 남고 그 아래가 비며, 장치 카드도
/// 이름과 상태만 있는 얇은 띠가 된다. 빈 자리를 늘어놓는 대신 카드 하나로 대신한다.
final class LoadingCell: UICollectionViewListCell {
    /// 배경·모서리를 직접 그린다 — 셀의 backgroundConfiguration 을 쓰면 insetGrouped 가
    /// 섹션 안의 위치에 따라 모서리를 깎는다(`UnifiedRateCell` 과 같은 이유).
    private let container = UIView()
    private let spinner = UIActivityIndicatorView(style: .large)
    private let label = UILabel()

    override init(frame: CGRect) {
        super.init(frame: frame)
        container.backgroundColor = Palette.cardBackground
        container.layer.cornerRadius = 14
        container.layer.masksToBounds = true

        spinner.color = Palette.subtle
        // 멈춰 있으면 숨겨져 스택에서 높이가 0이 된다 — 재사용으로 멈춘 채 돌아와도
        // 자리가 사라지지 않게 한다.
        spinner.hidesWhenStopped = false
        spinner.startAnimating()

        label.font = Typography.label
        label.textColor = Palette.subtle
        label.text = "장치를 확인하는 중…"
        label.textAlignment = .center

        automaticallyUpdatesBackgroundConfiguration = false
        configurationUpdateHandler = { cell, _ in
            cell.backgroundConfiguration = UIBackgroundConfiguration.clear()
            guard let cell = cell as? LoadingCell else { return }
            cell.spinner.startAnimating()
        }

        let content = UIStackView(arrangedSubviews: [spinner, label])
        content.axis = .vertical
        content.alignment = .center
        content.spacing = 10

        contentView.addSubview(container)
        container.snp.makeConstraints { make in
            make.edges.equalToSuperview().inset(UIEdgeInsets(top: 5, left: 0, bottom: 5, right: 0))
            // 높이를 명시하지 않으면 셀이 내용 높이(라벨 한 줄)까지만 줄어든다.
            make.height.equalTo(148)
        }
        container.addSubview(content)
        content.snp.makeConstraints { make in
            make.center.equalToSuperview()
            make.leading.trailing.equalToSuperview().inset(16)
        }
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) { fatalError("init(coder:) 는 쓰지 않는다") }
}

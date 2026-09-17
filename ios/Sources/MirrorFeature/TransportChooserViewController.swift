import DesignSystem
import SnapKit
import UIKit

/// 최초 실행(또는 설정의 "연결 재설정" 직후) 전용 — 자동으로 아무 전송이나 골라
/// 시작하지 않고 사용자가 먼저 고르게 한다.
///
/// 원래는 `UIAlertController(.actionSheet)`로 띄웠는데, 화면 중앙에 앵커를 둔
/// 팝오버라 하단 삼각형 포인터가 있는 말풍선처럼 보여 설정 메뉴 팝업과 구분이
/// 안 됐다(2026-09-17 사용자 확인). 이 선택은 "메뉴에서 뭘 고르는" 가벼운 동작이
/// 아니라 앱을 처음 쓰는 흐름의 일부라, 전체화면 안내로 분리했다 — 각 방식을
/// 설명할 자리도 그래야 생긴다(네트워크는 카메라 권한이 필요하다는 것도 미리
/// 알려줄 수 있다).
public final class TransportChooserViewController: UIViewController {
    public var onChoose: ((MirrorViewController.TransportKind) -> Void)?

    private let titleLabel = UILabel()
    private let subtitleLabel = UILabel()
    private let stack = UIStackView()

    public init() {
        super.init(nibName: nil, bundle: nil)
        modalPresentationStyle = .fullScreen
        isModalInPresentation = true
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) { fatalError("init(coder:) 는 쓰지 않는다") }

    public override func viewDidLoad() {
        super.viewDidLoad()
        view.backgroundColor = Palette.windowBackground

        titleLabel.text = "연결 방식을 선택하세요"
        titleLabel.font = .systemFont(ofSize: 22, weight: .bold)
        titleLabel.textColor = Palette.primaryText
        titleLabel.textAlignment = .center
        titleLabel.numberOfLines = 0

        subtitleLabel.text = "Mac의 AI Agent Monitor와 연결할 방식입니다.\n설정에서 언제든 바꿀 수 있어요."
        subtitleLabel.font = .systemFont(ofSize: 13)
        subtitleLabel.textColor = Palette.subtle
        subtitleLabel.textAlignment = .center
        subtitleLabel.numberOfLines = 0

        stack.axis = .vertical
        stack.spacing = 16

        let bleCard = makeOptionCard(
            emoji: "🔵",
            title: "BLE",
            subtitle: "Mac 근처에서 블루투스로 바로 연결합니다. 블루투스 권한이 필요해요."
        ) { [weak self] in self?.onChoose?(.ble) }
        stack.addArrangedSubview(bleCard)

        #if NETWORK_TRANSPORT
        let networkCard = makeOptionCard(
            emoji: "📶",
            title: "네트워크 (QR 스캔)",
            subtitle: "외부망에서도 연결됩니다. Mac 화면의 QR을 스캔하려면 카메라 권한이 필요해요."
        ) { [weak self] in self?.onChoose?(.network) }
        stack.addArrangedSubview(networkCard)
        #endif

        [titleLabel, subtitleLabel, stack].forEach(view.addSubview)

        titleLabel.snp.makeConstraints { make in
            make.centerY.equalToSuperview().offset(-100)
            make.leading.trailing.equalTo(view.safeAreaLayoutGuide).inset(24)
        }
        subtitleLabel.snp.makeConstraints { make in
            make.top.equalTo(titleLabel.snp.bottom).offset(8)
            make.leading.trailing.equalTo(view.safeAreaLayoutGuide).inset(24)
        }
        stack.snp.makeConstraints { make in
            make.top.equalTo(subtitleLabel.snp.bottom).offset(32)
            make.leading.trailing.equalTo(view.safeAreaLayoutGuide).inset(24)
        }
    }

    private func makeOptionCard(
        emoji: String,
        title: String,
        subtitle: String,
        action: @escaping () -> Void
    ) -> UIControl {
        let card = ActionCard()
        card.onTap = action
        card.backgroundColor = Palette.cardBackground
        card.layer.cornerRadius = 14

        let emojiLabel = UILabel()
        emojiLabel.text = emoji
        emojiLabel.font = .systemFont(ofSize: 28)

        let titleLabel = UILabel()
        titleLabel.text = title
        titleLabel.font = .systemFont(ofSize: 17, weight: .semibold)
        titleLabel.textColor = Palette.primaryText

        let subtitleLabel = UILabel()
        subtitleLabel.text = subtitle
        subtitleLabel.font = .systemFont(ofSize: 13)
        subtitleLabel.textColor = Palette.subtle
        subtitleLabel.numberOfLines = 0

        let textStack = UIStackView(arrangedSubviews: [titleLabel, subtitleLabel])
        textStack.axis = .vertical
        textStack.spacing = 4
        // UILabel은 기본적으로 터치를 받지 않아(isUserInteractionEnabled = false)
        // 부모로 그대로 통과되지만, 이 스택뷰 자체는 인터랙션이 켜져 있는 평범한
        // UIView라 라벨이 터치를 안 받으면 hitTest가 스택뷰 자신에서 멈춰버린다
        // — 그러면 카드(`ActionCard`, UIControl)까지 안 내려가 라벨 위를 눌러도
        // 반응이 없다(실기 확인 — 라벨 바깥 여백만 눌러야 눌리던 버그).
        textStack.isUserInteractionEnabled = false

        [emojiLabel, textStack].forEach(card.addSubview)
        emojiLabel.snp.makeConstraints { make in
            make.leading.equalToSuperview().offset(16)
            make.centerY.equalToSuperview()
            make.width.equalTo(36)
        }
        textStack.snp.makeConstraints { make in
            make.leading.equalTo(emojiLabel.snp.trailing).offset(12)
            make.trailing.equalToSuperview().offset(-16)
            make.top.equalToSuperview().offset(16)
            make.bottom.equalToSuperview().offset(-16)
        }
        return card
    }
}

/// 전체 카드 영역을 탭 가능하게 만드는 얇은 래퍼. 개별 라벨이 아니라 카드
/// 어디를 눌러도 반응해야 한다(작은 화면일수록 히트 영역이 중요하다).
private final class ActionCard: UIControl {
    var onTap: (() -> Void)?

    override init(frame: CGRect) {
        super.init(frame: frame)
        addTarget(self, action: #selector(tapped), for: .touchUpInside)
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) { fatalError("init(coder:) 는 쓰지 않는다") }

    @objc private func tapped() { onTap?() }

    override var isHighlighted: Bool {
        didSet { alpha = isHighlighted ? 0.7 : 1.0 }
    }
}

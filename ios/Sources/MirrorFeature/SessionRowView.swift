import DesignSystem
import MirrorFormat
import SnapKit
import UIKit
import Wire

/// `SessionList.svelte` 의 한 줄. 왼쪽은 점·에이전트·프로젝트·모델,
/// 오른쪽은 속도(또는 상태 단어)와 상대 시각.
public final class SessionRowView: UIView {

    private let dot = DotView(diameter: 6)
    private let leftLabel = UILabel()
    private let rightLabel = UILabel()
    private let relativeLabel = UILabel()

    /// attributedText 로 구간을 나눠 그리므로 평문은 여기서 꺼낸다.
    public var leftText: String? { leftLabel.attributedText?.string ?? leftLabel.text }
    public var rightText: String? { rightLabel.text }
    public var relativeText: String? { relativeLabel.text }
    public var dotColor: UIColor? { dot.color }

    public init() {
        super.init(frame: .zero)
        // 아래 configure 에서 attributedText 로 구간별 스타일을 지정한다.
        leftLabel.font = Typography.body
        leftLabel.textColor = Palette.primaryText
        rightLabel.font = Typography.rate
        relativeLabel.font = Typography.body
        relativeLabel.textColor = Palette.subtle

        [dot, leftLabel, rightLabel, relativeLabel].forEach(addSubview)

        dot.snp.makeConstraints { make in
            make.leading.equalToSuperview()
            make.centerY.equalToSuperview()
            make.width.height.equalTo(6)
        }
        leftLabel.snp.makeConstraints { make in
            make.leading.equalTo(dot.snp.trailing).offset(6)
            make.top.bottom.equalToSuperview().inset(6)
        }
        relativeLabel.snp.makeConstraints { make in
            make.trailing.equalToSuperview()
            make.centerY.equalToSuperview()
        }
        rightLabel.snp.makeConstraints { make in
            make.trailing.equalTo(relativeLabel.snp.leading).offset(-12)
            make.centerY.equalToSuperview()
            make.leading.greaterThanOrEqualTo(leftLabel.snp.trailing).offset(8)
        }
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) { fatalError("init(coder:) 는 쓰지 않는다") }

    public func configure(project: MirrorProject, kind: AgentKindCode, now: Date) {
        let agentName: String
        let agentColor: UIColor
        switch kind {
        case .claude: agentName = "Claude"; agentColor = Palette.claudeDot
        case .codex: agentName = "Codex"; agentColor = Palette.codexDot
        case .unknown: agentName = "?"; agentColor = Palette.dormantDot
        }

        // 원본 dotColor(): dormant 는 회색, idle 은 주황, active 는 에이전트 색.
        // .unknown 은 프로토콜이 앞서 나간 경우이므로 dormant 와 같이 조용히 취급한다.
        switch project.status {
        case .dormant, .unknown: dot.color = Palette.dormantDot
        case .idle: dot.color = Palette.idleDot
        case .active: dot.color = agentColor
        }

        // 원본은 한 줄 안에 세 가지 스타일이 공존한다(SessionList.svelte).
        //   <strong>Claude</strong>          → 굵게
        //   <span class="proj">· 이름</span>  → weight 500
        //   <span class="model subtle">모델</span> → 흐린 색
        // 단일 라벨로 뭉개면 전부 같게 보이므로 attributed string 으로 구간을 나눈다.
        let line = NSMutableAttributedString(
            string: agentName,
            attributes: [.font: Typography.strong, .foregroundColor: Palette.primaryText]
        )
        line.append(NSAttributedString(
            string: " · \(project.name)",
            attributes: [.font: Typography.medium, .foregroundColor: Palette.primaryText]
        ))
        line.append(NSAttributedString(
            string: " \(project.model)",
            attributes: [.font: Typography.body, .foregroundColor: Palette.subtle]
        ))
        leftLabel.attributedText = line

        switch project.status {
        case .active:
            rightLabel.text = "\(MirrorFormat.tokensPerSec(project.ratePerSec)) tok/s"
            rightLabel.textColor = Palette.rate
        case .idle:
            rightLabel.text = "idle"
            rightLabel.textColor = Palette.subtle
        case .dormant, .unknown:
            rightLabel.text = "dormant"
            rightLabel.textColor = Palette.subtle
        }

        relativeLabel.text = MirrorFormat.relativeTime(project.t, now: now)
    }
}

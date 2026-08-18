import DesignSystem
import MirrorFormat
import SnapKit
import UIKit
import Wire

/// `AgentCard.svelte` 이식. 위에서부터 헤더(점·이름·모델), tok/s, 대표 프로젝트·카운트다운,
/// 사용량 바 순서로 쌓는다. macOS 의 🔄 동기화 버튼은 읽기 전용 미러이므로 옮기지 않는다.
public final class AgentCardView: UIView {

    private let dot = DotView(diameter: 8)
    private let nameLabel = UILabel()
    private let modelLabel = UILabel()
    private let rateLabel = UILabel()
    private let unitLabel = UILabel()
    private let projectLabel = UILabel()
    private let countdownLabel = UILabel()
    private let quotaBarView = QuotaBarView()

    public var nameText: String? { nameLabel.text }
    public var modelText: String? { modelLabel.text }
    public var rateText: String? { rateLabel.text }
    public var projectText: String? { projectLabel.text }
    public var countdownText: String? { countdownLabel.isHidden ? nil : countdownLabel.text }
    public var dotColor: UIColor? { dot.color }
    /// autoPct/weeklyPct 전달이 뒤바뀌어도 컴파일은 되므로(둘 다 Float?), 실제 표시값을
    /// 검증할 수 있도록 다른 테스트 창구와 같은 스타일(읽기 전용 String?)로 노출한다.
    /// QuotaBarView 자체를 노출하지 않는 이유: configure() 를 우회해 바를 직접
    /// 조작할 수 있는 통로가 생기는 것을 막기 위해서다.
    public var quotaFivePercentText: String? { quotaBarView.fivePercentText }
    public var quotaWeeklyPercentText: String? { quotaBarView.weeklyPercentText }
    public var quotaFallbackText: String? { quotaBarView.fallbackText }
    /// 22pt 숫자와 "tok/s" 사이 간격. 상수 자체가 원본 CSS 에서 곧바로 읽히지 않는
    /// 유일한 값이라(아래 unitGap 주석 참고) 레이아웃 결과로도 고정해 둔다.
    public var rateToUnitGap: CGFloat { unitLabel.frame.minX - rateLabel.frame.maxX }

    /// 4.72(`.big` 의 22pt 폰트로 그려지는 공백 하나) + 4(`.unit { margin-left: 4px }`).
    static let unitGap: CGFloat = 4.72 + 4

    public init() {
        super.init(frame: .zero)
        backgroundColor = Palette.cardBackground
        layer.cornerRadius = 10

        nameLabel.font = Typography.name
        nameLabel.textColor = Palette.primaryText
        modelLabel.font = Typography.body
        modelLabel.textColor = Palette.subtle
        rateLabel.font = Typography.bigRate
        rateLabel.textColor = Palette.primaryText
        unitLabel.font = Typography.medium
        unitLabel.textColor = Palette.subtle
        unitLabel.text = "tok/s"
        projectLabel.font = Typography.body
        projectLabel.textColor = Palette.subtle
        countdownLabel.font = Typography.countdown
        countdownLabel.textColor = Palette.countdown
        countdownLabel.textAlignment = .right

        [dot, nameLabel, modelLabel, rateLabel, unitLabel,
         projectLabel, countdownLabel, quotaBarView].forEach(addSubview)

        dot.snp.makeConstraints { make in
            make.leading.equalToSuperview().offset(12)
            make.centerY.equalTo(nameLabel)
            make.width.height.equalTo(8)
        }
        nameLabel.snp.makeConstraints { make in
            make.leading.equalTo(dot.snp.trailing).offset(6)
            make.top.equalToSuperview().offset(10)
        }
        modelLabel.snp.makeConstraints { make in
            make.trailing.equalToSuperview().offset(-12)
            make.centerY.equalTo(nameLabel)
            make.leading.greaterThanOrEqualTo(nameLabel.snp.trailing).offset(8)
        }
        rateLabel.snp.makeConstraints { make in
            make.leading.equalToSuperview().offset(12)
            make.top.equalTo(nameLabel.snp.bottom).offset(4)
        }
        unitLabel.snp.makeConstraints { make in
            // AgentCard.svelte:90 `.unit { margin-left: 4px }` 만 옮기면 4pt 인데,
            // 원본의 실제 간격은 그보다 넓다. `.big`(AgentCard.svelte:89)은 flex 가
            // 아니라 **일반 블록**이라, 숫자와 <span class="unit"> 사이의 줄바꿈
            // (AgentCard.svelte:59-60)이 공백 하나로 접혀 **실제로 그려지고**, 그
            // 공백은 부모의 22px 폰트를 쓴다. 이 화면에서 유일한 non-flex 컨테이너라
            // 다른 간격은 전부 그냥 옮겨도 맞았다(flex 는 공백 텍스트 노드를 버린다).
            //   22pt bold 시스템 폰트의 공백 폭 = 4.72pt (실측)
            //   실제 간격 = 4.72(공백) + 4(margin-left) = 8.72pt
            make.leading.equalTo(rateLabel.snp.trailing).offset(AgentCardView.unitGap)
            make.firstBaseline.equalTo(rateLabel.snp.firstBaseline)
        }
        projectLabel.snp.makeConstraints { make in
            make.leading.equalToSuperview().offset(12)
            make.top.equalTo(rateLabel.snp.bottom).offset(2)
        }
        countdownLabel.snp.makeConstraints { make in
            make.trailing.equalToSuperview().offset(-12)
            make.centerY.equalTo(projectLabel)
            make.leading.greaterThanOrEqualTo(projectLabel.snp.trailing).offset(8)
        }
        quotaBarView.snp.makeConstraints { make in
            make.leading.equalToSuperview().offset(12)
            make.trailing.equalToSuperview().offset(-12)
            make.top.equalTo(projectLabel.snp.bottom).offset(6)
            make.bottom.equalToSuperview().offset(-10)
        }
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) { fatalError("init(coder:) 는 쓰지 않는다") }

    public func configure(agent: MirrorAgent, now: Date) {
        switch agent.kind {
        case .claude:
            nameLabel.text = "Claude Code"
            dot.color = Palette.claudeDot
        case .codex:
            nameLabel.text = "Codex"
            dot.color = Palette.codexDot
        case .unknown:
            nameLabel.text = "알 수 없음"
            dot.color = Palette.dormantDot
        }

        // 원본과 동일: active 를 우선하고, 없으면 첫 번째를 대표로 삼는다.
        let primary = agent.projects.first(where: { $0.status == .active }) ?? agent.projects.first
        modelLabel.text = primary?.model ?? "—"
        projectLabel.text = primary?.name ?? "no active session"
        rateLabel.text = MirrorFormat.tokensPerSec(agent.ratePerSec)

        if let resetAt = agent.r5 {
            countdownLabel.isHidden = false
            countdownLabel.text = MirrorFormat.countdown(resetAt: resetAt, now: now)
        } else {
            countdownLabel.isHidden = true
            countdownLabel.text = nil
        }

        quotaBarView.configure(
            tokens5h: agent.tokens5h,
            autoPct: agent.usedPct5h,
            weeklyPct: agent.usedPctWeekly,
            isReset5h: QuotaDisplay.isReset5h(resetAt: agent.r5, now: now)
        )
    }
}

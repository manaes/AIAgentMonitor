import DesignSystem
import MirrorFormat
import SnapKit
import UIKit
import Wire

/// `SessionList.svelte` 의 한 줄. 왼쪽은 점·에이전트·프로젝트·모델,
/// 오른쪽은 속도(또는 상태 단어)와 상대 시각.
public final class SessionRowView: UIView {

    private let dot = DotView(diameter: 6)
    /// 원본의 `.left { display: flex; gap: 6px }` 를 실제 뷰 간격으로 재현하기 위해
    /// 세 라벨을 스택뷰로 나눈다. 단일 라벨 + attributed string 은 spacing 비율이
    /// 원본과 어긋난다(리뷰 지적) — `.model { margin-left: 4px }` 가 `gap: 6px` 위에
    /// 더해져 모델 쪽이 더 떨어져 보이는데, 균일한 공백 문자로는 그 차이를 못 낸다.
    private let nameLabel = UILabel()
    private let projLabel = UILabel()
    private let modelLabel = UILabel()
    private let leftStack = UIStackView()
    private let rightLabel = UILabel()
    private let relativeLabel = UILabel()

    /// 세 라벨을 공백으로 이어 붙인 평문. 고정된 테스트 문자열
    /// ("Claude · foo claude-opus-5")과의 호환을 위해 유지한다.
    public var leftText: String? {
        [nameLabel.text, projLabel.text, modelLabel.text]
            .compactMap { $0 }
            .joined(separator: " ")
    }
    public var rightText: String? { rightLabel.text }
    /// active(속도) 와 그 외(상태 단어)의 폰트가 실제로 갈리는지 고정하기 위한 창구.
    public var rightFont: UIFont? { rightLabel.font }
    public var relativeText: String? { relativeLabel.text }
    public var dotColor: UIColor? { dot.color }

    /// 잘림 순서 검증용 테스트 창구. `xLabelWidth`는 레이아웃 이후 실제 폭,
    /// `xLabelIntrinsicWidth`는 한 줄로 다 보여줄 때 필요한 폭(잘리지 않았다면 서로 같다).
    public var nameLabelWidth: CGFloat { nameLabel.bounds.width }
    public var projLabelWidth: CGFloat { projLabel.bounds.width }
    public var modelLabelWidth: CGFloat { modelLabel.bounds.width }
    public var nameLabelIntrinsicWidth: CGFloat { nameLabel.intrinsicContentSize.width }
    public var projLabelIntrinsicWidth: CGFloat { projLabel.intrinsicContentSize.width }
    public var modelLabelIntrinsicWidth: CGFloat { modelLabel.intrinsicContentSize.width }

    public init() {
        super.init(frame: .zero)
        nameLabel.font = Typography.strong
        nameLabel.textColor = Palette.primaryText
        projLabel.font = Typography.medium
        projLabel.textColor = Palette.primaryText
        modelLabel.font = Typography.body
        modelLabel.textColor = Palette.subtle
        // rightLabel.font 는 여기서 정하지 않는다 — 상태에 따라 갈리므로
        // configure 의 switch 안에서 색과 함께 설정한다(아래 주석 참고).
        relativeLabel.font = Typography.body
        relativeLabel.textColor = Palette.subtle

        // 원본 CSS 에는 줄바꿈/잘림 규칙이 없지만, 실제 기기에서는 한 줄을 넘기면
        // 반드시 무언가 잘려야 한다. 정보량이 적은 순서대로 잘리게 우선순위를 정한다:
        // 에이전트 이름(가장 중요, 절대 안 잘림) > 프로젝트 이름 > 모델 이름(가장 반복적, 제일 먼저 잘림).
        [nameLabel, projLabel, modelLabel].forEach {
            $0.numberOfLines = 1
            $0.lineBreakMode = .byTruncatingTail
        }
        nameLabel.setContentCompressionResistancePriority(.required, for: .horizontal)
        projLabel.setContentCompressionResistancePriority(UILayoutPriority(700), for: .horizontal)
        modelLabel.setContentCompressionResistancePriority(UILayoutPriority(500), for: .horizontal)
        nameLabel.setContentHuggingPriority(.required, for: .horizontal)
        projLabel.setContentHuggingPriority(UILayoutPriority(700), for: .horizontal)
        modelLabel.setContentHuggingPriority(UILayoutPriority(500), for: .horizontal)

        leftStack.axis = .horizontal
        leftStack.spacing = 6
        [nameLabel, projLabel, modelLabel].forEach(leftStack.addArrangedSubview)
        // .model { margin-left: 4px } 이 .left 의 gap: 6px 위에 더해져 모델이
        // 10px 만큼 떨어진다 — 균일 6px 이 아니라 이 두 번째 간격만 넓힌다.
        leftStack.setCustomSpacing(10, after: projLabel)

        // 순서는 model(500) < 이 바닥(600) < project(700) < name(required) —
        // 여유가 줄어들 때 항상 "model 이 project 보다 먼저 줄어든다"가 유지되도록
        // 바닥을 project 보다 낮게 둔다.
        //
        // 주의: 이 우선순위(600)에서 바닥은 실제로는 작동하지 않는다. project(700)
        // 보다 낮은 제약은, 공간이 부족해지면 project 가 한 치도 양보하기 전에
        // 모델의 물리적 최소값인 0 까지 먼저 완전히 희생된다 — 즉 30pt 를 지켜주지
        // 못하고 그대로 통과해 사라진다(215~320pt 로 실측 확인,
        // testPathologicalLongModelNameCurrentlyCanStillReachZero 가 이 실제
        // 동작을 고정해둔다). 바닥을 진짜로 작동시키려면 project(700)보다 높은
        // 우선순위가 필요한데, 그러면 이번엔 이 순서가 뒤집혀 project 가 model
        // 보다 먼저, 그것도 크게 희생된다 — 라운드 3에서 실제로 그렇게 했다가
        // 존재하지 않는 31자짜리 모델명 픽스처 때문이었다는 게 드러나 되돌렸다.
        // 실제 모델명(claude-opus-5, claude-sonnet-5, haiku/opus/sonnet)과 흔한
        // 프로젝트 폴더명 조합에서는 초과폭이 10pt 안팎이라 이 바닥 근처에도 가지
        // 않는다 — 그러니 이 트레이드오프 자체가 실사용 경로에서는 발생하지 않는다.
        // 자세한 경위와 실측값은 task-5-report.md의 "Fix Round 4" 참고.
        modelLabel.snp.makeConstraints { make in
            make.width.greaterThanOrEqualTo(30).priority(600)
        }

        [dot, leftStack, rightLabel, relativeLabel].forEach(addSubview)

        dot.snp.makeConstraints { make in
            make.leading.equalToSuperview()
            make.centerY.equalToSuperview()
            make.width.height.equalTo(6)
        }
        leftStack.snp.makeConstraints { make in
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
            make.leading.greaterThanOrEqualTo(leftStack.snp.trailing).offset(8)
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
        nameLabel.text = agentName
        projLabel.text = "· \(project.name)"
        modelLabel.text = project.model

        // 오른쪽 칸은 상태에 따라 텍스트·색뿐 아니라 **폰트도** 갈린다.
        //   active  → SessionList.svelte:55 `.rate { font-weight: 600; tabular-nums }`
        //             = Typography.rate (11pt semibold, monospacedDigit)
        //   그 외    → SessionList.svelte:35 `<span class="subtle">{row.status}</span>`
        //             .subtle 은 색만 지정하고(:56), 크기는 app.css:22 의 11px,
        //             굵기는 상속된 normal 이다 = Typography.body (11pt regular)
        // 폰트를 init 에서 한 번만 정하면 idle/dormant/unknown 이 맥에는 없는
        // semibold·tabular 로 그려진다 — 그래서 여기 switch 안에서 색과 함께 정한다.
        switch project.status {
        case .active:
            rightLabel.text = "\(MirrorFormat.tokensPerSec(project.ratePerSec)) tok/s"
            rightLabel.font = Typography.rate
            rightLabel.textColor = Palette.rate
        case .idle:
            rightLabel.text = "idle"
            rightLabel.font = Typography.body
            rightLabel.textColor = Palette.subtle
        case .dormant:
            rightLabel.text = "dormant"
            rightLabel.font = Typography.body
            rightLabel.textColor = Palette.subtle
        case .unknown:
            // Wire.ActivityStatusCode.unknown 은 "미래에 Rust 가 새 코드를 추가했을 때
            // dormant 로 조용히 뭉개지 않기 위한" 값(MirrorSnapshot.swift 주석 참고).
            // 그러니 여기서도 dormant 라고 잘못 표시하지 않고 있는 그대로 알린다.
            rightLabel.text = "unknown"
            rightLabel.font = Typography.body
            rightLabel.textColor = Palette.subtle
        }

        relativeLabel.text = MirrorFormat.relativeTime(project.t, now: now)
    }
}

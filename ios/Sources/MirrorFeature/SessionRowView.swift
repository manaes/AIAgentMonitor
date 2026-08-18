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
        rightLabel.font = Typography.rate
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

        // 모델이 가장 먼저 잘리는 것과, 0 폭까지 사라지는 것은 다르다 — 후자는
        // 잘림이 아니라 정보 삭제다. iPhone 실기기 폭(약 215pt 가용)에서도 최소한
        // 말줄임표를 포함한 몇 글자는 남도록 바닥을 둔다.
        //
        // 우선순위는 project(700)보다 높은 800 으로 뒀다 — 처음엔 project(700)와
        // model 자체 compression resistance(500) 사이(600)를 시도했는데, 215pt 처럼
        // 이름+프로젝트 전체 폭만으로 이미 여유가 없는 실측 폭에서는 solver 가
        // "project 를 온전히 지키기(700) > model 바닥을 지키기(600)" 순으로 풀어
        // model 이 오히려 0 까지 무너졌다(레이아웃 후 실측 확인). 바닥이 project 의
        // 저항보다 먼저 지켜져야 "모델이 가장 먼저 줄어들되 바닥 아래로는 안 간다"가
        // 성립하므로, 바닥 우선순위를 project 보다 위(800, name 의 1000 보다는 아래)로
        // 올렸다 — 여유가 있을 때는 여전히 model(500) 이 project(700) 보다 먼저
        // 줄어들고, 여유가 다 떨어지면 그 다음엔 project 가 이 바닥을 위해 양보한다.
        modelLabel.snp.makeConstraints { make in
            make.width.greaterThanOrEqualTo(30).priority(800)
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

        switch project.status {
        case .active:
            rightLabel.text = "\(MirrorFormat.tokensPerSec(project.ratePerSec)) tok/s"
            rightLabel.textColor = Palette.rate
        case .idle:
            rightLabel.text = "idle"
            rightLabel.textColor = Palette.subtle
        case .dormant:
            rightLabel.text = "dormant"
            rightLabel.textColor = Palette.subtle
        case .unknown:
            // Wire.ActivityStatusCode.unknown 은 "미래에 Rust 가 새 코드를 추가했을 때
            // dormant 로 조용히 뭉개지 않기 위한" 값(MirrorSnapshot.swift 주석 참고).
            // 그러니 여기서도 dormant 라고 잘못 표시하지 않고 있는 그대로 알린다.
            rightLabel.text = "unknown"
            rightLabel.textColor = Palette.subtle
        }

        relativeLabel.text = MirrorFormat.relativeTime(project.t, now: now)
    }
}

import MirrorFormat
import SnapKit
import UIKit

/// `QuotaBar.svelte` 이식. 네 가지 표시 상태를 가진다.
/// 1) 동기화 후: 5h 바 (+ 주간 값이 있으면 주간 바)
/// 2) 리셋 직후: 5h 를 0% 로
/// 3) 동기화 전: 바 대신 "5h 토큰: N · 동기화 전"
/// 4) 조회 실패(unreadable): 바 대신 "5h 토큰: N · 한도 조회 실패"
///
/// **행 두 개(5h·주간)는 어떤 상태에서도 항상 그린다.** 값이 없다고 행을 통째로
/// 숨기면(과거 iOS 코드가 그랬다) 에이전트마다 카드 높이가 달라져 목록이 들쭉날쭉
/// 해진다 — 실제로 2026-09 요금제 변경 이후 Codex 가 5h 창 없이 주간만 돌려주면서
/// 재현됐다(맥은 `QuotaBar.svelte`(2026-09-15)에서 이미 같은 이유로 고쳐 뒀다).
/// 값이 없는 행은 막대 대신 안내 문구로 자리를 채운다.
public final class QuotaBarView: UIView {

    private let fiveRow = PercentRow(title: "5h")
    private let weeklyRow = PercentRow(title: "주간")
    private let stack = UIStackView()

    /// 테스트에서 표시 결과를 확인하기 위한 읽기 전용 창구.
    public var fivePercentText: String? { fiveRow.percentText }
    public var weeklyPercentText: String? { weeklyRow.percentText }
    /// 5h 행이 %가 아니라 안내 문구를 보여주는 중이면 그 문구.
    public var fallbackText: String? { fiveRow.unavailableText }
    /// 주간 행이 %가 아니라 안내 문구를 보여주는 중이면 그 문구.
    public var weeklyFallbackText: String? { weeklyRow.unavailableText }
    /// 5h 채움 막대의 실제 폭 비율(레이아웃 이후). 트랙 대비 채움 폭을 검증하기 위한 창구.
    public var fiveFillRatio: CGFloat? { fiveRow.fillRatio }

    public init() {
        super.init(frame: .zero)
        stack.axis = .vertical
        stack.spacing = 6
        addSubview(stack)
        stack.snp.makeConstraints { $0.edges.equalToSuperview() }

        [fiveRow, weeklyRow].forEach(stack.addArrangedSubview)
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) { fatalError("init(coder:) 는 쓰지 않는다") }

    /// - Parameter unreadable: 사용량 조회가 실패 중이면 true → %와 막대를 숨긴다.
    ///   맥은 실패 중에도 마지막 %를 함께 보내주지만(구버전 CYD 호환), 그 숫자는
    ///   지금 상태를 말해주지 못하므로 낡은 값을 멀쩡한 척 보여주지 않는다.
    ///   로컬 토큰 수는 서버 한도가 아니라 계속 유효하므로 그건 남긴다.
    public func configure(
        tokens5h: UInt32,
        autoPct: Float?,
        weeklyPct: Float?,
        isReset5h: Bool,
        unreadable: Bool = false
    ) {
        let pct = unreadable ? nil : QuotaDisplay.displayPercent(autoPct: autoPct, isReset: isReset5h)
        let weeklyPctValue = unreadable ? nil : weeklyPct
        // 값이 하나라도 왔다면 조회 자체는 성공한 것이다 — 그런데도 비어 있는 창은
        // "아직 안 받아온" 게 아니라 이 플랜엔 없는 창이다(QuotaBar.svelte 의
        // `synced` 와 동일한 구분).
        let synced = pct != nil || weeklyPctValue != nil
        func note() -> String {
            unreadable ? "한도 조회 실패" : (synced ? "지원하지 않음" : "동기화 전")
        }

        if let pct {
            fiveRow.apply(percent: pct)
        } else {
            // 원본은 tokens_in + tokens_out 을 합쳐 보여주는데, 전송 DTO 의 t5 가 이미 그 합이다.
            fiveRow.applyUnavailable(text: "5h 토큰: \(MirrorFormat.tokensTotal(tokens5h)) · \(note())")
        }

        if let weeklyPctValue {
            weeklyRow.apply(percent: min(100, weeklyPctValue))
        } else {
            weeklyRow.applyUnavailable(text: note())
        }
    }
}

/// 라벨 + 퍼센트 + 진행 바 한 세트.
private final class PercentRow: UIView {
    private let titleLabel = UILabel()
    private let percentLabel = UILabel()
    private let track = UIView()
    private let fill = UIView()
    private let gradient = CAGradientLayer()
    private var ratio: CGFloat = 0
    /// true면 percentLabel이 실제 %를, false면 안내 문구("동기화 전" 등)를 담고 있다 —
    /// 두 모드 모두 percentLabel 하나를 재사용하므로 `percentText`/`unavailableText`가
    /// 서로를 nil로 가리는 데 이 플래그가 필요하다.
    private var isShowingPercent = false

    var percentText: String? { isShowingPercent ? percentLabel.text : nil }
    /// 안내 문구를 보여주는 중이면 그 문구(예: "동기화 전", "지원하지 않음").
    var unavailableText: String? { isShowingPercent ? nil : percentLabel.text }
    /// 트랙 대비 채움 막대의 실제 폭 비율(레이아웃 후에만 유의미).
    var fillRatio: CGFloat? {
        let trackWidth = track.bounds.width
        guard trackWidth > 0 else { return nil }
        return fill.bounds.width / trackWidth
    }

    init(title: String) {
        super.init(frame: .zero)
        titleLabel.text = title
        titleLabel.font = Typography.label
        titleLabel.textColor = Palette.subtle

        percentLabel.font = Typography.percent
        percentLabel.textColor = Palette.percent
        percentLabel.textAlignment = .right

        track.backgroundColor = Palette.barTrack
        track.layer.cornerRadius = 3
        track.layer.masksToBounds = true

        gradient.startPoint = CGPoint(x: 0, y: 0.5)
        gradient.endPoint = CGPoint(x: 1, y: 0.5)
        fill.layer.addSublayer(gradient)
        fill.layer.cornerRadius = 3
        fill.layer.masksToBounds = true

        [titleLabel, percentLabel, track].forEach(addSubview)
        track.addSubview(fill)

        titleLabel.snp.makeConstraints { make in
            make.leading.top.equalToSuperview()
        }
        percentLabel.snp.makeConstraints { make in
            make.trailing.equalToSuperview()
            make.firstBaseline.equalTo(titleLabel.snp.firstBaseline)
        }
        track.snp.makeConstraints { make in
            make.top.equalTo(percentLabel.snp.bottom).offset(3)
            make.leading.trailing.bottom.equalToSuperview()
            make.height.equalTo(6)
        }
        fill.snp.makeConstraints { make in
            make.leading.top.bottom.equalToSuperview()
            make.width.equalToSuperview().multipliedBy(0).priority(.high)
        }
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) { fatalError("init(coder:) 는 쓰지 않는다") }

    func apply(percent: Float) {
        isShowingPercent = true
        percentLabel.textColor = Palette.percent
        track.alpha = 1
        // %.0f 는 C 의 짝수 반올림이라 정확히 .5 인 값에서 JS 의 toFixed(0)(항상 0에서
        // 먼 쪽으로 반올림)와 어긋난다. MirrorFormat.toFixed 가 그 규칙을 이미 golden
        // table 로 맞춰뒀으므로 자체 포맷 대신 그대로 재사용한다.
        percentLabel.text = MirrorFormat.toFixed(Double(percent), 0) + "%"
        ratio = CGFloat(max(0, min(100, percent)) / 100)
        let g = QuotaDisplay.gradient(forPercent: percent)
        gradient.colors = [UIColor(hex: g.startHex).cgColor, UIColor(hex: g.endHex).cgColor]
        fill.snp.remakeConstraints { make in
            make.leading.top.bottom.equalToSuperview()
            make.width.equalToSuperview().multipliedBy(max(ratio, 0.0001)).priority(.high)
        }
        setNeedsLayout()
    }

    /// 값이 없는 행(플랜 미지원/동기화 전/조회 실패) — %와 막대 대신 안내 문구를
    /// 같은 자리에 채운다. 행 자체를 숨기지 않는 것이 핵심이다(카드 높이 유지,
    /// `QuotaBarView` 문서 참고) — 대신 트랙을 흐리게(`.bar.idle` 과 동일한 의도)
    /// 만들어 "지금 값이 없는 자리"라는 걸 시각적으로도 드러낸다.
    func applyUnavailable(text: String) {
        isShowingPercent = false
        percentLabel.textColor = Palette.subtle
        percentLabel.text = text
        ratio = 0
        track.alpha = 0.55
        fill.snp.remakeConstraints { make in
            make.leading.top.bottom.equalToSuperview()
            make.width.equalToSuperview().multipliedBy(0.0001).priority(.high)
        }
        setNeedsLayout()
    }

    override func layoutSubviews() {
        super.layoutSubviews()
        gradient.frame = fill.bounds
    }
}

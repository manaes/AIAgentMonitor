import Foundation
import MirrorFormat
import Wire

/// 한 창(5h·주간)을 위젯에 어떻게 그릴지에 대한 결정. 뷰가 아니라 값이라 테스트할 수 있다.
///
/// 위젯 표시 규칙의 버그는 실기 스크린샷으로만 잡혀 왔다(2026-09-17: 주간 %가 있는데도
/// 위젯만 "동기화 전"으로 뜨던 건, 뷰 안에 있던 판정 로직이 리셋 시각까지 있어야 %를
/// 보여주게 짜여 있었기 때문이다). 레이아웃은 몰라도 판정 규칙만큼은 뷰 밖으로 빼서
/// 회귀 테스트로 고정한다.
public struct QuotaWindowDisplay: Equatable, Sendable {
    /// % 텍스트. nil 이면 대신 `fallbackText` 를 보여준다.
    public let percentText: String?
    /// 리셋까지 남은 시간. **% 와 독립이다** — 맥의 `/usage` 안전망 경로는 %만 주고
    /// 리셋 시각은 주지 않는다(`quota_proxy.rs` 의 `apply_usage_pct`). 둘을
    /// all-or-nothing 으로 묶으면 앱은 %를 보여주는데 위젯만 폴백으로 떨어진다.
    public let countdownText: String?
    /// 막대 채움 비율(0...100). nil 이면 막대를 그리지 않는다.
    public let percent: Float?
    /// %를 못 그릴 때 그 자리에 보여줄 문구.
    public let fallbackText: String?

    public init(percentText: String?, countdownText: String?, percent: Float?, fallbackText: String?) {
        self.percentText = percentText
        self.countdownText = countdownText
        self.percent = percent
        self.fallbackText = fallbackText
    }
}

public enum UsagePresentation {

    /// 5h 창. 이 창에는 리셋 카운트다운을 붙이지 않는다(위젯 폭이 좁아 주간 쪽에만 둔다).
    public static func fiveHour(for agent: MirrorAgent) -> QuotaWindowDisplay {
        guard agent.quotaError == nil, let pct = agent.usedPct5h else {
            return QuotaWindowDisplay(
                percentText: nil,
                countdownText: nil,
                percent: nil,
                fallbackText: fallbackText(for: agent)
            )
        }
        let clamped = min(100, pct)
        return QuotaWindowDisplay(
            percentText: percentText(clamped),
            countdownText: nil,
            percent: clamped,
            fallbackText: nil
        )
    }

    /// 주간 창. 리셋 시각(`rw`)이 없어도 %는 그대로 보여준다 — 위 `countdownText` 주석 참고.
    public static func weekly(for agent: MirrorAgent, now: Date) -> QuotaWindowDisplay {
        guard agent.quotaError == nil, let pct = agent.usedPctWeekly else {
            return QuotaWindowDisplay(
                percentText: nil,
                countdownText: nil,
                percent: nil,
                fallbackText: fallbackText(for: agent)
            )
        }
        let clamped = min(100, pct)
        return QuotaWindowDisplay(
            percentText: percentText(clamped),
            countdownText: agent.rw.flatMap { MirrorFormat.weeklyCountdown(resetAt: $0, now: now) },
            percent: clamped,
            fallbackText: nil
        )
    }

    /// 값이 없는 행의 안내 문구 — 맥 앱 `QuotaBarView.configure` 의 `note()` 와 같은 구분이다.
    /// 다른 창의 값이 하나라도 왔으면 조회 자체는 성공한 것이라 "동기화 전"이 아니라 이
    /// 플랜에 없는 창("지원하지 않음")이다(Codex 는 5h 창 없이 주간만 온다).
    public static func fallbackText(for agent: MirrorAgent) -> String {
        if let error = agent.quotaError { return error.displayText }
        let synced = agent.usedPct5h != nil || agent.usedPctWeekly != nil
        return synced ? "지원하지 않음" : "동기화 전"
    }

    /// 맥/CYD 와 같은 반올림 규칙을 쓴다(`MirrorFormat.toFixed` 의 golden table).
    private static func percentText(_ percent: Float) -> String {
        MirrorFormat.toFixed(Double(percent), 0) + "%"
    }
}

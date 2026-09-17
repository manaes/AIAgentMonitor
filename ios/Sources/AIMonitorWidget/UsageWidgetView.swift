import DesignSystem
import MirrorFormat
import SwiftUI
import UIKit
import WidgetKit
import Wire

struct UsageWidgetView: View {
    @Environment(\.widgetFamily) private var family
    let entry: UsageEntry

    var body: some View {
        Group {
            if let snapshot = entry.snapshot {
                content(for: snapshot)
            } else {
                emptyState
            }
        }
        // frame이 없으면 WidgetKit이 내용물의 고유 크기로만 배치해 중앙에
        // 뭉치고 나머지 영역이 빈다 — 위젯 전체 영역을 채우고 좌상단에 고정한다.
        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
        // iOS 17+ 위젯은 이 modifier가 없으면 배경이 제대로 안 그려진다.
        .containerBackground(.fill.tertiary, for: .widget)
    }

    private var emptyState: some View {
        VStack(spacing: 8) {
            Text("Mac 앱에서 먼저 연결하세요")
                .font(.caption)
                .multilineTextAlignment(.center)
            Button(intent: RefreshUsageIntent()) {
                Image(systemName: "arrow.clockwise")
            }
        }
        .padding()
        .widgetURL(URL(string: "aim://open"))
    }

    private func content(for snapshot: MirrorSnapshot) -> some View {
        let ordered = orderedForDisplay(snapshot.agents)
        let now = Date()

        return VStack(alignment: .leading, spacing: 6) {
            headerRow

            if family == .systemSmall {
                // 좁은 위젯은 세로 1열 — 에이전트 사이만 구분선을 넣는다.
                VStack(alignment: .leading, spacing: 6) {
                    ForEach(Array(ordered.enumerated()), id: \.offset) { index, agent in
                        agentCard(agent, now: now)
                        if index < ordered.count - 1 {
                            Divider().background(Color(Palette.separator))
                        }
                    }
                }
            } else {
                // Medium/Large 는 가로로 나란히 놓고 세로 구분선으로 나눈다.
                HStack(alignment: .top, spacing: 0) {
                    ForEach(Array(ordered.enumerated()), id: \.offset) { index, agent in
                        agentCard(agent, now: now)
                            .frame(maxWidth: .infinity, alignment: .leading)
                        if index < ordered.count - 1 {
                            Divider().background(Color(Palette.separator))
                        }
                    }
                }
            }
        }
        .padding()
    }

    /// 앱 브랜드 표기(로고 자리 SF Symbol) + 신선도 + 새로고침. 우하단 코너
    /// 오버레이 방식을 되돌리고, 참고 위젯(HRV)처럼 상단 한 줄 + 구분선으로 복귀.
    private var headerRow: some View {
        VStack(alignment: .leading, spacing: 6) {
            HStack {
                Image(systemName: "cpu")
                    .font(.system(size: 11))
                    .foregroundStyle(Color(Palette.subtle))
                Text("AI Monitor")
                    .font(Font(Typography.label))
                    .foregroundStyle(Color(Palette.subtle))
                Spacer()
                if let fetchedAt = entry.fetchedAt {
                    Text(fetchedAt, style: .relative)
                        .font(.system(size: 9))
                        .foregroundStyle(.secondary)
                }
                Button(intent: RefreshUsageIntent()) {
                    Image(systemName: "arrow.clockwise")
                        .font(.system(size: 10))
                }
                .buttonStyle(.plain)
            }
            Divider()
                .background(Color(Palette.separator))
        }
    }

    private func agentCard(_ agent: MirrorAgent, now: Date) -> some View {
        VStack(alignment: .leading, spacing: 6) {
            Text(agentName(agent.kind))
                .font(Font(Typography.name))
                .foregroundStyle(Color(Palette.primaryText))

            if agent.quotaError == nil, let p5 = agent.usedPct5h {
                let clamped5h = min(100, p5)
                quotaRow(
                    label: "5h",
                    percentText: MirrorFormat.toFixed(Double(clamped5h), 0) + "%",
                    countdownText: nil,
                    percent: clamped5h
                )
            } else {
                quotaRow(label: "5h", percentText: nil, countdownText: weeklyFallbackText(for: agent), percent: nil)
            }

            if let usage = weeklyUsage(for: agent, now: now) {
                quotaRow(label: "Week", percentText: usage.percentText, countdownText: usage.countdownText, percent: usage.percent)
            } else {
                quotaRow(label: "Week", percentText: nil, countdownText: weeklyFallbackText(for: agent), percent: nil)
            }
        }
    }

    /// 5h·주간 행을 공유하는 빌더 — `QuotaBarView`처럼 두 창을 같은 모양(라벨 +
    /// %/카운트다운 + 그라디언트 막대)으로 그린다. `percent`가 nil이면(에러/미동기화)
    /// 막대 없이 라벨 줄만 남긴다.
    private func quotaRow(label: String, percentText: String?, countdownText: String?, percent: Float?) -> some View {
        VStack(alignment: .leading, spacing: 2) {
            HStack {
                Text(label)
                    .font(Font(Typography.label))
                    .foregroundStyle(Color(Palette.subtle))
                Spacer()
                if let countdownText {
                    Text(countdownText)
                        .font(Font(Typography.countdown))
                        .foregroundStyle(Color(Palette.countdown))
                }
                if let percentText {
                    Text(percentText)
                        .font(Font(Typography.percent))
                        .foregroundStyle(Color(Palette.percent))
                }
            }
            if let percent {
                GeometryReader { geo in
                    let gradient = QuotaDisplay.gradient(forPercent: percent)
                    ZStack(alignment: .leading) {
                        Capsule().fill(Color(Palette.barTrack))
                        Capsule()
                            .fill(LinearGradient(
                                colors: [
                                    Color(UIColor(hex: gradient.startHex)),
                                    Color(UIColor(hex: gradient.endHex)),
                                ],
                                startPoint: .leading,
                                endPoint: .trailing
                            ))
                            .frame(width: geo.size.width * CGFloat(percent / 100))
                    }
                }
                .frame(height: 5)
            }
        }
    }

    private struct WeeklyUsage {
        let percentText: String
        let countdownText: String?
        let percent: Float
    }

    /// 5h % 를 가리는 규칙(Fix 2)과 동일하게, 주간 %도 quotaError 가 있으면 숨긴다.
    /// % 와 카운트다운은 서로 독립이다 — rw(주간 리셋 시각) 없이 usedPctWeekly 만
    /// 오는 폴백 경로가 실존해서(맥 백엔드 확인), 둘을 all-or-nothing으로 묶으면
    /// 메인 앱은 %를 보여주는데 위젯만 "동기화 전"으로 떨어지는 불일치가 생긴다
    /// (AgentCardView/QuotaBarView 도 이미 %와 카운트다운을 따로 게이팅한다).
    private func weeklyUsage(for agent: MirrorAgent, now: Date) -> WeeklyUsage? {
        guard agent.quotaError == nil, let pct = agent.usedPctWeekly else {
            return nil
        }
        let clamped = min(100, pct)
        return WeeklyUsage(
            percentText: MirrorFormat.toFixed(Double(clamped), 0) + "%",
            countdownText: agent.rw.flatMap { MirrorFormat.weeklyCountdown(resetAt: $0, now: now) },
            percent: clamped
        )
    }

    private func weeklyFallbackText(for agent: MirrorAgent) -> String {
        agent.quotaError?.displayText ?? "동기화 전"
    }

    private func agentName(_ kind: AgentKindCode) -> String {
        switch kind {
        case .claude: return "Claude Code"
        case .codex: return "Codex"
        case .antigravity: return "Antigravity"
        case .unknown: return "Agent"
        }
    }

    /// `MirrorViewController.orderedForDisplay`와 같은 순서(설계 §3.4) —
    /// 위젯만의 새 선택 알고리즘을 만들지 않는다.
    private func orderedForDisplay(_ agents: [MirrorAgent]) -> [MirrorAgent] {
        let claude = agents.filter { $0.kind == .claude }
        let codex = agents.filter { $0.kind == .codex }
        let others = agents.filter { $0.kind != .claude && $0.kind != .codex }
        return claude + codex + others
    }
}

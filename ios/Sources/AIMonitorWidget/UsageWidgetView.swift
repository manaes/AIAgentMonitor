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
            HStack {
                if let fetchedAt = entry.fetchedAt {
                    Text(fetchedAt, style: .relative)
                        .font(.caption2)
                        .foregroundStyle(.secondary)
                }
                Spacer()
                Button(intent: RefreshUsageIntent()) {
                    Image(systemName: "arrow.clockwise")
                }
                .buttonStyle(.plain)
            }

            if family == .systemSmall {
                // 작은 위젯은 폭이 좁아 막대 없이 이름·%·카운트다운만 한 줄로 쌓는다.
                VStack(alignment: .leading, spacing: 6) {
                    ForEach(Array(ordered.enumerated()), id: \.offset) { _, agent in
                        compactAgentRow(agent, now: now)
                    }
                }
            } else {
                LazyVGrid(columns: [GridItem(.flexible()), GridItem(.flexible())], spacing: 8) {
                    ForEach(Array(ordered.enumerated()), id: \.offset) { _, agent in
                        agentCard(agent, now: now)
                    }
                }
            }
        }
        .padding()
    }

    private func compactAgentRow(_ agent: MirrorAgent, now: Date) -> some View {
        HStack {
            Text(agentName(agent.kind))
                .font(Font(Typography.name))
                .foregroundStyle(Color(Palette.primaryText))
            Spacer()
            if let usage = weeklyUsage(for: agent, now: now) {
                VStack(alignment: .trailing, spacing: 1) {
                    Text(usage.percentText)
                        .font(Font(Typography.percent))
                        .foregroundStyle(Color(Palette.percent))
                    if let countdownText = usage.countdownText {
                        Text(countdownText)
                            .font(Font(Typography.countdown))
                            .foregroundStyle(Color(Palette.countdown))
                    }
                }
            } else {
                Text(weeklyFallbackText(for: agent))
                    .font(Font(Typography.label))
                    .foregroundStyle(.secondary)
            }
        }
    }

    private func agentCard(_ agent: MirrorAgent, now: Date) -> some View {
        VStack(alignment: .leading, spacing: 4) {
            Text(agentName(agent.kind))
                .font(Font(Typography.name))
                .foregroundStyle(Color(Palette.primaryText))

            if let fiveHourText = fiveHourPercentText(for: agent) {
                Text(fiveHourText)
                    .font(Font(Typography.label))
                    .foregroundStyle(Color(Palette.subtle))
            }

            if let usage = weeklyUsage(for: agent, now: now) {
                HStack {
                    Text(usage.percentText)
                        .font(Font(Typography.percent))
                        .foregroundStyle(Color(Palette.percent))
                    Spacer()
                    if let countdownText = usage.countdownText {
                        Text(countdownText)
                            .font(Font(Typography.countdown))
                            .foregroundStyle(Color(Palette.countdown))
                    }
                }
                GeometryReader { geo in
                    let gradient = QuotaDisplay.gradient(forPercent: usage.percent)
                    ZStack(alignment: .leading) {
                        Capsule()
                            .fill(Color(Palette.barTrack))
                        Capsule()
                            .fill(LinearGradient(
                                colors: [
                                    Color(UIColor(hex: gradient.startHex)),
                                    Color(UIColor(hex: gradient.endHex)),
                                ],
                                startPoint: .leading,
                                endPoint: .trailing
                            ))
                            .frame(width: geo.size.width * CGFloat(usage.percent / 100))
                    }
                }
                .frame(height: 6)
            } else {
                Text(weeklyFallbackText(for: agent))
                    .font(Font(Typography.label))
                    .foregroundStyle(.secondary)
            }
        }
        .padding(8)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(Color(Palette.cardBackground))
        .clipShape(RoundedRectangle(cornerRadius: 8))
    }

    private struct WeeklyUsage {
        let percentText: String
        let countdownText: String?
        let percent: Float
    }

    /// 5h % 를 가리는 규칙(Fix 2)과 동일하게, 주간 %도 quotaError 가 있으면 숨긴다.
    private func weeklyUsage(for agent: MirrorAgent, now: Date) -> WeeklyUsage? {
        guard agent.quotaError == nil, let pct = agent.usedPctWeekly, let resetAt = agent.rw else {
            return nil
        }
        let clamped = min(100, pct)
        return WeeklyUsage(
            percentText: MirrorFormat.toFixed(Double(clamped), 0) + "%",
            countdownText: MirrorFormat.weeklyCountdown(resetAt: resetAt, now: now),
            percent: clamped
        )
    }

    private func weeklyFallbackText(for agent: MirrorAgent) -> String {
        agent.quotaError?.displayText ?? "동기화 전"
    }

    /// Medium/Large 카드 전용 보조 표시. quotaError 는 5h·주간 공통 상태이므로
    /// (Wire/MirrorSnapshot.swift 의 MirrorAgent 문서 참고) weeklyUsage 와 같은
    /// 게이트를 쓴다 — 실패/미동기화 시엔 아예 줄을 생략한다(에러 안내는 주간
    /// 폴백 한 줄로 이미 전달되므로 중복 표시하지 않는다).
    private func fiveHourPercentText(for agent: MirrorAgent) -> String? {
        guard agent.quotaError == nil, let pct = agent.usedPct5h else { return nil }
        return "5h " + MirrorFormat.toFixed(Double(min(100, pct)), 0) + "%"
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

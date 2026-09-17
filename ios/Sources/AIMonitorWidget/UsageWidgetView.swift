import SwiftUI
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
        let shown = family == .systemSmall ? Array(ordered.prefix(1)) : ordered

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
            ForEach(Array(shown.enumerated()), id: \.offset) { _, agent in
                agentRow(agent)
            }
        }
        .padding()
    }

    private func agentRow(_ agent: MirrorAgent) -> some View {
        HStack {
            Text(agentName(agent.kind))
                .font(.subheadline.bold())
            Spacer()
            Text("\(Int(agent.ratePerSec)) tok/s")
                .font(.caption)
            if agent.quotaError == nil, let pct = agent.usedPct5h {
                Text("· 5h \(Int(pct))%")
                    .font(.caption)
                    .foregroundStyle(.secondary)
            }
        }
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

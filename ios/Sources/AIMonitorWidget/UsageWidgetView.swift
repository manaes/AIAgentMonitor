import DesignSystem
import MirrorFormat
import SwiftUI
import UIKit
import WidgetKit
import WidgetShared
import Wire

struct UsageWidgetView: View {
    @Environment(\.widgetFamily) private var family
    let entry: UsageEntry

    /// Small은 주간 사용량만 세로로 배치하고 헤더는 아이콘으로 줄인다.
    private var compact: Bool { family == .systemSmall }

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

        return VStack(alignment: .leading, spacing: compact ? 4 : 8) {
            headerRow

            if compact {
                // 고유 높이로 위부터 쌓고, 남는 공간은 마지막 카드 아래에 둔다.
                ViewThatFits(in: .vertical) {
                    compactCards(ordered, now: now, tight: false)
                    compactCards(ordered, now: now, tight: false, contentSpacing: 6)
                    compactCards(ordered, now: now, tight: true, contentSpacing: 6)
                    compactCards(ordered, now: now, tight: true, contentSpacing: 2)
                }
                .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
            } else {
                // Medium/Large 는 가로로 나란히 놓고 세로 구분선으로 나눈다.
                HStack(alignment: .top, spacing: 0) {
                    ForEach(Array(ordered.enumerated()), id: \.offset) { index, agent in
                        agentCard(agent, now: now)
                            .padding(.leading, index == 0 ? 0 : 12)
                            .padding(.trailing, index == ordered.count - 1 ? 0 : 12)
                            .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
                        if index < ordered.count - 1 {
                            Divider().background(Color(Palette.separator))
                        }
                    }
                }
                .frame(maxHeight: .infinity)
            }
        }
        // 둥근 모서리 안쪽의 안전 여백. 시스템 여백은 꺼져 있어 한 번만 적용된다.
        .padding(.horizontal, 16)
        .padding(.top, 16)
        .padding(.bottom, 24)
    }

    private func compactCards(_ agents: [MirrorAgent], now: Date, tight: Bool, contentSpacing: CGFloat = 8) -> some View {
        VStack(alignment: .leading, spacing: 0) {
            ForEach(Array(agents.enumerated()), id: \.offset) { index, agent in
                agentCard(agent, now: now, tight: tight, contentSpacing: contentSpacing)
                    .frame(maxWidth: .infinity, alignment: .leading)
                if index < agents.count - 1 {
                    Divider()
                        .background(Color(Palette.separator))
                        .padding(.vertical, tight ? 5 : 6)
                }
            }
        }
    }

    /// 앱 브랜드 표기(로고 자리 SF Symbol) + 신선도 + 새로고침. 우하단 코너
    /// 오버레이 방식을 되돌리고, 참고 위젯(HRV)처럼 상단 한 줄 + 구분선으로 복귀.
    private var headerRow: some View {
        VStack(alignment: .leading, spacing: compact ? 4 : 6) {
            HStack(spacing: 6) {
                Image(systemName: "cpu")
                    .font(.system(size: 11))
                    .foregroundStyle(Color(Palette.subtle))
                // Small은 폭이 좁아 "AI Monitor" 전체 텍스트가 줄바꿈되며 깨진다 —
                // 아이콘만으로도 브랜드 표기는 충분하다.
                if !compact {
                    Text("AI Monitor")
                        .font(Font(Typography.label))
                        .foregroundStyle(Color(Palette.subtle))
                        .lineLimit(1)
                }
                Spacer()
                if let fetchedAt = entry.fetchedAt {
                    Text(fetchedAt, style: .relative)
                        .font(.system(size: 9))
                        .foregroundStyle(.secondary)
                        .lineLimit(1)
                        // WidgetKit의 동적 날짜 텍스트는 실제 글자보다 넓게 배치된다.
                        // 프레임뿐 아니라 텍스트 자체도 trailing으로 정렬해야 한다.
                        .multilineTextAlignment(.trailing)
                        .frame(maxWidth: .infinity, alignment: .trailing)
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

    /// 콘텐츠는 위부터 쌓는다. 좁은 Small에서만 글자와 내부 간격을 압축한다.
    private func agentCard(_ agent: MirrorAgent, now: Date, tight: Bool = false, contentSpacing: CGFloat = 8) -> some View {
        // 표시 규칙(어떤 창을 %로 보여줄지, 값이 없을 때 무슨 문구를 쓸지)은 뷰가 아니라
        // WidgetShared 의 UsagePresentation 이 정한다 — 테스트로 고정돼 있다.
        let weekly = UsagePresentation.weekly(for: agent, now: now)
        let fiveHour = UsagePresentation.fiveHour(for: agent)

        return VStack(alignment: .leading, spacing: 0) {
            VStack(alignment: .leading, spacing: compact ? 1 : 3) {
                Text(agentName(agent.kind))
                    .font(tight ? .system(size: 11, weight: .semibold) : Font(Typography.name))
                    .foregroundStyle(Color(Palette.primaryText))
                    .lineLimit(1)
                // 모든 카드에 같은 한 줄을 예약한다. 내용 유무로 그래프가 움직이지 않는다.
                Text(weekly.countdownText ?? " ")
                    .font(compact ? .system(size: tight ? 8 : 9, weight: .semibold) : Font(Typography.countdown))
                    .foregroundStyle(Color(Palette.countdown))
                    .lineLimit(1)
                    .minimumScaleFactor(0.8)
                    .accessibilityHidden(weekly.countdownText == nil)
            }

            Color.clear.frame(height: contentSpacing)

            if !compact {
                quotaRow(
                    label: "5h",
                    percentText: fiveHour.percentText,
                    countdownText: fiveHour.fallbackText,
                    percent: fiveHour.percent
                )

                Color.clear.frame(height: 12)
            }

            quotaRow(
                label: "Week",
                percentText: weekly.percentText,
                // 카운트다운은 카드 상단에 이미 예약된 줄로 나가므로 여기선 폴백 문구만 쓴다.
                countdownText: weekly.fallbackText,
                percent: weekly.percent,
                tight: tight
            )
        }
    }

    /// 5h·주간 행을 공유하는 빌더 — `QuotaBarView`처럼 두 창을 같은 모양(라벨 +
    /// %/카운트다운 + 그라디언트 막대)으로 그린다. 값이 없어도 막대의 자리와
    /// 라벨 높이를 유지해 옆 카드의 같은 기간과 수평으로 맞춘다.
    private func quotaRow(label: String, percentText: String?, countdownText: String?, percent: Float?, tight: Bool = false) -> some View {
        VStack(alignment: .leading, spacing: compact ? 1 : 2) {
            HStack(spacing: 4) {
                Text(label)
                    .font(Font(Typography.label))
                    .foregroundStyle(Color(Palette.subtle))
                    .lineLimit(1)
                Spacer(minLength: 2)
                if let countdownText {
                    Text(countdownText)
                        .font(compact ? .system(size: 9, weight: .semibold) : Font(Typography.countdown))
                        .foregroundStyle(Color(Palette.countdown))
                        .lineLimit(1)
                        .minimumScaleFactor(0.7)
                        .layoutPriority(-1)
                }
                if let percentText {
                    Text(percentText)
                        .font(tight ? .system(size: 11, weight: .bold).monospacedDigit() : Font(Typography.percent))
                        .foregroundStyle(Color(Palette.percent))
                        .lineLimit(1)
                }
            }
            .frame(height: tight ? 12 : (compact ? 14 : 16))
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
                .frame(height: tight ? 4 : (compact ? 5 : 6))
            } else {
                Color.clear.frame(height: tight ? 4 : (compact ? 5 : 6))
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

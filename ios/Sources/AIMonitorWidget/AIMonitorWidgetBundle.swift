import SwiftUI
import WidgetKit
import WidgetShared

struct AIMonitorWidget: Widget {
    var body: some WidgetConfiguration {
        StaticConfiguration(kind: widgetKind, provider: UsageTimelineProvider()) { entry in
            UsageWidgetView(entry: entry)
        }
        .configurationDisplayName("AI Agent 사용량")
        .description("Mac AI Agent Monitor의 tok/s·쿼터 사용량을 보여줍니다.")
        .supportedFamilies([.systemSmall, .systemMedium, .systemLarge])
        // iOS 17+ 는 위젯 콘텐츠 둘레에 시스템 여백(~16pt)을 자동으로 붙인다.
        // 뷰 쪽 .padding 과 겹쳐 좌우 여백이 참고 위젯(HRV/UP)의 두 배로 보였다
        // (실기 2026-09-17). 시스템 여백을 끄고 UsageWidgetView 가 직접 관리한다.
        .contentMarginsDisabled()
    }
}

@main
struct AIMonitorWidgetBundle: WidgetBundle {
    var body: some Widget {
        AIMonitorWidget()
    }
}

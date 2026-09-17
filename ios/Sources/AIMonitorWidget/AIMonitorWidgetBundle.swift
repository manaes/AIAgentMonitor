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
    }
}

@main
struct AIMonitorWidgetBundle: WidgetBundle {
    var body: some Widget {
        AIMonitorWidget()
    }
}

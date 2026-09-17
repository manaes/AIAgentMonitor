/// App(메인 앱)과 AIMonitorWidget(익스텐션)이 같은 문자열을 쓰기 위한 상수.
/// 위젯 kind는 `Widget.body`의 `StaticConfiguration(kind:)`와
/// `WidgetCenter.shared.reloadTimelines(ofKind:)` 양쪽에 정확히 같은 값이어야
/// 하므로, 문자열 리터럴을 두 곳에 따로 적지 않고 이 상수 하나로 통일한다.
public let widgetKind = "AIMonitorWidget"

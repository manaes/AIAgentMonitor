import AppIntents
import NetworkTransport
import WidgetKit
import WidgetShared

/// 위젯 안의 새로고침 버튼이 실행하는 액션. 위젯 익스텐션 프로세스 안에서
/// 바로 실행되고 메인 앱은 열리지 않는다(`openAppWhenRun`의 기본값이 이미
/// false이지만, 이 설계의 핵심이라 명시적으로 적어 둔다).
struct RefreshUsageIntent: AppIntent {
    static var title: LocalizedStringResource = "사용량 새로고침"
    static var openAppWhenRun: Bool { false }

    @MainActor
    func perform() async throws -> some IntentResult {
        let client = NetworkClient()
        // 실패해도(타임아웃·페어링 없음 등) 캐시는 그대로 두고 위젯만
        // 다시 그린다 — "새로고침 실패" 표시는 UsageWidgetView가 신선도
        // 문구로 대신한다(설계 §4).
        if let snapshot = try? await client.fetchSnapshotOnce(timeoutSeconds: 7) {
            UsageCacheStore.save(snapshot, fetchedAt: Date())
        }
        WidgetCenter.shared.reloadTimelines(ofKind: widgetKind)
        return .result()
    }
}

import NetworkTransport
import WidgetKit
import WidgetShared
import Wire

struct UsageEntry: TimelineEntry {
    let date: Date
    let snapshot: MirrorSnapshot?
    let fetchedAt: Date?
}

struct UsageTimelineProvider: TimelineProvider {
    func placeholder(in context: Context) -> UsageEntry {
        UsageEntry(date: Date(), snapshot: nil, fetchedAt: nil)
    }

    func getSnapshot(in context: Context, completion: @escaping (UsageEntry) -> Void) {
        let cached = UsageCacheStore.load()
        completion(UsageEntry(date: Date(), snapshot: cached?.snapshot, fetchedAt: cached?.fetchedAt))
    }

    /// 캐시를 먼저 즉시 보여줄 준비를 하고, 짧은 타임아웃으로 최신값을
    /// 시도한다 — 성공하면 캐시도 갱신, 실패하면 기존 캐시값 그대로
    /// 엔트리를 만든다(설계 §3.4). 다음 갱신은 15분 뒤로 요청하지만 실제
    /// 실행 시점은 iOS 스케줄러가 정한다(설계 §1 성공기준 3).
    func getTimeline(in context: Context, completion: @escaping (Timeline<UsageEntry>) -> Void) {
        Task { @MainActor in
            let cached = UsageCacheStore.load()
            var snapshot = cached?.snapshot
            var fetchedAt = cached?.fetchedAt

            let client = NetworkClient()
            if let fresh = try? await client.fetchSnapshotOnce(timeoutSeconds: 7) {
                let now = Date()
                UsageCacheStore.save(fresh, fetchedAt: now)
                snapshot = fresh
                fetchedAt = now
            }

            let entry = UsageEntry(date: Date(), snapshot: snapshot, fetchedAt: fetchedAt)
            let nextRefresh = Date().addingTimeInterval(15 * 60)
            completion(Timeline(entries: [entry], policy: .after(nextRefresh)))
        }
    }
}

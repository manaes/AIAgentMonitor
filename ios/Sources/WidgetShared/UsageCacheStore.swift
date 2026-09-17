import Foundation
import Wire

/// App Group으로 App(메인 앱)과 AIMonitorWidget(위젯 익스텐션)이 공유하는
/// 마지막 스냅샷 캐시. 스냅샷과 수신 시각을 한 구조체로 묶어 저장한다 —
/// 따로 저장하면 "새 스냅샷인데 옛날 시각" 같은 불일치가 생길 수 있다
/// (설계 §3.1).
public enum UsageCacheStore {
    private static let suiteName = "group.co.kr.wannypark.aiagentmirror"
    private static let key = "latestSnapshot"

    private struct Cached: Codable {
        let snapshot: MirrorSnapshot
        let fetchedAt: Date
    }

    public static func save(_ snapshot: MirrorSnapshot, fetchedAt: Date) {
        guard let defaults = UserDefaults(suiteName: suiteName) else { return }
        guard let data = try? JSONEncoder().encode(Cached(snapshot: snapshot, fetchedAt: fetchedAt)) else { return }
        defaults.set(data, forKey: key)
    }

    public static func load() -> (snapshot: MirrorSnapshot, fetchedAt: Date)? {
        guard let defaults = UserDefaults(suiteName: suiteName),
              let data = defaults.data(forKey: key),
              let cached = try? JSONDecoder().decode(Cached.self, from: data) else { return nil }
        return (cached.snapshot, cached.fetchedAt)
    }

    /// "연결 재설정" 등으로 캐시를 지워야 할 때.
    public static func clear() {
        UserDefaults(suiteName: suiteName)?.removeObject(forKey: key)
    }
}

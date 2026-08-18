import Foundation

/// macOS 쪽 `src/lib/format.ts` 를 그대로 옮긴 것.
/// 두 화면이 나란히 놓였을 때 같은 숫자가 같은 모양으로 보여야 하므로
/// 반올림 방식과 경계값을 원본과 정확히 맞춘다.
public enum MirrorFormat {

    /// formatTokensPerSec: 1 미만은 "0", 1000 미만은 정수, 그 이상은 "N.Nk"
    public static func tokensPerSec(_ v: Float) -> String {
        if v < 1 { return "0" }
        if v < 1000 { return String(format: "%.0f", v) }
        return String(format: "%.1fk", v / 1000)
    }

    /// formatTokensTotal: 1000 미만 정수, 100만 미만 "N.Nk", 그 이상 "N.NNM"
    public static func tokensTotal(_ n: UInt32) -> String {
        if n < 1000 { return String(n) }
        if n < 1_000_000 { return String(format: "%.1fk", Double(n) / 1000) }
        return String(format: "%.2fM", Double(n) / 1_000_000)
    }

    /// relativeTime: 원본이 영문이므로 영문 그대로 둔다(두 화면 일치가 목적).
    public static func relativeTime(_ epochSecs: UInt64, now: Date) -> String {
        let nowSecs = UInt64(max(0, now.timeIntervalSince1970))
        // 미래 시각이 오면 UInt64 뺄셈이 언더플로하므로 0 으로 clamp 한다.
        let elapsed = nowSecs > epochSecs ? nowSecs - epochSecs : 0
        if elapsed < 5 { return "just now" }
        if elapsed < 60 { return "\(elapsed)s ago" }
        if elapsed < 3600 { return "\(elapsed / 60)m ago" }
        return "\(elapsed / 3600)h ago"
    }

    /// AgentCard.svelte 의 countdown 파생과 동일. 리셋 시각이 없으면 호출부가 nil 을 넘기지 않는다.
    public static func countdown(resetAt: UInt64, now: Date) -> String? {
        let nowSecs = UInt64(max(0, now.timeIntervalSince1970))
        if resetAt <= nowSecs { return "리셋됨" }
        let rem = resetAt - nowSecs
        let h = rem / 3600
        let m = (rem % 3600) / 60
        let s = rem % 60
        return h > 0 ? "약 \(h)시간 \(m)분 \(s)초 남음" : "약 \(m)분 \(s)초 남음"
    }
}

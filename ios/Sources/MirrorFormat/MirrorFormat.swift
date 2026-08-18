import Foundation

/// macOS 쪽 `src/lib/format.ts` 를 그대로 옮긴 것.
/// 두 화면이 나란히 놓였을 때 같은 숫자가 같은 모양으로 보여야 하므로
/// 반올림 방식과 경계값을 원본과 정확히 맞춘다.
public enum MirrorFormat {

    /// JS 의 toFixed 는 away-from-zero 로 반올림하는데 Swift 의 String(format:) 은
    /// C 라이브러리의 짝수 반올림을 쓴다. 두 화면에 같은 숫자가 보여야 하므로
    /// 포맷 전에 명시적으로 away-from-zero 로 맞춘다.
    private static func toFixed(_ v: Double, _ places: Int) -> String {
        let f = pow(10.0, Double(places))
        let rounded = (v * f).rounded(.toNearestOrAwayFromZero) / f
        return String(format: "%.\(places)f", rounded)
    }

    /// formatTokensPerSec: 1 미만은 "0", 1000 미만은 정수, 그 이상은 "N.Nk"
    public static func tokensPerSec(_ v: Float) -> String {
        // f32 로 전송된 값을 그대로 Double 로 넓힌다. Mac 쪽도 동일한 f32 값을
        // JSON 으로 파싱해 JS 의 double 로 다루므로, 정밀도를 더 얹지 않아야 두 값이 같아진다.
        let d = Double(v)
        if d < 1 { return "0" }
        if d < 1000 { return toFixed(d, 0) }
        return toFixed(d / 1000, 1) + "k"
    }

    /// formatTokensTotal: 1000 미만 정수, 100만 미만 "N.Nk", 그 이상 "N.NNM"
    public static func tokensTotal(_ n: UInt32) -> String {
        if n < 1000 { return String(n) }
        if n < 1_000_000 { return toFixed(Double(n) / 1000, 1) + "k" }
        return toFixed(Double(n) / 1_000_000, 2) + "M"
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

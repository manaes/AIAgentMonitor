import Foundation

/// macOS 쪽 `src/lib/format.ts` 를 그대로 옮긴 것.
/// 두 화면이 나란히 놓였을 때 같은 숫자가 같은 모양으로 보여야 하므로
/// 반올림 방식과 경계값을 원본과 정확히 맞춘다.
public enum MirrorFormat {

    /// v 가 places 자리에서 "진짜" 동점(예: 1.25 를 소수 1자리로— 다음 자리가 정확히
    /// 5 이고 그 뒤로 전부 0)인지 확인하고, 동점이면 away-from-zero 로 반올림한 문자열을
    /// 반환한다. 동점이 아니면 nil — 이 경우 %.*f 의 결과가 이미 JS 와 일치한다.
    ///
    /// 왜 필요한가: JS 의 toFixed 와 C 의 %.*f 는 둘 다 이진값을 정확히 반올림하지만,
    /// 동점에서만 규칙이 다르다 — C 는 짝수 쪽으로, JS 는 항상 0 에서 먼 쪽으로.
    /// (v * 10^places 를 미리 반올림하는 방식은 시도하지 않는다: 그 곱셈 자체가
    /// 원래 값에는 없던 동점을 만들어낸다. 예를 들어 1150/1000 은 정확히는
    /// 1.14999999999999991118... 인데 10 을 곱하면 반올림 오차로 정확히 11.5 가 되어
    /// 버리고, 그러면 실제로는 동점이 아닌 값을 동점으로 잘못 판단해 JS 와 어긋난다.)
    private static func awayFromZeroTieString(_ v: Double, places: Int) -> String? {
        // 이 앱의 도메인(토큰 수/속도)에는 음수가 없고, 유한한 값만 온다.
        // 와이어를 타고 오늘 NaN 이 실제로 들어올 경로는 없다(JSONDecoder 가 숫자가
        // 아닌 리터럴에서 던지고 JSON 자체에 NaN 표현이 없다) — 그래도 방어적으로
        // 여기서 막아, 동점 판정 로직이 이상한 문자열을 만들다 트랩하지 않게 한다.
        guard v.isFinite, v >= 0 else { return nil }

        // places 이후로 한참 더 전개해서 "진짜 5000...0" 인지, 아니면 "4999..." 나
        // "5000...01" 처럼 실제로는 동점이 아닌지 구분한다. 이 앱이 다루는 값들은
        // 배정밀도로도 십여 자리 안에서 끝나므로 30 자리 여유면 충분하다.
        let extraDigits = 30
        let deep = String(format: "%.\(places + extraDigits)f", v)
        guard let dotIndex = deep.firstIndex(of: ".") else { return nil }
        let intPart = deep[deep.startIndex..<dotIndex]
        let frac = deep[deep.index(after: dotIndex)...]
        let keepEnd = frac.index(frac.startIndex, offsetBy: places)
        let kept = frac[frac.startIndex..<keepEnd]
        let tail = frac[keepEnd...]
        guard tail.first == "5", tail.dropFirst().allSatisfy({ $0 == "0" }) else {
            return nil
        }

        // 동점이 맞다: 자른 자릿수(intPart+kept)를 1 만큼 올림(away-from-zero)한다.
        var digits = Array(intPart + kept)
        var i = digits.count - 1
        while i >= 0 {
            if digits[i] == "9" {
                digits[i] = "0"
                i -= 1
            } else {
                digits[i] = Character(String(digits[i].wholeNumberValue! + 1))
                break
            }
        }
        if i < 0 { digits.insert("1", at: 0) }

        let keptCount = kept.count
        let newIntLen = digits.count - keptCount
        let newInt = String(digits[0..<newIntLen])
        guard places > 0 else { return newInt }
        let newFrac = String(digits[newIntLen...])
        return "\(newInt).\(newFrac)"
    }

    private static func toFixed(_ v: Double, _ places: Int) -> String {
        if let tie = awayFromZeroTieString(v, places: places) { return tie }
        return String(format: "%.\(places)f", v)
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

import Foundation

/// 사용량 바 그라디언트 양끝 색. 뷰가 아니라 값이므로 여기서 결정하고 테스트한다.
public struct QuotaGradient: Equatable, Sendable {
    public let startHex: UInt32
    public let endHex: UInt32

    public init(startHex: UInt32, endHex: UInt32) {
        self.startHex = startHex
        self.endHex = endHex
    }
}

/// `QuotaBar.svelte` 의 파생 로직을 그대로 옮긴 것.
public enum QuotaDisplay {

    /// color(p) 와 동일한 임계치: 90 이상 주황→빨강, 70 이상 녹→주황, 그 외 녹→녹
    public static func gradient(forPercent p: Float) -> QuotaGradient {
        if p >= 90 { return QuotaGradient(startHex: 0xff9f0a, endHex: 0xff453a) }
        if p >= 70 { return QuotaGradient(startHex: 0x30d158, endHex: 0xff9f0a) }
        return QuotaGradient(startHex: 0x30d158, endHex: 0x34c759)
    }

    /// pct 파생: 리셋 직후면 0, 아니면 min(100, autoPct), 동기화 전이면 nil.
    /// 원본이 reset_5h 를 먼저 평가하므로 순서를 지킨다.
    public static func displayPercent(autoPct: Float?, isReset: Bool) -> Float? {
        if isReset { return 0 }
        guard let autoPct else { return nil }
        return min(100, autoPct)
    }

    /// isReset5h 파생: 리셋 시각을 알고 남은 시간이 0 이하일 때만 true.
    public static func isReset5h(resetAt: UInt64?, now: Date) -> Bool {
        guard let resetAt else { return false }
        let nowSecs = UInt64(max(0, now.timeIntervalSince1970))
        return resetAt <= nowSecs
    }
}

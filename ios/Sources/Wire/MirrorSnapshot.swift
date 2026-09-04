import Foundation

/// Rust `src-tauri/src/ble/wire.rs` 의 DTO 를 그대로 옮긴 것.
/// 짧은 키는 BLE 대역 절약을 위한 것이며, 골든 벡터로 양쪽을 묶어둔다.
public let protocolVersion: UInt8 = 1

public enum AgentKindCode: Equatable, Sendable {
    case claude
    case codex
    case antigravity
    case unknown

    init(code: UInt8) {
        switch code {
        case 0: self = .claude
        case 1: self = .codex
        case 2: self = .antigravity
        default: self = .unknown
        }
    }
}

public enum ActivityStatusCode: Equatable, Sendable {
    case active
    case idle
    case dormant
    /// Rust 가 새 상태 코드를 추가했을 때 dormant 로 조용히 뭉뚱그리지 않기 위한 값.
    /// AgentKindCode 와 같은 규칙을 쓴다 — 모르는 코드는 모른다고 표시한다.
    case unknown

    init(code: UInt8) {
        switch code {
        case 0: self = .active
        case 1: self = .idle
        case 2: self = .dormant
        default: self = .unknown
        }
    }
}

/// `MirrorAgent.e`(wire.rs `QuotaErrorKind::code`). 사용량을 못 읽고 있는 이유.
///
/// 맥은 **문장이 아니라 이 코드만** 보낸다(BLE 대역·CYD DRAM — wire.rs 주석 참고).
/// 그래서 화면에 띄울 문구는 이쪽에서 고른다. 모르는 코드는 `.unknown` 으로
/// fail-safe 하되 "읽을 수 없음"이라는 사실 자체는 그대로 보여준다 — 맥이 새
/// 종류를 추가했다고 해서 "정상"으로 되돌아가면 안 된다.
public enum QuotaErrorKindCode: Equatable, Sendable {
    case auth
    case launch
    case timeout
    case other
    case unknown

    init(code: UInt8) {
        switch code {
        case 1: self = .auth
        case 2: self = .launch
        case 3: self = .timeout
        case 4: self = .other
        default: self = .unknown
        }
    }

    /// 카드에 한 줄로 띄울 문구. 맥의 문장보다 짧다 — 폰 화면 폭에 맞춘다.
    public var displayText: String {
        switch self {
        case .auth: return "로그인 필요"
        case .launch: return "CLI 실행 실패"
        case .timeout: return "조회 시간 초과"
        case .other, .unknown: return "한도 조회 실패"
        }
    }
}

public struct MirrorProject: Decodable, Equatable, Sendable {
    public let id: UInt32
    public let n: String
    public let m: String
    public let r: Float
    public let t: UInt64
    public let s: UInt8

    public var name: String { n }
    public var model: String { m }
    public var ratePerSec: Float { r }
    public var lastEventAt: Date { Date(timeIntervalSince1970: TimeInterval(t)) }
    public var status: ActivityStatusCode { ActivityStatusCode(code: s) }
}

public struct MirrorAgent: Decodable, Equatable, Sendable {
    public let k: UInt8
    public let r: Float
    public let t5: UInt32
    public let p5: Float?
    public let r5: UInt64?
    public let pw: Float?
    public let rw: UInt64?
    /// 조회 실패 분류 코드. 정상이면 맥이 키 자체를 생략하므로 nil 이다.
    public let e: UInt8?
    public let pj: [MirrorProject]

    public var kind: AgentKindCode { AgentKindCode(code: k) }
    public var ratePerSec: Float { r }
    public var tokens5h: UInt32 { t5 }
    public var usedPct5h: Float? { p5 }
    public var usedPctWeekly: Float? { pw }
    public var resetAt5h: Date? { r5.map { Date(timeIntervalSince1970: TimeInterval($0)) } }
    public var resetAtWeekly: Date? { rw.map { Date(timeIntervalSince1970: TimeInterval($0)) } }
    public var projects: [MirrorProject] { pj }

    /// 사용량을 못 읽고 있는가. 이 값이 있으면 %·리셋 카운트다운을 숨긴다 —
    /// 맥이 실패 중에도 마지막 %를 함께 보내주지만(구버전 CYD 호환), 그 숫자는
    /// 지금 상태를 말해주지 못한다.
    public var quotaError: QuotaErrorKindCode? { e.map { QuotaErrorKindCode(code: $0) } }
}

public struct MirrorSnapshot: Decodable, Equatable, Sendable {
    public let v: UInt8
    public let t: UInt64
    public let a: [MirrorAgent]

    public var emittedAt: Date { Date(timeIntervalSince1970: TimeInterval(t)) }
    public var agents: [MirrorAgent] { a }
    public var isSupportedVersion: Bool { v == protocolVersion }
}

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
    public let pj: [MirrorProject]

    public var kind: AgentKindCode { AgentKindCode(code: k) }
    public var ratePerSec: Float { r }
    public var tokens5h: UInt32 { t5 }
    public var usedPct5h: Float? { p5 }
    public var usedPctWeekly: Float? { pw }
    public var resetAt5h: Date? { r5.map { Date(timeIntervalSince1970: TimeInterval($0)) } }
    public var resetAtWeekly: Date? { rw.map { Date(timeIntervalSince1970: TimeInterval($0)) } }
    public var projects: [MirrorProject] { pj }
}

public struct MirrorSnapshot: Decodable, Equatable, Sendable {
    public let v: UInt8
    public let t: UInt64
    public let a: [MirrorAgent]

    public var emittedAt: Date { Date(timeIntervalSince1970: TimeInterval(t)) }
    public var agents: [MirrorAgent] { a }
    public var isSupportedVersion: Bool { v == protocolVersion }
}

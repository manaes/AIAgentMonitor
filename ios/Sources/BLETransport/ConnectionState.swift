import Foundation

/// 스펙 7.3. 화면 상단에 항상 노출해 "왜 안 뜨는지" 가 미궁이 되지 않게 한다.
public enum ConnectionState: Equatable, Sendable {
    case idle
    case bluetoothOff
    case scanning
    case connecting
    case streaming
    case disconnected(reason: String)

    public var label: String {
        switch self {
        case .idle: return "대기 중"
        case .bluetoothOff: return "블루투스가 꺼져 있습니다"
        case .scanning: return "Mac 찾는 중…"
        case .connecting: return "연결 중…"
        case .streaming: return "연결됨"
        case .disconnected(let reason): return "연결 끊김 · \(reason)"
        }
    }
}

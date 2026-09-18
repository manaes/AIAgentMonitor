import Foundation

/// 페어링된 Mac 한 대. `endpointIdHex` 가 키다 — iroh EndpointId 는 Mac 마다
/// 고유하고 재설치 전까지 안정적이다.
public struct Device: Codable, Equatable, Identifiable, Sendable {
    public var endpointIdHex: String
    public var token: String
    public var relayUrl: String?
    public var addresses: [String]
    /// QR 로 받은 Mac 이름. Mac 이름을 나중에 바꿔도 갱신되지 않는다.
    public var macHostname: String?
    /// 사용자가 덮어쓴 이름. 있으면 이쪽이 우선한다.
    public var userLabel: String?
    public var sortIndex: Int

    public var id: String { endpointIdHex }

    public init(
        endpointIdHex: String,
        token: String,
        relayUrl: String?,
        addresses: [String],
        macHostname: String?,
        userLabel: String?,
        sortIndex: Int
    ) {
        self.endpointIdHex = endpointIdHex
        self.token = token
        self.relayUrl = relayUrl
        self.addresses = addresses
        self.macHostname = macHostname
        self.userLabel = userLabel
        self.sortIndex = sortIndex
    }

    /// 표시 이름 우선순위: 사용자가 붙인 이름 > Mac 이 보낸 이름 > EndpointId 앞 8자
    public var displayName: String {
        if let userLabel, !userLabel.isEmpty { return userLabel }
        if let macHostname, !macHostname.isEmpty { return macHostname }
        return String(endpointIdHex.prefix(8))
    }
}

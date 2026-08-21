import Foundation
import Security

/// BLE `TokenStore` 와 같은 이유(UserDefaults 는 백업에 평문으로 실린다)로 Keychain을
/// 쓰되, 계정 문자열을 분리한다 — BLE/네트워크는 별도 PairingManager 인스턴스를 쓰므로
/// (계획 문서 Phase 1 결정) 같은 기기라도 토큰이 서로 다른 신원이다.
public enum NetworkTokenStore {
    private static let service = "com.dgitx.aiagentmonitor.mirror"
    private static let tokenAccount = "network-pairing-token"
    /// 재스캔 없이 재연결하기 위한 Mac 의 EndpointId(hex, 32바이트). 값 자체는
    /// 비밀이 아니지만(공개키), 페어링 여부를 기기 밖으로 흘리지 않기 위해 같은
    /// Keychain 항목에 둔다.
    private static let endpointAccount = "network-pairing-endpoint"

    private static func baseQuery(account: String) -> [String: Any] {
        [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: service,
            kSecAttrAccount as String: account,
        ]
    }

    @discardableResult
    private static func save(_ value: String, account: String) -> Bool {
        clear(account: account)
        var q = baseQuery(account: account)
        q[kSecValueData as String] = Data(value.utf8)
        q[kSecAttrAccessible as String] = kSecAttrAccessibleWhenUnlockedThisDeviceOnly
        return SecItemAdd(q as CFDictionary, nil) == errSecSuccess
    }

    private static func load(account: String) -> String? {
        var q = baseQuery(account: account)
        q[kSecReturnData as String] = true
        q[kSecMatchLimit as String] = kSecMatchLimitOne
        var out: CFTypeRef?
        guard SecItemCopyMatching(q as CFDictionary, &out) == errSecSuccess,
              let data = out as? Data else { return nil }
        return String(data: data, encoding: .utf8)
    }

    private static func clear(account: String) {
        SecItemDelete(baseQuery(account: account) as CFDictionary)
    }

    @discardableResult
    public static func saveToken(_ token: String) -> Bool { save(token, account: tokenAccount) }
    public static func loadToken() -> String? { load(account: tokenAccount) }
    public static func clearToken() { clear(account: tokenAccount) }

    @discardableResult
    public static func saveEndpointIdHex(_ hex: String) -> Bool { save(hex, account: endpointAccount) }
    public static func loadEndpointIdHex() -> String? { load(account: endpointAccount) }
    public static func clearEndpointIdHex() { clear(account: endpointAccount) }
}

import Foundation
import Security

/// 페어링 토큰 보관소. UserDefaults 는 백업에 평문으로 실려 나가므로 쓰지 않는다.
public enum TokenStore {
    private static let service = "com.dgitx.aiagentmonitor.mirror"
    private static let account = "ble-pairing-token"

    private static var baseQuery: [String: Any] {
        [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: service,
            kSecAttrAccount as String: account,
        ]
    }

    public static func save(_ token: String) {
        clear()
        var q = baseQuery
        q[kSecValueData as String] = Data(token.utf8)
        // 기기가 잠긴 동안에는 읽히지 않아야 하고, 백업으로 다른 기기에 옮겨가서도 안 된다.
        q[kSecAttrAccessible as String] = kSecAttrAccessibleWhenUnlockedThisDeviceOnly
        SecItemAdd(q as CFDictionary, nil)
    }

    public static func load() -> String? {
        var q = baseQuery
        q[kSecReturnData as String] = true
        q[kSecMatchLimit as String] = kSecMatchLimitOne
        var out: CFTypeRef?
        guard SecItemCopyMatching(q as CFDictionary, &out) == errSecSuccess,
              let data = out as? Data else { return nil }
        return String(data: data, encoding: .utf8)
    }

    public static func clear() {
        SecItemDelete(baseQuery as CFDictionary)
    }
}

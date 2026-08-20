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

    /// 저장 성공 여부를 돌려준다. `SecItemAdd` 의 결과를 버리지 않는다 —
    /// 조용히 실패하면 사용자는 페어링이 된 줄 알고 있다가 앱을 재시작할 때
    /// 서야 코드를 다시 요구받는 걸 보게 된다(Mac 쪽 `ble-peers.json` 저장
    /// 실패를 last_error 로 노출한 것과 같은 이유).
    @discardableResult
    public static func save(_ token: String) -> Bool {
        clear()
        var q = baseQuery
        q[kSecValueData as String] = Data(token.utf8)
        // 기기가 잠긴 동안에는 읽히지 않아야 하고, 백업으로 다른 기기에 옮겨가서도 안 된다.
        q[kSecAttrAccessible as String] = kSecAttrAccessibleWhenUnlockedThisDeviceOnly
        return SecItemAdd(q as CFDictionary, nil) == errSecSuccess
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

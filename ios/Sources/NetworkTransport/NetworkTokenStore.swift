import Foundation
import Security
import os

/// BLE `TokenStore` 와 같은 이유(UserDefaults 는 백업에 평문으로 실린다)로 Keychain을
/// 쓰되, 계정 문자열을 분리한다 — BLE/네트워크는 별도 PairingManager 인스턴스를 쓰므로
/// (계획 문서 Phase 1 결정) 같은 기기라도 토큰이 서로 다른 신원이다.
public enum NetworkTokenStore {
    private static let logger = Logger(subsystem: "co.kr.wannypark.aiagentmirror", category: "NetworkTokenStore")

    private static let service = "co.kr.wannypark.aiagentmirror"
    /// 엔타이틀먼트(`keychain-access-groups`)에 적는 그룹에서 팀ID 접두사를 뺀 부분.
    private static let sharedGroupSuffix = "co.kr.wannypark.aiagentmirror.shared"
    /// 접두사를 못 알아냈을 때만 쓰는 값. 로컬에 "Juwan Park" 이름의 팀이 둘
    /// (4Z3DSP9QUS / LC8PY3D283) 있어서, 다른 팀으로 서명되면 이 문자열이 통째로
    /// 어긋난다(Project.swift 주석 참고).
    private static let fallbackTeamPrefix = "LC8PY3D283"

    /// Keychain access group. 엔타이틀먼트에는 `$(AppIdentifierPrefix)…` 매크로로
    /// 적지만 그건 Xcode 가 빌드 시 치환하는 값이라 런타임 Swift 에서는 쓸 수 없다.
    ///
    /// 치환 결과(팀ID)를 상수로 박아두면 다른 팀으로 서명되는 순간 Keychain 이 그런
    /// 그룹을 못 찾아 저장·조회가 **전부 조용히 실패**한다 — 반환값을 버리는 호출부가
    /// 많아 증상이 "페어링이 안 된다"로만 보인다. 그래서 접두사를 박지 않고, 우리 앱이
    /// 실제로 어떤 접두사로 서명됐는지 런타임에 확인한다.
    private static let accessGroup: String = {
        guard let discovered = discoverDefaultAccessGroup() else {
            logger.error("Keychain access group 자동 감지 실패 — \(fallbackTeamPrefix, privacy: .public) 로 폴백")
            return "\(fallbackTeamPrefix).\(sharedGroupSuffix)"
        }
        // 엔타이틀먼트의 첫 항목이 곧 기본 그룹이고 우리는 공유 그룹 하나만 선언하므로,
        // 보통은 감지값이 그대로 정답이다.
        if discovered.hasSuffix(sharedGroupSuffix) { return discovered }
        guard let prefix = discovered.split(separator: ".").first else {
            return "\(fallbackTeamPrefix).\(sharedGroupSuffix)"
        }
        return "\(prefix).\(sharedGroupSuffix)"
    }()

    /// access group 을 지정하지 않은 항목을 하나 넣어 보고, 시스템이 어떤 그룹에
    /// 넣었는지 되읽는다. 엔타이틀먼트를 런타임에 직접 읽을 공개 API 가 없어서 쓰는
    /// 표준 우회법이다. 확인이 끝나면 지워서 흔적을 남기지 않는다.
    private static func discoverDefaultAccessGroup() -> String? {
        let probe: [String: Any] = [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: service,
            kSecAttrAccount as String: "access-group-probe",
        ]

        var add = probe
        add[kSecValueData as String] = Data()
        add[kSecAttrAccessible as String] = kSecAttrAccessibleAfterFirstUnlockThisDeviceOnly
        add[kSecReturnAttributes as String] = true

        var out: CFTypeRef?
        var status = SecItemAdd(add as CFDictionary, &out)
        if status == errSecDuplicateItem {
            var lookup = probe
            lookup[kSecReturnAttributes as String] = true
            lookup[kSecMatchLimit as String] = kSecMatchLimitOne
            status = SecItemCopyMatching(lookup as CFDictionary, &out)
        }

        defer { SecItemDelete(probe as CFDictionary) }

        guard status == errSecSuccess,
              let attrs = out as? [String: Any],
              let group = attrs[kSecAttrAccessGroup as String] as? String else {
            logger.error("access group probe 실패 status=\(status, privacy: .public)")
            return nil
        }
        return group
    }
    private static let tokenAccount = "network-pairing-token"
    /// 재스캔 없이 재연결하기 위한 Mac 의 EndpointId(hex, 32바이트). 값 자체는
    /// 비밀이 아니지만(공개키), 페어링 여부를 기기 밖으로 흘리지 않기 위해 같은
    /// Keychain 항목에 둔다.
    private static let endpointAccount = "network-pairing-endpoint"
    /// QR 에서 함께 받은 relay URL/direct 주소. EndpointId 만으로는 discovery 가
    /// 우리 쪽에 등록돼 있지 않아 dial 이 안 된다(실기기에서 `no addressing
    /// information` 로 확인) — 재스캔 없이 재연결하려면 이 값들도 같이 저장해야 한다.
    private static let relayAccount = "network-pairing-relay"
    private static let addressesAccount = "network-pairing-addresses"

    private static func baseQuery(account: String) -> [String: Any] {
        [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: service,
            kSecAttrAccount as String: account,
            kSecAttrAccessGroup as String: accessGroup,
        ]
    }

    @discardableResult
    private static func save(_ value: String, account: String) -> Bool {
        clear(account: account)
        var q = baseQuery(account: account)
        q[kSecValueData as String] = Data(value.utf8)
        q[kSecAttrAccessible as String] = kSecAttrAccessibleWhenUnlockedThisDeviceOnly
        let status = SecItemAdd(q as CFDictionary, nil)
        if status != errSecSuccess {
            // 호출부 상당수가 `@discardableResult` 로 반환값을 버린다 — 여기서 남기지
            // 않으면 "페어링이 조용히 안 된다"는 증상만 남는다. -34018
            // (errSecMissingEntitlement) 은 access group 접두사가 어긋난 신호다.
            logger.error("Keychain 저장 실패 account=\(account, privacy: .public) status=\(status, privacy: .public)")
        }
        return status == errSecSuccess
    }

    private static func load(account: String) -> String? {
        var q = baseQuery(account: account)
        q[kSecReturnData as String] = true
        q[kSecMatchLimit as String] = kSecMatchLimitOne
        var out: CFTypeRef?
        let status = SecItemCopyMatching(q as CFDictionary, &out)
        guard status == errSecSuccess, let data = out as? Data else {
            // 저장된 적이 없는 건 정상이라 로그를 남기지 않는다. 그 외 상태는 설정
            // 문제일 수 있으므로 남긴다.
            if status != errSecItemNotFound {
                logger.error("Keychain 조회 실패 account=\(account, privacy: .public) status=\(status, privacy: .public)")
            }
            return nil
        }
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

    @discardableResult
    public static func saveRelayUrl(_ url: String?) -> Bool {
        guard let url else {
            clear(account: relayAccount)
            return true
        }
        return save(url, account: relayAccount)
    }
    public static func loadRelayUrl() -> String? { load(account: relayAccount) }

    /// 쉼표로 구분해 저장한다 — 주소 문자열 자체에 쉼표가 나올 일이 없다(IP:port).
    @discardableResult
    public static func saveAddresses(_ addresses: [String]) -> Bool {
        save(addresses.joined(separator: ","), account: addressesAccount)
    }
    public static func loadAddresses() -> [String] {
        guard let joined = load(account: addressesAccount), !joined.isEmpty else { return [] }
        return joined.split(separator: ",").map(String.init)
    }

    /// 저장된 페어링 정보를 전부 지운다 — 설정에서 "네트워크"를 고를 때마다
    /// 이전 연결 정보로 조용히 재연결하지 않고 QR 을 다시 스캔하게 하는 데 쓴다.
    public static func clearAll() {
        clearToken()
        clearEndpointIdHex()
        clear(account: relayAccount)
        clear(account: addressesAccount)
    }
}
